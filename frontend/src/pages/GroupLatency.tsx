import { useCallback, useEffect, useMemo, useRef, useState, type HTMLAttributes, type KeyboardEvent } from 'react';
import { Alert, Button, Card, Empty, Result, Segmented, Space, Spin, Switch, Table, Tag, Tooltip, Typography } from 'antd';
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
  computeGroupSnapshot,
  computeNodeOverall,
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
  const [sortMode, setSortMode] = useState<'default' | 'overall' | 'active'>('default');
  const [onlyAnomalies, setOnlyAnomalies] = useState(false);
  const [focusedNode, setFocusedNode] = useState<string | null>(null);
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
  const maxByTarget = useMemo(() => {
    const mx = new Map<number, number>();
    for (const tg of targets) {
      let hi = 0;
      for (const n of nodes) {
        if (!n.supported || n.unsupported_targets.includes(tg.id)) continue;
        const c = cells.get(cellKey(n.node_id, tg.id));
        if (c && c.avg != null && c.avg > hi) hi = c.avg;
      }
      if (hi > 0) mx.set(tg.id, hi);
    }
    return mx;
  }, [targets, nodes, cells]);
  const activeId = data?.target_id ?? null;
  const activeTarget = targets.find((x) => x.id === activeId);

  const overallsByNode = useMemo(() => {
    const m = new Map<string, ReturnType<typeof computeNodeOverall>>();
    for (const n of nodes) {
      m.set(n.node_id, computeNodeOverall(n, targets, (nid, tid) => cells.get(cellKey(nid, tid))));
    }
    return m;
  }, [nodes, targets, cells]);

  const snapshot = useMemo(
    () => computeGroupSnapshot(nodes, targets, Array.from(overallsByNode.values()), (nid, tid) => cells.get(cellKey(nid, tid))),
    [nodes, targets, overallsByNode, cells],
  );

  const matrixNodes = useMemo(() => {
    let list = onlyAnomalies
      ? nodes.filter((n) => overallsByNode.get(n.node_id)?.hasIssue)
      : nodes;
    if (sortMode === 'overall') {
      list = [...list].sort((a, b) => (overallsByNode.get(a.node_id)?.score ?? Infinity) - (overallsByNode.get(b.node_id)?.score ?? Infinity));
    } else if (sortMode === 'active' && activeId != null) {
      const colScore = (n: GroupLatencyNode) => {
        if (!n.supported || n.unsupported_targets.includes(activeId)) return Infinity;
        const c = cells.get(cellKey(n.node_id, activeId));
        if (!c) return Infinity;
        const slow = activeTarget?.kind === 'icmp' ? 0 : (c.retrans_rate ?? 0);
        if (c.avg == null) return 100_000 + c.loss * 100 + slow * 10;
        return c.avg + c.loss * 10 + slow * 5;
      };
      list = [...list].sort((a, b) => colScore(a) - colScore(b));
    }
    return list;
  }, [nodes, onlyAnomalies, sortMode, overallsByNode, activeId, activeTarget, cells]);

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
  // Keep focus only while the node exists and is supported for the active target;
  // when focused, focus takes priority over the 10-line auto-picker so switching
  // targets on >10-node groups never leaves a strip without its trend line.
  const activeFocusedNode = useMemo(() => {
    if (!focusedNode) return null;
    const found = nodes.find((n) => n.node_id === focusedNode);
    if (!found || !found.supported) return null;
    if (activeId != null && found.unsupported_targets.includes(activeId)) return null;
    return found.node_id;
  }, [focusedNode, nodes, activeId]);
  const colorDomain = useMemo(() => {
    if (!activeFocusedNode || pick.shown.includes(activeFocusedNode)) return pick.shown;
    return [...pick.shown, activeFocusedNode];
  }, [pick.shown, activeFocusedNode]);
  const points: ChartPoint[] = useMemo(() => {
    if (activeFocusedNode) {
      return allPoints.filter((p) => p.key === activeFocusedNode);
    }
    if (!pick.needsPicker) return allPoints;
    const shown = new Set(pick.shown);
    return allPoints.filter((p) => shown.has(p.key));
  }, [allPoints, pick.needsPicker, pick.shown, activeFocusedNode]);
  const withDate = range !== '1h' && range !== '6h';

  // One strip row per node for the target on show: the chosen target's own
  // series, the table's range-wide loss, and the newest point as "now".
  const stripRows: StripRow[] = useMemo(() => {
    const byNode = new Map<string, GroupLatencySeries>();
    for (const s of data?.series ?? []) byNode.set(s.node_id, s);
    const list = activeFocusedNode ? nodes.filter((n) => n.node_id === activeFocusedNode) : nodes;
    return list.map((n) => {
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
  }, [data, nodes, cells, activeId, activeTarget, nowSecs, activeFocusedNode, t]);

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
          <button
            type="button"
            className="rp-lat-node-link"
            data-testid={`latency-node-${n.node_id}`}
            onClick={() => navigate(`/nodes/${encodeURIComponent(n.node_id)}/latency`)}
          >
            <span>{nodeDisplayName(n)}</span>
            <svg aria-hidden="true" width="11" height="11" viewBox="0 0 12 12" fill="none">
              <path d="M3.5 8.5L8.5 3.5M8.5 3.5H4.5M8.5 3.5V7.5" stroke="currentColor" strokeWidth="1.4" strokeLinecap="round" strokeLinejoin="round" />
            </svg>
          </button>
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
    ...(targets.length > 1 ? [{
      title: t('latencyOverallCol'),
      key: 'overall',
      width: 144,
      render: (_: unknown, n: GroupLatencyNode) => {
        const ov = overallsByNode.get(n.node_id);
        if (!n.supported || !ov || ov.totalCount === 0) {
          return <Text type="secondary">-</Text>;
        }
        const isBestOverall = snapshot.bestNode?.nodeId === n.node_id && nodes.length >= 2;
        const lossClass = (ov.avgLoss ?? 0) >= LATENCY_LOSS_BAD_PCT ? ' is-bad' : (ov.avgLoss ?? 0) >= LATENCY_LOSS_WARN_PCT ? ' is-warn' : '';
        return (
          <div data-testid={`latency-overall-${n.node_id}`}>
            <div className="rp-lat-num" style={{ fontSize: 14, display: 'flex', alignItems: 'center' }}>
              <LevelDot level={ov.level} />
              <span>{ov.avgMs == null ? t('latencyAllLost') : formatMs(ov.avgMs)}</span>
              {isBestOverall && <span className="rp-lat-best">{t('latencyBest')}</span>}
            </div>
            <div className="rp-lat-metric-row">
              <span className={`rp-lat-chip${lossClass}`}>
                {t('latencyStatsFailLoss')} {formatLoss(ov.avgLoss)}
              </span>
            </div>
            <div className="rp-lat-overall-bar" aria-hidden="true">
              {ov.goodCount > 0 && <span style={{ flex: ov.goodCount, background: '#22c55e' }} />}
              {ov.warnCount > 0 && <span style={{ flex: ov.warnCount, background: '#f59e0b' }} />}
              {ov.badCount > 0 && <span style={{ flex: ov.badCount, background: '#ef4444' }} />}
            </div>
          </div>
        );
      },
    }] : []),
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
                {c.avg != null && (maxByTarget.get(tg.id) ?? 0) > 0 && (
                  <div className="rp-lat-microbar" aria-hidden="true">
                    <span
                      style={{
                        width: `${Math.max(12, Math.min(100, Math.round((c.avg / (maxByTarget.get(tg.id) || c.avg)) * 100)))}%`,
                        background: level === 'bad' ? '#fb7185' : level === 'warn' ? '#fbbf24' : '#34d399',
                      }}
                    />
                  </div>
                )}
              </div>
            </Tooltip>
          );
        },
      };
    }),
  ];

  const toggleFocusNode = (nid: string) => {
    setFocusedNode((prev) => (prev === nid ? null : nid));
  };

  const kpiNodes = activeId != null
    ? nodes.filter((n) =>
        n.supported
        && !n.unsupported_targets.includes(activeId)
        && (!pick.needsPicker || pick.shown.includes(n.node_id) || n.node_id === activeFocusedNode),
      )
    : [];

  const activeKpiStrip = activeId != null && kpiNodes.length > 0 ? (
    <div className="rp-lat-kpi-strip" data-testid="latency-node-filter">
      {kpiNodes.length > 1 && (
        <button
          type="button"
          data-testid="latency-focus-all"
          aria-pressed={activeFocusedNode == null}
          onClick={() => setFocusedNode(null)}
          className={`rp-lat-kpi-pill${activeFocusedNode == null ? ' is-focused' : ' is-muted'}`}
        >
          <span style={{ fontWeight: 500, color: activeFocusedNode == null ? '#5b4cc4' : 'var(--rp-text-secondary)' }}>
            {t('latencyCarrierAll')}
          </span>
        </button>
      )}
      {kpiNodes.map((n) => {
        const c = cells.get(cellKey(n.node_id, activeId));
        const colorIdx = Math.max(0, colorDomain.indexOf(n.node_id));
        const swatch = SERIES_COLORS_LIGHT[colorIdx % SERIES_COLORS_LIGHT.length];
        const isBest = bestByTarget.get(activeId) === n.node_id;
        const isFocused = activeFocusedNode === n.node_id;
        const isDimmed = activeFocusedNode != null && !isFocused;
        return (
          <button
            type="button"
            key={n.node_id}
            data-testid={`latency-focus-${n.node_id}`}
            aria-pressed={isFocused}
            onClick={() => toggleFocusNode(n.node_id)}
            className={`rp-lat-kpi-pill${isFocused ? ' is-focused' : isDimmed ? ' is-muted' : ''}`}
          >
            <span className="rp-lat-kpi-swatch" style={{ background: swatch }} />
            <span style={{ fontWeight: 500, color: 'var(--rp-text)' }}>{nodeDisplayName(n)}</span>
            <span className="rp-mono" style={{ color: 'var(--rp-text-secondary)' }}>
              {c ? (c.avg == null ? t('latencyAllLost') : formatMs(c.avg)) : '-'}
            </span>
            {c && (
              <span style={{ fontSize: 11, color: c.loss >= LATENCY_LOSS_BAD_PCT ? '#be123c' : c.loss >= LATENCY_LOSS_WARN_PCT ? '#b45309' : '#71717a' }}>
                {formatLoss(c.loss)}
              </span>
            )}
            {isBest && <span className="rp-lat-best" style={{ marginInlineStart: 0 }}>{t('latencyBest')}</span>}
          </button>
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

  const anomalyCount = snapshot.warnPairs + snapshot.badPairs;
  const canToggleAnomalies = anomalyCount > 0 || onlyAnomalies;
  const targetById = new Map(targets.map((tg) => [tg.id, tg]));
  const worstTarget = snapshot.worstPair ? targetById.get(snapshot.worstPair.targetId) : undefined;
  const healthLevel = snapshot.badPairs > 0
    ? 'bad'
    : snapshot.warnPairs > 0
      ? 'warn'
      : snapshot.totalPairs === 0
        ? 'none'
        : 'good';

  const snapshotBanner = nodes.length > 0 && snapshot.expectedPairs > 0 ? (
    <div className="rp-lat-snapshot-grid" data-testid="latency-snapshot">
      {/* Card 1: Global Probe Health */}
      <div
        role={canToggleAnomalies ? 'button' : undefined}
        tabIndex={canToggleAnomalies ? 0 : undefined}
        aria-pressed={canToggleAnomalies ? onlyAnomalies : undefined}
        onClick={canToggleAnomalies ? () => setOnlyAnomalies((v) => !v) : undefined}
        onKeyDown={canToggleAnomalies ? onActivate(() => setOnlyAnomalies((v) => !v)) : undefined}
        className={`rp-lat-snap-card${canToggleAnomalies ? ' is-clickable' : ''}`}
        data-testid="latency-snap-health"
      >
        <div className="rp-lat-snap-label">
          <span>{t('latencySnapHealth')}</span>
          {anomalyCount > 0 ? (
            <Tag color={snapshot.badPairs > 0 ? 'red' : 'orange'} style={{ marginInlineEnd: 0 }}>
              {t('latencySnapAnomalyPairs').replace('{n}', String(anomalyCount))}
            </Tag>
          ) : snapshot.totalPairs === 0 ? (
            <Tag style={{ marginInlineEnd: 0 }}>{t('latencyNoData')}</Tag>
          ) : snapshot.missingPairs > 0 ? (
            <Tag style={{ marginInlineEnd: 0 }}>{t('latencySnapMeasuredHealthy')}</Tag>
          ) : (
            <Tag color="green" style={{ marginInlineEnd: 0 }}>{t('latencySnapAllHealthy')}</Tag>
          )}
        </div>
        <div className="rp-lat-snap-main">
          <LevelDot level={healthLevel} />
          <span data-testid="latency-snap-ratio" data-good={snapshot.goodPairs} data-total={snapshot.expectedPairs}>
            {t('latencySnapHealthyPairs')
              .replace('{good}', String(snapshot.goodPairs))
              .replace('{total}', String(snapshot.expectedPairs))}
          </span>
          {snapshot.missingPairs > 0 && (
            <span className="rp-lat-chip" data-testid="latency-snap-missing" data-missing={snapshot.missingPairs}>
              {t('latencySnapMissingPairs').replace('{n}', String(snapshot.missingPairs))}
            </span>
          )}
        </div>
      </div>

      {/* Card 2: Carrier Avg Comparison */}
      <div className="rp-lat-snap-card">
        <div className="rp-lat-snap-label"><span>{t('latencySnapCarriers')}</span></div>
        <div className="rp-lat-carrier-row">
          {snapshot.carrierAvgs.map((ca) => (
            <button
              type="button"
              key={ca.carrier}
              onClick={() => onCarrierFilter(carrierFilter === ca.carrier ? 'all' : ca.carrier)}
              className={`rp-lat-carrier-stat${carrierFilter === ca.carrier ? ' is-active' : ''}`}
            >
              <span className={`rp-carrier-pill is-${ca.carrier}`}>{t(CARRIER_KEY[ca.carrier])}</span>
              <span className="rp-mono" style={{ fontWeight: 500, color: 'var(--rp-text)' }}>
                {ca.avgMs == null ? '-' : formatMs(ca.avgMs)}
              </span>
            </button>
          ))}
        </div>
      </div>

      {/* Card 3: Most Stable Node */}
      <div
        role={snapshot.bestNode ? 'button' : undefined}
        tabIndex={snapshot.bestNode ? 0 : undefined}
        onClick={snapshot.bestNode ? () => toggleFocusNode(snapshot.bestNode!.nodeId) : undefined}
        onKeyDown={snapshot.bestNode ? onActivate(() => toggleFocusNode(snapshot.bestNode!.nodeId)) : undefined}
        className={`rp-lat-snap-card${snapshot.bestNode ? ' is-clickable' : ''}`}
        data-testid="latency-snap-best"
      >
        <div className="rp-lat-snap-label">
          <span>{t('latencySnapBestNode')}</span>
          {snapshot.bestNode && <span className="rp-lat-best" style={{ marginInlineStart: 0 }}>{t('latencyBest')}</span>}
        </div>
        {snapshot.bestNode ? (
          <div className="rp-lat-snap-main">
            <LevelDot level={snapshot.bestNode.level} />
            <span>{names.get(snapshot.bestNode.nodeId) ?? snapshot.bestNode.nodeId}</span>
            <span className="rp-mono" style={{ fontSize: 13, fontWeight: 500, color: 'var(--rp-text-secondary)' }}>
              {formatMs(snapshot.bestNode.avgMs)} · {formatLoss(snapshot.bestNode.avgLoss)}
            </span>
          </div>
        ) : (
          <Text type="secondary">-</Text>
        )}
      </div>

      {/* Card 4: Weakest Link */}
      <div
        role={snapshot.worstPair ? 'button' : undefined}
        tabIndex={snapshot.worstPair ? 0 : undefined}
        onClick={
          snapshot.worstPair
            ? () => {
                setSelected(snapshot.worstPair!.targetId);
                setFocusedNode(snapshot.worstPair!.nodeId);
              }
            : undefined
        }
        onKeyDown={
          snapshot.worstPair
            ? onActivate(() => {
                setSelected(snapshot.worstPair!.targetId);
                setFocusedNode(snapshot.worstPair!.nodeId);
              })
            : undefined
        }
        className={`rp-lat-snap-card${snapshot.worstPair ? ' is-clickable' : ''}`}
        data-testid="latency-snap-worst"
      >
        <div className="rp-lat-snap-label"><span>{t('latencySnapWorstPair')}</span></div>
        {snapshot.worstPair ? (
          <div className="rp-lat-snap-main">
            <LevelDot level={snapshot.worstPair.level} />
            <span style={{ fontSize: 13.5 }}>
              {names.get(snapshot.worstPair.nodeId) ?? snapshot.worstPair.nodeId}
              {worstTarget ? ` → ${worstTarget.name}` : ''}
            </span>
            <span className={`rp-lat-chip is-${snapshot.worstPair.level === 'bad' ? 'bad' : 'warn'}`}>
              {snapshot.worstPair.avg == null ? t('latencyAllLost') : formatMs(snapshot.worstPair.avg)}
              {' · '}
              {formatLoss(snapshot.worstPair.loss)}
              {snapshot.worstPair.slow != null && snapshot.worstPair.slow > 0
                ? ` · ${t('latencyCardSlow')} ${formatLoss(snapshot.worstPair.slow)}`
                : ''}
            </span>
          </div>
        ) : snapshot.totalPairs === 0 ? (
          <Text type="secondary">{t('latencyNoData')}</Text>
        ) : snapshot.missingPairs > 0 ? (
          <div className="rp-lat-snap-main" style={{ color: 'var(--rp-text-secondary)', fontWeight: 500, fontSize: 14 }}>
            <LevelDot level="none" />
            <span>{t('latencySnapMeasuredNoIssue')}</span>
          </div>
        ) : (
          <div className="rp-lat-snap-main" style={{ color: '#047857', fontWeight: 500, fontSize: 14 }}>
            <LevelDot level="good" />
            <span>{t('latencySnapNoIssue')}</span>
          </div>
        )}
      </div>
    </div>
  ) : null;

  return (
    <div className="rp-lat">
      {header}
      {failAlert}
      {snapshotBanner}
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
          <Space size={[10, 6]} wrap>
            {(nodes.length > 1 || onlyAnomalies) && (
              <label className="rp-lat-smooth">
                <Switch
                  size="small"
                  checked={onlyAnomalies}
                  onChange={setOnlyAnomalies}
                  data-testid="latency-only-anomalies"
                  aria-label={t('latencyOnlyAnomalies')}
                />
                <span>{t('latencyOnlyAnomalies')}</span>
              </label>
            )}
            {nodes.length > 1 && (
              <Segmented
                size="small"
                data-testid="latency-matrix-sort"
                value={sortMode}
                onChange={(v) => setSortMode(v as 'default' | 'overall' | 'active')}
                options={[
                  { value: 'default', label: t('latencySortDefault') },
                  { value: 'overall', label: t('latencySortOverall') },
                  { value: 'active', label: t('latencySortActiveCol') },
                ]}
              />
            )}
            {availableCarriers.length > 1 ? (
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
            ) : null}
          </Space>
        }
      >
        <Table<GroupLatencyNode>
          className="rp-lat-matrix"
          data-testid="latency-matrix"
          size="small"
          rowKey="node_id"
          columns={columns}
          dataSource={matrixNodes}
          pagination={false}
          scroll={{ x: 'max-content' }}
          locale={{
            emptyText: (
              <Empty
                image={Empty.PRESENTED_IMAGE_SIMPLE}
                description={onlyAnomalies && nodes.length > 0 ? t('latencyNoAnomalies') : t('latencyGroupNoNodes')}
              >
                {onlyAnomalies && nodes.length > 0 && (
                  <Button size="small" onClick={() => setOnlyAnomalies(false)}>
                    {t('latencyShowAllNodes')}
                  </Button>
                )}
              </Empty>
            ),
          }}
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
          hideLegend={activeKpiStrip != null}
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
