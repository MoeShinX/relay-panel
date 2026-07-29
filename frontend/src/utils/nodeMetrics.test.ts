import { describe, expect, it } from 'vitest';
import { formatMetric, multiNodeGroupNames, seriesName } from './nodeMetrics';

const b = (group_name: string, node_id: string) => ({ group_name, node_id });

describe('formatMetric', () => {
  it('treats CPU and memory as percentages, not fractions', () => {
    // Regression: the formatter multiplied by 100, so a node at 20.3% CPU
    // rendered as "2026.7%" and the axis topped out above 2000%. The node
    // reports global_cpu_usage() and used/total*100 — both already 0..100 —
    // and the panel stores them unscaled.
    expect(formatMetric('cpu', 20.267)).toBe('20.3%');
    expect(formatMetric('mem', 64.5)).toBe('64.5%');
  });

  it('never renders a percentage above 100 for an in-range reading', () => {
    for (const v of [0, 0.5, 33.3, 99.9, 100]) {
      const pct = parseFloat(formatMetric('cpu', v));
      expect(pct).toBeLessThanOrEqual(100);
    }
  });

  it('rounds connections to a whole number with no unit', () => {
    expect(formatMetric('conn', 12.6)).toBe('13');
  });
});

describe('legend naming', () => {
  it('leaves a single-node group as just the group name', () => {
    // The short id is six hex characters of a random per-node id — it means
    // nothing to a reader, so it is noise when there is nothing to disambiguate.
    const buckets = [b('广港自用', 'a1b2c3d4e5'), b('广港自用', 'a1b2c3d4e5')];
    const multi = multiNodeGroupNames(buckets);
    expect(multi.size).toBe(0);
    expect(seriesName(buckets[0], multi)).toBe('广港自用');
  });

  it('appends the short node id only where a group has several nodes', () => {
    const buckets = [
      b('广港自用', '17e54b0000'),
      b('广港自用', 'e797c40000'),
      b('沪日自用', '7e211300'),
    ];
    const multi = multiNodeGroupNames(buckets);
    expect(multi.has('广港自用')).toBe(true);
    expect(multi.has('沪日自用')).toBe(false);

    expect(seriesName(buckets[0], multi)).toBe('广港自用/17e54b');
    expect(seriesName(buckets[1], multi)).toBe('广港自用/e797c4');
    // The single-node group stays clean even when another group is split.
    expect(seriesName(buckets[2], multi)).toBe('沪日自用');
  });

  it('keeps the two nodes of one group distinguishable', () => {
    // The whole reason the suffix exists: without it these collapse onto one
    // legend entry and the chart silently draws two lines as one series.
    const buckets = [b('g', 'aaaaaaaa'), b('g', 'bbbbbbbb')];
    const multi = multiNodeGroupNames(buckets);
    expect(seriesName(buckets[0], multi)).not.toBe(seriesName(buckets[1], multi));
  });

  it('handles an empty response', () => {
    expect(multiNodeGroupNames([]).size).toBe(0);
  });
});
