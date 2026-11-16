import { describe, expect, it } from 'vitest';
import { act, renderHook } from '@testing-library/react';
import { useSeriesPick } from './useSeriesPick';

const keysOf = (n: number) => Array.from({ length: n }, (_, i) => `k${i}`);

describe('useSeriesPick', () => {
  it('draws everything and needs no picker at ten series or fewer', () => {
    const keys = keysOf(10);
    const { result } = renderHook(() => useSeriesPick(keys, [...keys].reverse()));
    expect(result.current.needsPicker).toBe(false);
    expect(result.current.shown).toEqual(keys);
  });

  it('a picked key missing from one view is skipped, not forgotten', () => {
    const all = keysOf(12);
    const withoutK1 = all.filter((k) => k !== 'k1');
    let keys = all;
    const { result, rerender } = renderHook(() => useSeriesPick(keys, keys));
    act(() => result.current.setPicked(['k1', 'k2', 'k11']));
    expect(result.current.shown).toEqual(['k1', 'k2', 'k11']);

    // e.g. another range where k1 has no data
    keys = withoutK1;
    rerender();
    expect(result.current.shown).toEqual(['k2', 'k11']);
    expect(result.current.automatic).toBe(false);

    // back again: k1 is drawn once more
    keys = all;
    rerender();
    expect(result.current.shown).toEqual(['k1', 'k2', 'k11']);
  });

  it('with none of the pick present the view is automatic, and the pick returns later', () => {
    const all = keysOf(12);
    let keys = all;
    const { result, rerender } = renderHook(() => useSeriesPick(keys, keys));
    act(() => result.current.setPicked(['k11']));
    keys = all.slice(0, 11);
    rerender();
    expect(result.current.automatic).toBe(true);
    expect(result.current.shown).toEqual(keys.slice(0, 10));
    keys = all;
    rerender();
    expect(result.current.shown).toEqual(['k11']);
  });
});
