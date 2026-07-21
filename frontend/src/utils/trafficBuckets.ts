import type { TrafficHistoryBucket } from '../api/types';

/** One plotted point, already in the viewer's timezone. */
export interface TrafficPoint {
  /** X label. "MM-DD HH:00" hourly, "MM-DD" daily. */
  label: string;
  billed: number;
  real_upload: number;
  real_download: number;
}

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
  const byKey = new Map<string, TrafficPoint>();
  const order: string[] = [];

  const keyOf = (d: Date) => (hourly ? d.toISOString().slice(0, 13) : localDayKey(d));
  const labelOf = (d: Date) => (hourly ? localHourLabel(d) : localDayKey(d).slice(5));

  const upsert = (d: Date) => {
    const key = keyOf(d);
    let p = byKey.get(key);
    if (!p) {
      p = { label: labelOf(d), billed: 0, real_upload: 0, real_download: 0 };
      byKey.set(key, p);
      order.push(key);
    }
    return p;
  };

  // 1) Skeleton: every hour from `since` to `now`, all zeros. Stepping by hour
  //    (not by day) is what makes the daily path land each hour in the right
  //    LOCAL day — a day boundary in local time falls mid-UTC-day.
  const start = parseUtcBucket(since);
  for (let d = new Date(start); d <= now; d = new Date(d.getTime() + 3600_000)) {
    upsert(d);
  }

  // 2) Fold the data in. A bucket outside the skeleton (clock skew) still
  //    appears rather than being dropped.
  for (const b of buckets) {
    const p = upsert(parseUtcBucket(b.bucket));
    p.billed += b.billed_total;
    p.real_upload += b.real_upload;
    p.real_download += b.real_download;
  }

  return order.map((k) => byKey.get(k)!);
}
