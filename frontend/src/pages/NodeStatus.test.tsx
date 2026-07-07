import { describe, expect, it, vi, beforeEach, afterEach } from 'vitest';
import { render, screen, act } from '@testing-library/react';

// Mock the api client + auth hook before importing the page under test.
const { mockGet } = vi.hoisted(() => ({ mockGet: vi.fn() }));
const { mockUseAuth } = vi.hoisted(() => ({ mockUseAuth: vi.fn() }));

vi.mock('../api/client', () => ({
  default: { get: mockGet, delete: vi.fn() },
}));
vi.mock('../auth/useAuth', () => ({ useAuth: mockUseAuth }));

import NodeStatus from './NodeStatus';
import { stableGroupedRows, compareNodeRows } from '../components/nodes/sort';
import type { NodeDisplayRow } from '../api/types';

const ok = <T,>(data: T) => ({ code: 0, message: 'ok', data });

// Flush all pending microtasks/promises under fake timers. We deliberately
// avoid @testing-library's waitFor here: it polls on real timers and hangs when
// fake timers are installed. advanceTimersByTimeAsync(ms) drains the promise
// queue deterministically instead.
const flush = (ms = 0) => act(async () => { await vi.advanceTimersByTimeAsync(ms); });

const adminNode = {
  group_id: 1, group_name: 'admin-grp', node_id: 'n1', online: true,
  cpu: 5, mem: 5, connections: 0, uptime: 100, last_seen: new Date().toISOString(),
};
const sharedNode = {
  group_id: 2, group_name: 'shared-grp', node_id: 's1', online: true, connections: 0,
};
// v1.2: /system/version now also returns latest_node_version +
// node_version_check_failed. Existing tests ignore them (they assert on data
// source / polling, not the upgrade column), but the shape must match the
// real response so the fetch + state setters don't degrade.
const version = {
  current_version: '1.1.0',
  config_protocol_version: 2,
  latest_node_version: '1.1.0',
  node_version_check_failed: false,
};

beforeEach(() => {
  mockGet.mockReset();
  mockUseAuth.mockReset();
  vi.useFakeTimers();
});
afterEach(() => {
  vi.runOnlyPendingTimers();
  vi.useRealTimers();
});

describe('NodeStatus page data source', () => {
  it('admin reads /nodes + /system/version (version fetched once, not polled)', async () => {
    mockUseAuth.mockReturnValue({ isAdmin: true });
    mockGet.mockImplementation((url: string) => {
      if (url === '/nodes') return Promise.resolve(ok([adminNode]));
      if (url === '/system/version') return Promise.resolve(version);
      return Promise.reject(new Error(`unexpected ${url}`));
    });

    render(<NodeStatus />);
    await flush();

    expect(screen.getByText('admin-grp')).toBeInTheDocument();
    expect(mockGet).toHaveBeenCalledWith('/nodes');
    expect(mockGet).toHaveBeenCalledWith('/system/version');
    expect(mockGet).not.toHaveBeenCalledWith('/nodes/shared');

    // version must NOT be polled: advance several intervals, still exactly one
    await flush(15000);
    const versionCalls = mockGet.mock.calls.filter((c) => c[0] === '/system/version').length;
    expect(versionCalls).toBe(1);
  });

  it('regular user reads /nodes/shared, never /nodes or /system/version', async () => {
    mockUseAuth.mockReturnValue({ isAdmin: false });
    mockGet.mockImplementation((url: string) => {
      if (url === '/nodes/shared') return Promise.resolve(ok([sharedNode]));
      return Promise.reject(new Error(`unexpected ${url}`));
    });

    render(<NodeStatus />);
    await flush();

    expect(screen.getByText('shared-grp')).toBeInTheDocument();
    expect(mockGet).toHaveBeenCalledWith('/nodes/shared');
    expect(mockGet).not.toHaveBeenCalledWith('/nodes');
    expect(mockGet).not.toHaveBeenCalledWith('/system/version');
  });
});

describe('NodeStatus load-failure behavior', () => {
  it('shows the error result when the first admin load fails', async () => {
    mockUseAuth.mockReturnValue({ isAdmin: true });
    mockGet.mockImplementation((url: string) => {
      if (url === '/nodes') return Promise.reject(new Error('boom'));
      if (url === '/system/version') return Promise.resolve(version);
      return Promise.reject(new Error(`unexpected ${url}`));
    });

    render(<NodeStatus />);
    await flush();

    expect(screen.getByText('loadFailed')).toBeInTheDocument();
  });

  it('does not flash the error page back to stale data on a later poll failure', async () => {
    mockUseAuth.mockReturnValue({ isAdmin: true });
    let nodeCall = 0;
    mockGet.mockImplementation((url: string) => {
      if (url === '/system/version') return Promise.resolve(version);
      if (url === '/nodes') {
        nodeCall += 1;
        return nodeCall === 1
          ? Promise.resolve(ok([adminNode]))
          : Promise.reject(new Error('transient'));
      }
      return Promise.reject(new Error(`unexpected ${url}`));
    });

    render(<NodeStatus />);
    await flush();
    expect(screen.getByText('admin-grp')).toBeInTheDocument();

    // Trigger the poll; it fails. The page must surface the error and must NOT
    // keep showing the now-stale prior data underneath as if healthy.
    await flush(5000);
    expect(screen.getByText('loadFailed')).toBeInTheDocument();
    expect(screen.queryByText('admin-grp')).not.toBeInTheDocument();
  });
});

// v0.4.16 PR3: the node-status board must render groups in a STABLE order
// independent of the API/KVS return order, and keep multi-node groups sorted
// within themselves. These pin the pure sort + the rendered DOM order.

// Minimal row factory (only the fields the sort touches + a label to find in
// the DOM). public_ip is the legacy fallback for public_ipv4. Typed as
// NodeDisplayRow so the sort calls are type-checked; missing optional fields
// are fine (they're undefined in the type).
const mk = (
  group_id: number,
  group_name: string,
  opts: Partial<{
    node_id: string;
    public_ipv4: string;
    public_ip: string;
    public_ipv6: string;
    online: boolean;
  }> = {},
): NodeDisplayRow =>
  ({
    group_id,
    group_name,
    node_id: opts.node_id ?? `node-${group_id}`,
    public_ipv4: opts.public_ipv4,
    public_ip: opts.public_ip,
    public_ipv6: opts.public_ipv6,
    online: opts.online ?? true,
    cpu: 0,
    mem: 0,
    connections: 0,
    uptime: 0,
    last_seen: '',
  }) as NodeDisplayRow;

describe('stableGroupedRows — pure ordering', () => {
  it('sorts groups by group_id ascending regardless of input order', () => {
    const rows = [mk(2, 'b'), mk(1, 'a'), mk(4, 'd'), mk(3, 'c')];
    const groups = stableGroupedRows(rows);
    expect(groups.map(([gid]) => gid)).toEqual([1, 2, 3, 4]);
  });

  it('produces identical output for the same set in different orders', () => {
    const set = [mk(3, 'c'), mk(1, 'a'), mk(2, 'b')];
    // Two permutations of the same set must yield identical group sequences.
    const a = stableGroupedRows([set[0], set[1], set[2]]);
    const b = stableGroupedRows([set[2], set[0], set[1]]);
    expect(b).toEqual(a);
  });

  it('sorts nodes within a group by public_ipv4 (then legacy public_ip fallback)', () => {
    const rows = [
      mk(1, 'g', { node_id: 'n3', public_ipv4: '9.9.9.9' }),
      mk(1, 'g', { node_id: 'n1', public_ipv4: '1.1.1.1' }),
      mk(1, 'g', { node_id: 'n2', public_ip: '5.5.5.5' }), // legacy field only
    ];
    const [[, group]] = stableGroupedRows(rows);
    expect(group.map((r) => r.node_id)).toEqual(['n1', 'n2', 'n3']);
  });

  it('breaks ties on public_ipv6 then node_id', () => {
    // Same ipv4 -> ipv4 decides; same ipv4+ipv6 -> node_id decides.
    const rows = [
      mk(1, 'g', { node_id: 'zeta', public_ipv4: '1.1.1.1', public_ipv6: 'b::2' }),
      mk(1, 'g', { node_id: 'alpha', public_ipv4: '1.1.1.1', public_ipv6: 'a::1' }),
      mk(1, 'g', { node_id: 'mid', public_ipv4: '1.1.1.1', public_ipv6: 'a::1' }),
    ];
    const [[, group]] = stableGroupedRows(rows);
    // ipv6 a::1 before b::2; within a::1, node_id alpha before mid.
    expect(group.map((r) => r.node_id)).toEqual(['alpha', 'mid', 'zeta']);
  });

  it('sorts empty/missing IPs last within a group (stable, no flicker)', () => {
    const rows = [
      mk(1, 'g', { node_id: 'blank' }), // no IP yet
      mk(1, 'g', { node_id: 'hasip', public_ipv4: '1.1.1.1' }),
    ];
    const [[, group]] = stableGroupedRows(rows);
    expect(group.map((r) => r.node_id)).toEqual(['hasip', 'blank']);
  });
});

describe('compareNodeRows — direct comparator', () => {
  it('returns 0 for fully-equal sort keys', () => {
    expect(
      compareNodeRows(
        mk(1, 'g', { node_id: 'x', public_ipv4: '1.1.1.1', public_ipv6: '::1' }),
        mk(2, 'other', { node_id: 'x', public_ipv4: '1.1.1.1', public_ipv6: '::1' }),
      ),
    ).toBe(0);
  });
});

describe('NodeStatus rendered group order is stable across refreshes', () => {
  // Find the DEEPEST element whose textContent is exactly `text` (a group
  // header label), then return its document position. Header labels render as
  // leaf Text nodes inside the Collapse header, so the deepest exact match is
  // the header itself — not a wrapping ancestor. compareDocumentPosition tells
  // us relative order without relying on fragile array indices.
  const docOrderIdx = (text: string): Element => {
    const matches = Array.from(document.querySelectorAll('*')).filter(
      (el) => el.textContent === text && el.children.length === 0,
    );
    if (matches.length === 0) throw new Error(`no leaf element with text "${text}"`);
    return matches[0];
  };
  // Returns true if a comes before b in document order.
  const isBefore = (a: Element, b: Element) =>
    // Node.DOCUMENT_POSITION_FOLLOWING = 4: b follows a.
    (a.compareDocumentPosition(b) & Node.DOCUMENT_POSITION_FOLLOWING) !== 0;

  it('renders groups in ascending group_id order even when the API returns them shuffled', async () => {
    mockUseAuth.mockReturnValue({ isAdmin: true });
    mockGet.mockImplementation((url: string) => {
      if (url === '/nodes')
        return Promise.resolve(
          ok([
            mk(2, 'grp-two'),
            mk(4, 'grp-four'),
            mk(1, 'grp-one'),
            mk(3, 'grp-three'),
          ]),
        );
      if (url === '/system/version') return Promise.resolve(version);
      return Promise.reject(new Error(`unexpected ${url}`));
    });

    render(<NodeStatus />);
    await flush();

    // The four group headers must appear in ascending group_id order in the DOM.
    const labels = ['grp-one', 'grp-two', 'grp-three', 'grp-four'];
    for (let i = 1; i < labels.length; i++) {
      expect(isBefore(docOrderIdx(labels[i - 1]), docOrderIdx(labels[i]))).toBe(true);
    }
  });

  it('keeps the same rendered order when a later poll returns the same set reshuffled', async () => {
    mockUseAuth.mockReturnValue({ isAdmin: true });
    let call = 0;
    const set = [mk(3, 'grp-three'), mk(1, 'grp-one'), mk(2, 'grp-two')];
    mockGet.mockImplementation((url: string) => {
      if (url === '/system/version') return Promise.resolve(version);
      if (url === '/nodes') {
        call += 1;
        // Second call returns the SAME set in a different order.
        return Promise.resolve(ok(call === 1 ? set : [set[1], set[2], set[0]]));
      }
      return Promise.reject(new Error(`unexpected ${url}`));
    });

    render(<NodeStatus />);
    await flush();
    const order = ['grp-one', 'grp-two', 'grp-three'];
    const isAscending = () => {
      for (let i = 1; i < order.length; i++) {
        if (!isBefore(docOrderIdx(order[i - 1]), docOrderIdx(order[i]))) return false;
      }
      return true;
    };
    expect(isAscending()).toBe(true);

    // Trigger the 5s poll (reshuffled payload). Same set, different order →
    // the rendered order must be IDENTICAL (still ascending group_id).
    await flush(5000);
    expect(isAscending()).toBe(true);
  });

  it('renders a multi-node group with stable node order (admin /nodes)', async () => {
    mockUseAuth.mockReturnValue({ isAdmin: true });
    mockGet.mockImplementation((url: string) => {
      if (url === '/nodes')
        return Promise.resolve(
          ok([
            mk(1, 'g', { node_id: 'n3', public_ipv4: '9.9.9.9' }),
            mk(1, 'g', { node_id: 'n1', public_ipv4: '1.1.1.1' }),
            mk(1, 'g', { node_id: 'n2', public_ipv4: '5.5.5.5' }),
          ]),
        );
      if (url === '/system/version') return Promise.resolve(version);
      return Promise.reject(new Error(`unexpected ${url}`));
    });

    render(<NodeStatus />);
    await flush();

    // The three IPs render in distinct table cells; the lowest-numbered IP
    // must appear first in document order (ipv4-ascending within the group).
    expect(isBefore(docOrderIdx('1.1.1.1'), docOrderIdx('5.5.5.5'))).toBe(true);
    expect(isBefore(docOrderIdx('5.5.5.5'), docOrderIdx('9.9.9.9'))).toBe(true);
  });

  it('regular user /nodes/shared gets the same stable ordering', async () => {
    mockUseAuth.mockReturnValue({ isAdmin: false });
    mockGet.mockImplementation((url: string) => {
      if (url === '/nodes/shared')
        return Promise.resolve(
          ok([
            mk(2, 'shared-two'),
            mk(1, 'shared-one'),
          ]),
        );
      return Promise.reject(new Error(`unexpected ${url}`));
    });

    render(<NodeStatus />);
    await flush();

    expect(isBefore(docOrderIdx('shared-one'), docOrderIdx('shared-two'))).toBe(true);
  });
});
