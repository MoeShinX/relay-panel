import { describe, it, expect, beforeAll, afterAll } from 'vitest';
import { foldBuckets, localDayKey, parseUtcBucket } from './trafficBuckets';
import type { TrafficHistoryBucket } from '../api/types';

const b = (bucket: string, billed: number, up = 0, down = 0): TrafficHistoryBucket => ({
  bucket,
  billed_total: billed,
  real_upload: up,
  real_download: down,
});

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
