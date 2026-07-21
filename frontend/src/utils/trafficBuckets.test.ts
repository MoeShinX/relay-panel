import { describe, it, expect, beforeAll, afterAll } from 'vitest';
import {
  foldBuckets,
  localDayKey,
  parseUtcBucket,
  MAX_SERIES,
  OTHER_SERIES,
  SERIES_COLORS_LIGHT,
  SERIES_COLORS_DARK,
} from './trafficBuckets';
import type { TrafficHistoryBucket } from '../api/types';

const b = (
  bucket: string,
  billed: number,
  up = 0,
  down = 0,
  group = 'line-a',
): TrafficHistoryBucket => ({
  bucket,
  group_id: 1,
  group_name: group,
  billed_total: billed,
  real_upload: up,
  real_download: down,
});

/** Total billed across every slice — output is per (bucket, line) now. */
const sum = (points: { billed: number }[]) => points.reduce((n, p) => n + p.billed, 0);

describe('traffic bucket folding', () => {
  it('parses a stored bucket as UTC, not local time', () => {
    // Without the trailing Z this would be read as local time and shift by the
    // viewer's offset — the root cause of every "my traffic is on the wrong
    // day" report.
    expect(parseUtcBucket('2026-07-20 10:00:00').toISOString()).toBe('2026-07-20T10:00:00.000Z');
  });

  it('zero-fills quiet hours instead of collapsing the gap', () => {
    const points = foldBuckets(
      [b('2026-07-20 12:00:00', 500)],
      '2026-07-20 10:00:00',
      true,
      new Date('2026-07-20T13:00:00Z'),
    );
    // 10,11,12,13 → four columns, only one with data. A collapsed axis would
    // imply continuous usage.
    expect(points).toHaveLength(4);
    expect(points.filter((p) => p.billed > 0)).toHaveLength(1);
    expect(points.map((p) => p.billed)).toEqual([0, 0, 500, 0]);
  });

  it('sums several hours of the same day into one daily column', () => {
    const points = foldBuckets(
      [b('2026-07-20 10:00:00', 100, 10, 1), b('2026-07-20 11:00:00', 200, 20, 2)],
      '2026-07-20 09:00:00',
      false,
      new Date('2026-07-20T12:00:00Z'),
    );
    const total = points.reduce((n, p) => n + p.billed, 0);
    expect(total).toBe(300);
    expect(points.reduce((n, p) => n + p.real_upload, 0)).toBe(30);
    expect(points.reduce((n, p) => n + p.real_download, 0)).toBe(3);
  });

  it('keeps real and billed independent (rate != 1 lines)', () => {
    // A x3 line: billed is 3x real. If the chart derived one from the other,
    // the tooltip would contradict the quota.
    const points = foldBuckets(
      [b('2026-07-20 10:00:00', 3000, 500, 500)],
      '2026-07-20 10:00:00',
      true,
      new Date('2026-07-20T10:00:00Z'),
    );
    expect(points[0].billed).toBe(3000);
    expect(points[0].real_upload + points[0].real_download).toBe(1000);
  });

  it('surfaces a bucket that falls outside the skeleton (clock skew)', () => {
    const points = foldBuckets(
      [b('2026-07-21 05:00:00', 42)],
      '2026-07-20 10:00:00',
      true,
      new Date('2026-07-20T11:00:00Z'),
    );
    // Dropping it would silently hide real usage.
    expect(points.reduce((n, p) => n + p.billed, 0)).toBe(42);
  });
});

// The whole reason day-grouping is client-side. Pin the behaviour with a real
// non-UTC timezone rather than trusting the CI machine's locale.
describe('daily grouping uses the VIEWER timezone, not UTC', () => {
  const realOffset = Date.prototype.getTimezoneOffset;
  const realDay = Date.prototype.getDate;
  const realMonth = Date.prototype.getMonth;
  const realFullYear = Date.prototype.getFullYear;

  beforeAll(() => {
    // Simulate UTC+8 (CST): local time = UTC + 8h.
    const shifted = function (this: Date) {
      return new Date(this.getTime() + 8 * 3600_000);
    };
    Date.prototype.getTimezoneOffset = function () { return -480; };
    Date.prototype.getDate = function () { return shifted.call(this).getUTCDate(); };
    Date.prototype.getMonth = function () { return shifted.call(this).getUTCMonth(); };
    Date.prototype.getFullYear = function () { return shifted.call(this).getUTCFullYear(); };
  });
  afterAll(() => {
    Date.prototype.getTimezoneOffset = realOffset;
    Date.prototype.getDate = realDay;
    Date.prototype.getMonth = realMonth;
    Date.prototype.getFullYear = realFullYear;
  });

  it('files 23:00 UTC as the NEXT local day for a UTC+8 viewer', () => {
    // 2026-07-20 23:00 UTC is 2026-07-21 07:00 in UTC+8. A server-side UTC
    // grouping would put it on the 20th — visibly wrong to the operator.
    expect(localDayKey(parseUtcBucket('2026-07-20 23:00:00'))).toBe('2026-07-21');
  });

  it('splits one UTC day across two local days', () => {
    const points = foldBuckets(
      [
        b('2026-07-20 10:00:00', 100), // 18:00 local, 20th
        b('2026-07-20 23:00:00', 700), // 07:00 local, 21st
      ],
      '2026-07-20 10:00:00',
      false,
      new Date('2026-07-20T23:00:00Z'),
    );
    const byLabel = Object.fromEntries(points.map((p) => [p.label, p.billed]));
    expect(byLabel['07-20']).toBe(100);
    expect(byLabel['07-21']).toBe(700);
  });
});

describe('per-line slicing', () => {
  it('emits one slice per (bucket, line) so the chart can stack them', () => {
    const points = foldBuckets(
      [
        b('2026-07-20 10:00:00', 100, 0, 0, 'guangzhou'),
        b('2026-07-20 10:00:00', 700, 0, 0, 'hk'),
      ],
      '2026-07-20 10:00:00',
      true,
      new Date('2026-07-20T10:00:00Z'),
    );
    expect(points).toHaveLength(2);
    const byLine = Object.fromEntries(points.map((p) => [p.group, p.billed]));
    expect(byLine).toEqual({ guangzhou: 100, hk: 700 });
    // Same bucket → same x position, which is what makes them stack.
    expect(new Set(points.map((p) => p.label)).size).toBe(1);
  });

  it('folds the tail by VOLUME, not by first appearance', () => {
    // MAX_SERIES busy lines plus two tiny ones. The tiny ones must fold even
    // though one of them is alphabetically/positionally first — folding by
    // arrival order would hand a color to a line that moved one byte and hide
    // the one actually burning the quota.
    const buckets: TrafficHistoryBucket[] = [
      b('2026-07-20 10:00:00', 1, 0, 0, 'tiny-first'),
      ...Array.from({ length: MAX_SERIES }, (_, i) =>
        b('2026-07-20 10:00:00', 1000 * (i + 1), 0, 0, `big-${i}`),
      ),
      b('2026-07-20 10:00:00', 2, 0, 0, 'tiny-last'),
    ];
    const points = foldBuckets(
      buckets,
      '2026-07-20 10:00:00',
      true,
      new Date('2026-07-20T10:00:00Z'),
    );
    const lines = points.map((p) => p.group);
    expect(lines).toContain(OTHER_SERIES);
    expect(lines).not.toContain('tiny-first');
    expect(lines).not.toContain('tiny-last');
    expect(lines).toContain('big-0');
    // Folding must not lose bytes.
    expect(sum(points)).toBe(1 + 2 + 1000 * ((MAX_SERIES * (MAX_SERIES + 1)) / 2));
    // The two tiny lines merge into ONE "other" slice for this bucket.
    expect(points.filter((p) => p.group === OTHER_SERIES)).toHaveLength(1);
    expect(points.find((p) => p.group === OTHER_SERIES)!.billed).toBe(3);
  });

  it('keeps a quiet bucket on the axis even with no slice', () => {
    const points = foldBuckets(
      [b('2026-07-20 12:00:00', 500, 0, 0, 'gz')],
      '2026-07-20 10:00:00',
      true,
      new Date('2026-07-20T13:00:00Z'),
    );
    // Four hours on the axis; only one carries traffic.
    expect(new Set(points.map((p) => p.label)).size).toBe(4);
    expect(sum(points)).toBe(500);
  });
});

describe('categorical palette', () => {
  it('has a fixed slot order so a line keeps its color', () => {
    // Colors are assigned by slot, never cycled or regenerated. If this array
    // is ever reordered, every existing chart silently repaints.
    expect(SERIES_COLORS_LIGHT[0]).toBe('#6366f1');
    expect(SERIES_COLORS_LIGHT).toHaveLength(MAX_SERIES);
    expect(SERIES_COLORS_DARK).toHaveLength(MAX_SERIES);
  });

  it('excludes the reserved status red and the accent-colliding violet', () => {
    // Red is reserved for critical status and must never be "series 6"; it also
    // failed the adjacent-pair check against orange (ΔE 7.1 normal vision).
    // Violet collides with the indigo accent in slot 1.
    for (const banned of ['#e34948', '#e66767', '#4a3aa7', '#9085e9']) {
      expect(SERIES_COLORS_LIGHT).not.toContain(banned);
      expect(SERIES_COLORS_DARK).not.toContain(banned);
    }
  });

  it('has no duplicate hues', () => {
    expect(new Set(SERIES_COLORS_LIGHT).size).toBe(SERIES_COLORS_LIGHT.length);
    expect(new Set(SERIES_COLORS_DARK).size).toBe(SERIES_COLORS_DARK.length);
  });
});
