 
import { Table, Tag, Typography, Button, Tooltip } from 'antd';
import { CloudDownloadOutlined, CheckCircleOutlined, CloudServerOutlined } from '@ant-design/icons';
import type { Tfn } from './types';
import type { NodeDisplayRow } from '../../api/types';
import { NodeResourceBar, NodeDiskBar } from './NodeResourceBar';
import { formatBps, formatBytes, formatUptime, formatPercent } from '../../utils/format';
import { versionRelation, versionTagColor } from '../../utils/version';
import { NetworkCell } from './shared';
import { resolveNodeUpgrade } from './upgrade';

interface Props {
  rows: NodeDisplayRow[];
  panelProtocol: number;
  /** v1.2: the latest NODE release (bare, e.g. "1.1.0"). Nodes compare their
   *  own version against this — NOT the panel version. Empty when unknown. */
  latestNodeVersion: string;
  /** v1.2: the node-version lookup failed; show an unknown state. */
  nodeVersionCheckFailed: boolean;
  t: Tfn;
  openDetail: (row: NodeDisplayRow) => void;
  /** v1.0.10: admin-only. When set, a "节点更新" column shows a per-node upgrade
   *  icon (active when the node is behind the latest node release). Absent for
   *  the regular-user view. */
  onUpgrade?: (row: NodeDisplayRow) => void;
}

/** Desktop table for one group's nodes. Both admin and user share the same
 *  columns — the permission difference is in the data source (admin reads
 *  /nodes, user reads /nodes/shared) and the detail drawer. */
export function NodeDesktopTable({ rows, panelProtocol, latestNodeVersion, nodeVersionCheckFailed, t, openDetail, onUpgrade }: Props) {
  const labels = { d: t('uptimeDay'), h: t('uptimeHour'), m: t('uptimeMinute'), s: t('uptimeSecond') };

  const columns = [
    {
      title: t('status'), key: 'status', width: 84, fixed: 'left' as const,
      render: (_: unknown, r: NodeDisplayRow) => {
        const v = r.config_protocol_version;
        if (v != null && panelProtocol > 0 && v !== panelProtocol) {
          return <Tag color="red">{t('protocolIncompatible')}</Tag>;
        }
        return r.online ? <Tag color="green">{t('online')}</Tag> : <Tag>{t('offline')}</Tag>;
      },
    },
    // v1.0.10: node version moved forward, with the upgrade action right after it.
    // v1.2: compared against the latest NODE release (latestNodeVersion), not
    // the panel version.
    {
      title: t('nodeVersion'), dataIndex: 'node_version', key: 'node_version', width: 100,
      render: (v: string | null) => {
        if (!v) return <Typography.Text type="secondary">-</Typography.Text>;
        // v1.2: if the node-version check failed, we can't vouch for any
        // "behind/latest" colouring — show the bare version with no arrow.
        if (nodeVersionCheckFailed) return <Tag>{`v${v}`}</Tag>;
        const rel = versionRelation(v, latestNodeVersion);
        const label = rel === 'behind' ? `v${v} ↑` : `v${v}`;
        return <Tag color={versionTagColor(rel)}>{label}</Tag>;
      },
    },
    // v1.0.10: admin-only per-node upgrade icon. v1.2 (PR4+PR5 reconciled): the
    // eligibility ladder is shared with the mobile list via resolveNodeUpgrade
    // (PR4) AND compared against the latest NODE release with a failed-check
    // neutral state (PR5). Protocol-incompatible and a failed lookup both take
    // priority over version status.
    ...(onUpgrade ? [{
      title: t('nodeUpgrade'), key: 'upgrade', width: 72,
      render: (_: unknown, r: NodeDisplayRow) => {
        const { state } = resolveNodeUpgrade(r, latestNodeVersion, panelProtocol, nodeVersionCheckFailed);
        switch (state) {
          case 'none':
          case 'checkFailed':
          case 'unknown':
            return <Typography.Text type="secondary">-</Typography.Text>;
          case 'latest':
            return <Tooltip title={t('nodeUpgradeLatest')}><CheckCircleOutlined style={{ color: '#52c41a' }} /></Tooltip>;
          case 'ahead':
            return <Tooltip title={t('nodeVersionAhead')}><CheckCircleOutlined style={{ color: '#52c41a' }} /></Tooltip>;
          case 'docker':
            return <Tooltip title={t('nodeUpgradeDocker')}><CloudServerOutlined style={{ color: '#faad14' }} /></Tooltip>;
          case 'manual':
            return <Tooltip title={t('nodeUpgradeManual')}><CloudDownloadOutlined style={{ color: '#bfbfbf' }} /></Tooltip>;
          case 'protocolIncompatible':
            return <Tag color="red">{t('protocolIncompatible')}</Tag>;
          case 'upgradeable':
            return (
              <Tooltip title={t('nodeUpgradeTip').replace('{v}', latestNodeVersion)}>
                <Button
                  size="small"
                  type="link"
                  icon={<CloudDownloadOutlined />}
                  aria-label={t('nodeUpgrade')}
                  onClick={() => onUpgrade(r)}
                />
              </Tooltip>
            );
          case 'offline':
          default:
            return <Tooltip title={t('offline')}><CloudDownloadOutlined style={{ color: '#bfbfbf' }} /></Tooltip>;
        }
      },
    }] : []),
    {
      title: t('network'), key: 'network', width: 300,
      render: (_: unknown, r: NodeDisplayRow) => <NetworkCell row={r} t={t} />,
    },
    {
      title: t('connections'), dataIndex: 'connections', key: 'connections', width: 70,
      render: (v: number) => <span className="rp-mono">{v || 0}</span>,
    },
    {
      title: 'CPU', key: 'cpu', width: 84,
      render: (_: unknown, r: NodeDisplayRow) => <NodeResourceBar value={r.cpu} tooltip={`CPU: ${formatPercent(r.cpu)}`} />,
    },
    {
      title: t('mem'), key: 'mem', width: 100,
      render: (_: unknown, r: NodeDisplayRow) => <NodeResourceBar value={r.mem} tooltip={`${t('mem')}: ${formatPercent(r.mem)}`} />,
    },
    {
      title: t('disk'), key: 'disk', width: 100,
      render: (_: unknown, r: NodeDisplayRow) => <NodeDiskBar usagePercent={r.disk_usage_percent} used={r.disk_used} total={r.disk_total} mount={r.disk_mount} t={t} />,
    },
    {
      title: `${t('uploadRate')}/${t('downloadRate')}`, key: 'rate', width: 140,
      render: (_: unknown, r: NodeDisplayRow) => (
        <span className="rp-mono">{formatBps(r.upload_bps)} / {formatBps(r.download_bps)}</span>
      ),
    },
    {
      title: `${t('totalUpload')}/${t('totalDownload')}`, key: 'traffic', width: 150,
      render: (_: unknown, r: NodeDisplayRow) => (
        <span className="rp-mono">{formatBytes(r.boot_upload_bytes)} / {formatBytes(r.boot_download_bytes)}</span>
      ),
    },
    {
      title: t('systemUptime'), key: 'uptime', width: 90,
      render: (_: unknown, r: NodeDisplayRow) => <span className="rp-mono">{formatUptime(r.uptime, labels)}</span>,
    },
    {
      title: t('resourceDetails'), key: 'detail', width: 70, fixed: 'right' as const,
      render: (_: unknown, r: NodeDisplayRow) => (
        <Button size="small" type="link" onClick={() => openDetail(r)}>{t('resourceDetails')}</Button>
      ),
    },
  ];

  return (
    <Table
      dataSource={rows}
      columns={columns}
      rowKey={(r) => `${r.group_id}:${r.node_id || 'legacy'}`}
      pagination={false}
      size="small"
      scroll={{ x: 'max-content' }}
    />
  );
}
