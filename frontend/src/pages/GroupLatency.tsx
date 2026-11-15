import { useCallback, useEffect, useMemo, useRef, useState, type HTMLAttributes, type KeyboardEvent } from 'react';
import { Button, Card, Empty, Result, Space, Spin, Table, Tag, Tooltip, Typography } from 'antd';
import { ArrowLeftOutlined, DownloadOutlined, UndoOutlined } from '@ant-design/icons';
import { useNavigate, useParams } from 'react-router-dom';
import api from '../api/client';
import type {
  ApiEnvelope,
  GroupLatencyCell,
  GroupLatencyNode,
  GroupLatencyResponse,
  GroupLatencySeries,
  LatencyTarget,
} from '../api/types';
import { useI18n } from '../i18n/context';
import LatencyChart, { type LatencyChartHandle } from '../components/LatencyChart';
import LatencyControls from '../components/LatencyControls';
import LatencyStrips, { type StripRow } from '../components/LatencyStrips';
import {
  AUTO_REFRESH_MS,
  LEVEL_COLORS,
  autoRefreshes,
  granularityAllowed,
  latencyLevel,
  type LatencyGranularity,
  type LatencyRange,
} from '../utils/latency';
import {
  formatLoss,
  formatMs,
  groupChartPoints,
  nodeDisplayName,
  type ChartPoint,
} from '../utils/latencyChart';

const { Text, Title } = Typography;

type LoadState = 'loading' | 'ready' | 'failed' | 'notfound';

const CARRIER_KEY = {
  telecom: 'carrierTelecom',
  unicom: 'carrierUnicom',
  mobile: 'carrierMobile',
  other: 'carrierOther',
} as const;

const cellKey = (nodeId: string, targetId: number) => `${nodeId}\u0000${targetId}`;

/** Enter / Space activate a role="button" element the way a click does. */
const onActivate = (fn: () => void) => (e: KeyboardEvent) => {
  if (e.key === 'Enter' || e.key === ' ') {
    e.preventDefault();
    fn();
  }
};

/**
 * v1.2.21: one device group's latency at a glance. The top is a table of every
 * node against every target of the group, each cell the mean round trip and
 * loss over the chosen range, coloured like the single-node cards; the bottom
 * is one target's trend with a line per node. Clicking a column or a cell
 * switches the chart's target, clicking a node opens that node's own page.
 * Admin-only (RequireAdmin on the route, AdminOnly on every call).
 */
export default function GroupLatency() {
  const { t } = useI18n();
  const navigate = useNavigate();
  const { groupId = '' } = useParams();

  const [range, setRange] = useState<LatencyRange>('1h');
  const [granularity, setGranularity] = useState<LatencyGranularity>('auto');
  // null: let the server pick (the group's first target).
  const [selected, setSelected] = useState<number | null>(null);
  const [data, setData] = useState<GroupLatencyResponse | null>(null);
  const [state, setState] = useState<LoadState>('loading');
  // Bumped to remount the chart, which is how "reset zoom" returns the slider
  // to the full range.
  const [chartKey, setChartKey] = useState(0);
  const chartRef = useRef<LatencyChartHandle | null>(null);
  // Only the newest request may write the page (see NodeLatency).
  const requestRef = useRef(0);

  const load = useCallback(
    async (silent: boolean) => {
      const mine = ++requestRef.current;
      if (!silent) setState('loading');
      const target = selected != null ? `&target_id=${selected}` : '';
      try {
        const res = await api.get<unknown, ApiEnvelope<GroupLatencyResponse>>(
          `/admin/groups/${encodeURIComponent(groupId)}/latency?range=${range}&granularity=${granularity}${target}`,
        );
        if (mine !== requestRef.current) return;
        if (res.code === 404) {
          setState('notfound');
          return;
        }
        if (res.code === 400 && selected != null) {
          // The chosen target is gone (deleted meanwhile): fall back to the
          // group's first, which reloads through the effect below.
          setSelected(null);
          return;
        }
        if (res.code !== 0 || !res.data) {
          // A silent refresh that fails keeps the picture already on screen.
          if (!silent) setState('failed');
          return;
        }
        setData(res.data);
        setState('ready');
      } catch {
        if (mine === requestRef.current && !silent) setState('failed');
      }
    },
    [groupId, range, granularity, selected],
  );

  useEffect(() => {
    load(false);
  }, [load]);

  // Ranges up to 6 hours follow the clock.
  useEffect(() => {
    if (!autoRefreshes(range)) return undefined;
    const ti = setInterval(() => load(true), AUTO_REFRESH_MS);
    return () => clearInterval(ti);
  }, [range, load]);

  const onRange = (r: LatencyRange) => {
    setRange(r);
    if (!granularityAllowed(r, granularity)) setGranularity('auto');
  };

  const targets: LatencyTarget[] = useMemo(() => data?.targets ?? [], [data]);
  const nodes: GroupLatencyNode[] = useMemo(() => data?.nodes ?? [], [data]);
  const cells = useMemo(() => {
    const m = new Map<string, GroupLatencyCell>();
    for (const c of data?.matrix ?? []) m.set(cellKey(c.node_id, c.target_id), c);
    return m;
  }, [data]);
  const activeId = data?.target_id ?? null;
  const activeTarget = targets.find((x) => x.id === activeId);

  const points: ChartPoint[] = useMemo(
    () => (data ? groupChartPoints(data.series, data.nodes) : []),
    [data],
  );
  const names = useMemo(() => new Map(nodes.map((n) => [n.node_id, nodeDisplayName(n)])), [nodes]);
  const colorDomain = useMemo(() => nodes.map((n) => n.node_id), [nodes]);
  const withDate = range !== '1h' && range !== '6h';
  const hasPoints = points.length > 0;

  // One strip row per node for the target on show: the chosen target's own
  // series, the table's range-wide loss, and the newest point as "now".
  const stripRows: StripRow[] = useMemo(() => {
    const byNode = new Map<string, GroupLatencySeries>();
    for (const s of data?.series ?? []) byNode.set(s.node_id, s);
    return nodes.map((n) => {
      const name = nodeDisplayName(n);
      const base = { key: n.node_id, name, points: [], current: null, loss: null };
      if (!n.supported) return { ...base, note: t('latencyGroupNeedUpgrade') };
      if (activeId != null && n.unsupported_targets.includes(activeId)) {
        return { ...base, note: t('latencyGroupUnsupported') };
      }
      const pts = byNode.get(n.node_id)?.points ?? [];
      const newest = pts.reduce<(typeof pts)[number] | null>((a, p) => (a == null || p.ts > a.ts ? p : a), null);
      const c = activeId != null ? cells.get(cellKey(n.node_id, activeId)) : undefined;
      return {
        ...base,
        points: pts,
        current: newest ? { ms: newest.avg, loss: newest.loss } : null,
        loss: c ? c.loss : null,
      };
    });
  }, [data, nodes, cells, activeId, t]);

  const header = (
    <div className="rp-page-header" style={{ flexWrap: 'wrap', gap: 8 }}>
      <Space wrap size={[12, 8]}>
        <Button icon={<ArrowLeftOutlined />} onClick={() => navigate('/nodes')}>
          {t('latencyBack')}
        </Button>
        <Title level={4} style={{ margin: 0 }} data-testid="group-latency-title">
          {t('latencyGroupTitle')}
          {data?.group_name ? ` · ${data.group_name}` : ''}
        </Title>
      </Space>
      <LatencyControls range={range} granularity={granularity} onRange={onRange} onGranularity={setGranularity} />
    </div>
  );

  if (state === 'notfound') {
    return (
      <>
        {header}
        <Result status="404" title={t('latencyGroupNotFound')} />
      </>
    );
  }
  if (state === 'failed') {
    return (
      <>
        {header}
        <Result
          status="warning"
          title={t('latencyLoadFailed')}
          extra={<Button type="primary" onClick={() => load(false)}>{t('refresh')}</Button>}
        />
      </>
    );
  }
  if (state === 'loading' && !data) {
    return (
      <>
        {header}
        <div style={{ textAlign: 'center', padding: 48 }}><Spin /></div>
      </>
    );
  }
  if (data && targets.length === 0) {
    return (
      <>
        {header}
        <Result
          icon={<Empty image={Empty.PRESENTED_IMAGE_SIMPLE} />}
          title={t('latencyGroupNoTargets')}
          extra={<Button type="primary" onClick={() => navigate('/groups')}>{t('latencyGoGroups')}</Button>}
        />
      </>
    );
  }

  const pick = (id: number) => setSelected(id);

  const columns = [
    {
      title: t('latencyGroupNodeCol'),
      key: 'node',
      fixed: 'left' as const,
      width: 190,
      render: (_: unknown, n: GroupLatencyNode) => (
        <div>
          <Button
            type="link"
            size="small"
            data-testid={`latency-node-${n.node_id}`}
            onClick={() => navigate(`/nodes/${encodeURIComponent(n.node_id)}/latency`)}
            style={{ padding: 0, height: 'auto', fontWeight: 500 }}
          >
            {nodeDisplayName(n)}
          </Button>
          {n.public_ip ? (
            <div><Text type="secondary" className="rp-mono" style={{ fontSize: 12 }}>{n.public_ip}</Text></div>
          ) : null}
          {!n.supported ? (
            <Tooltip title={n.node_version ? `${t('latencyNodeVersion')}: v${n.node_version}` : t('latencyNodeTooOld')}>
              <Tag color="orange" data-testid={`latency-upgrade-${n.node_id}`} style={{ marginTop: 2 }}>
                {t('latencyGroupNeedUpgrade')}
              </Tag>
            </Tooltip>
          ) : null}
        </div>
      ),
    },
    ...targets.map((tg) => {
      const isActive = activeId === tg.id;
      return {
        key: `t-${tg.id}`,
        width: 150,
        // The click and key handlers sit on the header cell itself, not on
        // the title: antd repeats a title in a hidden measuring row, and a
        // second, invisible control there would be reachable by Tab.
        title: (
          <div
            style={{
              color: isActive ? 'var(--ant-color-primary, #6366f1)' : undefined,
              fontWeight: isActive ? 600 : undefined,
            }}
          >
            <div>{tg.name}</div>
            <Space size={4}>
              <Tag style={{ marginInlineEnd: 0, fontWeight: 400 }}>{t(CARRIER_KEY[tg.carrier])}</Tag>
              <Tag style={{ marginInlineEnd: 0, fontWeight: 400 }}>{tg.kind === 'icmp' ? 'ICMP' : 'TCP'}</Tag>
            </Space>
          </div>
        ),
        onHeaderCell: () =>
          ({
            tabIndex: 0,
            'aria-selected': isActive,
            'data-testid': `latency-col-${tg.id}`,
            onClick: () => pick(tg.id),
            onKeyDown: onActivate(() => pick(tg.id)),
            style: { cursor: 'pointer' },
          }) as HTMLAttributes<HTMLElement>,
        onCell: () => ({ style: isActive ? { background: 'var(--ant-color-primary-bg, rgba(99,102,241,0.06))' } : {} }),
        render: (_: unknown, n: GroupLatencyNode) => {
          if (!n.supported) {
            return <Text type="secondary" data-testid={`latency-cell-${n.node_id}-${tg.id}`} data-level="none">-</Text>;
          }
          if (n.unsupported_targets.includes(tg.id)) {
            return (
              <Tooltip title={t('latencyIcmpUnsupportedTip')}>
                <Tag color="orange" data-testid={`latency-cell-${n.node_id}-${tg.id}`} data-level="none">
                  {t('latencyGroupUnsupported')}
                </Tag>
              </Tooltip>
            );
          }
          const c = cells.get(cellKey(n.node_id, tg.id));
          if (!c) {
            return (
              <Text type="secondary" data-testid={`latency-cell-${n.node_id}-${tg.id}`} data-level="none">
                {t('latencyNoData')}
              </Text>
            );
          }
          const level = latencyLevel(c.avg, c.loss);
          return (
            <div
              role="button"
              tabIndex={0}
              data-testid={`latency-cell-${n.node_id}-${tg.id}`}
              data-level={level}
              onClick={() => pick(tg.id)}
              onKeyDown={onActivate(() => pick(tg.id))}
              style={{ cursor: 'pointer', lineHeight: 1.3 }}
            >
              <div style={{ fontSize: 16, fontWeight: 600, color: LEVEL_COLORS[level] }}>
                {c.avg == null ? t('latencyAllLost') : formatMs(c.avg)}
              </div>
              <Text type="secondary" style={{ fontSize: 12 }}>
                {t('latencyCardLoss')} {formatLoss(c.loss)}
              </Text>
            </div>
          );
        },
      };
    }),
  ];

  return (
    <>
      {header}
      <Card size="small" style={{ marginBottom: 16 }}>
        <Text type="secondary" style={{ display: 'block', marginBottom: 8, fontSize: 12 }}>
          {t('latencyGroupTableHint')}
        </Text>
        <Table<GroupLatencyNode>
          data-testid="latency-matrix"
          size="small"
          rowKey="node_id"
          columns={columns}
          dataSource={nodes}
          pagination={false}
          scroll={{ x: 'max-content' }}
          locale={{ emptyText: <Empty image={Empty.PRESENTED_IMAGE_SIMPLE} description={t('latencyGroupNoNodes')} /> }}
        />
      </Card>
      {nodes.length === 0 ? null : (
        <Card
          size="small"
          title={
            <Space size={8} wrap>
              <span data-testid="latency-current-view">
                {t('latencyGroupChartFor')}
                <Text strong>{activeTarget ? activeTarget.name : '-'}</Text>
              </span>
            </Space>
          }
          extra={
            <Space size={8} wrap>
              {autoRefreshes(range) ? (
                <Text type="secondary" style={{ fontSize: 12 }}>{t('latencyAutoRefresh')}</Text>
              ) : null}
              <Button size="small" icon={<UndoOutlined />} onClick={() => setChartKey((k) => k + 1)}>
                {t('latencyChartReset')}
              </Button>
              <Button
                size="small"
                icon={<DownloadOutlined />}
                disabled={!hasPoints}
                onClick={() => chartRef.current?.downloadImage?.(`latency-group-${groupId}-${range}`)}
              >
                {t('latencyChartDownload')}
              </Button>
            </Space>
          }
        >
          {!hasPoints ? (
            <Empty description={t('latencyChartEmpty')} image={Empty.PRESENTED_IMAGE_SIMPLE} />
          ) : (
            <LatencyChart
              key={chartKey}
              points={points}
              colorDomain={colorDomain}
              names={names}
              withDate={withDate}
              onReady={(chart) => {
                chartRef.current = chart;
              }}
            />
          )}
        </Card>
      )}
      {nodes.length === 0 || !data ? null : (
        <LatencyStrips
          testId="latency-strips"
          rows={stripRows}
          since={data.since}
          until={data.until}
          withDate={withDate}
        />
      )}
    </>
  );
}
