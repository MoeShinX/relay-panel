 
import { Tag, Typography, Collapse } from 'antd';
import { ArrowUpOutlined, ArrowDownOutlined } from '@ant-design/icons';
import type { NodeDisplayRow } from '../../api/types';
import type { Tfn } from './types';
import { NodeDesktopTable } from './NodeDesktopTable';
import { NodeMobileList } from './NodeMobileList';
import { formatBps } from '../../utils/format';

const { Text } = Typography;

interface Props {
  rows: NodeDisplayRow[];
  panelProtocol: number;
  currentVersion: string;
  isMobile: boolean;
  t: Tfn;
  openDetail: (row: NodeDisplayRow) => void;
  /** v1.0.10: admin-only per-node upgrade trigger (desktop table only). */
  onUpgrade?: (row: NodeDisplayRow) => void;
}

/** Per-group summary: online/total (placeholders excluded) + aggregate live
 *  upload/download across ONLINE nodes only. */
function groupSummary(rows: NodeDisplayRow[]) {
  const real = rows.filter((r) => r.node_id);
  const onlineRows = real.filter((r) => r.online);
  return {
    total: real.length,
    online: onlineRows.length,
    up: onlineRows.reduce((s, r) => s + (r.upload_bps || 0), 0),
    down: onlineRows.reduce((s, r) => s + (r.download_bps || 0), 0),
  };
}

/** One group block: header bar (name · ID · online/total · aggregate ↑↓) +
 *  either a desktop table or mobile list. Collapsible. A group with only a
 *  placeholder row shows "no node reporting". */
export function NodeGroupSection({ rows, panelProtocol, currentVersion, isMobile, t, openDetail, onUpgrade }: Props) {
  const head = rows[0];
  const { total, online, up, down } = groupSummary(rows);
  const region = head.region;
  const lineType = head.line_type;
  const onlyPlaceholder = rows.length === 1 && !head.node_id;

  const header = (
    <div style={{ display: 'flex', flexWrap: 'wrap', alignItems: 'center', gap: 8 }}>
      <Text strong>{head.group_name || '-'}</Text>
      <Text type="secondary" style={{ fontSize: 12 }}>ID: {head.group_id}</Text>
      {region ? <Tag>{region}</Tag> : null}
      {lineType ? <Tag color="blue">{lineType}</Tag> : null}
      <Tag color={online > 0 ? 'green' : undefined}>{online}/{total}</Tag>
      <span style={{ marginLeft: 'auto' }} className="rp-mono">
        <Text type="secondary" style={{ fontSize: 12 }}>
          <ArrowUpOutlined /> {formatBps(up)} <ArrowDownOutlined /> {formatBps(down)}
        </Text>
      </span>
    </div>
  );

  const body = onlyPlaceholder ? (
    <div style={{ padding: 12 }}>
      <Text type="secondary">{t('noNodeReportingInGroup')}</Text>
    </div>
  ) : isMobile ? (
    <div style={{ padding: 8 }}>
      <NodeMobileList rows={rows} panelProtocol={panelProtocol} t={t} openDetail={openDetail} />
    </div>
  ) : (
    <NodeDesktopTable rows={rows} panelProtocol={panelProtocol} currentVersion={currentVersion} t={t} openDetail={openDetail} onUpgrade={onUpgrade} />
  );

  return (
    <Collapse
      defaultActiveKey={['1']}
      style={{ marginBottom: 16 }}
      items={[{ key: '1', label: header, children: body }]}
    />
  );
}
