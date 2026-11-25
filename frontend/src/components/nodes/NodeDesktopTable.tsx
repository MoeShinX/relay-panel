 
import { Table, Tag, Typography, Button, Tooltip, Popconfirm } from 'antd';
import { CloudDownloadOutlined, CheckCircleOutlined, CloudServerOutlined, DeleteOutlined } from '@ant-design/icons';
import type { Tfn } from './types';
import type { NodeDisplayRow } from '../../api/types';
import { NodeResourceBar, NodeDiskBar } from './NodeResourceBar';
import { formatBps, formatBytes, formatUptime } from '../../utils/format';
import { versionRelation, versionTagColor } from '../../utils/version';
import { NetworkCell, cpuTooltip, memTooltip, wsDownTag, addressSummary } from './shared';
import { resolveNodeUpgrade } from './upgrade';
import { nodeDesktopColumnWidths } from './tableLayout';
import { UPGRADE_COL_CLASS } from './upgradeAlign';
import { NodeLabelCell } from './NodeLabelCell';

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
  /** v1.0.10: admin-only. When set, the last column also offers a delete icon
   *  on offline rows. Absent for the regular-user view. */
  showUpgrade?: boolean;
  /** v1.2.21: admin-only. Opens the node's latency page (see NetworkCell). */
  onOpenLatency?: (row: NodeDisplayRow) => void;
  /** v1.2.5: admin-only. Removes this node's status record. Offered ONLY for
   *  offline rows, inside the single right-fixed column. */
  onDelete?: (row: NodeDisplayRow) => void;
}

/** Desktop table for one group's nodes. Both admin and user share the same
 *  columns — the permission difference is in the data source (admin reads
 *  /nodes, user reads /nodes/shared) and the detail drawer. */
export function NodeDesktopTable({ rows, panelProtocol, latestNodeVersion, nodeVersionCheckFailed, t, openDetail, showUpgrade, onDelete, onOpenLatency }: Props) {
  const labels = { d: t('uptimeDay'), h: t('uptimeHour'), m: t('uptimeMinute'), s: t('uptimeSecond') };
  // v1.3.3: the delete action lives inside the detail column, so the table has
  // exactly one right-fixed column. Only an offline row offers it, and only when
  // the caller passed onDelete (admin view).
  const canRemove = !!onDelete;

  const columns = [
    {
      // v1.3.3: status stays first and left-fixed — it is the one column a
      // reader looks at before anything else.
      title: t('status'), key: 'status', width: nodeDesktopColumnWidths.status, fixed: 'left' as const,
      render: (_: unknown, r: NodeDisplayRow) => {
        const v = r.config_protocol_version;
        if (v != null && panelProtocol > 0 && v !== panelProtocol) {
          return <Tag color="red">{t('protocolIncompatible')}</Tag>;
        }
        return r.online ? (
          <>
            <Tag color="green"><span className="rp-status-dot is-online" aria-hidden="true" />{t('online')}</Tag>
            {wsDownTag(r, t)}
          </>
        ) : (
          <Tag><span className="rp-status-dot is-offline" aria-hidden="true" />{t('offline')}</Tag>
        );
      },
    },
    {
      // v1.3.3: the node's identity column — the admin's name over a one-line
      // address. It used to be two separate columns (a conditional "name" and a
      // 300px network cell) and the reader had to pair them by eye. Long IPv6
      // addresses are truncated here; the full addresses are in the drawer.
      title: t('nodeColumn'), key: 'node', width: nodeDesktopColumnWidths.node,
      render: (_: unknown, r: NodeDisplayRow) => {
        const addr = addressSummary(r);
        // An unnamed node has no identity to print above the address, so the
        // address IS the identity: one line, not a "-" followed by the same
        // information. A named node shows the name first and the address under
        // it, which is the pair the reader needs to tell two machines apart.
        if (!r.label) {
          return addr.text ? (
            <span className="rp-node-identity-addr rp-mono" title={addr.full} style={{ fontSize: 12.5, color: 'var(--rp-text)' }}>
              {addr.text}{addr.extra > 0 ? ` +${addr.extra}` : ''}
            </span>
          ) : <NodeLabelCell row={r} t={t} />;
        }
        return (
          <div className="rp-node-identity">
            <NodeLabelCell row={r} t={t} />
            {addr.text ? (
              <span className="rp-node-identity-addr rp-mono" title={addr.full}>
                {addr.text}{addr.extra > 0 ? ` +${addr.extra}` : ''}
              </span>
            ) : null}
          </div>
        );
      },
    },
    {
      // v1.3.3: live rate right after the identity block — this is the figure
      // the page exists for and it used to sit behind a horizontal scroll.
      // Upload and download are separate right-aligned lines so the numbers line
      // up digit for digit; each value+unit stays on one line.
      title: `${t('uploadRate')} / ${t('downloadRate')}`, key: 'rate', width: nodeDesktopColumnWidths.rate,
      render: (_: unknown, r: NodeDisplayRow) => (
        <div className="rp-node-rates">
          <span className="rp-node-rate-line rp-mono rp-node-throughput">{formatBps(r.upload_bps)}</span>
          <span className="rp-node-rate-line rp-mono rp-node-throughput" data-direction="down">{formatBps(r.download_bps)}</span>
        </div>
      ),
    },
    {
      title: t('connections'), dataIndex: 'connections', key: 'connections', width: nodeDesktopColumnWidths.connections,
      render: (v: number) => <span className={`rp-count-pill${!v ? ' is-zero' : ''}`}>{v || 0}</span>,
    },
    {
      title: 'CPU', key: 'cpu', width: nodeDesktopColumnWidths.cpu,
      render: (_: unknown, r: NodeDisplayRow) => <NodeResourceBar value={r.cpu} tooltip={cpuTooltip(r, t)} />,
    },
    {
      title: t('mem'), key: 'mem', width: nodeDesktopColumnWidths.mem,
      render: (_: unknown, r: NodeDisplayRow) => <NodeResourceBar value={r.mem} tooltip={memTooltip(r, t)} />,
    },
    {
      // v1.3.3: the full dual-stack cell (country flags, both addresses, the
      // latency entry point) keeps its job; it just moved after the resources.
      title: t('network'), key: 'network', width: nodeDesktopColumnWidths.network,
      render: (_: unknown, r: NodeDisplayRow) => <NetworkCell row={r} t={t} onOpenLatency={onOpenLatency ? () => onOpenLatency(r) : undefined} />,
    },
    {
      // v1.0.10: node version, compared against the latest NODE release.
      title: t('nodeVersion'), dataIndex: 'node_version', key: 'node_version', width: nodeDesktopColumnWidths.nodeVersion,
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
    // Admin-only upgrade column: a read-only state per node (upgrading is done
    // from the group and page buttons). v1.2 (PR4+PR5 reconciled): the
    // eligibility ladder is shared with the mobile list via resolveNodeUpgrade
    // (PR4) AND compared against the latest NODE release with a failed-check
    // neutral state (PR5). Protocol-incompatible and a failed lookup both take
    // priority over version status.
    ...(showUpgrade ? [{
      title: t('nodeUpgrade'), key: 'upgrade', width: nodeDesktopColumnWidths.nodeUpgrade,
      // Centred, so the icon's centre is the column's centre: the upgrade-all
      // and upgrade-group icons above are lined up on it (see UpgradeSlot).
      align: 'center' as const, className: UPGRADE_COL_CLASS,
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
            // Behind and upgradable: only a marker. The upgrade itself is the
            // group / "upgrade all" button, which counts this node.
            return (
              <Tooltip title={t('nodeUpgradeTip').replace('{v}', latestNodeVersion)}>
                <span role="img" aria-label={t('nodeUpgradeTip').replace('{v}', latestNodeVersion)}>
                  <CloudDownloadOutlined style={{ color: 'var(--ant-color-primary, #18181b)' }} />
                </span>
              </Tooltip>
            );
          case 'noControlChannel':
            return <Tooltip title={t('nodeUpgradeNoWs')}><CloudDownloadOutlined style={{ color: '#bfbfbf' }} /></Tooltip>;
          case 'offline':
          default:
            return <Tooltip title={t('offline')}><CloudDownloadOutlined style={{ color: '#bfbfbf' }} /></Tooltip>;
        }
      },
    }] : []),
    {
      title: t('disk'), key: 'disk', width: nodeDesktopColumnWidths.disk,
      render: (_: unknown, r: NodeDisplayRow) => <NodeDiskBar usagePercent={r.disk_usage_percent} used={r.disk_used} total={r.disk_total} mount={r.disk_mount} disks={r.disks} t={t} />,
    },
    {
      title: `${t('totalUpload')}/${t('totalDownload')}`, key: 'traffic', width: nodeDesktopColumnWidths.traffic,
      render: (_: unknown, r: NodeDisplayRow) => (
        <span className="rp-mono rp-node-throughput" style={{ color: '#52525b' }}>
          {formatBytes(r.boot_upload_bytes)} / {formatBytes(r.boot_download_bytes)}
        </span>
      ),
    },
    {
      title: t('systemUptime'), key: 'uptime', width: nodeDesktopColumnWidths.uptime,
      render: (_: unknown, r: NodeDisplayRow) => <span className="rp-mono rp-node-uptime">{formatUptime(r.uptime, labels)}</span>,
    },
    {
      // v1.3.3: the detail link and (for an admin, on an offline row) the delete
      // icon share ONE right-fixed column. They used to be two fixed columns,
      // 142px in total, and "系统运行时长" disappeared under them at 1440px.
      // Online rows show no delete affordance at all — deleting an online node's
      // record achieves nothing, the next report recreates it.
      title: t('resourceDetails'), key: 'detail', width: nodeDesktopColumnWidths.detail, fixed: 'right' as const,
      render: (_: unknown, r: NodeDisplayRow) => (
        <span className="rp-node-detail-actions">
          <Button size="small" type="link" onClick={() => openDetail(r)}>{t('resourceDetails')}</Button>
          {canRemove && !r.online ? (
            <Popconfirm
              title={t('removeNodeTitle')}
              description={t('removeNodeHint')}
              okText={t('confirmRemoveNode')}
              cancelText={t('cancel')}
              okButtonProps={{ danger: true }}
              onConfirm={() => onDelete?.(r)}
            >
              <Button
                size="small" type="text" danger
                icon={<DeleteOutlined />}
                aria-label={t('removeNodeTitle')}
                title={t('removeNodeTitle')}
              />
            </Popconfirm>
          ) : null}
        </span>
      ),
    },
  ];

  return (
    <Table
      dataSource={rows}
      columns={columns}
      rowKey={(r) => `${r.group_id}:${r.node_id || 'legacy'}`}
      // v1.2.5: offline rows are greyed (see .rp-node-offline in theme.css).
      // Their numbers are the last report, not a live reading — and since these
      // rows stay listed until an admin removes them, telling them apart at a
      // glance is what keeps the list readable.
      rowClassName={(r) => (r.online ? '' : 'rp-node-offline')}
      pagination={false}
      size="small"
      scroll={{ x: 'max-content' }}
    />
  );
}
