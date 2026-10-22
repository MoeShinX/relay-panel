import { useRef, useState } from 'react';
import { Button, Modal, Tooltip, message } from 'antd';
import { CloudDownloadOutlined } from '@ant-design/icons';
import api from '../../api/client';
import type { ApiEnvelope } from '../../api/client';
import type { NodeDisplayRow } from '../../api/types';
import type { Tfn } from './types';

/** How many nodes the confirm box lists before collapsing the rest. */
const LIST_LIMIT = 10;
/** How many failures the result toast spells out. */
const FAILURE_LIMIT = 5;

interface Props {
  /** The nodes to upgrade — already filtered through `upgradableNodes`, so this
   *  component never re-decides eligibility. Renders nothing when empty. */
  nodes: NodeDisplayRow[];
  latestNodeVersion: string;
  /** The label template, with `{n}` for the node count (upgradeAll / upgradeGroup).
   *  Shown as the tooltip and the aria-label: the button itself is icon-only. */
  labelKey: 'upgradeAll' | 'upgradeGroup';
  t: Tfn;
  /** Called once the batch has been dispatched (success or not), to refresh. */
  onDone?: () => void;
  testId?: string;
}

interface Failure {
  node: NodeDisplayRow;
  reason: string;
}

/** What the admin recognises a node by: its IPv4, else any IP, else the head of
 *  its id. */
function nodeLabel(n: NodeDisplayRow): string {
  return n.public_ipv4 || n.public_ip || n.public_ipv6 || (n.node_id ?? '').slice(0, 8) || '-';
}

/** The backend answers 200 with `code != 0`, but a network error or a non-2xx
 *  rejects: pull the message out of whichever it was. */
function failureReason(err: unknown, fallback: string): string {
  const data = (err as { response?: { data?: { message?: unknown } } })?.response?.data;
  return typeof data?.message === 'string' && data.message ? data.message : fallback;
}

/**
 * v1.2.16: "upgrade all" / "upgrade this group". Asks once, then fires the
 * existing per-node `POST /nodes/{group}/upgrade/{node}` for every node at the
 * same time — no new endpoint, and every node still gets its own audit entry.
 * Admin-only by construction: callers only mount it for admins.
 */
export function UpgradeAllButton({ nodes, latestNodeVersion, labelKey, t, onDone, testId }: Props) {
  const [loading, setLoading] = useState(false);
  // State alone is not enough to stop a double click: two clicks in the same
  // tick both see loading === false.
  const busyRef = useRef(false);

  if (nodes.length === 0) return null;

  const run = async () => {
    // Snapshot: the list may refresh (and shrink) while the requests are out.
    const targets = nodes.slice();
    const results = await Promise.allSettled(
      targets.map((n) =>
        api.post<unknown, ApiEnvelope<null>>(`/nodes/${n.group_id}/upgrade/${n.node_id}`, {}),
      ),
    );
    const failures: Failure[] = [];
    results.forEach((r, i) => {
      if (r.status === 'fulfilled') {
        if (r.value.code !== 0) failures.push({ node: targets[i], reason: r.value.message || t('nodeUpgradeFailed') });
      } else {
        failures.push({ node: targets[i], reason: failureReason(r.reason, t('nodeUpgradeFailed')) });
      }
    });
    const okCount = targets.length - failures.length;
    if (failures.length === 0) {
      message.success(t('upgradeAllSent').replace('{n}', String(okCount)));
    } else {
      const shown = failures.slice(0, FAILURE_LIMIT);
      const content = (
        <div style={{ textAlign: 'left' }}>
          <div>
            {t('upgradeAllPartial').replace('{ok}', String(okCount)).replace('{fail}', String(failures.length))}
          </div>
          {shown.map((f) => (
            <div key={`${f.node.group_id}:${f.node.node_id}`}>
              {nodeLabel(f.node)}：{f.reason}
            </div>
          ))}
          {failures.length > shown.length && (
            <div>{t('upgradeAllFailedMore').replace('{n}', String(failures.length - shown.length))}</div>
          )}
        </div>
      );
      message.warning({ content, duration: 10 });
    }
    onDone?.();
  };

  const confirm = () => {
    if (busyRef.current) return;
    const listed = nodes.slice(0, LIST_LIMIT);
    const rest = nodes.length - listed.length;
    Modal.confirm({
      title: t('upgradeAllConfirmTitle').replace('{n}', String(nodes.length)),
      content: (
        <div>
          <p>
            {t('upgradeAllConfirmBody')
              .replace('{n}', String(nodes.length))
              .replace('{v}', latestNodeVersion || 'latest')}
          </p>
          <ul style={{ margin: '0 0 8px', paddingLeft: 20, maxHeight: 240, overflowY: 'auto' }}>
            {listed.map((n) => (
              <li key={`${n.group_id}:${n.node_id}`}>
                <span className="rp-mono">{nodeLabel(n)}</span>
                {n.node_version ? ` · v${n.node_version}` : ''}
              </li>
            ))}
            {rest > 0 && <li>{t('upgradeAllConfirmMore').replace('{n}', String(rest))}</li>}
          </ul>
          <p style={{ margin: 0 }}>{t('upgradeAllConfirmWarn')}</p>
        </div>
      ),
      okText: t('upgradeAllOk'),
      cancelText: t('cancel'),
      onOk: async () => {
        if (busyRef.current) return;
        busyRef.current = true;
        setLoading(true);
        try {
          await run();
        } finally {
          busyRef.current = false;
          setLoading(false);
        }
      },
    });
  };

  const label = t(labelKey).replace('{n}', String(nodes.length));
  return (
    <Tooltip title={label}>
      <Button
        size="small"
        type="link"
        // The very button of the per-node upgrade (NodeDesktopTable): same
        // type, size and icon, so the three read as one family.
        icon={<CloudDownloadOutlined />}
        aria-label={label}
        loading={loading}
        data-testid={testId}
        // The group button sits inside a Collapse header: a click (or Enter /
        // Space on the focused button) must not bubble up and fold the group.
        onClick={(e) => { e.stopPropagation(); confirm(); }}
        onKeyDown={(e) => e.stopPropagation()}
      />
    </Tooltip>
  );
}
