import { latencyLevel, type LatencyLevel } from './latency';
import { MAX_SERIES } from './trafficBuckets';

/**
 * Which lines a chart draws once it has more series than the palette has
 * colours. Averaging machines into an "Other" line means nothing for CPU or
 * latency, so the viewer picks instead; until they do, the ten that need
 * attention first are drawn. Pure functions, no React: the ranking is the part
 * worth testing on its own.
 */

/** Mean of the finite values; null when there are none. */
export function meanOf(values: Iterable<number | null | undefined>): number | null {
  let sum = 0;
  let n = 0;
  for (const v of values) {
    if (v == null || !Number.isFinite(v)) continue;
    sum += v;
    n += 1;
  }
  return n > 0 ? sum / n : null;
}

/** Keys by descending score, those without a score last. The sort is stable,
 *  so equal scores keep the order the keys came in. */
export function rankByScore(keys: string[], score: (key: string) => number | null | undefined): string[] {
  const scored = keys.map((key, i) => ({ key, i, s: score(key) }));
  scored.sort((a, b) => {
    const as = a.s != null && Number.isFinite(a.s);
    const bs = b.s != null && Number.isFinite(b.s);
    if (as !== bs) return as ? -1 : 1;
    if (as && bs && a.s !== b.s) return (b.s as number) - (a.s as number);
    return a.i - b.i;
  });
  return scored.map((x) => x.key);
}

/** What a latency line is ranked on: the range's mean round trip, loss (or
 *  failed probes) and share of slow handshakes. `slow` null / absent = not
 *  reported. */
export interface LatencyReading {
  avg: number | null | undefined;
  loss: number | null | undefined;
  slow?: number | null | undefined;
}

const LEVEL_ORDER: Record<LatencyLevel, number> = { bad: 3, warn: 2, good: 1, none: 0 };

/** Sort key of a reading, bigger = needs attention sooner. A key with no
 *  reading at all (no cell, no data, a node or target that cannot be probed)
 *  scores below everything, including a reading of "none". */
function attention(r: LatencyReading | null | undefined): [number, number, number, number, number] {
  if (!r) return [-1, 0, 0, 0, 0];
  const level = latencyLevel(r.avg, r.loss, r.slow);
  const loss = r.loss ?? 0;
  const allLost = r.avg == null && loss > 0;
  // An unknown share does not claim to be better than a known 0%: the two tie
  // here and the mean decides, as it does for a line that has no such figure.
  return [LEVEL_ORDER[level], allLost ? 1 : 0, loss, r.slow ?? 0, r.avg ?? 0];
}

/** Keys worst first: level (bad, warn, good, none) by the same thresholds as
 *  the cards and the table (latency, loss and slow-handshake share of the same
 *  window), a line that lost every probe ahead of the rest of its level, then
 *  higher loss, then higher slow share, then higher mean latency. Keys with no reading
 *  come last; ties keep the incoming order. */
export function rankByAttention(
  keys: string[],
  reading: (key: string) => LatencyReading | null | undefined,
): string[] {
  const rows = keys.map((key, i) => ({ key, i, a: attention(reading(key)) }));
  rows.sort((x, y) => {
    for (let k = 0; k < 5; k++) {
      if (x.a[k] !== y.a[k]) return y.a[k] - x.a[k];
    }
    return x.i - y.i;
  });
  return rows.map((r) => r.key);
}

/** The explicit pick without keys that no longer exist, in the order given;
 *  null (automatic) when none are left or nothing was picked. */
export function pruneSelection(picked: string[] | null, keys: string[]): string[] | null {
  if (!picked) return null;
  const live = new Set(keys);
  const kept = picked.filter((k) => live.has(k));
  return kept.length > 0 ? kept : null;
}

/** The keys to draw, in the chart's normal order: the explicit pick, or else the
 *  first `max` of the ranking. Never more than `max`. */
export function drawnKeys(
  keys: string[],
  explicit: string[] | null,
  ranked: string[],
  max: number = MAX_SERIES,
): string[] {
  const chosen = new Set((explicit ?? ranked).slice(0, max));
  return keys.filter((k) => chosen.has(k));
}
