import { describe, expect, it } from 'vitest';
import type { LatencySeries } from '../api/types';
import { chartPoints, formatLoss, formatMs, lossMarkers } from './latencyChart';

const series: LatencySeries[] = [
  {
    target_id: 1,
    name: 'A',
    carrier: 'telecom',
    kind: 'tcp',
    points: [
      { ts: 600, avg: 10, min: 8, max: 14, loss: 0 },
      { ts: 660, avg: 12, min: 9, max: 20, loss: 16.7 },
    ],
  },
  {
    target_id: 2,
    name: 'B',
    carrier: 'unicom',
    kind: 'icmp',
    points: [{ ts: 600, avg: null, min: null, max: null, loss: 100 }],
  },
];

describe('chartPoints', () => {
  it('flattens every target, keyed by target id', () => {
    const pts = chartPoints(series, null);
    expect(pts).toHaveLength(3);
    expect(pts.map((p) => p.key)).toEqual(['1', '1', '2']);
    expect(pts[0].time.getTime()).toBe(600_000);
  });

  it('keeps an all-lost bucket as a gap (null), never a zero', () => {
    const lost = chartPoints(series, 2)[0];
    expect(lost.value).toBeNull();
    expect(lost.loss).toBe(100);
  });

  it('only the selected target when one is chosen', () => {
    expect(chartPoints(series, 1).map((p) => p.key)).toEqual(['1', '1']);
    expect(chartPoints(series, 99)).toEqual([]);
  });
});

describe('lossMarkers', () => {
  it('marks each bucket that lost something, however little', () => {
    const m = lossMarkers(chartPoints(series, null));
    expect(m.map((x) => [x.key, x.loss])).toEqual([['1', 16.7], ['2', 100]]);
  });
});

describe('formatting', () => {
  it('shows ms with sensible precision and a dash for nothing', () => {
    expect(formatMs(null)).toBe('-');
    expect(formatMs(12.345)).toBe('12.3 ms');
    expect(formatMs(250.4)).toBe('250 ms');
    expect(formatLoss(null)).toBe('-');
    expect(formatLoss(0)).toBe('0%');
    expect(formatLoss(0.25)).toBe('0.25%');
    expect(formatLoss(16.666)).toBe('16.7%');
  });
});
