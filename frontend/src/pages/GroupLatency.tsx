import { useCallback, useEffect, useMemo, useRef, useState, type HTMLAttributes, type KeyboardEvent } from 'react';
import { Alert, Button, Card, Empty, Result, Segmented, Space, Spin, Table, Tag, Tooltip, Typography } from 'antd';
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
import { useNowSecs } from '../hooks/useNowSecs';
import { rankByAttention } from '../utils/seriesPick';
import { SERIES_COLORS_LIGHT } from '../utils/trafficBuckets';
import {
  LATENCY_LOSS_BAD_PCT,
  LATENCY_LOSS_WARN_PCT,
  LATENCY_SLOW_BAD_PCT,
  LATENCY_SLOW_WARN_PCT,
  autoRefreshMs,
  autoRefreshNoteKey,
  granularityAllowed,
  isStaleSample,
  latencyLevel,
  type LatencyGranularity,
  type LatencyRange,
} from '../utils/latency';
import {
  formatClock,
  formatLoss,
  formatMs,
  groupChartPoints,
  nodeDisplayName,
  showRaw,
  slowText,
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
  const [carrierFilter, setCarrierFilter] = useState<'all' | LatencyTarget['carrier']>('all');
  // null: let the server pick (the group's first target).
  const [selected, setSelected] = useState<number | null>(null);
  const [data, setData] = useState<GroupLatencyResponse | null>(null);
  const [state, setState] = useState<LoadState>('loading');
  // See NodeLatency: when the last load succeeded, whether the refreshes since
  // have failed, and the browser's clock that stale readings are judged by.
  const [lastSuccessAt, setLastSuccessAt] = useState<number | null>(null);
  const [refreshFailed, setRefreshFailed] = useState(false);
  const [nowSecs, refreshNow] = useNowSecs();
  // Only the newest request may write the page (see NodeLatency).
  const requestRef = useRef(0);

  const load = useCallback(
    async (silent: boolean) => {
      const mine = ++requestRef.current;
      if (!silent) {
        setState('loading');
        setRefreshFailed(false);
      }
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
          // A silent refresh that fails keeps the picture already on screen,
          // with a warning that it is getting old.
          if (silent) setRefreshFailed(true);
          else setState('failed');
          return;
        }
        setData(res.data);
        setState('ready');
        setLastSuccessAt(Date.now());
        setRefreshFailed(false);
        refreshNow();
      } catch {
        if (mine !== requestRef.current) return;
        if (silent) setRefreshFailed(true);
        else setState('failed');
      }
    },
    [groupId, range, granularity, selected, refreshNow],
  );

  useEffect(() => {
    load(false);
  }, [load]);

  // Every range re-reads itself (a minute, or five for 7d / 30d).
  useEffect(() => {
    const ti = setInterval(() => load(true), autoRefreshMs(range));
    return () => clearInterval(ti);
  }, [range, load]);

  const onRange = (r: LatencyRange) => {
    setRange(r);
    if (!granularityAllowed(r, granularity)) setGranularity('1h');
  };

  const targets: LatencyTarget[] = useMemo(() => data?.targets ?? [], [data]);
  const availableCarriers = useMemo(() => {
    const seen = new Set<LatencyTarget['carrier']>();
    for (const tg of targets) seen.add(tg.carrier);
    return (['telecom', 'unicom', 'mobile', 'other'] as const).filter((c) => seen.has(c));
  }, [targets]);
  const visibleTargets = useMemo(
    () => (carrierFilter === 'all' ? targets : targets.filter((tg) => tg.carrier === carrierFilter)),
    [targets, carrierFilter],
  );
  const nodes: GroupLatencyNode[] = useMemo(() => data?.nodes ?? [], [data]);
  const cells = useMemo(() => {
    const m = new Map<string, GroupLatencyCell>();
    for (const c of data?.matrix ?? []) m.set(cellKey(c.node_id, c.target_id), c);
    return m;
  }, [data]);
  const bestByTarget = useMemo(() => {
    const best = new Map<number, string>();
    for (const tg of targets) {
      let minVal = Infinity;
      let minNode = '';
      let validCount = 0;
      for (const n of nodes) {
        if (!n.supported || n.unsupported_targets.includes(tg.id)) continue;
        const c = cells.get(cellKey(n.node_id, tg.id));
        if (c && c.avg != null) {
          validCount += 1;
          if (c.avg < minVal) {
            minVal = c.avg;
            minNode = n.node_id;
          }
        }
      }
      if (validCount >= 2 && minNode) best.set(tg.id, minNode);
    }
    return best;
  }, [targets, nodes, cells]);
  const activeId = data?.target_id ?? null;
  const activeTarget = targets.find((x) => x.id === activeId);

  const allPoints: ChartPoint[] = useMemo(
    () => (data ? groupChartPoints(data.series, data.nodes, activeTarget?.kind ?? null) : []),
    [data, activeTarget],
  );
  const names = useMemo(() => new Map(nodes.map((n) => [n.node_id, nodeDisplayName(n)])), [nodes]);
  // One line per node. Past ten the palette runs out: the picker chooses which
  // to draw, by default the ten that need attention first for this target
  // (worst level by latency, loss and slow-handshake share, then loss, then slow
  // share, then mean; a node that cannot be probed or has no reading last).
  const nodeKeys = useMemo(() => nodes.map((n) => n.node_id), [nodes]);
  const ranked = useMemo(() => {
    const byId = new Map(nodes.map((n) => [n.node_id, n]));
    return rankByAttention(nodeKeys, (id) => {
      const n = byId.get(id);
      if (!n || !n.supported || activeId == null || n.unsupported_targets.includes(activeId)) return null;
      const c = cells.get(cellKey(id, activeId));
      return c ? { avg: c.avg, loss: c.loss, slow: c.retrans_rate } : undefined;
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
      const kind = activeTarget?.kind;
      const base = { key: n.node_id, name, kind, points: [], current: null, loss: null, slow: null };
      if (!n.supported) return { ...base, note: t('latencyGroupNeedUpgrade') };
      if (activeId != null && n.unsupported_targets.includes(activeId)) {
        return { ...base, note: t('latencyGroupUnsupported') };
      }
      const pts = byNode.get(n.node_id)?.points ?? [];
      const newest = pts.reduce<(typeof pts)[number] | null>((a, p) => (a == null || p.ts > a.ts ? p : a), null);
      const c = activeId != null ? cells.get(cellKey(n.node_id, activeId)) : undefined;
      // The real time of the node's newest minute, which an hourly bucket
      // cannot tell (its bucket is "the current hour" for a node quiet for
      // 15 minutes and one that is live alike). Only a response without it
      // falls back to the bucket's end.
      const latestTs = byNode.get(n.node_id)?.latest_ts;
      const sampleTs = newest ? (latestTs ?? newest.ts + (data?.bucket_secs ?? 0)) : null;
      return {
        ...base,
        points: pts,
        current: newest
          ? {
              ms: newest.avg,
              loss: newest.loss,
              slow: newest.retrans_rate ?? null,
              rawMs: newest.raw_avg ?? null,
              sampleTs,
              stale: isStaleSample(sampleTs, nowSecs),
            }
          : null,
        loss: c ? c.loss : null,
        slow: c?.retrans_rate ?? null,
      };
    });
  }, [data, nodes, cells, activeId, activeTarget, nowSecs, t]);

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

  const updatedAt = lastSuccessAt != null ? formatClock(new Date(lastSuccessAt), true) : null;
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
        {updatedAt ? (
          <Text type="secondary" style={{ fontSize: 12 }} data-testid="latency-updated-at" data-time={updatedAt}>
            {t('latencyUpdatedAt')} {updatedAt}
          </Text>
        ) : null}
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
  // A background refresh failed: what is below is from the last good load.
  const failAlert =
    refreshFailed && updatedAt != null ? (
      <Alert
        type="warning"
        showIcon
        style={{ marginBottom: 12 }}
        data-testid="latency-refresh-failed"
        data-time={updatedAt}
        title={t('latencyRefreshFailed').replace('{time}', updatedAt)}
      />
    ) : null;

  if (data && targets.length === 0) {
    return (
      <div className="rp-lat">
        {header}
        {failAlert}
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
    ...visibleTargets.map((tg) => {
      const isActive = activeId === tg.id;
      return {
        key: `t-${tg.id}`,
        width: 168,
        title: (
          <div>
            <div className="rp-lat-colname">{tg.name}</div>
            <div className="rp-lat-head-sub">
              <span>{tg.kind === 'icmp' ? 'ICMP' : 'TCP'}</span>
              <span className={`rp-carrier-pill is-${tg.carrier}`}>{t(CARRIER_KEY[tg.carrier])}</span>
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
        onCell: (n: GroupLatencyNode) => {
          const supported = n.supported && !n.unsupported_targets.includes(tg.id);
          const c = supported ? cells.get(cellKey(n.node_id, tg.id)) : undefined;
          const lvl = c ? latencyLevel(c.avg, c.loss, c.retrans_rate) : 'none';
          const heat = lvl === 'good' || lvl === 'warn' || lvl === 'bad' ? ` is-heat-${lvl}` : '';
          return { className: `rp-lat-col-cell${isActive ? ' is-active' : ''}${heat}` };
        },
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
          const level = latencyLevel(c.avg, c.loss, c.retrans_rate);
          const slow = slowText(c.retrans_rate, tg.kind);
          const isBest = bestByTarget.get(tg.id) === n.node_id;
          const failKey = tg.kind === 'tcp' ? 'latencyCardFailed' : 'latencyCardLoss';
          const lossSeverity = c.loss >= LATENCY_LOSS_BAD_PCT ? ' is-bad' : c.loss >= LATENCY_LOSS_WARN_PCT ? ' is-warn' : '';
          const slowSeverity =
            c.retrans_rate != null && c.retrans_rate >= LATENCY_SLOW_BAD_PCT
              ? ' is-bad'
              : c.retrans_rate != null && c.retrans_rate >= LATENCY_SLOW_WARN_PCT
                ? ' is-warn'
                : '';
          const tipContent = (
            <div style={{ minWidth: 160 }}>
              <div style={{ fontWeight: 500, marginBottom: 4 }}>{nodeDisplayName(n)} → {tg.name}</div>
              <div>{t('latencyCardAvg')}: {c.avg == null ? t('latencyAllLost') : formatMs(c.avg)}</div>
              {showRaw(c.avg, c.raw_avg, c.retrans_rate) ? (
                <div>{t('latencyCardRaw')}: {formatMs(c.raw_avg)}</div>
              ) : null}
              <div>{t(failKey)}: {formatLoss(c.loss)}</div>
              {slow == null ? null : <div>{t('latencyCardSlow')}: {slow}</div>}
              <div style={{ opacity: 0.72 }}>{t('latencyProbesSent')}: {c.sent}</div>
            </div>
          );
          return (
            <Tooltip title={tipContent} mouseEnterDelay={0.25}>
              <div
                role="button"
                tabIndex={0}
                data-testid={`latency-cell-${n.node_id}-${tg.id}`}
                data-level={level}
                onClick={() => pickTarget(tg.id)}
                onKeyDown={onActivate(() => pickTarget(tg.id))}
                style={{ cursor: 'pointer', lineHeight: 1.3 }}
              >
                <div className="rp-lat-num" style={{ fontSize: 15, display: 'flex', alignItems: 'center' }}>
                  <LevelDot level={level} />
                  <span>{c.avg == null ? t('latencyAllLost') : formatMs(c.avg)}</span>
                  {isBest && <span className="rp-lat-best">{t('latencyBest')}</span>}
                </div>
                {showRaw(c.avg, c.raw_avg, c.retrans_rate) ? (
                  <span className="rp-lat-sub" data-testid={`latency-cell-raw-${n.node_id}-${tg.id}`}>
                    {t('latencyCardRaw')} {formatMs(c.raw_avg)}
                  </span>
                ) : null}
                <div className="rp-lat-metric-row">
                  <span className={`rp-lat-chip${lossSeverity}`}>
                    {t(failKey)} {formatLoss(c.loss)}
                  </span>
                  {slow == null ? null : (
                    <>
                      <span style={{ color: '#cbd5e1', fontSize: 10 }}>{' · '}</span>
                      <span className={`rp-lat-chip${slowSeverity}`} data-testid={`latency-cell-slow-${n.node_id}-${tg.id}`}>
                        {t('latencyCardSlow')} {slow}
                      </span>
                    </>
                  )}
                </div>
              </div>
            </Tooltip>
          );
        },
      };
    }),
  ];

  const activeKpiStrip = activeId != null && nodes.length > 0 ? (
    <div className="rp-lat-kpi-strip">
      {nodes.filter((n) => n.supported && !n.unsupported_targets.includes(activeId)).map((n) => {
        const c = cells.get(cellKey(n.node_id, activeId));
        const colorIdx = Math.max(0, colorDomain.indexOf(n.node_id));
        const swatch = SERIES_COLORS_LIGHT[colorIdx % SERIES_COLORS_LIGHT.length];
        const isBest = bestByTarget.get(activeId) === n.node_id;
        return (
          <div key={n.node_id} className="rp-lat-kpi-pill">
            <span className="rp-lat-kpi-swatch" style={{ background: swatch }} />
            <span style={{ fontWeight: 500, color: 'var(--rp-text)' }}>{nodeDisplayName(n)}</span>
            <span className="rp-mono" style={{ color: 'var(--rp-text-secondary)' }}>
              {c ? (c.avg == null ? t('latencyAllLost') : formatMs(c.avg)) : '-'}
            </span>
            {c && (
              <span style={{ fontSize: 11, color: c.loss >= LATENCY_LOSS_BAD_PCT ? '#be123c' : c.loss >= LATENCY_LOSS_WARN_PCT ? '#b45309' : '#64748b' }}>
                {formatLoss(c.loss)}
              </span>
            )}
            {isBest && <span className="rp-lat-best" style={{ marginInlineStart: 0 }}>{t('latencyBest')}</span>}
          </div>
        );
      })}
    </div>
  ) : null;

  const onCarrierFilter = (next: 'all' | LatencyTarget['carrier']) => {
    setCarrierFilter(next);
    if (next !== 'all') {
      const subset = targets.filter((tg) => tg.carrier === next);
      if (subset.length > 0 && !subset.some((tg) => tg.id === activeId)) {
        setSelected(subset[0].id);
      }
    }
  };

  return (
    <div className="rp-lat">
      {header}
      {failAlert}
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
                  <div style={{ marginTop: 8 }}>{t('latencySlowHelp')}</div>
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
        extra={
          availableCarriers.length > 1 ? (
            <Segmented
              size="small"
              value={carrierFilter}
              onChange={(v) => onCarrierFilter(v as 'all' | LatencyTarget['carrier'])}
              options={[
                { value: 'all', label: `${t('latencyCarrierAll')} (${targets.length})` },
                ...availableCarriers.map((c) => ({
                  value: c,
                  label: t(CARRIER_KEY[c]),
                })),
              ]}
            />
          ) : null
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
            <Space size={8} wrap>
              <span data-testid="latency-current-view">
                {t('latencyGroupChartFor')}
                <Text strong>{activeTarget ? activeTarget.name : '-'}</Text>
              </span>
              {activeTarget && (
                <span className={`rp-carrier-pill is-${activeTarget.carrier}`}>
                  {activeTarget.kind === 'icmp' ? 'ICMP' : 'TCP'} · {t(CARRIER_KEY[activeTarget.carrier])}
                </span>
              )}
            </Space>
          }
          picker={picker}
          stats={activeKpiStrip}
          points={points}
          colorDomain={colorDomain}
          names={names}
          withDate={withDate}
          autoRefreshNote={t(autoRefreshNoteKey(range))}
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
          nowSecs={nowSecs}
        />
      )}
    </div>
  );
}
