 
import { Tag, Typography, Collapse } from 'antd';
import { ArrowUpOutlined, ArrowDownOutlined } from '@ant-design/icons';
import type { NodeDisplayRow } from '../../api/types';
import type { Tfn } from './types';
import { NodeDesktopTable } from './NodeDesktopTable';
import { NodeMobileList } from './NodeMobileList';
import { formatBps } from '../../utils/format';
import { rateSummary } from './shared';
import { upgradableNodes } from './upgrade';
import { UpgradeAllButton } from './UpgradeAllButton';

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
  /** v1.2.16: admin-only. When set, the header shows "upgrade this group (N)"
   *  for the group's upgradable nodes; called after the batch is dispatched so
   *  the page can refresh. Absent for the regular-user view. */
  onBatchUpgraded?: () => void;
}

/** One group block: header bar (name · ID · online/total · aggregate ↑↓) +
 *  either a desktop table or mobile list. Collapsible. A group with only a
 *  placeholder row shows "no node reporting". */
export function NodeGroupSection({ rows, panelProtocol, latestNodeVersion, nodeVersionCheckFailed, isMobile, t, openDetail, onUpgrade, onDelete, onBatchUpgraded }: Props) {
  const head = rows[0];
  const { total, online, up, down } = rateSummary(rows);
  const region = head.region;
  const lineType = head.line_type;
  const onlyPlaceholder = rows.length === 1 && !head.node_id;
  // Same eligibility as the per-node button (see upgrade.ts), counted per group.
  const upgradable = onBatchUpgraded
    ? upgradableNodes(rows, latestNodeVersion, panelProtocol, nodeVersionCheckFailed)
    : [];

  const header = (
    <div style={{ display: 'flex', flexWrap: 'wrap', alignItems: 'center', gap: 8 }}>
      <Text strong>{head.group_name || '-'}</Text>
      <Text type="secondary" style={{ fontSize: 12 }}>ID: {head.group_id}</Text>
      {region ? <Tag>{region}</Tag> : null}
      {lineType ? <Tag color="blue">{lineType}</Tag> : null}
      <Tag color={online > 0 ? 'green' : undefined}>{online}/{total}</Tag>
      <span style={{ marginLeft: 'auto', display: 'inline-flex', flexWrap: 'wrap', alignItems: 'center', gap: 8 }}>
        <UpgradeAllButton
          nodes={upgradable}
          latestNodeVersion={latestNodeVersion}
          labelKey="upgradeGroup"
          t={t}
          onDone={onBatchUpgraded}
          testId={`upgrade-group-${head.group_id}`}
        />
        <span className="rp-mono">
          <Text type="secondary" style={{ fontSize: 12 }}>
            <ArrowUpOutlined /> {formatBps(up)} <ArrowDownOutlined /> {formatBps(down)}
          </Text>
        </span>
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
