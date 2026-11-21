import { useCallback, useEffect, useMemo, useRef, useState, type HTMLAttributes, type KeyboardEvent } from 'react';
import { Button, Card, Empty, Result, Space, Spin, Table, Tag, Tooltip, Typography } from 'antd';
import { ArrowLeftOutlined, InfoCircleOutlined } from '@ant-design/icons';
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
import LatencyControls from '../components/LatencyControls';
import LatencyStrips, { type StripRow } from '../components/LatencyStrips';
import LatencyTrendCard, { ChartBox, ChartBoxMessage } from '../components/LatencyTrendCard';
import LevelDot from '../components/LevelDot';
import SeriesPicker from '../components/SeriesPicker';
import { useSeriesPick } from '../hooks/useSeriesPick';
import { rankByAttention } from '../utils/seriesPick';
import {
  AUTO_REFRESH_MS,
  LATENCY_LOSS_BAD_PCT,
  LATENCY_LOSS_WARN_PCT,
  autoRefreshes,
  granularityAllowed,
  latencyLevel,
  type LatencyGranularity,
  type LatencyRange,
} from '../utils/latency';
import {
  formatLoss,
  formatMs,
  formatRetrans,
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

  const [range, setRange] = useState<LatencyRange>('24h');
  const [granularity, setGranularity] = useState<LatencyGranularity>('1h');
  // null: let the server pick (the group's first target).
  const [selected, setSelected] = useState<number | null>(null);
  const [data, setData] = useState<GroupLatencyResponse | null>(null);
  const [state, setState] = useState<LoadState>('loading');
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
    if (!granularityAllowed(r, granularity)) setGranularity('1h');
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

  const allPoints: ChartPoint[] = useMemo(
    () => (data ? groupChartPoints(data.series, data.nodes) : []),
    [data],
  );
  const names = useMemo(() => new Map(nodes.map((n) => [n.node_id, nodeDisplayName(n)])), [nodes]);
  // One line per node. Past ten the palette runs out: the picker chooses which
  // to draw, by default the ten that need attention first for this target
  // (worst latency level, then loss, then mean; a node that cannot be probed or
  // has no reading last).
  const nodeKeys = useMemo(() => nodes.map((n) => n.node_id), [nodes]);
  const ranked = useMemo(() => {
    const byId = new Map(nodes.map((n) => [n.node_id, n]));
    return rankByAttention(nodeKeys, (id) => {
      const n = byId.get(id);
      if (!n || !n.supported || activeId == null || n.unsupported_targets.includes(activeId)) return null;
      return cells.get(cellKey(id, activeId));
    });
  }, [nodes, nodeKeys, cells, activeId]);
  const pick = useSeriesPick(nodeKeys, ranked);
  const colorDomain = pick.shown;
  const points: ChartPoint[] = useMemo(() => {
    if (!pick.needsPicker) return allPoints;
    const shown = new Set(pick.shown);
    return allPoints.filter((p) => shown.has(p.key));
  }, [allPoints, pick.needsPicker, pick.shown]);
  const withDate = range !== '1h' && range !== '6h';

  // One strip row per node for the target on show: the chosen target's own
  // series, the table's range-wide loss, and the newest point as "now".
  const stripRows: StripRow[] = useMemo(() => {
    const byNode = new Map<string, GroupLatencySeries>();
    for (const s of data?.series ?? []) byNode.set(s.node_id, s);
    return nodes.map((n) => {
      const name = nodeDisplayName(n);
      const base = { key: n.node_id, name, points: [], current: null, loss: null, retrans: null };
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
        retrans: c?.retrans_rate ?? null,
      };
    });
  }, [data, nodes, cells, activeId, t]);

  const picker = pick.needsPicker ? (
    <SeriesPicker
      testId="series-picker"
      placeholder={t('seriesPickNodes')}
      options={nodes.map((n) => ({ value: n.node_id, label: names.get(n.node_id) ?? n.node_id }))}
      value={pick.shown}
      automatic={pick.automatic}
      onChange={pick.setPicked}
    />
  ) : null;

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
      <div className="rp-lat">
        {header}
        <Result status="404" title={t('latencyGroupNotFound')} />
      </div>
    );
  }
  if (state === 'failed') {
    return (
      <div className="rp-lat">
        {header}
        <Card size="small" className="rp-lat-card">
          <ChartBox>
            <ChartBoxMessage>
              <Result
                status="warning"
                style={{ padding: 0 }}
                title={t('latencyLoadFailed')}
                extra={<Button type="primary" onClick={() => load(false)}>{t('refresh')}</Button>}
              />
            </ChartBoxMessage>
          </ChartBox>
        </Card>
      </div>
    );
  }
  if (state === 'loading' && !data) {
    return (
      <div className="rp-lat">
        {header}
        <Card size="small" className="rp-lat-card">
          <ChartBox>
            <ChartBoxMessage><Spin /></ChartBoxMessage>
          </ChartBox>
        </Card>
      </div>
    );
  }
  if (data && targets.length === 0) {
    return (
      <div className="rp-lat">
        {header}
        <Result
          icon={<Empty image={Empty.PRESENTED_IMAGE_SIMPLE} />}
          title={t('latencyGroupNoTargets')}
          extra={<Button type="primary" onClick={() => navigate('/groups')}>{t('latencyGoGroups')}</Button>}
        />
      </div>
    );
  }

  const pickTarget = (id: number) => setSelected(id);

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
          {n.public_ip ? <span className="rp-lat-node-ip rp-mono">{n.public_ip}</span> : null}
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
          <div>
            <div className="rp-lat-colname">{tg.name}</div>
            <div className="rp-lat-head-sub">
              {tg.kind === 'icmp' ? 'ICMP' : 'TCP'} · {t(CARRIER_KEY[tg.carrier])}
            </div>
          </div>
        ),
        onHeaderCell: () =>
          ({
            tabIndex: 0,
            'aria-selected': isActive,
            'data-testid': `latency-col-${tg.id}`,
            onClick: () => pickTarget(tg.id),
            onKeyDown: onActivate(() => pickTarget(tg.id)),
            className: `rp-lat-col-head${isActive ? ' is-active' : ''}`,
          }) as HTMLAttributes<HTMLElement>,
        onCell: () => ({ className: `rp-lat-col-cell${isActive ? ' is-active' : ''}` }),
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
              onClick={() => pickTarget(tg.id)}
              onKeyDown={onActivate(() => pickTarget(tg.id))}
              style={{ cursor: 'pointer', lineHeight: 1.3 }}
            >
              <div className="rp-lat-num" style={{ fontSize: 15 }}>
                <LevelDot level={level} />
                {c.avg == null ? t('latencyAllLost') : formatMs(c.avg)}
              </div>
              <span className="rp-lat-sub">
                <span className={c.loss >= LATENCY_LOSS_BAD_PCT ? 'is-bad' : c.loss >= LATENCY_LOSS_WARN_PCT ? 'is-warn' : undefined}>
                  {t('latencyCardLoss')} {formatLoss(c.loss)}
                </span>
                {formatRetrans(c.retrans_rate) == null ? null : (
                  <span data-testid={`latency-cell-retrans-${n.node_id}-${tg.id}`}>
                    {' · '}
                    {t('latencyCardRetrans')} {formatRetrans(c.retrans_rate)}
                  </span>
                )}
              </span>
            </div>
          );
        },
      };
    }),
  ];

  return (
    <div className="rp-lat">
      {header}
      <Card
        size="small"
        className="rp-lat-card"
        style={{ marginBottom: 16 }}
        title={
          <Space size={6}>
            <span>{t('latencyGroupMatrixTitle')}</span>
            <Tooltip
              placement="bottomLeft"
              title={
                <div style={{ maxWidth: 360 }}>
                  <div>{t('latencyGroupTableHint')}</div>
                  <div style={{ marginTop: 8 }}>{t('latencyRetransHelp')}</div>
                </div>
              }
            >
              <InfoCircleOutlined
                data-testid="latency-retrans-help"
                tabIndex={0}
                aria-label={t('latencyGroupTableHint')}
                style={{ color: 'var(--ant-color-text-tertiary, #6b7280)' }}
              />
            </Tooltip>
          </Space>
        }
      >
        <Table<GroupLatencyNode>
          className="rp-lat-matrix"
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
        <LatencyTrendCard
          title={
            <span data-testid="latency-current-view">
              {t('latencyGroupChartFor')}
              <Text strong>{activeTarget ? activeTarget.name : '-'}</Text>
            </span>
          }
          picker={picker}
          points={points}
          colorDomain={colorDomain}
          names={names}
          withDate={withDate}
          autoRefresh={autoRefreshes(range)}
          loading={state === 'loading'}
          downloadName={`latency-group-${groupId}-${range}`}
        />
      )}
      {nodes.length === 0 || !data ? null : (
        <LatencyStrips
          testId="latency-strips"
          rows={stripRows}
          since={data.since}
          until={data.until}
          bucketSecs={data.bucket_secs}
          withDate={withDate}
        />
      )}
    </div>
  );
}
