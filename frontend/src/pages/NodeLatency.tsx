import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { Alert, Button, Card, Empty, Result, Space, Spin, Tag, Tooltip, Typography } from 'antd';
import { ArrowLeftOutlined } from '@ant-design/icons';
import { useNavigate, useParams } from 'react-router-dom';
import api from '../api/client';
import type { ApiEnvelope, LatencySeries, LatencySummary, NodeLatencyResponse, NodeStatus } from '../api/types';
import { useI18n } from '../i18n/context';
import LatencyControls from '../components/LatencyControls';
import LatencyStats, { type StatsRow } from '../components/LatencyStats';
import LatencyStrips, { type StripRow } from '../components/LatencyStrips';
import LatencyTrendCard, { ChartBox, ChartBoxMessage } from '../components/LatencyTrendCard';
import LevelDot from '../components/LevelDot';
import SeriesPicker from '../components/SeriesPicker';
import { useSeriesPick } from '../hooks/useSeriesPick';
import { rankByAttention } from '../utils/seriesPick';
import {
  AUTO_REFRESH_MS,
  autoRefreshes,
  granularityAllowed,
  latencyLevel,
  type LatencyGranularity,
  type LatencyRange,
} from '../utils/latency';
import {
  chartPoints,
  formatLoss,
  formatMs,
  formatRetrans,
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

/**
 * v1.2.21: one node's latency to the targets of its device group — a card per
 * target and a line chart. Admin-only (the route is wrapped in RequireAdmin, and
 * every call here is AdminOnly on the server).
 *
 * The page never decides whether data is "good": the cards are coloured by
 * fixed thresholds (utils/latency.ts), and a bucket that lost every probe shows
 * as a gap with a marker rather than a zero.
 */
export default function NodeLatency() {
  const { t } = useI18n();
  const navigate = useNavigate();
  const { nodeId = '' } = useParams();

  const [range, setRange] = useState<LatencyRange>('24h');
  const [granularity, setGranularity] = useState<LatencyGranularity>('1h');
  const [data, setData] = useState<NodeLatencyResponse | null>(null);
  const [state, setState] = useState<LoadState>('loading');
  const [selected, setSelected] = useState<number | null>(null);
  const [node, setNode] = useState<{ name: string; ip: string } | null>(null);
  // Only the newest request may write the page: a slow answer for a range the
  // admin has since left must not overwrite the one they are looking at.
  const requestRef = useRef(0);

  const load = useCallback(
    async (silent: boolean) => {
      const mine = ++requestRef.current;
      if (!silent) setState('loading');
      try {
        const res = await api.get<unknown, ApiEnvelope<NodeLatencyResponse>>(
          `/admin/nodes/${encodeURIComponent(nodeId)}/latency?range=${range}&granularity=${granularity}`,
        );
        if (mine !== requestRef.current) return;
        if (res.code === 404) {
          setState('notfound');
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
    [nodeId, range, granularity],
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

  // The node's name and address for the header, from the admin node list.
  useEffect(() => {
    let alive = true;
    api
      .get<unknown, ApiEnvelope<NodeStatus[]>>('/nodes')
      .then((res) => {
        if (!alive || res.code !== 0) return;
        const row = (res.data || []).find((n) => n.node_id === nodeId);
        if (!row) return;
        const r = row as NodeStatus & { public_ipv4?: string | null; public_ip?: string | null };
        setNode({
          name: row.label || row.group_name || '',
          ip: r.public_ipv4 || r.public_ip || '',
        });
      })
      .catch(() => {});
    return () => {
      alive = false;
    };
  }, [nodeId]);

  const onRange = (r: LatencyRange) => {
    setRange(r);
    if (!granularityAllowed(r, granularity)) setGranularity('1h');
  };

  const series: LatencySeries[] = useMemo(() => data?.series ?? [], [data]);
  const summaryById = useMemo(() => {
    const m = new Map<number, LatencySummary>();
    for (const s of data?.summary ?? []) m.set(s.target_id, s);
    return m;
  }, [data]);
  const unsupportedIds = useMemo(() => new Set(data?.unsupported_targets ?? []), [data]);

  // A selected target that no longer exists (deleted meanwhile) selects nothing.
  const selectedSeries = selected != null ? series.find((s) => s.target_id === selected) : undefined;
  const onlyId = selectedSeries ? selectedSeries.target_id : null;

  const names = useMemo(() => new Map(series.map((s) => [String(s.target_id), s.name])), [series]);
  // One line per target. Past ten the palette runs out: the picker chooses
  // which to draw, by default the ten that need attention first over the range
  // (worst latency level, then loss, then mean; a target this node cannot probe
  // or with no reading last). A selected card narrows the chart to that one
  // target and the picker steps aside.
  const targetKeys = useMemo(() => series.map((s) => String(s.target_id)), [series]);
  const ranked = useMemo(
    () =>
      rankByAttention(targetKeys, (k) => {
        const id = Number(k);
        return unsupportedIds.has(id) ? null : summaryById.get(id);
      }),
    [targetKeys, summaryById, unsupportedIds],
  );
  const pick = useSeriesPick(targetKeys, ranked);
  const showPicker = pick.needsPicker && onlyId == null;
  const colorDomain = onlyId == null ? pick.shown : targetKeys;
  const points: ChartPoint[] = useMemo(() => {
    if (onlyId != null || !pick.needsPicker) return chartPoints(series, onlyId);
    const shown = new Set(pick.shown);
    return chartPoints(series.filter((s) => shown.has(String(s.target_id))), null);
  }, [series, onlyId, pick.needsPicker, pick.shown]);
  const withDate = range !== '1h' && range !== '6h';

  // The stats table above the chart: the selected target, or every target.
  const statsRows: StatsRow[] = useMemo(
    () =>
      series
        .filter((s) => onlyId == null || s.target_id === onlyId)
        .map((s) => {
          const sum = summaryById.get(s.target_id);
          if (unsupportedIds.has(s.target_id)) {
            return {
              key: String(s.target_id), name: s.name, note: t('latencyIcmpUnsupported'),
              latest: null, avg: null, min: null, max: null, loss: null, retrans: null,
            };
          }
          return {
            key: String(s.target_id),
            name: s.name,
            latest: sum?.latest ?? null,
            latestAllLost: sum?.latest_ts != null && sum.latest == null,
            avg: sum?.avg ?? null,
            min: sum?.min ?? null,
            max: sum?.max ?? null,
            loss: sum?.loss ?? null,
            retrans: sum?.retrans_rate ?? null,
          };
        }),
    [series, summaryById, unsupportedIds, onlyId, t],
  );

  // One strip row per target, over the full range whatever card is selected.
  // "Now" and the range loss are the cards' own figures.
  const stripRows: StripRow[] = useMemo(
    () =>
      series.map((s) => {
        const sum = summaryById.get(s.target_id);
        const base = { key: String(s.target_id), name: s.name, points: s.points, current: null, loss: null, retrans: null };
        if (unsupportedIds.has(s.target_id)) return { ...base, points: [], note: t('latencyIcmpUnsupported') };
        return {
          ...base,
          current: sum?.latest_ts != null ? { ms: sum.latest, loss: sum.latest_loss } : null,
          loss: sum?.loss ?? null,
          retrans: sum?.retrans_rate ?? null,
        };
      }),
    [series, summaryById, unsupportedIds, t],
  );

  const picker = showPicker ? (
    <SeriesPicker
      testId="series-picker"
      placeholder={t('seriesPickTargets')}
      options={series.map((s) => ({ value: String(s.target_id), label: s.name }))}
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
        <Title level={4} style={{ margin: 0 }} data-testid="latency-title">
          {t('latencyTitle')}
          {node?.name ? ` · ${node.name}` : ''}
        </Title>
        {node?.ip ? <Text type="secondary" className="rp-mono">{node.ip}</Text> : null}
        {/* The group whose targets these are; a way back to its comparison. */}
        {data?.group_name ? (
          <Tooltip title={t('latencyGroupLink')}>
            <Tag
              color="blue"
              data-testid="latency-group-link"
              style={{ cursor: 'pointer', marginInlineEnd: 0 }}
              onClick={() => navigate(`/groups/${data.group_id}/latency`)}
            >
              {data.group_name}
            </Tag>
          </Tooltip>
        ) : null}
      </Space>
      <LatencyControls range={range} granularity={granularity} onRange={onRange} onGranularity={setGranularity} />
    </div>
  );

  if (state === 'notfound') {
    return (
      <div className="rp-lat">
        {header}
        <Result status="404" title={t('latencyNodeNotFound')} />
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

  if (data && !data.supported) {
    return (
      <div className="rp-lat">
        {header}
        <Alert
          type="warning"
          showIcon
          message={t('latencyNodeTooOld')}
          description={data.node_version ? `${t('latencyNodeVersion')}: v${data.node_version}` : undefined}
        />
      </div>
    );
  }

  // The admin switched this machine's monitor off (Groups page): the panel no
  // longer records it, and what is below is from before that.
  const offAlert = data?.monitoring_off ? (
    <Alert
      type="info"
      showIcon
      style={{ marginBottom: 12 }}
      data-testid="latency-monitoring-off"
      title={t('latencyMonitoringOff')}
    />
  ) : null;

  if (data && series.length === 0) {
    return (
      <div className="rp-lat">
        {header}
        {offAlert}
        <Result
          icon={<Empty image={Empty.PRESENTED_IMAGE_SIMPLE} />}
          title={t('latencyNoTargets')}
          extra={<Button type="primary" onClick={() => navigate('/groups')}>{t('latencyGoGroups')}</Button>}
        />
      </div>
    );
  }

  const cards = (
    <div
      style={{ display: 'grid', gridTemplateColumns: 'repeat(auto-fill, minmax(min(100%, 150px), 1fr))', gap: 12, marginBottom: 16 }}
      data-testid="latency-cards"
    >
      {series.map((s) => {
        const sum = summaryById.get(s.target_id);
        const unsupported = unsupportedIds.has(s.target_id);
        const hasLatest = sum?.latest_ts != null;
        const level = hasLatest
          ? latencyLevel(sum?.latest, sum?.latest_loss)
          : latencyLevel(sum?.avg, sum?.loss);
        const isSelected = onlyId === s.target_id;
        let big: string;
        if (unsupported) big = '-';
        else if (hasLatest && sum?.latest != null) big = formatMs(sum.latest);
        else if (hasLatest) big = t('latencyAllLost');
        else big = t('latencyNoData');
        const toggle = () => setSelected(isSelected ? null : s.target_id);
        return (
          <Card
            key={s.target_id}
            size="small"
            hoverable
            role="button"
            tabIndex={0}
            aria-pressed={isSelected}
            data-testid={`latency-card-${s.target_id}`}
            onClick={toggle}
            onKeyDown={(e) => {
              if (e.key === 'Enter' || e.key === ' ') {
                e.preventDefault();
                toggle();
              }
            }}
            className={`rp-lat-target${isSelected ? ' is-selected' : ''}`}
          >
            <div className="rp-lat-colname">{s.name}</div>
            <div className="rp-lat-head-sub" style={{ marginBottom: 6 }}>
              {s.kind === 'icmp' ? 'ICMP' : 'TCP'} · {t(CARRIER_KEY[s.carrier])}
            </div>
            <div
              className="rp-lat-target-value"
              data-testid={`latency-card-value-${s.target_id}`}
              data-level={unsupported ? 'none' : level}
            >
              <LevelDot level={unsupported ? 'none' : level} />
              {big}
            </div>
            {unsupported ? (
              <Tooltip title={t('latencyIcmpUnsupportedTip')}>
                <Tag color="orange" style={{ marginTop: 4 }}>{t('latencyIcmpUnsupported')}</Tag>
              </Tooltip>
            ) : (
              <span className="rp-lat-sub">
                {t('latencyCardAvg')} {formatMs(sum?.avg)} · {t('latencyCardLoss')} {formatLoss(sum?.loss)}
                {formatRetrans(sum?.retrans_rate) == null ? null : (
                  <span data-testid={`latency-card-retrans-${s.target_id}`}>
                    {' · '}
                    {t('latencyCardRetrans')} {formatRetrans(sum?.retrans_rate)}
                  </span>
                )}
              </span>
            )}
          </Card>
        );
      })}
    </div>
  );

  return (
    <div className="rp-lat">
      {header}
      {offAlert}
      {cards}
      <LatencyTrendCard
        title={
          <Space size={8} wrap>
            <span data-testid="latency-current-view">
              {t('latencyCurrentView')}
              <Text strong>{selectedSeries ? selectedSeries.name : t('latencyAllTargets')}</Text>
            </span>
            <Text type="secondary" style={{ fontSize: 12, fontWeight: 400 }}>{t('latencyCardHint')}</Text>
          </Space>
        }
        picker={picker}
        stats={<LatencyStats testId="latency-stats" rows={statsRows} />}
        points={points}
        colorDomain={colorDomain}
        names={names}
        withDate={withDate}
        autoRefresh={autoRefreshes(range)}
        loading={state === 'loading'}
        downloadName={`latency-${nodeId}-${range}`}
      />
      {data ? (
        <LatencyStrips testId="latency-strips" rows={stripRows} since={data.since} until={data.until} bucketSecs={data.bucket_secs} withDate={withDate} />
      ) : null}
    </div>
  );
}
