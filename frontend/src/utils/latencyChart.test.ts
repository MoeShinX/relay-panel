import { describe, expect, it } from 'vitest';
import type { LatencySeries } from '../api/types';
import {
  chartPoints,
  formatLoss,
  formatMs,
  groupChartPoints,
  lossMarkers,
  nodeDisplayName,
  stripLatencyLevel,
  stripSegmentCount,
  stripSegments,
} from './latencyChart';
import type { LatencyPoint } from '../api/types';

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

describe('the group chart', () => {
  const nodes = [
    { node_id: 'n1', label: 'HK-1' },
    { node_id: '0123456789abcdef', label: null },
    { node_id: 'short', label: null },
  ];

  it('names a node by its label, else by its id (shortened when long)', () => {
    expect(nodeDisplayName(nodes[0])).toBe('HK-1');
    expect(nodeDisplayName(nodes[1])).toBe('01234567…def');
    expect(nodeDisplayName(nodes[2])).toBe('short');
  });

  it('draws a line per node, keyed by node id and named for it, gaps kept as null', () => {
    const pts = groupChartPoints(
      [
        { node_id: 'n1', points: [{ ts: 600, avg: 10, min: 8, max: 14, loss: 0 }] },
        { node_id: '0123456789abcdef', points: [{ ts: 600, avg: null, min: null, max: null, loss: 100 }] },
        { node_id: 'gone', points: [{ ts: 660, avg: 5, min: 4, max: 6, loss: 0 }] },
      ],
      nodes as never,
    );
    expect(pts.map((p) => [p.key, p.name, p.value])).toEqual([
      ['n1', 'HK-1', 10],
      ['0123456789abcdef', '01234567…def', null],
      // a node the table no longer lists still plots, under its id
      ['gone', 'gone', 5],
    ]);
    expect(pts[0].time.getTime()).toBe(600_000);
    expect(lossMarkers(pts).map((m) => m.key)).toEqual(['0123456789abcdef']);
  });
});

const pt = (ts: number, avg: number | null, loss = 0): LatencyPoint => ({
  ts,
  avg,
  min: avg,
  max: avg,
  loss,
});

describe('stripSegments', () => {
  it('always returns the asked number of slices, 30 by default, tiling the range', () => {
    const segs = stripSegments([], 1000, 1000 + 3000);
    expect(segs).toHaveLength(30);
    expect(segs[0].start).toBe(1000);
    expect(segs[0].end).toBe(1100);
    expect(segs[29].end).toBe(4000);
    for (let i = 1; i < segs.length; i += 1) expect(segs[i].start).toBe(segs[i - 1].end);
  });

  it('no points at all: every slice is no-data, not zero and not lost', () => {
    for (const g of stripSegments([], 0, 3000)) {
      expect(g).toMatchObject({ count: 0, avg: null, loss: null, allLost: false });
    }
  });

  it('averages the points of a slice (null averages ignored) and their loss', () => {
    const segs = stripSegments([pt(10, 20, 0), pt(20, 40, 10), pt(30, null, 100)], 0, 3000);
    expect(segs[0].count).toBe(3);
    expect(segs[0].avg).toBe(30);
    expect(segs[0].loss).toBeCloseTo(110 / 3);
    expect(segs[0].allLost).toBe(false);
  });

  it('leaves slices without points as no-data while others have values', () => {
    const segs = stripSegments([pt(10, 20), pt(2950, 50)], 0, 3000);
    expect(segs[0].avg).toBe(20);
    expect(segs[1]).toMatchObject({ count: 0, avg: null, loss: null });
    expect(segs[28]).toMatchObject({ count: 0, avg: null });
    expect(segs[29].avg).toBe(50);
  });

  it('a slice whose points all lost every probe is all-lost, with its loss', () => {
    const segs = stripSegments([pt(10, null, 100), pt(20, null, 100)], 0, 3000);
    expect(segs[0]).toMatchObject({ count: 2, avg: null, loss: 100, allLost: true });
  });

  it('a point on an inner boundary belongs to the later slice', () => {
    const segs = stripSegments([pt(100, 10), pt(99, 30)], 0, 3000);
    expect(segs[0].avg).toBe(30);
    expect(segs[1].avg).toBe(10);
  });

  it('a point at since is in the first slice, one at until in the last, others outside are ignored', () => {
    const segs = stripSegments([pt(0, 11), pt(3000, 22), pt(-1, 99), pt(3001, 99)], 0, 3000);
    expect(segs[0]).toMatchObject({ count: 1, avg: 11 });
    expect(segs[29]).toMatchObject({ count: 1, avg: 22 });
    expect(segs.reduce((a, g) => a + g.count, 0)).toBe(2);
  });

  it('an empty or inverted range yields no-data slices instead of dividing by zero', () => {
    for (const segs of [stripSegments([pt(5, 10)], 5, 5), stripSegments([pt(5, 10)], 9, 5)]) {
      expect(segs).toHaveLength(30);
      expect(segs.every((g) => g.count === 0 && g.avg === null)).toBe(true);
    }
  });
});

describe('stripSegmentCount', () => {
  it('one slice per bucket, at most 30, at least 1', () => {
    // a day of hourly buckets (since aligned down to the hour, until = now + 1)
    expect(stripSegmentCount(0, 24 * 3600 + 1800, 3600)).toBe(25);
    // an hour of 5-minute buckets
    expect(stripSegmentCount(0, 3600 + 1, 300)).toBe(13);
    // an hour of minute buckets has more than enough
    expect(stripSegmentCount(0, 3600 + 1, 60)).toBe(30);
    expect(stripSegmentCount(0, 10, 3600)).toBe(1);
    // nonsense input keeps the default
    expect(stripSegmentCount(0, 3600, 0)).toBe(30);
  });

  it('with that count every hourly point of a day lands in its own slice', () => {
    const since = 0;
    const until = 24 * 3600 + 1800;
    const n = stripSegmentCount(since, until, 3600);
    const points = Array.from({ length: 25 }, (_, k) => ({ ts: k * 3600, avg: 20, min: 20, max: 20, loss: 0 })) as LatencyPoint[];
    const segs = stripSegments(points, since, until, n);
    expect(segs.every((g) => g.count === 1)).toBe(true);
  });
});

describe('stripLatencyLevel', () => {
  it('grey without data, red when all lost, otherwise by the latency thresholds', () => {
    expect(stripLatencyLevel({ count: 0, avg: null, allLost: false })).toBe('none');
    expect(stripLatencyLevel({ count: 2, avg: null, allLost: true })).toBe('bad');
    expect(stripLatencyLevel({ count: 1, avg: 80, allLost: false })).toBe('good');
    expect(stripLatencyLevel({ count: 1, avg: 120, allLost: false })).toBe('warn');
    expect(stripLatencyLevel({ count: 1, avg: 201, allLost: false })).toBe('bad');
  });
});
