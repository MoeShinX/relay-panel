import type { LatencySeries } from '../api/types';

/** One plotted point of one target. `value` is the bucket's mean round trip,
 *  null when every probe in it was lost (the line then breaks, which is the
 *  honest picture of an outage). */
export interface ChartPoint {
  time: Date;
  /** The target id as text: the series key, and what the legend lists. */
  key: string;
  name: string;
  value: number | null;
  min: number | null;
  max: number | null;
  loss: number;
}

/** A marker at the foot of the chart for a bucket that lost probes. */
export interface LossMarker {
  time: Date;
  key: string;
  name: string;
  loss: number;
}

/** The lines to draw: every target, or only `onlyId` when a card is selected. */
export function chartPoints(series: LatencySeries[], onlyId: number | null): ChartPoint[] {
  const out: ChartPoint[] = [];
  for (const s of series) {
    if (onlyId != null && s.target_id !== onlyId) continue;
    for (const p of s.points) {
      out.push({
        time: new Date(p.ts * 1000),
        key: String(s.target_id),
        name: s.name,
        value: p.avg,
        min: p.min,
        max: p.max,
        loss: p.loss,
      });
    }
  }
  return out;
}

/** Where probes were lost: one marker per bucket with any loss, so an outage
 *  and a stray dropped packet are both visible without reading the tooltip. */
export function lossMarkers(points: ChartPoint[]): LossMarker[] {
  return points
    .filter((p) => p.loss > 0)
    .map((p) => ({ time: p.time, key: p.key, name: p.name, loss: p.loss }));
}

const pad = (n: number) => String(n).padStart(2, '0');

/** Axis / tooltip time. A range of a day or more needs the date too. */
export function formatChartTime(d: Date, withDate: boolean): string {
  const hm = `${pad(d.getHours())}:${pad(d.getMinutes())}`;
  return withDate ? `${pad(d.getMonth() + 1)}-${pad(d.getDate())} ${hm}` : hm;
}

export function formatMs(v: number | null | undefined): string {
  if (v == null) return '-';
  return v >= 100 ? `${Math.round(v)} ms` : `${v.toFixed(1)} ms`;
}

export function formatLoss(v: number | null | undefined): string {
  if (v == null) return '-';
  return v === 0 ? '0%' : v < 1 ? `${v.toFixed(2)}%` : `${v.toFixed(1)}%`;
}
