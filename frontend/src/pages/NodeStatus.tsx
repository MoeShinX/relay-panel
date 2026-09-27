import { useEffect, useMemo, useRef, useState } from 'react';
import { Spin, Result, Empty, Modal, message } from 'antd';
import { LineChartOutlined } from '@ant-design/icons';
import api from '../api/client';
import type { ApiEnvelope, NodeStatus, SharedNodeSummary, NodeDisplayRow, NodeLiveRate } from '../api/types';
import { useI18n } from '../i18n/context';
import { useAuth } from '../auth/useAuth';
import { NodeGroupSection } from '../components/nodes/NodeGroupSection';
import { NodeDetailDrawer } from '../components/nodes/NodeDetailDrawer';
import { stableGroupedRows } from '../components/nodes/sort';

type AnyNodeRow = NodeDisplayRow;

/** v1.2.11: how often the page asks for pushed rates. Matches the node's push
 *  interval — polling faster only re-reads the same reading. */
const LIVE_RATE_POLL_MS = 2000;

const liveKey = (groupId: number, nodeId: string) => `${groupId}:${nodeId}`;

interface VersionInfo {
  current_version: string;
  config_protocol_version?: number;
  /** v1.2: the latest NODE release (bare, e.g. "1.1.0"), resolved from the
   *  highest node-v* GitHub release. Nodes compare their version against THIS,
   *  not the panel version. Empty when no node release exists. */
  latest_node_version?: string;
  /** v1.2: true when the node-version lookup failed. The UI must show an
   *  "unknown / check failed" state instead of a green "up to date" or an
   *  upgrade button. */
  node_version_check_failed?: boolean;
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
  // v1.2: nodes compare against the latest NODE release (latest_node_version),
  // NOT the panel's current_version. Renamed from currentVersion to make the
  // semantics obvious at every call site.
  const [latestNodeVersion, setLatestNodeVersion] = useState('');
  const [nodeVersionCheckFailed, setNodeVersionCheckFailed] = useState(false);
  const [panelProtocol, setPanelProtocol] = useState(0);
  const [detailRow, setDetailRow] = useState<AnyNodeRow | null>(null);
  // Guards against overlapping polls: on a slow network (axios 10s timeout vs
  // 5s interval) a new tick could otherwise fire before the previous request
  // returned, stacking requests.
  const inFlightRef = useRef(false);
  // v1.2.11: latest pushed rate per node, keyed by liveKey(). Replaced wholesale
  // on each poll, so a node whose feed went quiet drops back to its status
  // report figure instead of freezing on an old number.
  const [liveRates, setLiveRates] = useState<Map<string, NodeLiveRate>>(() => new Map());
  const liveInFlightRef = useRef(false);

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

  const loadLiveRates = async () => {
    // Nobody is looking: skip rather than poll a background tab every 2s.
    if (document.hidden || liveInFlightRef.current) return;
    liveInFlightRef.current = true;
    try {
      const res = await api.get<unknown, ApiEnvelope<NodeLiveRate[]>>('/nodes/live-rates');
      const next = new Map<string, NodeLiveRate>();
      if (res.code === 0) {
        for (const r of res.data || []) next.set(liveKey(r.group_id, r.node_id), r);
      }
      setLiveRates(next);
    } catch {
      // Fall back to the report's figures rather than showing stale live ones.
      setLiveRates(new Map());
    } finally {
      liveInFlightRef.current = false;
    }
  };

  const loadVersion = async () => {
    try {
      const res = await api.get<unknown, VersionInfo>('/system/version');
      setPanelProtocol(res.config_protocol_version || 0);
      // v1.2: the node upgrade target is the latest node release, not the
      // panel version. A failed lookup sets the "check failed" flag so the UI
      // shows an unknown state instead of a wrong upgrade button.
      setLatestNodeVersion(res.latest_node_version || '');
      setNodeVersionCheckFailed(!!res.node_version_check_failed);
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

  // v1.2.11: the live rate runs on its own, faster clock. Everything else on
  // the page (CPU, memory, disk, cumulative traffic) keeps the 5s refresh and
  // the node's own ~10s report cadence.
  useEffect(() => {
    loadLiveRates();
    const ti = setInterval(loadLiveRates, LIVE_RATE_POLL_MS);
    return () => clearInterval(ti);
  }, []);

  // v1.0.10: admin triggers a directed node self-upgrade. Confirm first (the
  // node restarts, so its forwarding blips for a few seconds).
  const handleUpgrade = (row: AnyNodeRow) => {
    if (!row.node_id) return;
    Modal.confirm({
      title: t('nodeUpgradeConfirmTitle'),
      content: t('nodeUpgradeConfirm').replace('{v}', latestNodeVersion || 'latest'),
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

  const baseRows: AnyNodeRow[] | null = isAdmin ? adminRows : userRows;
  // Overlay the pushed rate on online nodes that have one. The group header
  // sums row rates, so it picks the live figures up with no change of its own.
  const rows = useMemo(() => {
    if (!baseRows || liveRates.size === 0) return baseRows;
    return baseRows.map((r) => {
      const live = r.online && r.node_id ? liveRates.get(liveKey(r.group_id, r.node_id)) : undefined;
      return live ? { ...r, upload_bps: live.upload_bps, download_bps: live.download_bps } : r;
    });
  }, [baseRows, liveRates]);
  const groups = useMemo(() => (rows ? stableGroupedRows(rows) : null), [rows]);
  // The drawer follows the node, not the snapshot taken when it was opened, so
  // its figures (the live rate included) keep updating while it stays open.
  const detailCurrent = detailRow
    ? rows?.find((r) => r.group_id === detailRow.group_id && r.node_id === detailRow.node_id) ?? detailRow
    : null;

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

  // v1.2.5: drop one node's status record. Admin-only, and the button is only
  // rendered on offline rows — see NodeDesktopTable for why.
  const handleDelete = async (row: NodeDisplayRow) => {
    try {
      const qs = row.node_id ? `?node_id=${encodeURIComponent(row.node_id)}` : '';
      const res = await api.delete<unknown, ApiEnvelope<null>>(`/nodes/${row.group_id}${qs}`);
      if (res.code !== 0) { message.error(res.message || t('nodeRemoveFailed')); return; }
      message.success(t('nodeRemoved'));
      refresh();
    } catch {
      message.error(t('nodeRemoveFailed'));
    }
  };

  return (
    <>
      <h2 className="rp-page-title"><LineChartOutlined /> {title}</h2>
      {groups.map(([gid, groupRows]) => (
        <NodeGroupSection
          key={gid}
          rows={groupRows}
          panelProtocol={panelProtocol}
          latestNodeVersion={latestNodeVersion}
          nodeVersionCheckFailed={nodeVersionCheckFailed}
          isMobile={isMobile}
          t={t}
          openDetail={setDetailRow}
          onUpgrade={isAdmin ? handleUpgrade : undefined}
          onDelete={isAdmin ? handleDelete : undefined}
        />
      ))}
      <NodeDetailDrawer
        row={detailCurrent}
        open={detailRow !== null}
        onClose={() => setDetailRow(null)}
        isAdmin={isAdmin}
        panelProtocol={panelProtocol}
        onDeleted={refresh}
      />
    </>
  );
}
