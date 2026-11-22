import { useMemo } from 'react';
import { Card, Typography } from 'antd';
import { useI18n } from '../i18n/context';
import { useIsMobile } from '../hooks/useIsMobile';
import type { LatencyPoint } from '../api/types';
import LevelDot from './LevelDot';
import { LEVEL_COLORS, latencyLevel, lossStripLevel, type LatencyKind, type LatencyLevel } from '../utils/latency';
import {
  formatAvailability,
  formatChartTime,
  formatLoss,
  formatMs,
  formatSampleTime,
  showRaw,
  slowText,
  stripLatHeights,
  stripLatencyLevel,
  stripLossHeights,
  stripSegmentCount,
  stripSegments,
  type StripSegment,
} from '../utils/latencyChart';

const { Text } = Typography;

/** The newest reading of a row: `ms` null means every probe of it was lost. A
 *  null `current` is "no reading". */
export interface StripCurrent {
  ms: number | null;
  /** Loss and slow-handshake share of that same reading, which tint the number
   *  like the cards do (a null share is not 0%: it is ignored). */
  loss?: number | null;
  slow?: number | null;
  /** The reading's mean with slow handshakes included. */
  rawMs?: number | null;
  /** When the reading was sampled (unix seconds), and whether that is too long
   *  ago to call it current: a stale reading has no colour and no number. */
  sampleTs?: number | null;
  stale?: boolean;
}

export interface StripRow {
  key: string;
  name: string;
  /** What the row probes: TCP says "failed" and "slow handshakes", ICMP "loss". */
  kind?: LatencyKind;
  /** Set instead of drawing strips (node too old, target unsupported). */
  note?: string;
  points: LatencyPoint[];
  current: StripCurrent | null;
  /** Failed probes / loss over the whole selected range, as the table / card shows it. */
  loss: number | null;
  /** Slow-handshake share over the same range; null / absent when not reported. */
  slow?: number | null;
}

interface Props {
  rows: StripRow[];
  since: number;
  until: number;
  /** Data bucket of the points; caps the slice count so no slice falls between points. */
  bucketSecs: number;
  /** A range of a day or more needs the date in the slice labels too. */
  withDate: boolean;
  /** The page's clock (unix seconds), for the date of a stale reading's time. */
  nowSecs: number;
  testId?: string;
}

function Strip({
  rowKey,
  kind,
  segs,
  heights,
  levelOf,
  describe,
}: {
  rowKey: string;
  kind: 'lat' | 'loss';
  segs: StripSegment[];
  heights: number[];
  levelOf: (g: StripSegment) => LatencyLevel;
  describe: (g: StripSegment) => string;
}) {
  return (
    <div className="rp-lat-strip" data-testid={`strip-${kind}-${rowKey}`}>
      {segs.map((g, i) => {
        const level = levelOf(g);
        const text = describe(g);
        const h = heights[i] ?? 60;
        return (
          <div
            key={i}
            role="img"
            aria-label={text}
            title={text}
            data-testid={`strip-${kind}-${rowKey}-${i}`}
            data-level={level}
            className="rp-lat-slice"
            style={{ background: LEVEL_COLORS[level], height: `${h}%` }}
          />
        );
      })}
    </div>
  );
}

/** The loss number without its unit, so the unit can be set small. */
const lossNumber = (v: number | null) => (v == null ? '-' : formatLoss(v).replace('%', ''));
const msNumber = (v: number) => (v >= 100 ? String(Math.round(v)) : v.toFixed(1));

/**
 * The status strips under the trend chart: per row, the current latency over a
 * run of 30 equal slices of the selected range (each coloured by its mean
 * latency), and beside it the range's loss over a run coloured by each slice's
 * mean loss. The data is the series the page already holds; nothing is fetched.
 * Slice hover text is the slice's time span, mean latency and loss.
 */
export default function LatencyStrips({ rows, since, until, bucketSecs, withDate, nowSecs, testId }: Props) {
  const { t } = useI18n();
  const mobile = useIsMobile();

  const prepared = useMemo(
    () => {
      const n = stripSegmentCount(since, until, bucketSecs);
      return rows.map((r) => {
        const segs = r.note ? [] : stripSegments(r.points, since, until, n);
        return {
          row: r,
          segs,
          latHeights: stripLatHeights(segs),
          lossHeights: stripLossHeights(segs),
        };
      });
    },
    [rows, since, until, bucketSecs],
  );

  const span = (g: StripSegment) =>
    `${formatChartTime(new Date(g.start * 1000), withDate)} - ${formatChartTime(new Date(g.end * 1000), withDate)}`;
  const latText = (g: StripSegment) =>
    g.count === 0 ? t('latencyNoData') : g.allLost ? t('latencyAllLost') : formatMs(g.avg);
  const lossText = (g: StripSegment) => (g.loss == null ? t('latencyNoData') : formatLoss(g.loss));
  const failKey = (kind?: LatencyKind) => (kind === 'tcp' ? 'latencyCardFailed' : 'latencyCardLoss');
  const describeFor = (kind?: LatencyKind) => (g: StripSegment) => {
    const raw = showRaw(g.avg, g.rawAvg, g.slow) ? ` (${t('latencyCardRaw')} ${formatMs(g.rawAvg)})` : '';
    const slow = g.count === 0 ? null : slowText(g.slow, kind);
    return (
      `${span(g)}\n${t('latencyCardAvg')} ${latText(g)}${raw} · ${t(failKey(kind))} ${lossText(g)}` +
      (slow == null ? '' : ` · ${t('latencyCardSlow')} ${slow}`)
    );
  };

  const headStyle = { display: 'flex', alignItems: 'baseline', justifyContent: 'space-between', gap: 8, minHeight: 26 } as const;

  const startAxis = formatChartTime(new Date(since * 1000), withDate);
  const legend = (
    <div className="rp-lat-legend">
      <span className="rp-lat-legend-item">
        <span className="rp-lat-legend-swatch" style={{ background: LEVEL_COLORS.good }} />
        {t('latencyLegendGood')}
      </span>
      <span className="rp-lat-legend-item">
        <span className="rp-lat-legend-swatch" style={{ background: LEVEL_COLORS.warn }} />
        {t('latencyLegendWarn')}
      </span>
      <span className="rp-lat-legend-item">
        <span className="rp-lat-legend-swatch" style={{ background: LEVEL_COLORS.bad }} />
        {t('latencyLegendBad')}
      </span>
    </div>
  );

  return (
    <Card size="small" className="rp-lat-card" title={t('latencyStripsTitle')} extra={legend} style={{ marginTop: 16 }} data-testid={testId}>
      <div style={{ display: 'flex', flexDirection: 'column', gap: 12 }}>
        {prepared.map(({ row, segs, latHeights, lossHeights }) => {
          const cur = row.current;
          const stale = cur?.stale === true;
          const curLevel: LatencyLevel =
            cur == null || stale ? 'none' : cur.ms == null ? 'bad' : latencyLevel(cur.ms, cur.loss, cur.slow);
          const rangeSlow = slowText(row.slow, row.kind);
          const describe = describeFor(row.kind);
          const lossLevel = lossStripLevel(row.loss);
          const avail = formatAvailability(row.loss);
          const slaClass = row.loss == null ? '' : row.loss >= 5 ? 'is-bad' : row.loss >= 1 ? 'is-warn' : 'is-good';
          return (
            <div
              key={row.key}
              data-testid={`strip-row-${row.key}`}
              className="rp-lat-strip-card"
              style={{
                display: 'grid',
                gridTemplateColumns: mobile ? 'minmax(0, 1fr)' : 'minmax(0, 1fr) minmax(0, 1fr)',
                columnGap: 24,
                rowGap: 14,
              }}
            >
              <div style={{ minWidth: 0 }}>
                <div style={headStyle}>
                  <span style={{ display: 'inline-flex', alignItems: 'center', gap: 8, minWidth: 0 }}>
                    <Text strong ellipsis style={{ minWidth: 0, fontSize: 13.5 }}>{row.name}</Text>
                    {!row.note && avail != null && (
                      <span
                        className={`rp-lat-sla-pill ${slaClass}`}
                        data-testid={`strip-sla-${row.key}`}
                        title={`${t('latencyAvailability')} ${avail}`}
                      >
                        {t('latencyAvailability')} {avail}
                      </span>
                    )}
                  </span>
                  {row.note ? null : (
                    <span data-testid={`strip-current-${row.key}`} data-level={curLevel} style={{ whiteSpace: 'nowrap' }}>
                      {cur == null ? (
                        <Text type="secondary" style={{ fontSize: 13 }}>{t('latencyNoData')}</Text>
                      ) : stale ? (
                        <Text type="secondary" style={{ fontSize: 13 }} data-testid={`strip-stale-${row.key}`}>
                          <LevelDot level="none" />
                          {t('latencyNoNewData')}
                          {cur.sampleTs != null ? (
                            <span style={{ marginLeft: 6 }}>
                              {t('latencyLastAt')} <span data-testid={`strip-stale-time-${row.key}`}>{formatSampleTime(cur.sampleTs, nowSecs * 1000)}</span>
                            </span>
                          ) : null}
                        </Text>
                      ) : cur.ms == null ? (
                        <span className="rp-lat-big" style={{ fontSize: 15 }}>
                          <LevelDot level="bad" />
                          {t('latencyAllLost')}
                        </span>
                      ) : (
                        <>
                          <LevelDot level={curLevel} />
                          <span className="rp-lat-big">{msNumber(cur.ms)}</span>
                          <span className="rp-lat-big-unit">ms</span>
                          {showRaw(cur.ms, cur.rawMs, cur.slow) ? (
                            <Text type="secondary" style={{ fontSize: 12, marginLeft: 6 }} data-testid={`strip-raw-${row.key}`}>
                              {t('latencyCardRaw')} {formatMs(cur.rawMs)}
                            </Text>
                          ) : null}
                        </>
                      )}
                    </span>
                  )}
                </div>
                {row.note ? (
                  <Text type="secondary" style={{ display: 'block', marginTop: 6, fontSize: 12 }} data-testid={`strip-note-${row.key}`}>
                    {row.note}
                  </Text>
                ) : (
                  <>
                    <Strip rowKey={row.key} kind="lat" segs={segs} heights={latHeights} levelOf={stripLatencyLevel} describe={describe} />
                    <div className="rp-lat-strip-axis" aria-hidden="true">
                      <span>{startAxis}</span>
                      <span>{t('latencyNowLabel')}</span>
                    </div>
                  </>
                )}
              </div>
              {row.note ? null : (
                <div style={{ minWidth: 0 }}>
                  <div style={headStyle}>
                    <Text type="secondary" style={{ fontSize: 12.5 }}>
                      {t(row.kind === 'tcp' ? 'latencyStripsFailed' : 'latencyStripsLoss')}
                    </Text>
                    <span style={{ whiteSpace: 'nowrap' }}>
                    {rangeSlow == null ? null : (
                      <Text type="secondary" style={{ fontSize: 12, marginRight: 8 }} data-testid={`strip-slow-value-${row.key}`}>
                        {t('latencyCardSlow')} {rangeSlow}
                      </Text>
                    )}
                    <span data-testid={`strip-loss-value-${row.key}`} data-level={lossLevel} style={{ whiteSpace: 'nowrap' }}>
                      <LevelDot level={lossLevel} />
                      <span className="rp-lat-big">{lossNumber(row.loss)}</span>
                      {row.loss == null ? null : <span className="rp-lat-big-unit">%</span>}
                    </span>
                    </span>
                  </div>
                  <Strip
                    rowKey={row.key}
                    kind="loss"
                    segs={segs}
                    heights={lossHeights}
                    levelOf={(g) => lossStripLevel(g.loss)}
                    describe={describe}
                  />
                  <div className="rp-lat-strip-axis" aria-hidden="true">
                    <span>{startAxis}</span>
                    <span>{t('latencyNowLabel')}</span>
                  </div>
                </div>
              )}
            </div>
          );
        })}
      </div>
    </Card>
  );
}
