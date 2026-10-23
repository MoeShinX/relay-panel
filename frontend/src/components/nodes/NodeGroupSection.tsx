 
import type { HTMLAttributes, ReactNode } from 'react';
import { Tag, Typography, Collapse } from 'antd';
import { ArrowUpOutlined, ArrowDownOutlined } from '@ant-design/icons';
import type { NodeDisplayRow } from '../../api/types';
import type { Tfn } from './types';
import { NodeDesktopTable } from './NodeDesktopTable';
import { NodeMobileList } from './NodeMobileList';
import { formatBps } from '../../utils/format';
import { rateSummary } from './shared';

const { Text } = Typography;

interface Props {
  rows: NodeDisplayRow[];
  panelProtocol: number;
  /** v1.2: the latest NODE release (bare, e.g. "1.1.0"). Nodes compare their
   *  own version against this — NOT the panel version. */
  latestNodeVersion: string;
  /** v1.2: the node-version lookup failed; tables show an unknown state. */
  nodeVersionCheckFailed: boolean;
  isMobile: boolean;
  t: Tfn;
  openDetail: (row: NodeDisplayRow) => void;
  /** v1.0.10: admin-only per-node upgrade trigger (desktop table only). */
  onUpgrade?: (row: NodeDisplayRow) => void;
  onDelete?: (row: NodeDisplayRow) => void;
  /** Admin-only drag handle for reordering the groups, shown at the start of
   *  the header. The caller supplies it (it needs the sortable context). */
  dragHandle?: ReactNode;
  /** Admin-only: spread on the whole header row so the group can be grabbed
   *  anywhere on it (pointer drag). A plain click still folds the group. */
  dragHeaderProps?: HTMLAttributes<HTMLDivElement>;
}

/** One group block: header bar (name · ID · online/total · aggregate ↑↓) +
 *  either a desktop table or mobile list. Collapsible. A group with only a
 *  placeholder row shows "no node reporting". */
export function NodeGroupSection({ rows, panelProtocol, latestNodeVersion, nodeVersionCheckFailed, isMobile, t, openDetail, onUpgrade, onDelete, dragHandle, dragHeaderProps }: Props) {
  const head = rows[0];
  const { total, online, up, down } = rateSummary(rows);
  const region = head.region;
  const lineType = head.line_type;
  const onlyPlaceholder = rows.length === 1 && !head.node_id;

  const header = (
    <div
      {...dragHeaderProps}
      style={{ display: 'flex', flexWrap: 'wrap', alignItems: 'center', gap: 8, ...dragHeaderProps?.style }}
    >
      {dragHandle}
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
      <NodeMobileList
        rows={rows}
        panelProtocol={panelProtocol}
        latestNodeVersion={latestNodeVersion}
        nodeVersionCheckFailed={nodeVersionCheckFailed}
        t={t}
        openDetail={openDetail}
        onUpgrade={onUpgrade}
      />
    </div>
  ) : (
    <NodeDesktopTable rows={rows} panelProtocol={panelProtocol} latestNodeVersion={latestNodeVersion} nodeVersionCheckFailed={nodeVersionCheckFailed} t={t} openDetail={openDetail} onUpgrade={onUpgrade} onDelete={onDelete} />
  );

  return (
    <Collapse
      defaultActiveKey={['1']}
      style={{ marginBottom: 16 }}
      items={[{ key: '1', label: header, children: body }]}
    />
  );
}
