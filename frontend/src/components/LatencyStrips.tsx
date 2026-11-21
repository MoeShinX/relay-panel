import { useMemo } from 'react';
import { Card, Typography } from 'antd';
import { useI18n } from '../i18n/context';
import { useIsMobile } from '../hooks/useIsMobile';
import type { LatencyPoint } from '../api/types';
import LevelDot from './LevelDot';
import { LEVEL_COLORS, latencyLevel, lossStripLevel, type LatencyLevel } from '../utils/latency';
import {
  formatChartTime,
  formatLoss,
  formatMs,
  formatRetrans,
  stripLatencyLevel,
  stripSegmentCount,
  stripSegments,
  type StripSegment,
} from '../utils/latencyChart';

const { Text } = Typography;

/** The newest reading of a row: `ms` null means every probe of it was lost. A
 *  null `current` is "no reading". */
export interface StripCurrent {
  ms: number | null;
  /** Loss of that same reading, which tints the number like the cards do. */
  loss?: number | null;
}

export interface StripRow {
  key: string;
  name: string;
  /** Set instead of drawing strips (node too old, target unsupported). */
  note?: string;
  points: LatencyPoint[];
  current: StripCurrent | null;
  /** Loss over the whole selected range, as the table / card shows it. */
  loss: number | null;
  /** Retransmit share over the same range; null / absent when not reported. */
  retrans?: number | null;
}

interface Props {
  rows: StripRow[];
  since: number;
  until: number;
  /** Data bucket of the points; caps the slice count so no slice falls between points. */
  bucketSecs: number;
  /** A range of a day or more needs the date in the slice labels too. */
  withDate: boolean;
  testId?: string;
}

function Strip({
  rowKey,
  kind,
  segs,
  levelOf,
  describe,
}: {
  rowKey: string;
  kind: 'lat' | 'loss';
  segs: StripSegment[];
  levelOf: (g: StripSegment) => LatencyLevel;
  describe: (g: StripSegment) => string;
}) {
  return (
    <div className="rp-lat-strip" data-testid={`strip-${kind}-${rowKey}`}>
      {segs.map((g, i) => {
        const level = levelOf(g);
        const text = describe(g);
        return (
          <div
            key={i}
            role="img"
            aria-label={text}
            title={text}
            data-testid={`strip-${kind}-${rowKey}-${i}`}
            data-level={level}
            className="rp-lat-slice"
            style={{ background: LEVEL_COLORS[level] }}
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
export default function LatencyStrips({ rows, since, until, bucketSecs, withDate, testId }: Props) {
  const { t } = useI18n();
  const mobile = useIsMobile();

  const prepared = useMemo(
    () => {
      const n = stripSegmentCount(since, until, bucketSecs);
      return rows.map((r) => ({ row: r, segs: r.note ? [] : stripSegments(r.points, since, until, n) }));
    },
    [rows, since, until, bucketSecs],
  );

  const span = (g: StripSegment) =>
    `${formatChartTime(new Date(g.start * 1000), withDate)} - ${formatChartTime(new Date(g.end * 1000), withDate)}`;
  const latText = (g: StripSegment) =>
    g.count === 0 ? t('latencyNoData') : g.allLost ? t('latencyAllLost') : formatMs(g.avg);
  const lossText = (g: StripSegment) => (g.loss == null ? t('latencyNoData') : formatLoss(g.loss));
  const retransText = (g: StripSegment) => {
    const r = formatRetrans(g.retrans);
    return r == null ? '' : ` · ${t('latencyCardRetrans')} ${r}`;
  };
  const describe = (g: StripSegment) =>
    `${span(g)}\n${t('latencyCardAvg')} ${latText(g)} · ${t('latencyCardLoss')} ${lossText(g)}${retransText(g)}`;

  const headStyle = { display: 'flex', alignItems: 'baseline', justifyContent: 'space-between', gap: 8, minHeight: 26 } as const;

  return (
    <Card size="small" className="rp-lat-card" title={t('latencyStripsTitle')} style={{ marginTop: 16 }} data-testid={testId}>
      <div style={{ display: 'flex', flexDirection: 'column', gap: 20 }}>
        {prepared.map(({ row, segs }) => {
          const cur = row.current;
          const curLevel: LatencyLevel =
            cur == null ? 'none' : cur.ms == null ? 'bad' : latencyLevel(cur.ms, cur.loss);
          const lossLevel = lossStripLevel(row.loss);
          return (
            <div
              key={row.key}
              data-testid={`strip-row-${row.key}`}
              style={{
                display: 'grid',
                gridTemplateColumns: mobile ? 'minmax(0, 1fr)' : 'minmax(0, 1fr) minmax(0, 1fr)',
                columnGap: 28,
                rowGap: 12,
              }}
            >
              <div style={{ minWidth: 0 }}>
                <div style={headStyle}>
                  <Text strong ellipsis style={{ minWidth: 0 }}>{row.name}</Text>
                  {row.note ? null : (
                    <span data-testid={`strip-current-${row.key}`} data-level={curLevel} style={{ whiteSpace: 'nowrap' }}>
                      {cur == null ? (
                        <Text type="secondary" style={{ fontSize: 13 }}>{t('latencyNoData')}</Text>
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
                  <Strip rowKey={row.key} kind="lat" segs={segs} levelOf={stripLatencyLevel} describe={describe} />
                )}
              </div>
              {row.note ? null : (
                <div style={{ minWidth: 0 }}>
                  <div style={headStyle}>
                    <Text type="secondary" style={{ fontSize: 13 }}>{t('latencyStripsLoss')}</Text>
                    <span style={{ whiteSpace: 'nowrap' }}>
                    {formatRetrans(row.retrans) == null ? null : (
                      <Text type="secondary" style={{ fontSize: 12, marginRight: 8 }} data-testid={`strip-retrans-value-${row.key}`}>
                        {t('latencyCardRetrans')} {formatRetrans(row.retrans)}
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
                    levelOf={(g) => lossStripLevel(g.loss)}
                    describe={describe}
                  />
                </div>
              )}
            </div>
          );
        })}
      </div>
    </Card>
  );
}
