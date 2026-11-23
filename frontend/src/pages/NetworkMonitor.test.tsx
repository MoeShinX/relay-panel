import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { MemoryRouter, Route, Routes, useLocation } from 'react-router-dom';

const { mockGet } = vi.hoisted(() => ({ mockGet: vi.fn() }));
vi.mock('../api/client', () => ({ default: { get: mockGet } }));

vi.mock('@ant-design/charts', () => ({
  Line: ({ data }: { data: unknown[] }) => <div data-testid="chart" data-points={data.length} />,
}));

import NetworkMonitor, { LAST_LATENCY_GROUP_KEY, LegacyGroupLatencyRedirect } from './NetworkMonitor';

const ok = <T,>(data: T) => ({ code: 0, message: 'ok', data });

const NOW = Math.floor(Date.now() / 1000);

function groupLatencyPayload(groupId: number, groupName: string) {
  return {
    group_id: groupId,
    group_name: groupName,
    interval_secs: 10,
    range: '24h',
    granularity: '1h',
    bucket_secs: 3600,
    since: NOW - 86400,
    until: NOW,
    targets: [{ id: 1, name: 'Beijing Telecom', kind: 'tcp', host: '198.51.100.1', port: 443, carrier: 'telecom', sort_order: 1 }],
    nodes: [{ node_id: `node-${groupId}`, label: `HK-${groupId}`, public_ip: '192.0.2.10', node_version: '1.2.14', supported: true, unsupported_targets: [], last_seen: new Date().toISOString() }],
    matrix: [{ node_id: `node-${groupId}`, target_id: 1, avg: 25, loss: 0, sent: 60 }],
    target_id: 1,
    series: [{ node_id: `node-${groupId}`, latest_ts: NOW - 30, points: [{ ts: NOW - 60, avg: 25, min: 24, max: 26, loss: 0 }] }],
  };
}

function LocationProbe() {
  const loc = useLocation();
  return <div data-testid="location-probe">{`${loc.pathname}${loc.search}`}</div>;
}

function renderMonitor(initialUrl = '/latency') {
  return render(
    <MemoryRouter initialEntries={[initialUrl]}>
      <LocationProbe />
      <Routes>
        <Route path="/latency" element={<NetworkMonitor />} />
        <Route path="/groups/:groupId/latency" element={<LegacyGroupLatencyRedirect />} />
        <Route path="/groups" element={<div data-testid="groups-page">groups page</div>} />
      </Routes>
    </MemoryRouter>,
  );
}

function serveGroups(list: Array<{ id: number; name: string; targets: number }>) {
  const byId = new Map(list.map((g) => [g.id, g]));
  mockGet.mockImplementation((url: string) => {
    if (url === '/admin/latency/groups') {
      return Promise.resolve(ok(list.map((g) => ({ group_id: g.id, targets: g.targets }))));
    }
    if (url === '/groups') {
      return Promise.resolve(ok(list.map((g) => ({ id: g.id, name: g.name }))));
    }
    const m = /^\/admin\/groups\/(\d+)\/latency/.exec(url);
    if (m) {
      const id = Number(m[1]);
      const g = byId.get(id);
      return Promise.resolve(ok(groupLatencyPayload(id, g?.name ?? `Group-${id}`)));
    }
    return Promise.resolve(ok([]));
  });
}

beforeEach(() => {
  mockGet.mockReset();
  window.localStorage.clear();
});

afterEach(() => {
  vi.restoreAllMocks();
});

describe('NetworkMonitor page', () => {
  it('renders empty state when 0 groups have latency targets and links to /groups', async () => {
    serveGroups([]);
    renderMonitor();
    expect(await screen.findByTestId('network-monitor-empty')).toBeInTheDocument();
    expect(screen.getByText('networkMonitorEmpty')).toBeInTheDocument();
    fireEvent.click(screen.getByText('networkMonitorGoGroups'));
    expect(await screen.findByTestId('groups-page')).toBeInTheDocument();
  });

  it('hides group switcher when exactly 1 group is monitored and embeds GroupLatency without back button', async () => {
    serveGroups([{ id: 5, name: 'HK-Core', targets: 3 }]);
    renderMonitor();
    await waitFor(() => {
      expect(screen.getByTestId('group-latency-title')).toHaveTextContent('HK-Core');
    });
    expect(screen.queryByTestId('monitor-group-switcher')).toBeNull();
    expect(screen.queryByText('latencyBack')).toBeNull();
    expect(mockGet).toHaveBeenCalledWith(expect.stringContaining('/admin/groups/5/latency'));
  });

  it('shows Segmented switcher for 2..5 groups, prefers ?group=, and switches groups cleanly', async () => {
    window.localStorage.setItem(LAST_LATENCY_GROUP_KEY, '1');
    serveGroups([
      { id: 1, name: 'HK-Edge', targets: 3 },
      { id: 2, name: 'JP-Edge', targets: 2 },
      { id: 3, name: 'SG-Edge', targets: 4 },
    ]);
    renderMonitor('/latency?group=2');

    expect(await screen.findByTestId('monitor-group-segmented')).toBeInTheDocument();
    await waitFor(() => {
      expect(screen.getByTestId('group-latency-title')).toHaveTextContent('JP-Edge');
    });
    expect(mockGet).toHaveBeenCalledWith(expect.stringContaining('/admin/groups/2/latency'));

    // Switch to SG-Edge (id 3): updates URL, localStorage, and fetches group 3 latency
    fireEvent.click(screen.getByText('SG-Edge'));
    await waitFor(() => {
      expect(screen.getByTestId('group-latency-title')).toHaveTextContent('SG-Edge');
    });
    expect(mockGet).toHaveBeenCalledWith(expect.stringContaining('/admin/groups/3/latency'));
    expect(screen.getByTestId('location-probe')).toHaveTextContent('/latency?group=3');
    expect(window.localStorage.getItem(LAST_LATENCY_GROUP_KEY)).toBe('3');
  });

  it('uses Select instead of Segmented when more than 5 groups are monitored', async () => {
    serveGroups([
      { id: 1, name: 'Group-1', targets: 1 },
      { id: 2, name: 'Group-2', targets: 1 },
      { id: 3, name: 'Group-3', targets: 1 },
      { id: 4, name: 'Group-4', targets: 1 },
      { id: 5, name: 'Group-5', targets: 1 },
      { id: 6, name: 'Group-6', targets: 1 },
    ]);
    renderMonitor();
    expect(await screen.findByTestId('monitor-group-select')).toBeInTheDocument();
    expect(screen.queryByTestId('monitor-group-segmented')).toBeNull();
  });

  it('falls back to the first group and rewrites URL when ?group= is invalid or unmonitored', async () => {
    window.localStorage.setItem(LAST_LATENCY_GROUP_KEY, '2');
    serveGroups([
      { id: 1, name: 'HK-First', targets: 3 },
      { id: 2, name: 'JP-Second', targets: 2 },
    ]);
    renderMonitor('/latency?group=999');

    await waitFor(() => {
      expect(screen.getByTestId('group-latency-title')).toHaveTextContent('HK-First');
      expect(screen.getByTestId('location-probe')).toHaveTextContent('/latency?group=1');
    });
  });

  it('restores the last viewed group from localStorage when ?group= is absent', async () => {
    window.localStorage.setItem(LAST_LATENCY_GROUP_KEY, '2');
    serveGroups([
      { id: 1, name: 'HK-First', targets: 3 },
      { id: 2, name: 'JP-Second', targets: 2 },
    ]);
    renderMonitor('/latency');
    await waitFor(() => {
      expect(screen.getByTestId('group-latency-title')).toHaveTextContent('JP-Second');
    });
    expect(mockGet).toHaveBeenCalledWith(expect.stringContaining('/admin/groups/2/latency'));
  });

  it('renders normally even if localStorage throws on read and write', async () => {
    vi.spyOn(Storage.prototype, 'getItem').mockImplementation(() => {
      throw new Error('storage disabled');
    });
    vi.spyOn(Storage.prototype, 'setItem').mockImplementation(() => {
      throw new Error('storage quota exceeded');
    });
    serveGroups([
      { id: 1, name: 'HK-First', targets: 3 },
      { id: 2, name: 'JP-Second', targets: 2 },
    ]);
    renderMonitor('/latency');
    await waitFor(() => {
      expect(screen.getByTestId('group-latency-title')).toHaveTextContent('HK-First');
    });
    fireEvent.click(screen.getByText('JP-Second'));
    await waitFor(() => {
      expect(screen.getByTestId('group-latency-title')).toHaveTextContent('JP-Second');
    });
  });

  it('redirects legacy /groups/3/latency (preserving extra query params) to /latency?group=3', async () => {
    serveGroups([
      { id: 1, name: 'HK-First', targets: 2 },
      { id: 3, name: 'SG-Target', targets: 2 },
    ]);
    renderMonitor('/groups/3/latency?foo=bar');

    await waitFor(() => {
      expect(screen.getByTestId('group-latency-title')).toHaveTextContent('SG-Target');
    });
    expect(screen.getByTestId('location-probe')).toHaveTextContent('/latency?foo=bar&group=3');
  });
});
