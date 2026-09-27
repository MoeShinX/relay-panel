import { useEffect, useRef, useState } from 'react';
import { Alert, Button, Modal, Spin, Tooltip, Typography, message } from 'antd';
import api from '../api/client';
import type { ApiEnvelope } from '../api/types';
import { useI18n } from '../i18n/context';

/** v1.2.11: GET /system/panel-update. `available` = the host updater is
 *  installed; without it the button falls back to the manual steps. */
export interface PanelUpdateStatus {
  available: boolean;
  /** idle | requested | running | succeeded | failed | up_to_date | pinned */
  state: string;
  from_version: string;
  to_version: string;
  started_at: number;
  finished_at: number;
  message: string;
  log_tail: string;
}

type Phase = 'idle' | 'updating' | 'done' | 'result' | 'timeout';

const POLL_MS = 3000;
/** deploy.sh pulls images and waits up to a minute for the panel; ten minutes
 *  is far past any real run, and past it we stop claiming to know. */
const GIVE_UP_MS = 10 * 60 * 1000;
/** Host states that end a run without the panel coming back on a new version. */
const TERMINAL = new Set(['failed', 'pinned', 'up_to_date']);

interface Props {
  currentVersion: string;
  targetVersion: string;
  /** Where the manual steps live, for panels without the host updater. */
  manualUrl: string;
}

/**
 * v1.2.11: one-click panel update. The panel cannot replace its own container,
 * so this only ASKS: the server's relaypanel-updater unit does the update and
 * the panel restarts on the new version. While that happens this page loses
 * its server for a few seconds, so progress is decided by what comes back —
 * the version /health reports — not by any single response.
 */
export function PanelUpdateButton({ currentVersion, targetVersion, manualUrl }: Props) {
  const { t } = useI18n();
  const [status, setStatus] = useState<PanelUpdateStatus | null>(null);
  const [phase, setPhase] = useState<Phase>('idle');
  const [starting, setStarting] = useState(false);
  // A status file can still hold the PREVIOUS run's result. Only results
  // stamped after this belong to the run this page is watching.
  const prevStartedAt = useRef(0);
  const watchSince = useRef(0);

  useEffect(() => {
    let cancelled = false;
    (async () => {
      try {
        const r = await api.get<unknown, ApiEnvelope<PanelUpdateStatus>>('/system/panel-update');
        if (cancelled || r.code !== 0 || !r.data) return;
        setStatus(r.data);
        // Reopened mid-update (a reload, another tab): keep watching.
        if (r.data.state === 'requested' || r.data.state === 'running') {
          prevStartedAt.current = r.data.state === 'running' ? r.data.started_at - 1 : r.data.started_at;
          watchSince.current = Date.now();
          setPhase('updating');
        }
      } catch {
        /* no status: the manual-steps button is shown */
      }
    })();
    return () => {
      cancelled = true;
    };
  }, []);

  useEffect(() => {
    if (phase !== 'updating') return;
    const tick = async () => {
      // Back on a new version? That is the answer, whatever else happened.
      try {
        const h = await api.get<unknown, { status: string; version: string }>('/health');
        if (h.version && h.version !== currentVersion) {
          setPhase('done');
          window.setTimeout(() => window.location.reload(), 1500);
          return;
        }
      } catch {
        /* the panel is restarting — expected, keep waiting */
      }
      try {
        const r = await api.get<unknown, ApiEnvelope<PanelUpdateStatus>>('/system/panel-update');
        if (r.code === 0 && r.data) {
          setStatus(r.data);
          if (r.data.started_at > prevStartedAt.current && TERMINAL.has(r.data.state)) {
            setPhase('result');
            return;
          }
        }
      } catch {
        /* restarting */
      }
      if (Date.now() - watchSince.current > GIVE_UP_MS) setPhase('timeout');
    };
    const id = window.setInterval(tick, POLL_MS);
    return () => window.clearInterval(id);
  }, [phase, currentVersion]);

  const start = async () => {
    setStarting(true);
    try {
      const r = await api.post<unknown, ApiEnvelope<{ target: string }>>('/system/panel-update');
      if (r.code !== 0) {
        message.error(r.message || t('panelUpdateRequestFailed'));
        return;
      }
      prevStartedAt.current = status?.started_at ?? 0;
      watchSince.current = Date.now();
      setPhase('updating');
    } catch {
      message.error(t('panelUpdateRequestFailed'));
    } finally {
      setStarting(false);
    }
  };

  const confirm = () =>
    Modal.confirm({
      title: t('panelUpdateConfirmTitle').replace('{version}', targetVersion),
      content: t('panelUpdateConfirmBody'),
      okText: t('updateNow'),
      cancelText: t('cancel'),
      onOk: start,
    });

  const resultTitle =
    status?.state === 'pinned'
      ? t('panelUpdatePinned')
      : status?.state === 'up_to_date'
        ? t('panelUpdateUpToDate')
        : t('panelUpdateFailed');

  return (
    <>
      {status?.available ? (
        <Button size="small" type="primary" loading={starting} onClick={confirm}>
          {t('updateNow')}
        </Button>
      ) : (
        <Tooltip title={status ? t('panelUpdateManualHint') : undefined}>
          <Button size="small" type="primary" href={manualUrl} target="_blank">
            {t('updateNow')}
          </Button>
        </Tooltip>
      )}

      <Modal
        open={phase !== 'idle'}
        title={t('panelUpdateTitle')}
        closable={phase !== 'updating' && phase !== 'done'}
        maskClosable={false}
        footer={
          phase === 'result' || phase === 'timeout' ? (
            <Button onClick={() => setPhase('idle')}>{t('close')}</Button>
          ) : null
        }
        onCancel={() => setPhase('idle')}
      >
        {phase === 'updating' && (
          <div style={{ display: 'flex', gap: 16, alignItems: 'center', padding: '8px 0' }}>
            <Spin />
            <Typography.Text>{t('panelUpdating').replace('{version}', targetVersion)}</Typography.Text>
          </div>
        )}
        {phase === 'done' && (
          <Alert type="success" showIcon title={t('panelUpdateDone')} />
        )}
        {phase === 'timeout' && (
          <Alert type="warning" showIcon title={t('panelUpdateTimeout')} />
        )}
        {phase === 'result' && status && (
          <>
            <Alert
              type={status.state === 'failed' ? 'error' : 'warning'}
              showIcon
              title={resultTitle}
              description={status.message || undefined}
            />
            {status.log_tail && (
              <details style={{ marginTop: 12 }}>
                <summary>{t('panelUpdateLog')}</summary>
                <pre className="rp-mono" style={{ maxHeight: 240, overflow: 'auto', fontSize: 12, whiteSpace: 'pre-wrap' }}>
                  {status.log_tail}
                </pre>
              </details>
            )}
          </>
        )}
      </Modal>
    </>
  );
}
