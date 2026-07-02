import { useEffect, useMemo, useRef, useState } from 'react';
import { Spin, Result, Empty, Modal, message } from 'antd';
import { LineChartOutlined } from '@ant-design/icons';
import api from '../api/client';
import type { ApiEnvelope, NodeStatus, SharedNodeSummary, NodeDisplayRow } from '../api/types';
import { useI18n } from '../i18n/context';
import { useAuth } from '../auth/useAuth';
import { NodeGroupSection } from '../components/nodes/NodeGroupSection';
import { NodeDetailDrawer } from '../components/nodes/NodeDetailDrawer';
import { stableGroupedRows } from '../components/nodes/sort';

type AnyNodeRow = NodeDisplayRow;

interface VersionInfo {
  current_version: string;
  config_protocol_version?: number;
}

/** Hook: is the viewport mobile-width? Re-evaluates on resize. */
function useIsMobile(breakpoint = 768): boolean {
  const [mobile, setMobile] = useState(() => window.innerWidth < breakpoint);
  useEffect(() => {
    const onResize = () => setMobile(window.innerWidth < breakpoint);
    window.addEventListener('resize', onResize);
    return () => window.removeEventListener('resize', onResize);
  }, [breakpoint]);
  return mobile;
}

/**
 * v0.4.15 PR3: unified full-width node status board. Both admins and regular
 * users land here after login (via the sidebar). Admin reads /nodes; regular
 * users read /nodes/shared (server-side field filtering — the frontend never
 * hides sensitive fields client-side).
 */
export default function NodeStatus() {
  const { t } = useI18n();
  const { isAdmin } = useAuth();
  const isMobile = useIsMobile();

  const [adminRows, setAdminRows] = useState<NodeStatus[] | null>(null);
  const [userRows, setUserRows] = useState<SharedNodeSummary[] | null>(null);
  const [loadFailed, setLoadFailed] = useState(false);
  const [currentVersion, setCurrentVersion] = useState('');
  const [panelProtocol, setPanelProtocol] = useState(0);
  const [detailRow, setDetailRow] = useState<AnyNodeRow | null>(null);
  // Guards against overlapping polls: on a slow network (axios 10s timeout vs
  // 5s interval) a new tick could otherwise fire before the previous request
  // returned, stacking requests.
  const inFlightRef = useRef(false);

  const loadAdmin = async () => {
    try {
      const res = await api.get<unknown, ApiEnvelope<NodeStatus[]>>('/nodes');
      if (res.code !== 0) {
        setLoadFailed(true);
        return;
      }
      setLoadFailed(false);
      setAdminRows(res.data || []);
    } catch {
      setLoadFailed(true);
    }
  };

  const loadUser = async () => {
    try {
      const res = await api.get<unknown, ApiEnvelope<SharedNodeSummary[]>>('/nodes/shared');
      if (res.code !== 0) {
        setLoadFailed(true);
        return;
      }
      setLoadFailed(false);
      setUserRows(res.data || []);
    } catch {
      setLoadFailed(true);
    }
  };

  const loadVersion = async () => {
    try {
      const res = await api.get<unknown, VersionInfo>('/system/version');
      setCurrentVersion(res.current_version || '');
      setPanelProtocol(res.config_protocol_version || 0);
    } catch { /* ignore */ }
  };

  const refresh = async () => {
    // Skip this tick if the previous request is still outstanding.
    if (inFlightRef.current) return;
    inFlightRef.current = true;
    try {
      await (isAdmin ? loadAdmin() : loadUser());
    } finally {
      inFlightRef.current = false;
    }
  };

  // Poll node status every 5s. The version info is NOT polled — it's static
  // for the lifetime of a panel process, so it's fetched once on mount (admin
  // only). loadFailed is cleared only on a successful response (inside the
  // load* fns), so a transient poll failure no longer flashes the error page
  // back to stale data every 5s.
  useEffect(() => {
    if (isAdmin) loadVersion();
    refresh();
    const ti = setInterval(refresh, 5000);
    return () => clearInterval(ti);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [isAdmin]);

  // v1.0.10: admin triggers a directed node self-upgrade. Confirm first (the
  // node restarts, so its forwarding blips for a few seconds).
  const handleUpgrade = (row: AnyNodeRow) => {
    if (!row.node_id) return;
    Modal.confirm({
      title: t('nodeUpgradeConfirmTitle'),
      content: t('nodeUpgradeConfirm').replace('{v}', currentVersion || 'latest'),
      okText: t('nodeUpgradeOk'),
      cancelText: t('cancel'),
      onOk: async () => {
        try {
          const res = await api.post<unknown, ApiEnvelope<null>>(
            `/nodes/${row.group_id}/upgrade/${row.node_id}`,
            {},
          );
          if (res.code !== 0) { message.error(res.message); return; }
          message.success(t('nodeUpgradeSent'));
        } catch { message.error(t('nodeUpgradeFailed')); }
      },
    });
  };

  const rows: AnyNodeRow[] | null = isAdmin ? adminRows : userRows;
  const groups = useMemo(() => (rows ? stableGroupedRows(rows) : null), [rows]);

  const title = t('nodeStatus');

  // Load failure (DB error / request failure) — not a normal empty state.
  // v0.4.15 PR3: applies to admins too (loadAdmin now surfaces failures).
  if (loadFailed) {
    return (
      <>
        <h2 className="rp-page-title"><LineChartOutlined /> {title}</h2>
        <Result status="warning" title={t('loadFailed')} subTitle={t('loadFailedRetry')} />
      </>
    );
  }

  if (rows === null || groups === null) {
    return <div style={{ textAlign: 'center', padding: 48 }}><Spin /></div>;
  }

  // No groups at all.
  if (groups.length === 0) {
    return (
      <>
        <h2 className="rp-page-title"><LineChartOutlined /> {title}</h2>
        <Result
          status="info"
          icon={<Empty image={Empty.PRESENTED_IMAGE_SIMPLE} />}
          title={isAdmin ? t('noNodesHint') : t('adminNoLines')}
        />
      </>
    );
  }

  return (
    <>
      <h2 className="rp-page-title"><LineChartOutlined /> {title}</h2>
      {groups.map(([gid, groupRows]) => (
        <NodeGroupSection
          key={gid}
          rows={groupRows}
          panelProtocol={panelProtocol}
          currentVersion={currentVersion}
          isMobile={isMobile}
          t={t}
          openDetail={setDetailRow}
          onUpgrade={isAdmin ? handleUpgrade : undefined}
        />
      ))}
      <NodeDetailDrawer
        row={detailRow}
        open={detailRow !== null}
        onClose={() => setDetailRow(null)}
        isAdmin={isAdmin}
        panelProtocol={panelProtocol}
        onDeleted={refresh}
      />
    </>
  );
}
