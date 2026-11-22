import type {
  GroupLatencyCell,
  GroupLatencyNode,
  GroupLatencySeries,
  LatencyPoint,
  LatencySeries,
  LatencyTarget,
} from '../api/types';
import { STRIP_SEGMENTS, latencyLevel, type LatencyKind, type LatencyLevel } from './latency';
import { rankByAttention } from './seriesPick';

/** One plotted point of one line. `value` is the bucket's mean round trip,
 *  null when every probe in it was lost (the line then breaks, which is the
 *  honest picture of an outage). */
export interface ChartPoint {
  time: Date;
  /** The line's key (a target id on a node's page, a node id on a group's
   *  page, as text): the series key, and what the legend lists. */
  key: string;
  name: string;
  /** What the line is drawn at: the bucket's mean, or its smoothed value when
   *  the chart's smooth switch is on. */
  value: number | null;
  /** The bucket's own mean, whatever the line shows. Tooltips read this. */
  raw: number | null;
  /** The bucket's mean with slow handshakes included (`raw` leaves them out). */
  rawAvg: number | null;
  min: number | null;
  max: number | null;
  loss: number;
  /** Percent of answered TCP probes that were slow handshakes (connect took a
   *  second or more); null = not reported. */
  slow: number | null;
  /** What the line probes: TCP targets say "failed" and "slow handshakes", ICMP
   *  ones "loss" and no handshake figure; null when the page does not know. */
  kind: LatencyKind | null;
}

/** A marker for a bucket that lost probes. It sits on the line at that point;
 *  where the bucket lost everything the line has a gap, and the marker sits on
 *  the baseline under it. */
export interface LossMarker {
  time: Date;
  key: string;
  name: string;
  loss: number;
  /** Where to draw it: the line's value at the point, 0 for a gap. */
  y: number;
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
        raw: p.avg,
        rawAvg: p.raw_avg ?? null,
        min: p.min,
        max: p.max,
        loss: p.loss,
        slow: p.retrans_rate ?? null,
        kind: s.kind,
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
export function groupChartPoints(
  series: GroupLatencySeries[],
  nodes: GroupLatencyNode[],
  kind: LatencyKind | null = null,
): ChartPoint[] {
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
        raw: p.avg,
        rawAvg: p.raw_avg ?? null,
        min: p.min,
        max: p.max,
        loss: p.loss,
        slow: p.retrans_rate ?? null,
        kind,
      });
    }
  }
  return out;
}

/** Where nothing came back: one marker per bucket that lost every probe (the
 *  line has a gap there). Partial loss is not marked: with one-minute buckets a
 *  single dropped probe is already 17%, and marking it would dot an ordinary
 *  line red all over. The tooltip and the "recent status" loss strip show the
 *  loss of every bucket. `baseline` is where a gap
 *  (no line to sit on) gets its marker: the bottom edge of the plot. */
export function lossMarkers(points: ChartPoint[], baseline = 0): LossMarker[] {
  return points
    .filter((p) => p.loss >= 100)
    .map((p) => ({ time: p.time, key: p.key, name: p.name, loss: p.loss, y: p.value ?? baseline }));
}

/** The classic chart's markers: one for every bucket that lost anything, all on
 *  the x axis (y 0), as the pages drew them before the polish. */
export function classicLossMarkers(points: ChartPoint[]): LossMarker[] {
  return points
    .filter((p) => p.loss > 0)
    .map((p) => ({ time: p.time, key: p.key, name: p.name, loss: p.loss, y: 0 }));
}

/** The lowest value the y axis starts at: a little under the lowest drawn
 *  value, rounded down to a tidy step, and never below 0. A chart of lines at
 *  35-70 ms should not spend its lower half on 0-35. */
export function yDomainMin(points: ChartPoint[]): number {
  let lo = Infinity;
  let hi = -Infinity;
  for (const p of points) {
    if (p.value == null) continue;
    lo = Math.min(lo, p.value);
    hi = Math.max(hi, p.value);
  }
  if (!Number.isFinite(lo)) return 0;
  const span = hi - lo;
  const raw = lo - Math.max(span * 0.15, hi * 0.05, 1);
  if (raw <= 0) return 0;
  const target = Math.max(span, 1) / 4;
  const pow = 10 ** Math.floor(Math.log10(target));
  const m = target / pow;
  const step = (m >= 5 ? 5 : m >= 2 ? 2 : 1) * pow;
  return Math.max(0, Math.floor(raw / step) * step);
}

/** Weight of the newest bucket in the smoothing. */
export const SMOOTH_ALPHA = 0.35;

/** The chart's "smooth" switch: an exponential moving average of each line's
 *  mean, `s = alpha * x + (1 - alpha) * s`, oldest bucket first. A gap (null)
 *  stays a gap and is skipped: it is neither zero nor interpolated, and the
 *  average carries on from the last value before it. Only `value` (what is
 *  drawn) changes; `raw`, `rawAvg`, min, max, loss and slow stay the bucket's own, and
 *  nothing outside the chart (table, cards, strips) ever sees this. Returns a
 *  new array in the input's order. */
export function smoothPoints(points: ChartPoint[], alpha: number = SMOOTH_ALPHA): ChartPoint[] {
  const order = new Map<string, number[]>();
  points.forEach((p, i) => {
    const list = order.get(p.key);
    if (list) list.push(i);
    else order.set(p.key, [i]);
  });
  const out = points.slice();
  for (const idx of order.values()) {
    idx.sort((a, b) => points[a].time.getTime() - points[b].time.getTime());
    let s: number | null = null;
    for (const i of idx) {
      const v = points[i].value;
      if (v == null) continue;
      s = s == null ? v : alpha * v + (1 - alpha) * s;
      out[i] = { ...points[i], value: s };
    }
  }
  return out;
}

/** The most points any one line has: the chart draws symbols only when that is
 *  small enough for them not to crowd. */
export function maxPointsPerKey(points: ChartPoint[]): number {
  const n = new Map<string, number>();
  let max = 0;
  for (const p of points) {
    const c = (n.get(p.key) ?? 0) + 1;
    n.set(p.key, c);
    if (c > max) max = c;
  }
  return max;
}

/** Lines of up to this many points get a symbol on every point; longer ones
 *  only show the point under the cursor. */
export const SYMBOL_MAX_POINTS = 30;

/** The labels the chart tooltips use. */
export interface TooltipLabels {
  allLost: string;
  loss: string;
  /** What a TCP target's loss is called ("failed"). */
  failed: string;
  slow: string;
  rawAvg: string;
}

/** The loss label of a target kind: TCP probes fail (DNS, refused, timeout),
 *  ICMP ones are lost; an unknown kind keeps the neutral loss. */
export function lossLabel(kind: LatencyKind | null | undefined, labels: { loss: string; failed: string }): string {
  return kind === 'tcp' ? labels.failed : labels.loss;
}

/** The slow-handshake share as text. ICMP has no handshake: null, leave it
 *  out. A TCP target whose data does not carry the figure (an older node, data
 *  from before the upgrade): "—", never 0%. Unknown kind: only a real figure. */
export function slowText(v: number | null | undefined, kind: LatencyKind | null | undefined): string | null {
  if (kind === 'icmp') return null;
  if (v == null) return kind === 'tcp' ? '—' : null;
  return formatLoss(v);
}

/** Whether the raw mean (slow handshakes included) is worth showing next to the
 *  filtered one: only when some probe was slow and it actually differs. */
export function showRaw(
  avg: number | null | undefined,
  rawAvg: number | null | undefined,
  slow: number | null | undefined,
): boolean {
  if (rawAvg == null || slow == null || !(slow > 0)) return false;
  return avg == null || Math.abs(rawAvg - avg) >= 0.05;
}

/** One line of the chart tooltip: "20.1 ms (raw 992 ms) · failed 0.2% · slow
 *  17%". Reads the bucket's own mean even when the line is smoothed; the raw
 *  mean appears only when slow handshakes make it differ. */
export function tooltipText(
  d: Pick<ChartPoint, 'raw' | 'rawAvg' | 'loss' | 'slow' | 'kind'>,
  labels: TooltipLabels,
): string {
  let head = d.raw == null ? labels.allLost : formatMs(d.raw);
  if (showRaw(d.raw, d.rawAvg, d.slow)) head += ` (${labels.rawAvg} ${formatMs(d.rawAvg)})`;
  const parts = [head, `${lossLabel(d.kind, labels)} ${formatLoss(d.loss)}`];
  const s = slowText(d.slow, d.kind);
  if (s != null) parts.push(`${labels.slow} ${s}`);
  return parts.join(' · ');
}

/** Local clock time, "21:58:26" (with seconds) or "21:58". */
export function formatClock(d: Date, seconds = false): string {
  const hm = `${pad(d.getHours())}:${pad(d.getMinutes())}`;
  return seconds ? `${hm}:${pad(d.getSeconds())}` : hm;
}

/** When a sample was taken, for "last 21:40"; the date too once it is not from
 *  today. */
export function formatSampleTime(tsSecs: number, nowMs: number): string {
  const d = new Date(tsSecs * 1000);
  const n = new Date(nowMs);
  const sameDay = d.getFullYear() === n.getFullYear() && d.getMonth() === n.getMonth() && d.getDate() === n.getDate();
  return sameDay ? formatClock(d) : formatChartTime(d, true);
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

/** One slice of the selected range, for the status strips. */
export interface StripSegment {
  /** Unix seconds: the slice covers [start, end), the last one also `end`. */
  start: number;
  end: number;
  /** How many points fell in the slice. */
  count: number;
  /** Mean of the points' averages (null points ignored); null when none. */
  avg: number | null;
  /** Mean of the points' raw averages (slow handshakes included); null when none. */
  rawAvg: number | null;
  /** Mean of the points' loss percentages; null when the slice has no points. */
  loss: number | null;
  /** Mean of the points' slow-handshake shares that carry one; null when none does. */
  slow: number | null;
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
    rawSum: 0,
    rawN: 0,
    lossSum: 0,
    slowSum: 0,
    slowN: 0,
  }));
  if (width > 0) {
    for (const p of points) {
      if (!(p.ts >= since && p.ts <= until)) continue;
      const i = Math.min(n - 1, Math.floor(((p.ts - since) * n) / (until - since)));
      const g = segs[i];
      g.count += 1;
      g.lossSum += p.loss;
      if (p.retrans_rate != null) {
        g.slowSum += p.retrans_rate;
        g.slowN += 1;
      }
      if (p.raw_avg != null) {
        g.rawSum += p.raw_avg;
        g.rawN += 1;
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
    rawAvg: g.rawN > 0 ? g.rawSum / g.rawN : null,
    loss: g.count > 0 ? g.lossSum / g.count : null,
    slow: g.slowN > 0 ? g.slowSum / g.slowN : null,
    allLost: g.count > 0 && g.avgN === 0,
  }));
}

/** Colour class of a latency strip slice: grey with no data, red when every
 *  probe was lost, otherwise by the latency thresholds and the slice's own
 *  share of slow handshakes (loss has its own strip). */
export function stripLatencyLevel(
  seg: Pick<StripSegment, 'count' | 'avg' | 'allLost'> & Partial<Pick<StripSegment, 'slow'>>,
): LatencyLevel {
  if (seg.count === 0) return 'none';
  if (seg.allLost) return 'bad';
  return latencyLevel(seg.avg, null, seg.slow ?? null);
}

/** Percentage heights (0–100) for variable-height latency heartbeat pillars.
 *  Empty slices stay at a quiet 28% baseline; all-lost slices rise to 100%;
 *  answered slices scale across [42%, 100%] when the row's range exceeds 2 ms,
 *  avoiding false visual drama when a rock-solid line only jitters by < 2 ms. */
export function stripLatHeights(segs: StripSegment[]): number[] {
  let lo = Infinity;
  let hi = -Infinity;
  for (const g of segs) {
    if (g.count > 0 && g.avg != null) {
      lo = Math.min(lo, g.avg);
      hi = Math.max(hi, g.avg);
    }
  }
  const span = Number.isFinite(lo) ? hi - lo : 0;
  return segs.map((g) => {
    if (g.count === 0) return 28;
    if (g.allLost || g.avg == null) return 100;
    if (span < 2) return 60;
    return Math.round(42 + ((g.avg - lo) / span) * 58);
  });
}

/** Percentage heights (0–100) for variable-height packet-loss pillars.
 *  0% loss sits at a calm 46% baseline; any non-zero loss pops taller (64%–100%). */
export function stripLossHeights(segs: StripSegment[]): number[] {
  return segs.map((g) => {
    if (g.count === 0 || g.loss == null) return 28;
    if (g.loss <= 0) return 46;
    const ratio = Math.min(1, g.loss / 10);
    return Math.round(64 + ratio * 36);
  });
}

/** Delivery / connectivity percentage formatted from range packet loss. */
export function formatAvailability(loss: number | null | undefined): string | null {
  if (loss == null) return null;
  const avail = Math.max(0, Math.min(100, 100 - loss));
  if (avail >= 99.995) return '100%';
  return `${avail.toFixed(avail >= 99 ? 2 : 1)}%`;
}

export interface NodeOverallSummary {
  nodeId: string;
  avgMs: number | null;
  avgLoss: number | null;
  goodCount: number;
  warnCount: number;
  badCount: number;
  missingCount: number;
  totalCount: number;
  level: LatencyLevel;
  hasIssue: boolean;
  /** Lower is better; Infinity when the node has no valid readings. */
  score: number;
}

export function computeNodeOverall(
  node: GroupLatencyNode,
  targets: LatencyTarget[],
  getCell: (nodeId: string, targetId: number) => GroupLatencyCell | undefined,
): NodeOverallSummary {
  if (!node.supported) {
    return {
      nodeId: node.node_id, avgMs: null, avgLoss: null,
      goodCount: 0, warnCount: 0, badCount: 0, missingCount: 0, totalCount: 0,
      level: 'none', hasIssue: false, score: Infinity,
    };
  }
  let msSum = 0;
  let msN = 0;
  let lossSum = 0;
  let lossN = 0;
  let slowSum = 0;
  let slowN = 0;
  let goodCount = 0;
  let warnCount = 0;
  let badCount = 0;
  let missingCount = 0;
  for (const tg of targets) {
    if (node.unsupported_targets.includes(tg.id)) continue;
    const c = getCell(node.node_id, tg.id);
    if (!c) {
      missingCount += 1;
      continue;
    }
    const slow = tg.kind === 'icmp' ? null : c.retrans_rate;
    const lvl = latencyLevel(c.avg, c.loss, slow);
    if (lvl === 'good') goodCount += 1;
    else if (lvl === 'warn') warnCount += 1;
    else if (lvl === 'bad') badCount += 1;
    else missingCount += 1;
    lossSum += c.loss;
    lossN += 1;
    if (slow != null) {
      slowSum += slow;
      slowN += 1;
    }
    if (c.avg != null) {
      msSum += c.avg;
      msN += 1;
    }
  }
  const totalCount = goodCount + warnCount + badCount;
  const avgMs = msN > 0 ? msSum / msN : null;
  const avgLoss = lossN > 0 ? lossSum / lossN : null;
  const avgSlow = slowN > 0 ? slowSum / slowN : 0;
  const hasIssue = warnCount > 0 || badCount > 0;
  let level: LatencyLevel = 'none';
  if (totalCount > 0) {
    if (badCount > 0) level = 'bad';
    else if (warnCount > 0) level = 'warn';
    else level = latencyLevel(avgMs, avgLoss, slowN > 0 ? avgSlow : null);
  }
  const score = avgMs == null
    ? (lossN > 0 ? 100_000 + (avgLoss ?? 100) * 100 : Infinity)
    : avgMs + (avgLoss ?? 0) * 15 + avgSlow * 5 + badCount * 150 + warnCount * 25 + missingCount * 40;
  return {
    nodeId: node.node_id,
    avgMs,
    avgLoss,
    goodCount,
    warnCount,
    badCount,
    missingCount,
    totalCount,
    level,
    hasIssue,
    score,
  };
}

export interface GroupSnapshot {
  goodPairs: number;
  warnPairs: number;
  badPairs: number;
  missingPairs: number;
  /** Measured pairs (good + warn + bad). */
  totalPairs: number;
  /** All probeable pairs (measured + missing). */
  expectedPairs: number;
  carrierAvgs: Array<{ carrier: LatencyTarget['carrier']; avgMs: number | null; avgLoss: number; level: LatencyLevel }>;
  bestNode: NodeOverallSummary | null;
  worstPair: {
    nodeId: string;
    targetId: number;
    avg: number | null;
    loss: number;
    slow: number | null;
    level: LatencyLevel;
  } | null;
}

export function computeGroupSnapshot(
  nodes: GroupLatencyNode[],
  targets: LatencyTarget[],
  overalls: NodeOverallSummary[],
  getCell: (nodeId: string, targetId: number) => GroupLatencyCell | undefined,
): GroupSnapshot {
  let goodPairs = 0;
  let warnPairs = 0;
  let badPairs = 0;
  let missingPairs = 0;

  interface CandidatePair {
    key: string;
    nodeId: string;
    targetId: number;
    avg: number | null;
    loss: number;
    slow: number | null;
    level: LatencyLevel;
  }
  const degraded: CandidatePair[] = [];

  const byCarrier = new Map<LatencyTarget['carrier'], { msSum: number; msN: number; lossSum: number; lossN: number }>();

  for (const tg of targets) {
    let bucket = byCarrier.get(tg.carrier);
    if (!bucket) {
      bucket = { msSum: 0, msN: 0, lossSum: 0, lossN: 0 };
      byCarrier.set(tg.carrier, bucket);
    }
    for (const n of nodes) {
      if (!n.supported || n.unsupported_targets.includes(tg.id)) continue;
      const c = getCell(n.node_id, tg.id);
      if (!c) {
        missingPairs += 1;
        continue;
      }
      const slow = tg.kind === 'icmp' ? null : (c.retrans_rate ?? null);
      const lvl = latencyLevel(c.avg, c.loss, slow);
      if (lvl === 'good') goodPairs += 1;
      else if (lvl === 'warn') warnPairs += 1;
      else if (lvl === 'bad') badPairs += 1;
      else missingPairs += 1;

      bucket.lossSum += c.loss;
      bucket.lossN += 1;
      if (c.avg != null) {
        bucket.msSum += c.avg;
        bucket.msN += 1;
      }

      if (lvl === 'warn' || lvl === 'bad') {
        degraded.push({
          key: `${n.node_id}\u0000${tg.id}`,
          nodeId: n.node_id,
          targetId: tg.id,
          avg: c.avg,
          loss: c.loss,
          slow,
          level: lvl,
        });
      }
    }
  }

  let worstPair: GroupSnapshot['worstPair'] = null;
  if (degraded.length > 0) {
    const byKey = new Map(degraded.map((d) => [d.key, d]));
    const [worstKey] = rankByAttention(
      degraded.map((d) => d.key),
      (k) => {
        const d = byKey.get(k);
        return d ? { avg: d.avg, loss: d.loss, slow: d.slow } : null;
      },
    );
    const picked = byKey.get(worstKey);
    if (picked) {
      worstPair = {
        nodeId: picked.nodeId,
        targetId: picked.targetId,
        avg: picked.avg,
        loss: picked.loss,
        slow: picked.slow,
        level: picked.level,
      };
    }
  }

  const carrierOrder: LatencyTarget['carrier'][] = ['telecom', 'unicom', 'mobile', 'other'];
  const carrierAvgs = carrierOrder
    .filter((cr) => byCarrier.has(cr))
    .map((cr) => {
      const b = byCarrier.get(cr)!;
      const avgMs = b.msN > 0 ? b.msSum / b.msN : null;
      const avgLoss = b.lossN > 0 ? b.lossSum / b.lossN : 0;
      return {
        carrier: cr,
        avgMs,
        avgLoss,
        level: b.lossN === 0 ? ('none' as LatencyLevel) : latencyLevel(avgMs, avgLoss),
      };
    });

  let bestNode: NodeOverallSummary | null = null;
  for (const o of overalls) {
    if (!Number.isFinite(o.score) || o.avgMs == null) continue;
    if (!bestNode || o.score < bestNode.score) bestNode = o;
  }

  const totalPairs = goodPairs + warnPairs + badPairs;
  return {
    goodPairs,
    warnPairs,
    badPairs,
    missingPairs,
    totalPairs,
    expectedPairs: totalPairs + missingPairs,
    carrierAvgs,
    bestNode,
    worstPair,
  };
}
