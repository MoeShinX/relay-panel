import { describe, expect, it } from 'vitest';
import { aggregateNodesByGroup, safeNumber, groupStatus } from './aggregate';
import type { NodeStatus } from '../../api/types';

/** Minimal NodeStatus factory. Only set what a test cares about; the rest are
 *  the fields aggregateNodesByGroup reads (cpu/mem/uptime/last_seen required by
 *  the type but never touched by the aggregation). */
function node(group_id: number, over: Partial<NodeStatus> = {}): NodeStatus {
  return {
    group_id,
    group_name: `g${group_id}`,
    cpu: 0,
    mem: 0,
    connections: 0,
    uptime: 0,
    last_seen: '',
    ...over,
  } as NodeStatus;
}

describe('safeNumber', () => {
  it('keeps finite non-negative numbers', () => {
    expect(safeNumber(0)).toBe(0);
    expect(safeNumber(42)).toBe(42);
    expect(safeNumber(3.5)).toBe(3.5);
  });

  it('rejects NaN / Infinity / negatives / non-numbers', () => {
    expect(safeNumber(NaN)).toBe(0);
    expect(safeNumber(Infinity)).toBe(0);
    expect(safeNumber(-1)).toBe(0);
    expect(safeNumber(undefined)).toBe(0);
    expect(safeNumber(null)).toBe(0);
    expect(safeNumber('5')).toBe(0);
  });
});

describe('groupStatus', () => {
  it('all online → online', () => {
    expect(groupStatus(2, 2)).toBe('online');
  });
  it('some online → partial', () => {
    expect(groupStatus(1, 2)).toBe('partial');
  });
  it('none online → offline', () => {
    expect(groupStatus(0, 2)).toBe('offline');
  });
  it('zero total → offline', () => {
    expect(groupStatus(0, 0)).toBe('offline');
  });
});

describe('aggregateNodesByGroup — status', () => {
  it('two online nodes in one group → one row, nodes 2/2, status online', () => {
    const out = aggregateNodesByGroup([
      node(1, { node_id: 'a', online: true }),
      node(1, { node_id: 'b', online: true }),
    ]);
    expect(out).toHaveLength(1);
    expect(out[0].online_nodes).toBe(2);
    expect(out[0].total_nodes).toBe(2);
    expect(out[0].status).toBe('online');
  });

  it('one online + one offline → 1/2, status partial', () => {
    const out = aggregateNodesByGroup([
      node(1, { node_id: 'a', online: true }),
      node(1, { node_id: 'b', online: false }),
    ]);
    expect(out[0].online_nodes).toBe(1);
    expect(out[0].total_nodes).toBe(2);
    expect(out[0].status).toBe('partial');
  });

  it('both offline → 0/2, status offline', () => {
    const out = aggregateNodesByGroup([
      node(1, { node_id: 'a', online: false }),
      node(1, { node_id: 'b', online: false }),
    ]);
    expect(out[0].online_nodes).toBe(0);
    expect(out[0].total_nodes).toBe(2);
    expect(out[0].status).toBe('offline');
  });
});

describe('aggregateNodesByGroup — ordering', () => {
  it('outputs groups sorted by group_id ascending regardless of input order', () => {
    const out = aggregateNodesByGroup([node(2), node(1)]);
    expect(out.map((g) => g.group_id)).toEqual([1, 2]);
  });

  it('follows the panel-wide group order when the rows carry one', () => {
    const out = aggregateNodesByGroup([
      node(1, { group_sort_order: 3 }),
      node(2, { group_sort_order: 1 }),
      node(3, { group_sort_order: 2 }),
      node(4), // no known order: last
    ]);
    expect(out.map((g) => g.group_id)).toEqual([2, 3, 1, 4]);
  });
});

describe('aggregateNodesByGroup — live rate & connections (online only)', () => {
  it('sums upload_bps / download_bps / connections over ONLINE nodes only', () => {
    const out = aggregateNodesByGroup([
      node(1, { node_id: 'a', online: true, connections: 3, upload_bps: 100, download_bps: 200 }),
      node(1, { node_id: 'b', online: false, connections: 5, upload_bps: 999, download_bps: 999 }),
    ]);
    expect(out[0].connections).toBe(3);
    expect(out[0].upload_bps).toBe(100);
    expect(out[0].download_bps).toBe(200);
  });
});

describe('aggregateNodesByGroup — cumulative traffic (all rows with records)', () => {
  it('includes offline nodes still holding a status record', () => {
    const out = aggregateNodesByGroup([
      node(1, { node_id: 'a', online: true, boot_upload_bytes: 1000, boot_download_bytes: 2000 }),
      // offline but still has a status record → contributes its last bytes
      node(1, { node_id: 'b', online: false, boot_upload_bytes: 3000, boot_download_bytes: 4000 }),
    ]);
    expect(out[0].upload_bytes).toBe(4000);
    expect(out[0].download_bytes).toBe(6000);
  });
});

describe('aggregateNodesByGroup — NaN safety', () => {
  it('missing / NaN / Infinity / negative fields never produce NaN', () => {
    const out = aggregateNodesByGroup([
      node(1, {
        node_id: 'a', online: true,
        connections: NaN, upload_bps: Infinity, download_bps: -5,
        boot_upload_bytes: undefined as unknown as number,
        boot_download_bytes: null as unknown as number,
      }),
    ]);
    expect(out[0].connections).toBe(0);
    expect(out[0].upload_bps).toBe(0);
    expect(out[0].download_bps).toBe(0);
    expect(out[0].upload_bytes).toBe(0);
    expect(out[0].download_bytes).toBe(0);
  });
});
