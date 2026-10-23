import { describe, expect, it } from 'vitest';
import type { NodeDisplayRow } from '../../api/types';
import {
  hasGroupOrder,
  mergeVisibleOrder,
  moveGroup,
  stableGroupedRows,
  withGroupOrder,
} from './sort';

const row = (group_id: number, group_sort_order?: number | null): NodeDisplayRow => ({
  group_id,
  group_name: `g${group_id}`,
  node_id: `n${group_id}`,
  group_sort_order,
});

describe('moveGroup — an insert, not a swap', () => {
  const abcde = [1, 2, 3, 4, 5]; // A B C D E

  it('dragging up: D onto B gives A D B C E (not A D C B E)', () => {
    expect(moveGroup(abcde, 4, 2)).toEqual([1, 4, 2, 3, 5]);
  });

  it('dragging down: B onto D gives A C D B E (the mirror image)', () => {
    expect(moveGroup(abcde, 2, 4)).toEqual([1, 3, 4, 2, 5]);
  });

  it('to the very top', () => {
    expect(moveGroup(abcde, 5, 1)).toEqual([5, 1, 2, 3, 4]);
  });

  it('to the very bottom', () => {
    expect(moveGroup(abcde, 1, 5)).toEqual([2, 3, 4, 5, 1]);
  });

  it('one step up or down is the same as a swap of neighbours', () => {
    expect(moveGroup(abcde, 3, 2)).toEqual([1, 3, 2, 4, 5]);
    expect(moveGroup(abcde, 3, 4)).toEqual([1, 2, 4, 3, 5]);
  });

  it('returns the same array when nothing moves', () => {
    expect(moveGroup(abcde, 3, 3)).toBe(abcde);
    expect(moveGroup(abcde, 3, 99)).toBe(abcde);
    expect(moveGroup(abcde, 99, 3)).toBe(abcde);
  });

  it('does not change the array it was given', () => {
    const ids = [1, 2, 3];
    moveGroup(ids, 3, 1);
    expect(ids).toEqual([1, 2, 3]);
  });
});

describe('stableGroupedRows — display order', () => {
  it('follows group_sort_order, not group_id', () => {
    const groups = stableGroupedRows([row(1, 3), row(2, 1), row(3, 2)]);
    expect(groups.map(([gid]) => gid)).toEqual([2, 3, 1]);
  });

  it('breaks a tie on group_id', () => {
    const groups = stableGroupedRows([row(7, 0), row(3, 0), row(5, 0)]);
    expect(groups.map(([gid]) => gid)).toEqual([3, 5, 7]);
  });

  it('puts a group with no known order after the ones that have one', () => {
    const groups = stableGroupedRows([row(1, undefined), row(9, 5), row(2, null), row(8, 1)]);
    expect(groups.map(([gid]) => gid)).toEqual([8, 9, 1, 2]);
  });

  it('gives the same result whatever order the rows arrive in', () => {
    const rows = [row(1, 2), row(2, 3), row(3, 1)];
    const a = stableGroupedRows(rows);
    const b = stableGroupedRows([...rows].reverse());
    expect(b).toEqual(a);
  });
});

describe('hasGroupOrder', () => {
  it('is true only for a group whose order the server gave', () => {
    expect(hasGroupOrder([row(1, 0)])).toBe(true);
    expect(hasGroupOrder([row(1, undefined)])).toBe(false);
    expect(hasGroupOrder([row(1, null)])).toBe(false);
  });
});

describe('mergeVisibleOrder', () => {
  it('the shown groups take the shown slots; the hidden ones stay put', () => {
    // 9 is not on the page and keeps slot 3. Shown 1,2,3,4 -> 1,4,2,3.
    expect(mergeVisibleOrder([1, 2, 9, 3, 4], [1, 4, 2, 3])).toEqual([1, 4, 9, 2, 3]);
  });

  it('is the shown order itself when every group is shown', () => {
    expect(mergeVisibleOrder([1, 2, 3], [3, 1, 2])).toEqual([3, 1, 2]);
  });

  it('ignores a shown id the server no longer knows', () => {
    expect(mergeVisibleOrder([1, 2, 3], [3, 99, 1, 2])).toEqual([3, 1, 2]);
  });
});

describe('withGroupOrder', () => {
  it('numbers the given groups from 1 and leaves other rows alone', () => {
    const rows = [row(1, 1), row(2, 2), row(5, undefined)];
    const out = withGroupOrder(rows, [2, 1]);
    expect(out.map((r) => [r.group_id, r.group_sort_order])).toEqual([
      [1, 2],
      [2, 1],
      [5, undefined],
    ]);
    expect(out[2]).toBe(rows[2]);
  });
});
