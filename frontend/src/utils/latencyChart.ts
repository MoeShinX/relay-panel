import type { GroupLatencyNode, GroupLatencySeries, LatencyPoint, LatencySeries } from '../api/types';
import { STRIP_SEGMENTS, latencyLevel, type LatencyLevel } from './latency';

/** One plotted point of one line. `value` is the bucket's mean round trip,
 *  null when every probe in it was lost (the line then breaks, which is the
 *  honest picture of an outage). */
export interface ChartPoint {
  time: Date;
  /** The line's key (a target id on a node's page, a node id on a group's
   *  page, as text): the series key, and what the legend lists. */
  key: string;
  name: string;
  value: number | null;
  min: number | null;
  max: number | null;
  loss: number;
  /** Percent of answered probes that were TCP retransmits; null = not reported. */
  retrans: number | null;
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
        retrans: p.retrans_rate ?? null,
      });
    }
  }
  return out;
}

/** What a node is called on the comparison page: its name when it has one,
 *  otherwise a shortened node id. */
export function nodeDisplayName(n: Pick<GroupLatencyNode, 'node_id' | 'label'>): string {
  if (n.label) return n.label;
  return n.node_id.length > 12 ? `${n.node_id.slice(0, 8)}…${n.node_id.slice(-3)}` : n.node_id;
}

/** The lines of the group chart: one per node, keyed by node id and named for
 *  the node, for the one target the chart shows. */
export function groupChartPoints(series: GroupLatencySeries[], nodes: GroupLatencyNode[]): ChartPoint[] {
  const names = new Map(nodes.map((n) => [n.node_id, nodeDisplayName(n)]));
  const out: ChartPoint[] = [];
  for (const s of series) {
    const name = names.get(s.node_id) ?? s.node_id;
    for (const p of s.points) {
      out.push({
        time: new Date(p.ts * 1000),
        key: s.node_id,
        name,
        value: p.avg,
        min: p.min,
        max: p.max,
        loss: p.loss,
        retrans: p.retrans_rate ?? null,
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

/** The retransmit rate as text, same style as the loss; null when the data does
 *  not carry it, so callers leave it out instead of showing "0%". */
export function formatRetrans(v: number | null | undefined): string | null {
  return v == null ? null : formatLoss(v);
}

/** One slice of the selected range, for the status strips. */
export interface StripSegment {
  /** Unix seconds: the slice covers [start, end), the last one also `end`. */
  start: number;
  end: number;
  /** How many points fell in the slice. */
  count: number;
  /** Mean of the points' averages (null points ignored); null when none. */
  avg: number | null;
  /** Mean of the points' loss percentages; null when the slice has no points. */
  loss: number | null;
  /** Mean of the points' retransmit rates that carry one; null when none does. */
  retrans: number | null;
  /** Points exist but every one lost all its probes. */
  allLost: boolean;
}

/** How many strip slices a range gets: one per data bucket, at most
 *  STRIP_SEGMENTS. Slicing finer than the buckets would leave grey gaps between
 *  points (hourly data over a day, 5-minute data over an hour). */
export function stripSegmentCount(since: number, until: number, bucketSecs: number): number {
  if (!(bucketSecs > 0) || !(until > since)) return STRIP_SEGMENTS;
  return Math.max(1, Math.min(STRIP_SEGMENTS, Math.ceil((until - since) / bucketSecs)));
}

/** Cut [since, until] into `n` equal slices and fold the points into them. A
 *  point belongs to the slice holding its timestamp; one exactly on an inner
 *  boundary goes to the later slice, one at `until` to the last. Points
 *  outside the range are ignored. A slice with no point has no data (avg and
 *  loss null); with points that all lost every probe it is `allLost`. */
export function stripSegments(
  points: LatencyPoint[],
  since: number,
  until: number,
  n: number = STRIP_SEGMENTS,
): StripSegment[] {
  const width = (until - since) / n;
  const segs = Array.from({ length: n }, (_, i) => ({
    start: since + i * width,
    end: i === n - 1 ? until : since + (i + 1) * width,
    count: 0,
    avgSum: 0,
    avgN: 0,
    lossSum: 0,
    retransSum: 0,
    retransN: 0,
  }));
  if (width > 0) {
    for (const p of points) {
      if (!(p.ts >= since && p.ts <= until)) continue;
      const i = Math.min(n - 1, Math.floor(((p.ts - since) * n) / (until - since)));
      const g = segs[i];
      g.count += 1;
      g.lossSum += p.loss;
      if (p.retrans_rate != null) {
        g.retransSum += p.retrans_rate;
        g.retransN += 1;
      }
      if (p.avg != null) {
        g.avgSum += p.avg;
        g.avgN += 1;
      }
    }
  }
  return segs.map((g) => ({
    start: g.start,
    end: g.end,
    count: g.count,
    avg: g.avgN > 0 ? g.avgSum / g.avgN : null,
    loss: g.count > 0 ? g.lossSum / g.count : null,
    retrans: g.retransN > 0 ? g.retransSum / g.retransN : null,
    allLost: g.count > 0 && g.avgN === 0,
  }));
}

/** Colour class of a latency strip slice: grey with no data, red when every
 *  probe was lost, otherwise by the latency thresholds alone (loss has its own
 *  strip). */
export function stripLatencyLevel(seg: Pick<StripSegment, 'count' | 'avg' | 'allLost'>): LatencyLevel {
  if (seg.count === 0) return 'none';
  if (seg.allLost) return 'bad';
  return latencyLevel(seg.avg, null);
}
