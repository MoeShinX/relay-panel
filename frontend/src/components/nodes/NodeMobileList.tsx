 
import { Typography, Button, Space } from 'antd';
import type { Tfn } from './types';
import type { NodeDisplayRow } from '../../api/types';
import { NodeResourceBar } from './NodeResourceBar';
import { NetworkCell, statusTag } from './shared';
import { formatBps, formatUptime } from '../../utils/format';

const { Text } = Typography;

interface Props {
  rows: NodeDisplayRow[];
  panelProtocol: number;
  /** v1.2: the latest NODE release (bare, e.g. "1.1.0"). Nodes compare their
   *  own version against this — NOT the panel version. Empty when unknown. */
  latestNodeVersion?: string;
  /** v1.2: the node-version lookup failed. */
  nodeVersionCheckFailed?: boolean;
  t: Tfn;
  openDetail: (row: NodeDisplayRow) => void;
  /** v1.0.10/v1.2: admin-only per-node upgrade trigger. Accepted here for the
   *  mobile upgrade affordance (PR4); PR5 just threads it so the prop is
   *  available. Unused on mobile for now. */
  onUpgrade?: (row: NodeDisplayRow) => void;
}

/** Mobile-friendly compact list — one card per node. No wide table, no
 *  horizontal scroll. Shows: status + network + speed + resource bars +
 *  uptime + a details button. */
// v1.2: latestNodeVersion / nodeVersionCheckFailed / onUpgrade are accepted via
// Props (threaded by NodeGroupSection) for the mobile upgrade affordance, but
// the mobile upgrade UI itself is added in a later PR — they're intentionally
// not destructured here yet.
export function NodeMobileList({ rows, panelProtocol, t, openDetail }: Props) {
  const labels = { d: t('uptimeDay'), h: t('uptimeHour'), m: t('uptimeMinute'), s: t('uptimeSecond') };

  return (
    <Space direction="vertical" style={{ width: '100%' }} size={8}>
      {rows.map((r) => {
        const isPlaceholder = !r.node_id;
        return (
          <div
            key={`${r.group_id}:${r.node_id || 'none'}`}
            style={{ border: '1px solid #f0f0f0', borderRadius: 8, padding: 10 }}
          >
            <div style={{ display: 'flex', justifyContent: 'space-between', alignItems: 'center', marginBottom: 6 }}>
              {statusTag(r, t, panelProtocol)}
              <Button size="small" type="link" disabled={isPlaceholder} onClick={() => openDetail(r)}>
                {t('resourceDetails')}
              </Button>
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
