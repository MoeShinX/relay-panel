
import { Typography, Button, Space, Tag, Tooltip } from 'antd';
import { CloudDownloadOutlined, CheckCircleOutlined, CloudServerOutlined } from '@ant-design/icons';
import type { Tfn } from './types';
import type { NodeDisplayRow } from '../../api/types';
import { NodeResourceBar, DiskList, hasDiskList } from './NodeResourceBar';
import { NetworkCell, cpuTooltip, memTooltip, statusTag, addressSummary } from './shared';
import { formatBps, formatUptime } from '../../utils/format';
import { versionRelation, versionTagColor } from '../../utils/version';
import { resolveNodeUpgrade, type NodeUpgradeState } from './upgrade';
import { NodeLabelCell } from './NodeLabelCell';

const { Text } = Typography;

/** v1.3.3: the smallest tap target the page allows for an entry point. */
const MIN_TAP = 44;

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
  /** Admin view: show each node's version and upgrade state. */
  showUpgrade?: boolean;
  /** v1.2.21: admin-only. Opens the node's latency page (see NetworkCell). */
  onOpenLatency?: (row: NodeDisplayRow) => void;
}

/** Mobile-friendly compact list — one card per node. No wide table, no
 *  horizontal scroll.
 *
 *  v1.3.3: the card now leads with what the page is for — the machine's name,
 *  whether it is up, and its live upload/download rate. Version, upgrade state
 *  and the detail entry sit together underneath, and every tappable thing meets
 *  the 44×44px minimum. */
export function NodeMobileList({ rows, panelProtocol, latestNodeVersion = '', nodeVersionCheckFailed = false, t, openDetail, showUpgrade, onOpenLatency }: Props) {
  const labels = { d: t('uptimeDay'), h: t('uptimeHour'), m: t('uptimeMinute'), s: t('uptimeSecond') };

  return (
    <Space orientation="vertical" style={{ width: '100%' }} size={8}>
      {rows.map((r) => {
        const isPlaceholder = !r.node_id;
        // v1.2 (PR4+PR5): the upgrade affordance mirrors the desktop ladder
        // (PR4) and compares against the latest NODE release (PR5), via the
        // shared resolveNodeUpgrade helper.
        const { state: upgradeState } = showUpgrade
          ? resolveNodeUpgrade(r, latestNodeVersion, panelProtocol, nodeVersionCheckFailed)
          : { state: 'none' as NodeUpgradeState };
        const addr = addressSummary(r).text;
        return (
          <div
            key={`${r.group_id}:${r.node_id || 'none'}`}
            // v1.2.5: offline cards are greyed, same signal as the desktop
            // table's offline rows — every figure on the card is the node's
            // last report rather than a live reading.
            className={r.online ? undefined : 'rp-node-offline-card'}
            style={{ border: '1px solid var(--rp-border)', borderRadius: 10, padding: 12, background: r.online ? '#ffffff' : undefined }}
          >
            {/* Identity: the name (or the address when unnamed) and the status
                tag share the first line, so "which machine, is it up" is the
                first thing read. */}
            <div style={{ display: 'flex', justifyContent: 'space-between', alignItems: 'flex-start', gap: 8, marginBottom: 4 }}>
              <div style={{ minWidth: 0, flex: 1 }}>
                {!isPlaceholder && r.label ? (
                  <div style={{ fontWeight: 600, fontSize: 15, lineHeight: 1.3 }}>
                    <NodeLabelCell row={r} t={t} wide />
                  </div>
                ) : (
                  <div className="rp-mono" style={{ fontSize: 13.5, fontWeight: 500, color: 'var(--rp-text)' }}>
                    {addr || (isPlaceholder ? '-' : r.node_id)}
                  </div>
                )}
                {!isPlaceholder && r.label && addr ? (
                  <div className="rp-mono rp-node-identity-addr" style={{ marginTop: 2 }}>{addr}</div>
                ) : null}
              </div>
              {statusTag(r, t, panelProtocol)}
            </div>

            {/* The live rate leads the body: it is what an operator opens this
                page to see. */}
            <div className="rp-node-mobile-rate">
              <span className="rp-node-rate-line rp-mono">
                <span style={{ color: 'var(--rp-text-tertiary)', fontSize: 11, marginInlineEnd: 4 }}>↑</span>
                {formatBps(r.upload_bps)}
              </span>
              <span className="rp-node-rate-line rp-mono" data-direction="down">
                <span style={{ color: 'var(--rp-text-tertiary)', fontSize: 11, marginInlineEnd: 4 }}>↓</span>
                {formatBps(r.download_bps)}
              </span>
            </div>

            <NetworkCell row={r} t={t} onOpenLatency={onOpenLatency ? () => onOpenLatency(r) : undefined} />

            {/* Version / upgrade / detail. Each entry point is at least 44×44px
                (see MIN_TAP), including the detail link that used to be a
                small inline button. */}
            <div style={{ display: 'flex', alignItems: 'center', gap: 6, flexWrap: 'wrap', marginTop: 4 }}>
              {showUpgrade && r.node_version ? (
                <Tag color={nodeVersionCheckFailed ? undefined : versionTagColor(versionRelation(r.node_version, latestNodeVersion))} className="rp-mono">
                  {`v${r.node_version}`}
                </Tag>
              ) : null}
              {showUpgrade ? (
                <MobileUpgradeAffordance
                  state={upgradeState}
                  latestNodeVersion={latestNodeVersion}
                  t={t}
                />
              ) : null}
              <Button
                type="link"
                disabled={isPlaceholder}
                onClick={() => openDetail(r)}
                aria-label={t('resourceDetails')}
                style={{ minHeight: MIN_TAP, minWidth: MIN_TAP, paddingInline: 8 }}
              >
                {t('resourceDetails')}
              </Button>
              <span className="rp-node-meta" style={{ fontSize: 12, marginInlineStart: 'auto' }}>
                {t('systemUptime')}: {formatUptime(r.uptime, labels)}
              </span>
            </div>

            <div style={{ display: 'flex', gap: 12, marginTop: 6 }}>
              <div style={{ flex: 1 }}>
                <Text type="secondary" style={{ fontSize: 11 }}>CPU</Text>
                <NodeResourceBar value={r.cpu} tooltip={cpuTooltip(r, t)} />
              </div>
              <div style={{ flex: 1 }}>
                <Text type="secondary" style={{ fontSize: 11 }}>{t('mem')}</Text>
                <NodeResourceBar value={r.mem} tooltip={memTooltip(r, t)} />
              </div>
              <div style={{ flex: 1 }}>
                <Text type="secondary" style={{ fontSize: 11 }}>{t('disk')}</Text>
                <NodeResourceBar
                  value={r.disk_usage_percent}
                  tooltip={hasDiskList(r.disks) ? <DiskList disks={r.disks} /> : `${t('disk')}: ${r.disk_usage_percent ?? '-'}%`}
                />
              </div>
            </div>
          </div>
        );
      })}
    </Space>
  );
}

/** The mobile upgrade state for one node — mirrors the desktop upgrade
 *  column's icon/tooltip ladder (read-only: upgrading is done from the group
 *  and page buttons). */
function MobileUpgradeAffordance({
  state,
  latestNodeVersion,
  t,
}: {
  state: NodeUpgradeState;
  latestNodeVersion: string;
  t: Tfn;
}) {
  // v1.3.3: 44×44 minimum tap target (was 32×32, under the mobile guidance).
  const tapStyle: React.CSSProperties = { minWidth: 44, minHeight: 44, display: 'inline-flex', alignItems: 'center', justifyContent: 'center', padding: 0 };
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
    case 'ahead':
      // leading build (node newer than the latest node release): green check
      // with a "leading build" tooltip, never an upgrade/downgrade offer.
      return (
        <Tooltip title={t('nodeVersionAhead')}>
          <span style={tapStyle} aria-label={t('nodeVersionAhead')} role="img">
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
          <span style={tapStyle} aria-label={t('nodeUpgradeTip').replace('{v}', latestNodeVersion)} role="img">
            <CloudDownloadOutlined style={{ color: 'var(--ant-color-primary, #18181b)' }} />
          </span>
        </Tooltip>
      );
    case 'noControlChannel':
      return (
        <Tooltip title={t('nodeUpgradeNoWs')}>
          <span style={tapStyle} aria-label={t('nodeUpgradeNoWs')} role="img">
            <CloudDownloadOutlined style={{ color: '#bfbfbf' }} />
          </span>
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
