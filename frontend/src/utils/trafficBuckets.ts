import type { TrafficHistoryBucket } from '../api/types';

/** One plotted point: a (time bucket, line) slice, in the viewer's timezone. */
export interface TrafficPoint {
  /** X label. "MM-DD HH:00" hourly, "MM-DD" daily. */
  label: string;
  /** Line (inbound device group) name — the stacking + legend series. */
  group: string;
  billed: number;
  real_upload: number;
  real_download: number;
}

/**
 * Categorical series colors in FIXED order, shared by every multi-line chart
 * (traffic, node metrics, latency) — slot N is always the same hue, so a line
 * keeps its color when another line is filtered out or drops to zero.
 *
 * Ten well separated hues in Tableau 10 order, so a dashboard or device group
 * of up to ten nodes never repeats a color. (v1.2.21 and earlier had six, two
 * of them greens; the seventh node took the first color again.)
 *
 * Red is left out on purpose: it is reserved for status (the latency charts'
 * loss markers, error states) and must never double as "series N". Tableau's
 * red slot is taken by olive instead. Violet stays out too.
 *
 * A few steps sit below 3:1 against the surface; the legend and tooltip provide
 * the relief — identity is never carried by color alone here.
 */
export const SERIES_COLORS_LIGHT = [
  '#4e79a7', // blue
  '#f28e2b', // orange
  '#59a14f', // green
  '#b07aa1', // mauve
  '#76b7b2', // teal
  '#edc948', // yellow
  '#ff9da7', // pink
  '#9c755f', // brown
  '#79706e', // grey
  '#bcbd22', // olive
];

/** The same hues on the dark surface (they hold up there as they are). */
export const SERIES_COLORS_DARK = [...SERIES_COLORS_LIGHT];

/** Legend style of the line charts: a filled dot instead of G2's thin default
 *  stroke, big enough that each line's color reads at a glance. */
export const LINE_LEGEND_ITEM = {
  itemMarker: 'circle',
  itemMarkerSize: 16,
  itemLabelFontSize: 13,
} as const;

/** Stroke width of the lines in those charts. */
export const LINE_WIDTH = 2;

/**
 * Lines that get their own color before the tail folds into "Other".
 *
 * Past the palette an 11th hue would have to be generated, and a generated hue is
 * indistinguishable from an existing one under CVD — which is the entire point
 * of validating the palette. Folding keeps the chart honest instead.
 */
export const MAX_SERIES = SERIES_COLORS_LIGHT.length;

/** Label for the folded tail. Kept out of i18n on purpose — see the component,
 *  which translates it at render time. */
export const OTHER_SERIES = '__other__';

/** Parse a stored 'YYYY-MM-DD HH:00:00' UTC bucket into a Date. */
export function parseUtcBucket(s: string): Date {
  return new Date(s.replace(' ', 'T') + 'Z');
}

/** Local-timezone day key, YYYY-MM-DD. */
export function localDayKey(d: Date): string {
  const p = (n: number) => String(n).padStart(2, '0');
  return `${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())}`;
}

/** Local hour label, "MM-DD HH:00". */
export function localHourLabel(d: Date): string {
  const p = (n: number) => String(n).padStart(2, '0');
  return `${p(d.getMonth() + 1)}-${p(d.getDate())} ${p(d.getHours())}:00`;
}

/**
 * Fold UTC hour buckets into the plotted series, in the VIEWER'S timezone.
 *
 * This is the part that actually goes wrong, so it lives outside the component
 * and is unit-tested: the API stores UTC hours, but a UTC "day" starts at 08:00
 * for a UTC+8 viewer — grouping server-side would visibly misfile yesterday's
 * traffic for every non-UTC operator.
 *
 * Also zero-fills: an honest time axis shows quiet hours as empty columns
 * rather than silently collapsing the gap and implying continuous usage.
 *
 * @param since  inclusive UTC lower bound ('YYYY-MM-DD HH:00:00') from the API
 * @param now    upper bound (injectable for tests)
 */
export function foldBuckets(
  buckets: TrafficHistoryBucket[],
  since: string,
  hourly: boolean,
  now: Date = new Date(),
): TrafficPoint[] {
  const keyOf = (d: Date) => (hourly ? d.toISOString().slice(0, 13) : localDayKey(d));
  const labelOf = (d: Date) => (hourly ? localHourLabel(d) : localDayKey(d).slice(5));

  // Which lines make the cut. Ranked by total volume so the biggest consumers
  // keep their own color and the long tail folds — the opposite (first-seen
  // order) would give a color to a line that moved one byte and fold the one
  // actually burning the quota.
  const totals = new Map<string, number>();
  for (const b of buckets) {
    totals.set(b.group_name, (totals.get(b.group_name) ?? 0) + b.billed_total);
  }
  const ranked = [...totals.entries()].sort((a, b) => b[1] - a[1]).map(([g]) => g);
  const named = new Set(ranked.slice(0, MAX_SERIES));
  const seriesOf = (g: string) => (named.has(g) ? g : OTHER_SERIES);

  // Keyed by (time bucket, line): the chart stacks one bar per bucket out of
  // one segment per line.
  const byKey = new Map<string, TrafficPoint>();
  const timeOrder: string[] = [];

  const touchBucket = (d: Date) => {
    const tk = keyOf(d);
    if (!timeOrder.includes(tk)) timeOrder.push(tk);
    return tk;
  };
  const upsert = (d: Date, group: string) => {
    const tk = touchBucket(d);
    const key = `${tk}\u0000${group}`;
    let p = byKey.get(key);
    if (!p) {
      p = { label: labelOf(d), group, billed: 0, real_upload: 0, real_download: 0 };
      byKey.set(key, p);
    }
    return p;
  };

  // 1) Skeleton: every hour from `since` to `now`. Stepping by hour (not by
  //    day) is what makes the daily path land each hour in the right LOCAL day
  //    — a local day boundary falls mid-UTC-day.
  const start = parseUtcBucket(since);
  for (let d = new Date(start); d <= now; d = new Date(d.getTime() + 3600_000)) {
    touchBucket(d);
  }

  // 2) Fold the data in. A bucket outside the skeleton (clock skew) still
  //    appears rather than being dropped.
  for (const b of buckets) {
    const p = upsert(parseUtcBucket(b.bucket), seriesOf(b.group_name));
    p.billed += b.billed_total;
    p.real_upload += b.real_upload;
    p.real_download += b.real_download;
  }

  // 3) Emit in time order. Empty buckets contribute no segment (a stacked
  //    chart renders the gap by having nothing there) but the x axis still
  //    shows them, because `timeOrder` carries the full skeleton.
  const out: TrafficPoint[] = [];
  for (const tk of timeOrder) {
    const slices = [...byKey.entries()].filter(([k]) => k.startsWith(`${tk}\u0000`));
    if (slices.length === 0) {
      // Placeholder so the axis keeps the quiet hour visible.
      const d = hourly ? new Date(`${tk}:00:00Z`) : new Date(`${tk}T00:00:00Z`);
      out.push({
        label: hourly ? localHourLabel(d) : tk.slice(5),
        group: ranked[0] ?? OTHER_SERIES,
        billed: 0,
        real_upload: 0,
        real_download: 0,
      });
      continue;
    }
    for (const [, p] of slices) out.push(p);
  }
  return out;
}
