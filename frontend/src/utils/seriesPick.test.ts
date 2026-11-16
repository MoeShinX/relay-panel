import { describe, expect, it } from 'vitest';
import { drawnKeys, meanOf, pruneSelection, rankByAttention, rankByScore, type LatencyReading } from './seriesPick';
import { MAX_SERIES } from './trafficBuckets';

const letters = (n: number) => Array.from({ length: n }, (_, i) => String.fromCharCode(97 + i));

describe('meanOf', () => {
  it('averages the finite values and ignores the rest', () => {
    expect(meanOf([2, 4, null, undefined, NaN])).toBe(3);
  });
  it('is null when there is nothing to average', () => {
    expect(meanOf([])).toBeNull();
    expect(meanOf([null, undefined])).toBeNull();
  });
});

describe('rankByScore', () => {
  it('puts the highest score first', () => {
    const score: Record<string, number> = { a: 10, b: 50, c: 30 };
    expect(rankByScore(['a', 'b', 'c'], (k) => score[k])).toEqual(['b', 'c', 'a']);
  });
  it('puts keys without a score last, in their own order', () => {
    const score: Record<string, number | null> = { a: null, b: 1, c: null, d: 0 };
    expect(rankByScore(['a', 'b', 'c', 'd'], (k) => score[k])).toEqual(['b', 'd', 'a', 'c']);
  });
  it('keeps the incoming order on ties', () => {
    expect(rankByScore(['x', 'y', 'z'], () => 7)).toEqual(['x', 'y', 'z']);
  });
  it('a zero is a score, not "no data"', () => {
    const score: Record<string, number | null> = { a: null, b: 0 };
    expect(rankByScore(['a', 'b'], (k) => score[k])).toEqual(['b', 'a']);
  });
});

describe('rankByAttention', () => {
  const rank = (readings: Record<string, LatencyReading | undefined>) =>
    rankByAttention(Object.keys(readings), (k) => readings[k]);

  it('orders bad before warn before good', () => {
    // by latencyLevel: good up to 80 ms, warn up to 200 ms
    expect(
      rank({
        fine: { avg: 20, loss: 0 },
        slow: { avg: 150, loss: 0 },
        awful: { avg: 900, loss: 0 },
      }),
    ).toEqual(['awful', 'slow', 'fine']);
  });

  it('loss decides within a level, then the mean', () => {
    expect(
      rank({
        a: { avg: 900, loss: 6 },
        b: { avg: 300, loss: 40 },
        c: { avg: 700, loss: 6 },
      }),
    ).toEqual(['b', 'a', 'c']);
  });

  it('a latency that is fine but lossy outranks a clean slow one in its level', () => {
    expect(
      rank({
        clean: { avg: 150, loss: 0 },
        lossy: { avg: 20, loss: 2 },
      }),
    ).toEqual(['lossy', 'clean']);
  });

  it('a line that lost everything is the worst of all', () => {
    expect(
      rank({
        slow: { avg: 5000, loss: 60 },
        lost: { avg: null, loss: 100 },
        warn: { avg: 150, loss: 0 },
      }),
    ).toEqual(['lost', 'slow', 'warn']);
  });

  it('even when the loss figure is below 100 an all-lost reading leads its level', () => {
    expect(
      rank({
        slow: { avg: 5000, loss: 60 },
        lost: { avg: null, loss: 30 },
      }),
    ).toEqual(['lost', 'slow']);
  });

  it('no cell or no reading goes last, after a good one', () => {
    expect(
      rank({
        nothing: undefined,
        empty: { avg: null, loss: 0 },
        fine: { avg: 5, loss: 0 },
        unknownLoss: { avg: null, loss: null },
      }),
    ).toEqual(['fine', 'empty', 'unknownLoss', 'nothing']);
  });

  it('keeps the incoming order on ties', () => {
    expect(
      rank({
        a: { avg: 20, loss: 0 },
        b: { avg: 20, loss: 0 },
        c: undefined,
        d: undefined,
        e: { avg: 20, loss: 0 },
      }),
    ).toEqual(['a', 'b', 'e', 'c', 'd']);
  });
});

describe('pruneSelection', () => {
  it('is null (automatic) with nothing picked', () => {
    expect(pruneSelection(null, ['a', 'b'])).toBeNull();
  });
  it('drops keys that no longer exist and keeps the order given', () => {
    expect(pruneSelection(['c', 'x', 'a'], ['a', 'b', 'c'])).toEqual(['c', 'a']);
  });
  it('is null when nothing is left', () => {
    expect(pruneSelection(['x', 'y'], ['a', 'b'])).toBeNull();
    expect(pruneSelection([], ['a'])).toBeNull();
  });
});

describe('drawnKeys', () => {
  const keys = letters(14);

  it('automatic: the first ten of the ranking, in the chart order', () => {
    const ranked = [...keys].reverse();
    const out = drawnKeys(keys, null, ranked);
    expect(out).toHaveLength(MAX_SERIES);
    expect(out).toEqual(keys.slice(4)); // e..n, the ten ranked first, back in chart order
  });

  it('explicit: just the pick, in the chart order', () => {
    expect(drawnKeys(keys, ['k', 'b', 'f'], keys)).toEqual(['b', 'f', 'k']);
  });

  it('never more than ten, even from an over-long pick', () => {
    expect(drawnKeys(keys, keys, keys)).toHaveLength(MAX_SERIES);
  });
});
