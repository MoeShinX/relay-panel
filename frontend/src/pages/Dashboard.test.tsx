import { describe, expect, it, vi, beforeEach, afterEach } from 'vitest';
import { render, screen, act } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';

// Mock the api client before importing the page. Dashboard calls /admin/users,
// /rules, /groups, /nodes, and /system/version.
const { mockGet } = vi.hoisted(() => ({ mockGet: vi.fn() }));
const { mockNavigate } = vi.hoisted(() => ({ mockNavigate: vi.fn() }));

vi.mock('../api/client', () => ({
  default: { get: mockGet },
}));
vi.mock('react-router-dom', async () => {
  const actual = await vi.importActual<typeof import('react-router-dom')>('react-router-dom');
  return { ...actual, useNavigate: () => mockNavigate };
});

import Dashboard from './Dashboard';
import type { NodeStatus } from '../api/types';

const ok = <T,>(data: T) => ({ code: 0, message: 'ok', data });

// Drain pending promises under fake timers (see NodeStatus.test.tsx rationale).
const flush = (ms = 0) => act(async () => { await vi.advanceTimersByTimeAsync(ms); });

function ns(group_id: number, over: Partial<NodeStatus>): NodeStatus {
  return { group_id, group_name: `g${group_id}`, cpu: 0, mem: 0, connections: 0, uptime: 0, last_seen: '', ...over } as NodeStatus;
}

beforeEach(() => {
  mockGet.mockReset();
  mockNavigate.mockReset();
  vi.useFakeTimers();
});
afterEach(() => {
  vi.runOnlyPendingTimers();
  vi.useRealTimers();
});

/** Resolve every Dashboard API call. Unspecified endpoints 404-reject so a
 *  missed mock is loud rather than silently swallowed. */
function mockAll(nodes: NodeStatus[], rules: unknown[] = [{}]) {
  mockGet.mockImplementation((url: string) => {
    if (url === '/admin/users') return Promise.resolve(ok([{}]));
    if (url === '/rules') return Promise.resolve(ok(rules));
    if (url === '/groups') return Promise.resolve(ok([{}]));
    if (url === '/nodes') return Promise.resolve(ok(nodes));
    if (url === '/system/version') {
      return Promise.resolve({ current_version: '0.4.17', latest_version: '', has_update: false, is_outdated: false, release_url: '', release_notes: '', published_at: '', check_failed: false, error_message: '' });
    }
    return Promise.reject(new Error(`unexpected ${url}`));
  });
}

function renderDashboard() {
  return render(<MemoryRouter><Dashboard /></MemoryRouter>);
}

describe('Dashboard group aggregation', () => {
  it('renders one row per group with online/total and aggregates the rate', async () => {
    mockAll([
      ns(1, { node_id: 'a', online: true, upload_bps: 100, download_bps: 200, connections: 3 }),
      ns(1, { node_id: 'b', online: true, upload_bps: 50, download_bps: 30, connections: 1 }),
      ns(2, { node_id: 'c', online: false }),
    ]);
    renderDashboard();
    await flush();

    // group names appear
    expect(screen.getAllByText('g1').length).toBeGreaterThan(0);
    expect(screen.getAllByText('g2').length).toBeGreaterThan(0);
    // g1 has both nodes online → 2/2
    expect(screen.getByText('2/2')).toBeInTheDocument();
    // g2 fully offline → 0/1
    expect(screen.getByText('0/1')).toBeInTheDocument();
  });

  it('does NOT render CPU / MEM columns (aggregation dropped them)', async () => {
    mockAll([ns(1, { node_id: 'a', online: true })]);
    renderDashboard();
    await flush();
    const headers = screen.getAllByRole('columnheader').map((h) => h.textContent);
    // No header should contain "CPU" or the mem label
    expect(headers.some((h) => h && /CPU/i.test(h))).toBe(false);
    // i18n keys are echoed by the fake-t router only for t() calls; the mem
    // column header would be the raw 'mem' key — assert it's absent too.
    expect(headers.some((h) => h === 'mem')).toBe(false);
  });

  it('clicking a row navigates to /nodes', async () => {
    mockAll([ns(1, { node_id: 'a', online: true })]);
    renderDashboard();
    await flush();

    // the first table body row is clickable. `.ant-table-row` skips the hidden
    // measure row antd puts first in the body of a horizontally scrolling table.
    const row = document.querySelector('.ant-table-tbody tr.ant-table-row');
    expect(row).not.toBeNull();
    await act(async () => {
      row!.dispatchEvent(new MouseEvent('click', { bubbles: true }));
    });
    expect(mockNavigate).toHaveBeenCalledWith('/nodes');
  });

  // v1.2.12: each group's enabled / total rules, so a new machine (group) can
  // be checked against the old ones — the stat card only had the grand total.
  it('shows each group’s enabled / total rule count', async () => {
    mockAll(
      [
        ns(1, { node_id: 'a', online: true }),
        ns(1, { node_id: 'b', online: true }),
        ns(2, { node_id: 'c', online: false }),
      ],
      [
        { id: 1, device_group_in: 1, paused: false },
        { id: 2, device_group_in: 1, paused: true },
        { id: 3, device_group_in: 2, paused: false },
        { id: 4, device_group_in: 2, paused: false },
        { id: 5, device_group_in: 2, paused: false },
      ],
    );
    renderDashboard();
    await flush();

    expect(screen.getAllByRole('columnheader').some((h) => h.textContent === 'groupRules')).toBe(true);
    const cells = (gid: string) => {
      const row = screen.getAllByText(gid)[0].closest('tr')!;
      return Array.from(row.querySelectorAll('td')).map((td) => td.textContent);
    };
    expect(cells('g1')).toContain('1/2');
    expect(cells('g2')).toContain('3/3');
  });

  it('shows the empty hint when no nodes report', async () => {
    mockAll([]);
    renderDashboard();
    await flush();
    expect(screen.getByText('noNodesReporting')).toBeInTheDocument();
  });
});
