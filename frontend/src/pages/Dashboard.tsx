import { Card, Col, Row, Statistic, Table, Tag, Typography, Alert, Modal, Button, Space, Tooltip } from 'antd';
import {
  CloudServerOutlined, ApiOutlined, UserOutlined, DashboardOutlined, ReloadOutlined,
  ArrowUpOutlined, ArrowDownOutlined,
} from '@ant-design/icons';
import { useEffect, useMemo, useState } from 'react';
import { useNavigate } from 'react-router-dom';
import api from '../api/client';
import type { ApiEnvelope, User, ForwardRule, DeviceGroup, NodeStatus } from '../api/types';
import { useI18n } from '../i18n/context';
import { aggregateNodesByGroup } from '../components/nodes/aggregate';
import { formatBps, formatBytes } from '../utils/format';

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
  const [stats, setStats] = useState({ users: 0, rules: 0, groups: 0 });
  const [nodes, setNodes] = useState<NodeStatus[]>([]);
  const [versionInfo, setVersionInfo] = useState<VersionInfo | null>(null);
  const [showChangelog, setShowChangelog] = useState(false);
  const [versionRefreshing, setVersionRefreshing] = useState(false);

  const load = async () => {
    try {
      const [users, rules, groups] = await Promise.all([
        api.get<unknown, ApiEnvelope<User[]>>('/admin/users'),
        api.get<unknown, ApiEnvelope<ForwardRule[]>>('/rules'),
        api.get<unknown, ApiEnvelope<DeviceGroup[]>>('/groups'),
      ]);
      setStats({
        users: users.data?.length || 0,
        rules: rules.data?.length || 0,
        groups: groups.data?.length || 0,
      });
    } catch { /* ignore */ }

    try {
      const nodeStatus = await api.get<unknown, ApiEnvelope<NodeStatus[]>>('/nodes');
      setNodes(nodeStatus.data || []);
    } catch { /* ignore */ }
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
    const ti = setInterval(load, 10000);
    const tv = setInterval(checkVersion, 1800000); // 30 min
    return () => { clearInterval(ti); clearInterval(tv); };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // v0.4.17: collapse raw /nodes rows into one summary per group. The whole
  //  row is clickable → /nodes for per-node detail (kept there to avoid
  //  duplicating the node-status page on the dashboard).
  const groups = useMemo(() => aggregateNodesByGroup(nodes), [nodes]);

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
      title: t('status'), key: 'status', width: 100,
      render: (_: unknown, r: ReturnType<typeof aggregateNodesByGroup>[number]) => (
        <Tag color={statusColor(r.status)}>{statusLabel(r.status)}</Tag>
      ),
    },
    {
      title: t('groupNodes'), key: 'nodes', width: 90,
      render: (_: unknown, r: ReturnType<typeof aggregateNodesByGroup>[number]) => (
        <span className="rp-mono">{r.online_nodes}/{r.total_nodes}</span>
      ),
    },
    {
      title: t('connections'), dataIndex: 'connections', key: 'connections', width: 90,
      render: (v: number) => <span className="rp-mono">{v}</span>,
    },
    {
      title: t('groupRate'), key: 'rate', width: 180,
      render: (_: unknown, r: ReturnType<typeof aggregateNodesByGroup>[number]) => (
        <span className="rp-mono" style={{ display: 'inline-flex', flexWrap: 'wrap', gap: 8 }}>
          <Text type="secondary" style={{ fontSize: 12 }}>
            <ArrowUpOutlined /> {formatBps(r.upload_bps)}
          </Text>
          <Text type="secondary" style={{ fontSize: 12 }}>
            <ArrowDownOutlined /> {formatBps(r.download_bps)}
          </Text>
        </span>
      ),
    },
    {
      title: t('groupTraffic'), key: 'traffic', width: 200,
      render: (_: unknown, r: ReturnType<typeof aggregateNodesByGroup>[number]) => (
        <Tooltip title={t('groupTrafficHint')}>
          <span className="rp-mono" style={{ display: 'inline-flex', flexWrap: 'wrap', gap: 8 }}>
            <Text type="secondary" style={{ fontSize: 12 }}>
              <ArrowUpOutlined /> {formatBytes(r.upload_bytes)}
            </Text>
            <Text type="secondary" style={{ fontSize: 12 }}>
              <ArrowDownOutlined /> {formatBytes(r.download_bytes)}
            </Text>
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
              <Button size="small" type="primary" href={DEPLOY_DOC_URL} target="_blank">{t('updateNow')}</Button>
            </Space>
          }
        />
      )}
      {!versionInfo?.check_failed && !showUpdateBanner && versionInfo && (
        <Alert
          type="success"
          showIcon
          style={{ marginBottom: 16 }}
          title={`${t('currentVersion')}: v${versionInfo.current_version} · ${t('upToDate')}`}
        />
      )}

      <div className="rp-page-header">
        <h2 className="rp-page-title">
          <DashboardOutlined /> {t('dashboard')}
          {versionInfo && (
            <Space size="middle" style={{ marginLeft: 16, fontSize: 13, fontWeight: 'normal' }}>
              <Text type="secondary">
                {t('currentVersion')}: <span className="rp-mono">v{versionInfo.current_version}</span>
              </Text>
              {versionInfo.has_update && (
                <Text type="warning">
                  {t('latestVersion')}: <span className="rp-mono">{versionInfo.latest_version}</span>
                </Text>
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
      <Row gutter={16} style={{ marginBottom: 20 }}>
        <Col span={8}>
          <Card className="rp-stat-card"><Statistic title={t('users')} value={stats.users} prefix={<UserOutlined style={{ color: 'var(--rp-primary)' }} />} /></Card>
        </Col>
        <Col span={8}>
          <Card className="rp-stat-card"><Statistic title={t('forwardRules')} value={stats.rules} prefix={<ApiOutlined style={{ color: 'var(--rp-primary)' }} />} /></Card>
        </Col>
        <Col span={8}>
          <Card className="rp-stat-card"><Statistic title={t('deviceGroups')} value={stats.groups} prefix={<CloudServerOutlined style={{ color: 'var(--rp-primary)' }} />} /></Card>
        </Col>
      </Row>

      <Card title={t('nodeStatus')} extra={<Text type="secondary" style={{ fontSize: 12 }}>{t('autoRefresh10s')}</Text>}>
        {groups.length === 0
          ? <div style={{ textAlign: 'center', padding: '40px 0', color: 'var(--rp-text-tertiary)', fontSize: 13 }}>{t('noNodesReporting')}</div>
          : <Table
              dataSource={groups}
              columns={groupColumns}
              rowKey="group_id"
              pagination={false}
              size="small"
              onRow={() => ({ onClick: () => navigate('/nodes'), style: { cursor: 'pointer' } })}
            />
        }
      </Card>

      <Modal
        title={t('changelogTitle') + ' — ' + (versionInfo?.latest_version || '')}
        open={showChangelog}
        onCancel={() => setShowChangelog(false)}
        footer={[
          <Button key="github" href={versionInfo?.release_url} target="_blank">{t('openReleasePage')}</Button>,
          <Button key="close" type="primary" onClick={() => setShowChangelog(false)}>{t('cancel')}</Button>,
        ]}
      >
        <Text type="secondary" style={{ fontSize: 12 }}>
          {t('currentVersion')}: {versionInfo?.current_version} · {versionInfo?.published_at}
        </Text>
        <pre style={{ whiteSpace: 'pre-wrap', fontSize: 13, marginTop: 12 }}>
          {versionInfo?.release_notes || 'No release notes.'}
        </pre>
      </Modal>
    </>
  );
}
