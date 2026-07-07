
import { Typography, Button, Space, Tag, Tooltip } from 'antd';
import { CloudDownloadOutlined, CheckCircleOutlined, CloudServerOutlined } from '@ant-design/icons';
import type { Tfn } from './types';
import type { NodeDisplayRow } from '../../api/types';
import { NodeResourceBar } from './NodeResourceBar';
import { NetworkCell, statusTag } from './shared';
import { formatBps, formatUptime } from '../../utils/format';
import { versionRelation, versionTagColor } from '../../utils/version';
import { resolveNodeUpgrade, type NodeUpgradeState } from './upgrade';

const { Text } = Typography;

interface Props {
  rows: NodeDisplayRow[];
  panelProtocol: number;
  /** v1.2: the latest NODE release (bare, e.g. "1.1.0"). Nodes compare their
   *  own version against this — NOT the panel version. Empty when unknown. */
  latestNodeVersion?: string;
  /** v1.2: the node-version lookup failed; show a neutral state. */
  nodeVersionCheckFailed?: boolean;
  t: Tfn;
  openDetail: (row: NodeDisplayRow) => void;
  /** v1.0.10/v1.2: admin-only per-node upgrade trigger. When set, each card
   *  shows the node version + an upgrade affordance mirroring the desktop
   *  ladder (PR4), compared against the latest NODE release (PR5). */
  onUpgrade?: (row: NodeDisplayRow) => void;
}

/** Mobile-friendly compact list — one card per node. No wide table, no
 *  horizontal scroll. Shows: status + version + upgrade + network + speed +
 *  resource bars + uptime + a details button. */
export function NodeMobileList({ rows, panelProtocol, latestNodeVersion = '', nodeVersionCheckFailed = false, t, openDetail, onUpgrade }: Props) {
  const labels = { d: t('uptimeDay'), h: t('uptimeHour'), m: t('uptimeMinute'), s: t('uptimeSecond') };

  return (
    <Space orientation="vertical" style={{ width: '100%' }} size={8}>
      {rows.map((r) => {
        const isPlaceholder = !r.node_id;
        // v1.2 (PR4+PR5): the upgrade affordance mirrors the desktop ladder
        // (PR4) and compares against the latest NODE release (PR5), via the
        // shared resolveNodeUpgrade helper.
        const { state: upgradeState } = onUpgrade
          ? resolveNodeUpgrade(r, latestNodeVersion, panelProtocol, nodeVersionCheckFailed)
          : { state: 'none' as NodeUpgradeState };
        return (
          <div
            key={`${r.group_id}:${r.node_id || 'none'}`}
            style={{ border: '1px solid #f0f0f0', borderRadius: 8, padding: 10 }}
          >
            <div style={{ display: 'flex', justifyContent: 'space-between', alignItems: 'center', marginBottom: 6, flexWrap: 'wrap', gap: 4 }}>
              {statusTag(r, t, panelProtocol)}
              <span style={{ display: 'inline-flex', alignItems: 'center', gap: 6, flexWrap: 'wrap' }}>
                {/* node version tag (only when the admin upgrade view is on).
                    When the node-version check failed, render the bare version
                    with no behind-arrow (we can't vouch for any colouring). */}
                {onUpgrade && r.node_version ? (
                  <Tag color={nodeVersionCheckFailed ? undefined : versionTagColor(versionRelation(r.node_version, latestNodeVersion))} className="rp-mono">
                    {`v${r.node_version}`}
                  </Tag>
                ) : null}
                {/* upgrade affordance — same ladder as desktop */}
                {onUpgrade ? (
                  <MobileUpgradeAffordance
                    state={upgradeState}
                    latestNodeVersion={latestNodeVersion}
                    t={t}
                    onUpgrade={() => onUpgrade(r)}
                  />
                ) : null}
                <Button
                  size="small"
                  type="link"
                  disabled={isPlaceholder}
                  onClick={() => openDetail(r)}
                  aria-label={t('resourceDetails')}
                >
                  {t('resourceDetails')}
                </Button>
              </span>
            </div>
            <NetworkCell row={r} t={t} />
            <div style={{ fontSize: 12, color: '#888', marginTop: 4 }}>
              ↑ {formatBps(r.upload_bps)} ↓ {formatBps(r.download_bps)}
              {' · '}{t('systemUptime')}: {formatUptime(r.uptime, labels)}
            </div>
            <div style={{ display: 'flex', gap: 12, marginTop: 6 }}>
              <div style={{ flex: 1 }}>
                <Text type="secondary" style={{ fontSize: 11 }}>CPU</Text>
                <NodeResourceBar value={r.cpu} tooltip={`CPU: ${r.cpu ?? '-'}%`} />
              </div>
              <div style={{ flex: 1 }}>
                <Text type="secondary" style={{ fontSize: 11 }}>{t('mem')}</Text>
                <NodeResourceBar value={r.mem} tooltip={`${t('mem')}: ${r.mem ?? '-'}%`} />
              </div>
              <div style={{ flex: 1 }}>
                <Text type="secondary" style={{ fontSize: 11 }}>{t('disk')}</Text>
                <NodeResourceBar value={r.disk_usage_percent} tooltip={`${t('disk')}: ${r.disk_usage_percent ?? '-'}%`} />
              </div>
            </div>
          </div>
        );
      })}
    </Space>
  );
}

/** The mobile upgrade affordance for one node — mirrors the desktop upgrade
 *  column's icon/tooltip ladder, but as a tappable 32×32 target (mobile tap
 *  sizing). */
function MobileUpgradeAffordance({
  state,
  latestNodeVersion,
  t,
  onUpgrade,
}: {
  state: NodeUpgradeState;
  latestNodeVersion: string;
  t: Tfn;
  onUpgrade: () => void;
}) {
  // 32×32 minimum tap target (WCAG / mobile guidance).
  const tapStyle: React.CSSProperties = { minWidth: 32, minHeight: 32, display: 'inline-flex', alignItems: 'center', justifyContent: 'center', padding: 0 };
  switch (state) {
    case 'none':
    case 'checkFailed':
      // checkFailed → neutral "-" (a failed lookup must not show a green check
      // or an upgrade button). Use the dedicated message when available.
      return (
        <Tooltip title={state === 'checkFailed' ? t('nodeVersionCheckFailed') : undefined}>
          <Typography.Text type="secondary" aria-label={t('nodeUpgrade')}>-</Typography.Text>
        </Tooltip>
      );
    case 'unknown':
      return <Typography.Text type="secondary" aria-label={t('nodeUpgrade')}>-</Typography.Text>;
    case 'latest':
      return (
        <Tooltip title={t('nodeUpgradeLatest')}>
          <span style={tapStyle} aria-label={t('nodeUpgradeLatest')} role="img">
            <CheckCircleOutlined style={{ color: '#52c41a' }} />
          </span>
        </Tooltip>
      );
    case 'docker':
      return (
        <Tooltip title={t('nodeUpgradeDocker')}>
          <span style={tapStyle} aria-label={t('nodeUpgradeDocker')} role="img">
            <CloudServerOutlined style={{ color: '#faad14' }} />
          </span>
        </Tooltip>
      );
    case 'manual':
      return (
        <Tooltip title={t('nodeUpgradeManual')}>
          <span style={tapStyle} aria-label={t('nodeUpgradeManual')} role="img">
            <CloudDownloadOutlined style={{ color: '#bfbfbf' }} />
          </span>
        </Tooltip>
      );
    case 'protocolIncompatible':
      return <Tag color="red" aria-label={t('protocolIncompatible')}>{t('protocolIncompatible')}</Tag>;
    case 'upgradeable':
      return (
        <Tooltip title={t('nodeUpgradeTip').replace('{v}', latestNodeVersion)}>
          <Button
            size="small"
            type="link"
            icon={<CloudDownloadOutlined />}
            aria-label={t('nodeUpgrade')}
            style={tapStyle}
            onClick={onUpgrade}
          />
        </Tooltip>
      );
    case 'offline':
    default:
      return (
        <Tooltip title={t('offline')}>
          <span style={tapStyle} aria-label={t('offline')} role="img">
            <CloudDownloadOutlined style={{ color: '#bfbfbf' }} />
          </span>
        </Tooltip>
      );
  }
}
