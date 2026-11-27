import { Card, Col, Row, Table, Tag, Typography, Alert, Modal, Button, Space, Tooltip } from 'antd';
import {
  ClusterOutlined, ApiOutlined, UserOutlined, DashboardOutlined, ReloadOutlined,
  ArrowUpOutlined, ArrowDownOutlined,
} from '@ant-design/icons';
import { useEffect, useMemo, useState } from 'react';
import { useNavigate } from 'react-router-dom';
import api from '../api/client';
import type { ApiEnvelope, User, ForwardRule, DeviceGroup, NodeStatus } from '../api/types';
import { useI18n } from '../i18n/context';
import { aggregateNodesByGroup } from '../components/nodes/aggregate';
import { connectionsTooltip } from '../components/nodes/shared';
import TrafficChart from '../components/TrafficChart';
import NodeMetricsChart from '../components/NodeMetricsChart';
import { formatBps, formatBytes } from '../utils/format';
import { PanelUpdateButton } from '../components/PanelUpdateButton';
import { useIsMobile, useTableScroll } from '../hooks/useIsMobile';

const { Text } = Typography;

interface VersionInfo {
  current_version: string;
  latest_version: string;
  has_update: boolean;
  is_outdated: boolean;
  release_url: string;
  release_notes: string;
  published_at: string;
  /** True if the GitHub release check itself failed (network / parse / non-2xx).
   *  In this case latest_version is unreliable; surface a "check failed" hint
   *  instead of letting the user think they are up to date. */
  check_failed: boolean;
  /** Short human-readable error from the last failed check. Empty on success. */
  error_message: string;
}

const IGNORED_KEY = 'relaypanel_ignored_version';
// The "Update Now" button points here. README.md is the user-facing entry
// doc with the one-line install + a dedicated upgrade section, so it's a better
// landing page than the (more ops-focused) docs/DEPLOYMENT.md.
const DEPLOY_DOC_URL = 'https://github.com/MoeShinX/relay-panel/blob/main/README.md#-更新';

export default function Dashboard() {
  const { t } = useI18n();
  const navigate = useNavigate();
  const isMobile = useIsMobile();
  const tableScroll = useTableScroll();
  const [stats, setStats] = useState({ users: 0, rules: 0, groups: 0 });
  // v1.2.0: kept whole (not just the count) for the traffic chart's drill-down.
  const [ruleList, setRuleList] = useState<ForwardRule[]>([]);
  const [nodes, setNodes] = useState<NodeStatus[]>([]);
  // v1.3.3: when the last successful /nodes read happened, and whether the most
  // recent one failed. The summary line must keep showing the older figures and
  // THEIR time — never stamp old data with a fresh "just updated".
  const [nodesUpdatedAt, setNodesUpdatedAt] = useState<Date | null>(null);
  const [nodesStale, setNodesStale] = useState(false);
  const [versionInfo, setVersionInfo] = useState<VersionInfo | null>(null);
  const [showChangelog, setShowChangelog] = useState(false);
  const [versionRefreshing, setVersionRefreshing] = useState(false);
  // See load(): a failed read must not be counted as zero.
  const [loadFailed, setLoadFailed] = useState(false);

  const load = async () => {
    try {
      const [users, rules, groups] = await Promise.all([
        api.get<unknown, ApiEnvelope<User[]>>('/admin/users'),
        api.get<unknown, ApiEnvelope<ForwardRule[]>>('/rules'),
        api.get<unknown, ApiEnvelope<DeviceGroup[]>>('/groups'),
      ]);
      // These reads report a database failure as `code: 500` with `data: null`
      // inside a 200 response. Counting a null payload would put "0 users,
      // 0 rules, 0 groups" on the dashboard — the most alarming possible way
      // to render an outage, and wrong. Keep the last known numbers and say
      // that the refresh failed instead.
      if (users.code !== 0 || rules.code !== 0 || groups.code !== 0) {
        setLoadFailed(true);
      } else {
        setLoadFailed(false);
        setStats({
          users: users.data?.length || 0,
          rules: rules.data?.length || 0,
          groups: groups.data?.length || 0,
        });
        setRuleList(rules.data || []);
      }
    } catch {
      setLoadFailed(true);
    }

    try {
      const nodeStatus = await api.get<unknown, ApiEnvelope<NodeStatus[]>>('/nodes');
      // v1.3.3: a failed read must not look like a fresh one. `code !== 0` is
      // the backend's way of reporting a DB failure inside a 200, so it counts
      // as a failure too: keep the previous rows and their timestamp.
      if (nodeStatus.code !== 0 || !Array.isArray(nodeStatus.data)) {
        setNodesStale(true);
        return;
      }
      setNodes(nodeStatus.data);
      setNodesUpdatedAt(new Date());
      setNodesStale(false);
    } catch {
      setNodesStale(true);
    }
  };

  const checkVersion = async (forceRefresh = false) => {
    if (forceRefresh) setVersionRefreshing(true);
    try {
      // ?refresh=true bypasses the 30-minute server-side cache so the manual
      // "check update" button always queries GitHub live. Backend accepts
      // true/false/1/0; frontend standardizes on "true".
      const url = forceRefresh ? '/system/version?refresh=true' : '/system/version';
      const res = await api.get<unknown, VersionInfo>(url);
      setVersionInfo(res);
    } catch (err: unknown) {
      // axios errors expose the response on err.response (AxiosError); other
      // throws just have a message. Read whatever is available and surface it
      // so the user can diagnose (was hardcoded "Network error" which always
      // showed even when the panel reached GitHub but a different layer
      // failed).
      const e = err as {
        response?: { data?: { error_message?: string; message?: string } };
        message?: string;
      };
      const serverMsg =
        e?.response?.data?.error_message ||
        e?.response?.data?.message ||
        e?.message;
      setVersionInfo({
        current_version: versionInfo?.current_version || '',
        latest_version: versionInfo?.latest_version || '',
        has_update: false,
        is_outdated: false,
        release_url: '',
        release_notes: '',
        published_at: '',
        check_failed: true,
        error_message: serverMsg || 'Unknown error',
      });
    } finally {
      if (forceRefresh) setVersionRefreshing(false);
    }
  };

  useEffect(() => {
    load();
    checkVersion();
    const ti = setInterval(() => {
      if (!document.hidden) void load();
    }, 10000);
    const tv = setInterval(() => {
      if (!document.hidden) void checkVersion();
    }, 1800000); // 30 min
    const onVis = () => {
      if (!document.hidden) void load();
    };
    document.addEventListener('visibilitychange', onVis);
    return () => {
      clearInterval(ti);
      clearInterval(tv);
      document.removeEventListener('visibilitychange', onVis);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const chartRules = useMemo(
    () => ruleList.map((r) => ({ id: r.id, name: r.name })),
    [ruleList],
  );
  const fmtVer = (v?: string) => (!v ? '' : v.startsWith('v') ? v : `v${v}`);

  // v0.4.17: collapse raw /nodes rows into one summary per group. The whole
  //  row is clickable → /nodes for per-node detail (kept there to avoid
  //  duplicating the node-status page on the dashboard).
  const groups = useMemo(() => aggregateNodesByGroup(nodes), [nodes]);
  // v1.3.3: the fleet's online/offline count for the summary above the KPI
  // cards. Placeholder rows (a group with no node reporting) carry no node_id
  // and are not machines; counting them would say "1 offline" forever on a
  // freshly created group.
  const fleet = useMemo(() => {
    const real = nodes.filter((n) => n.node_id);
    const online = real.filter((n) => n.online).length;
    return { online, total: real.length, offline: real.length - online };
  }, [nodes]);
  // The summary's clock: HH:MM in the operator's own timezone, because the page
  // is read next to a wall clock, not to a UTC log.
  const updatedLabel = useMemo(() => {
    if (!nodesUpdatedAt) return '';
    return `${String(nodesUpdatedAt.getHours()).padStart(2, '0')}:${String(nodesUpdatedAt.getMinutes()).padStart(2, '0')}`;
  }, [nodesUpdatedAt]);
  // v1.2.12: enabled / total rules per inbound group. Every node of a group
  // runs that group's rules, so this is also each machine's count when a
  // machine has its own group — and how a new machine is checked against the
  // old ones.
  const rulesByGroup = useMemo(() => {
    const m = new Map<number, { enabled: number; total: number }>();
    for (const r of ruleList) {
      const c = m.get(r.device_group_in) ?? { enabled: 0, total: 0 };
      c.total += 1;
      if (!r.paused) c.enabled += 1;
      m.set(r.device_group_in, c);
    }
    return m;
  }, [ruleList]);

  const statusColor = (s: string) =>
    s === 'online' ? 'green' : s === 'partial' ? 'orange' : 'red';
  const statusLabel = (s: string) =>
    s === 'online' ? t('groupStatusOnline')
      : s === 'partial' ? t('groupStatusPartial')
        : t('groupStatusOffline');

  const groupColumns = [
    {
      title: t('groupName'), key: 'group', width: 200,
      render: (_: unknown, r: ReturnType<typeof aggregateNodesByGroup>[number]) => (
        <Space size={6}>
          <Text strong>{r.group_name || '-'}</Text>
          <Text type="secondary" style={{ fontSize: 12 }}>ID: {r.group_id}</Text>
        </Space>
      ),
    },
    {
      title: t('status'), key: 'status', width: 110,
      render: (_: unknown, r: ReturnType<typeof aggregateNodesByGroup>[number]) => (
        <Tag color={statusColor(r.status)}>
          <span className={`rp-status-dot is-${r.status === 'online' ? 'online' : r.status === 'partial' ? 'partial' : 'offline'}`} />
          {statusLabel(r.status)}
        </Tag>
      ),
    },
    {
      title: t('groupNodes'), key: 'nodes', width: 90,
      render: (_: unknown, r: ReturnType<typeof aggregateNodesByGroup>[number]) => (
        <Tag color={r.online_nodes > 0 ? 'green' : undefined} style={{ marginInlineEnd: 0 }}>
          <span className="rp-mono">{r.online_nodes}/{r.total_nodes}</span>
        </Tag>
      ),
    },
    {
      title: t('groupRules'), key: 'rules', width: 100,
      render: (_: unknown, r: ReturnType<typeof aggregateNodesByGroup>[number]) => {
        const c = rulesByGroup.get(r.group_id) ?? { enabled: 0, total: 0 };
        return (
          <Tooltip title={t('groupRulesHint')}>
            <span className={`rp-count-pill${c.total === 0 ? ' is-zero' : ''}`}>{c.enabled}/{c.total}</span>
          </Tooltip>
        );
      },
    },
    {
      title: t('connections'), dataIndex: 'connections', key: 'connections', width: 90,
      render: (v: number, r: ReturnType<typeof aggregateNodesByGroup>[number]) => (
        <Tooltip title={connectionsTooltip({ connections: r.connections, tcp_connections: r.tcp_connections, udp_connections: r.udp_connections }, t)}>
          <span className={`rp-count-pill${!v ? ' is-zero' : ''}`}>{v}</span>
        </Tooltip>
      ),
    },
    {
      title: t('groupRate'), key: 'rate', width: 196,
      render: (_: unknown, r: ReturnType<typeof aggregateNodesByGroup>[number]) => (
        <span className="rp-mono" style={{ display: 'inline-flex', flexWrap: 'wrap', gap: 10 }}>
          <span className="rp-rate-up"><ArrowUpOutlined /> {formatBps(r.upload_bps)}</span>
          <span className="rp-rate-down"><ArrowDownOutlined /> {formatBps(r.download_bps)}</span>
        </span>
      ),
    },
    {
      title: t('groupTraffic'), key: 'traffic', width: 210,
      render: (_: unknown, r: ReturnType<typeof aggregateNodesByGroup>[number]) => (
        <Tooltip title={t('groupTrafficHint')}>
          <span className="rp-mono" style={{ display: 'inline-flex', flexWrap: 'wrap', gap: 10 }}>
            <span className="rp-rate-up"><ArrowUpOutlined /> {formatBytes(r.upload_bytes)}</span>
            <span className="rp-rate-down"><ArrowDownOutlined /> {formatBytes(r.download_bytes)}</span>
          </span>
        </Tooltip>
      ),
    },
  ];

  // Version banner logic
  const ignoredVersion = localStorage.getItem(IGNORED_KEY);
  const showUpdateBanner = versionInfo?.has_update
    && versionInfo.latest_version !== ignoredVersion;

  const handleIgnore = () => {
    if (versionInfo) {
      localStorage.setItem(IGNORED_KEY, versionInfo.latest_version);
      setVersionInfo({ ...versionInfo, has_update: false });
    }
  };

  const bannerMsg = versionInfo?.is_outdated
    ? t('versionOutdated').replace('{version}', versionInfo.latest_version)
    : t('newVersionFound').replace('{version}', versionInfo?.latest_version || '');

  return (
    <>
      {loadFailed && (
        <Alert
          type="error"
          showIcon
          style={{ marginBottom: 16 }}
          title={t('loadFailed')}
          description={t('loadFailedRetry')}
        />
      )}
      {versionInfo?.check_failed && (
        <Alert
          type="warning"
          showIcon
          style={{ marginBottom: 16 }}
          title={t('updateCheckFailed')}
          description={versionInfo.error_message || t('versionCheckFailed')}
        />
      )}
      {showUpdateBanner && (
        <Alert
          type={versionInfo?.is_outdated ? 'error' : 'info'}
          showIcon
          style={{ marginBottom: 16 }}
          title={bannerMsg}
          action={
            <Space>
              <Button size="small" onClick={handleIgnore}>{t('ignoreVersion')}</Button>
              <Button size="small" onClick={() => setShowChangelog(true)}>{t('viewChangelog')}</Button>
              <PanelUpdateButton
                currentVersion={versionInfo?.current_version || ''}
                targetVersion={versionInfo?.latest_version || ''}
                manualUrl={DEPLOY_DOC_URL}
              />
            </Space>
          }
        />
      )}
      <div className="rp-page-header">
        <h2 className="rp-page-title">
          <DashboardOutlined /> {t('dashboard')}
          {versionInfo && (
            <Space size={8} wrap style={{ fontSize: 13, fontWeight: 'normal' }}>
              <Tag style={{ marginInlineEnd: 0 }}>
                <span className="rp-mono">{fmtVer(versionInfo.current_version)}</span>
              </Tag>
              {!versionInfo.check_failed && !showUpdateBanner && !versionInfo.has_update && (
                <Tag color="green" style={{ marginInlineEnd: 0 }}>
                  <span className="rp-status-dot is-online" />
                  {t('upToDate')}
                </Tag>
              )}
              {versionInfo.has_update && (
                <Tag color="gold" style={{ marginInlineEnd: 0 }}>
                  {t('latestVersion')}: <span className="rp-mono">{fmtVer(versionInfo.latest_version)}</span>
                </Tag>
              )}
            </Space>
          )}
        </h2>
        <Tooltip title={t('checkUpdate')}>
          <Button
            icon={<ReloadOutlined />}
            loading={versionRefreshing}
            onClick={() => checkVersion(true)}
          >
            {t('checkUpdate')}
          </Button>
        </Tooltip>
      </div>
      {/* v1.3.3: a one-line fleet summary above the KPI cards — how many
          machines are up, how many are not, and how old the reading is. An
          admin who lands here after a notification should not have to open the
          node board to find out. */}
      {fleet.total > 0 && updatedLabel && (
        <div className="rp-fleet-summary" data-testid="fleet-summary" data-stale={nodesStale ? '1' : undefined}>
          <span className="rp-fleet-summary-online" data-testid="fleet-online">
            {t('onlineNodesOfTotal').replace('{online}', String(fleet.online)).replace('{total}', String(fleet.total))}
          </span>
          {fleet.offline > 0 && (
            <>
              <span className="rp-fleet-summary-sep" aria-hidden="true">·</span>
              <button
                type="button"
                className="rp-fleet-summary-offline"
                data-testid="fleet-offline"
                onClick={() => navigate('/nodes?status=offline')}
              >
                {t('nodeStatusSummaryOffline').replace('{n}', String(fleet.offline))}
              </button>
            </>
          )}
          <span className="rp-fleet-summary-sep" aria-hidden="true">·</span>
          <span className="rp-fleet-summary-time" data-testid="fleet-updated" title={nodesStale ? t('loadFailed') : undefined}>
            {(nodesStale
              ? t('nodeStatusSummaryStale')
              : t('nodeStatusSummaryUpdated')
            ).replace('{time}', updatedLabel)}
          </span>
        </div>
      )}
      <Row gutter={isMobile ? [8, 8] : [16, 16]} style={{ marginBottom: 20 }}>
        <Col xs={24} sm={8}>
          <Card className="rp-kpi-card" styles={{ body: { padding: '20px 22px' } }}>
            <div style={{ display: 'flex', justifyContent: 'space-between', alignItems: 'center' }}>
              <div>
                <div style={{ fontSize: 13, color: 'var(--rp-text-secondary)', marginBottom: 6 }}>{t('users')}</div>
                <div className="rp-mono" style={{ fontSize: 30, fontWeight: 600, color: 'var(--rp-text)', lineHeight: 1.15 }}>{stats.users}</div>
              </div>
              <span className="rp-icon-tile" style={{ background: '#f6f5fd', color: '#6c5dd8' }}>
                <UserOutlined />
              </span>
            </div>
          </Card>
        </Col>
        <Col xs={24} sm={8}>
          <Card className="rp-kpi-card" styles={{ body: { padding: '20px 22px' } }}>
            <div style={{ display: 'flex', justifyContent: 'space-between', alignItems: 'center' }}>
              <div>
                <div style={{ fontSize: 13, color: 'var(--rp-text-secondary)', marginBottom: 6 }}>{t('forwardRules')}</div>
                <div className="rp-mono" style={{ fontSize: 30, fontWeight: 600, color: 'var(--rp-text)', lineHeight: 1.15 }}>{stats.rules}</div>
              </div>
              <span className="rp-icon-tile" style={{ background: '#f0f9ff', color: '#0284c7' }}>
                <ApiOutlined />
              </span>
            </div>
          </Card>
        </Col>
        <Col xs={24} sm={8}>
          <Card className="rp-kpi-card" styles={{ body: { padding: '20px 22px' } }}>
            <div style={{ display: 'flex', justifyContent: 'space-between', alignItems: 'center' }}>
              <div>
                <div style={{ fontSize: 13, color: 'var(--rp-text-secondary)', marginBottom: 6 }}>{t('deviceGroups')}</div>
                <div className="rp-mono" style={{ fontSize: 30, fontWeight: 600, color: 'var(--rp-text)', lineHeight: 1.15 }}>{stats.groups}</div>
              </div>
              <span className="rp-icon-tile" style={{ background: '#ecfdf5', color: '#059669' }}>
                <ClusterOutlined />
              </span>
            </div>
          </Card>
        </Col>
      </Row>

      {/* v1.2.0: fleet-wide traffic trend, with per-rule drill-down. */}
      <TrafficChart rules={chartRules} />

      {/* v1.2.4: node CPU / memory / connections, directly below the traffic
          chart — the two answer "how much" and "was the box coping", which is
          the order you ask them in. Admin-only endpoint, and this page is
          already admin-only. */}
      <NodeMetricsChart />

      <Card title={t('nodeStatus')} extra={<Text type="secondary" style={{ fontSize: 12 }}>{t('autoRefresh10s')}</Text>}>
        {groups.length === 0
          ? <div style={{ textAlign: 'center', padding: '40px 0', color: 'var(--rp-text-tertiary)', fontSize: 13 }}>{t('noNodesReporting')}</div>
          : <Table
              dataSource={groups}
              columns={groupColumns}
              rowKey="group_id"
              pagination={false}
              size="small"
              scroll={tableScroll}
              onRow={() => ({ onClick: () => navigate('/nodes'), style: { cursor: 'pointer' } })}
            />
        }
      </Card>

      <Modal
        title={t('changelogTitle') + ' — ' + (versionInfo?.latest_version || '')}
        open={showChangelog}
        onCancel={() => setShowChangelog(false)}
        footer={[
          <Button key="github" href={versionInfo?.release_url} target="_blank" rel="noopener noreferrer">{t('openReleasePage')}</Button>,
          <Button key="close" type="primary" onClick={() => setShowChangelog(false)}>{t('cancel')}</Button>,
        ]}
      >
        <Text type="secondary" style={{ fontSize: 12 }}>
          {t('currentVersion')}: {fmtVer(versionInfo?.current_version)} · {versionInfo?.published_at}
        </Text>
        <pre style={{ whiteSpace: 'pre-wrap', fontSize: 13, marginTop: 12 }}>
          {versionInfo?.release_notes || 'No release notes.'}
        </pre>
      </Modal>
    </>
  );
}
