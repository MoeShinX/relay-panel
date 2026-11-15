import type { GroupLatencyNode, GroupLatencySeries, LatencySeries } from '../api/types';

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
