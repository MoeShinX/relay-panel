import { describe, expect, it, vi, beforeEach } from 'vitest';
import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { MemoryRouter, Route, Routes, useParams } from 'react-router-dom';

const { mockGet } = vi.hoisted(() => ({ mockGet: vi.fn() }));
const { mockUseAuth } = vi.hoisted(() => ({ mockUseAuth: vi.fn() }));
vi.mock('../api/client', () => ({
  default: { get: mockGet, put: vi.fn(), delete: vi.fn() },
}));
vi.mock('../auth/useAuth', () => ({ useAuth: mockUseAuth }));

import NodeStatus from './NodeStatus';

const ok = <T,>(data: T) => ({ code: 0, message: 'ok', data });
const node = (over: Record<string, unknown> = {}) => ({
  group_id: 11, group_name: 'edge-group', node_id: 'abc123', online: true,
  public_ipv4: '192.0.2.20', cpu: 5, mem: 5, connections: 0, uptime: 100,
  last_seen: new Date().toISOString(), ...over,
});
const version = { current_version: '1.2.21', config_protocol_version: 4, latest_node_version: '1.2.12', node_version_check_failed: false };
/** `/admin/latency/groups`: the groups that have targets, with their counts. */
const groupsWith = (...ids: number[]) => ok(ids.map((group_id) => ({ group_id, targets: 2 })));
const adminApi = (rows: unknown[], latencyGroups: unknown) => (url: string) =>
  Promise.resolve(url === '/nodes' ? ok(rows) : url === '/system/version' ? version : url === '/admin/latency/groups' ? latencyGroups : ok([]));

function NodeLatencyStub() {
  const { nodeId } = useParams();
  return <div>latency page for {nodeId}</div>;
}
function GroupLatencyStub() {
  const { groupId } = useParams();
  return <div>group latency page for {groupId}</div>;
}

function renderPage() {
  return render(
    <MemoryRouter initialEntries={['/nodes']}>
      <Routes>
        <Route path="/nodes" element={<NodeStatus />} />
        <Route path="/nodes/:nodeId/latency" element={<NodeLatencyStub />} />
        <Route path="/groups/:groupId/latency" element={<GroupLatencyStub />} />
      </Routes>
    </MemoryRouter>,
  );
}

beforeEach(() => {
  mockGet.mockReset();
  mockUseAuth.mockReset();
});

describe('the latency icon beside a node\'s IP', () => {
  it('an admin gets a monitor icon for a node of a group with targets, opening that node\'s page', async () => {
    mockUseAuth.mockReturnValue({ isAdmin: true });
    mockGet.mockImplementation(adminApi([node()], groupsWith(11)));
    renderPage();
    await screen.findByText('192.0.2.20');
    const link = await screen.findByTestId('latency-link');
    await userEvent.click(link);
    expect(await screen.findByText('latency page for abc123')).toBeInTheDocument();
  });

  it('a regular user gets no such icon, and never asks which groups have targets', async () => {
    mockUseAuth.mockReturnValue({ isAdmin: false });
    mockGet.mockImplementation((url: string) =>
      Promise.resolve(url === '/nodes/shared' ? ok([{ ...node(), node_id: 'abc123' }]) : ok([])));
    renderPage();
    await screen.findByText('192.0.2.20');
    expect(screen.queryByTestId('latency-link')).toBeNull();
    expect(screen.queryByTestId('group-latency-11')).toBeNull();
    expect(mockGet).not.toHaveBeenCalledWith('/admin/latency/groups');
  });

  it('a node the admin switched the monitor off for has no icon, its neighbours keep theirs', async () => {
    mockUseAuth.mockReturnValue({ isAdmin: true });
    mockGet.mockImplementation(adminApi(
      [
        node({ latency_enabled: false }),
        node({ node_id: 'def456', public_ipv4: '192.0.2.21', latency_enabled: true }),
        node({ node_id: 'ghi789', public_ipv4: '192.0.2.22' }), // an older panel's row: no flag, on
      ],
      groupsWith(11),
    ));
    renderPage();
    await screen.findByText('192.0.2.20');
    await screen.findByText('192.0.2.22');
    const links = await screen.findAllByTestId('latency-link');
    expect(links).toHaveLength(2);
    await userEvent.click(links[0]);
    expect(await screen.findByText('latency page for def456')).toBeInTheDocument();
  });

  it('a node row with no node id (legacy status record) has no icon either', async () => {
    mockUseAuth.mockReturnValue({ isAdmin: true });
    mockGet.mockImplementation(adminApi([node({ node_id: null })], groupsWith(11)));
    renderPage();
    await screen.findByText('edge-group');
    expect(screen.queryByTestId('latency-link')).toBeNull();
  });

  it('an admin sees no icon for a group that has no targets, only for the ones that do', async () => {
    mockUseAuth.mockReturnValue({ isAdmin: true });
    mockGet.mockImplementation(adminApi(
      [
        node(),
        node({ group_id: 12, group_name: 'other-group', node_id: 'def456', public_ipv4: '192.0.2.21' }),
      ],
      groupsWith(12),
    ));
    renderPage();
    await screen.findByText('192.0.2.20');
    await screen.findByText('192.0.2.21');
    expect(mockGet).toHaveBeenCalledWith('/admin/latency/groups');
    const links = await screen.findAllByTestId('latency-link');
    expect(links).toHaveLength(1);
    await userEvent.click(links[0]);
    expect(await screen.findByText('latency page for def456')).toBeInTheDocument();
  });

  it('with no group configured there is no icon at all', async () => {
    mockUseAuth.mockReturnValue({ isAdmin: true });
    mockGet.mockImplementation(adminApi([node()], groupsWith()));
    renderPage();
    await screen.findByText('192.0.2.20');
    expect(screen.queryByTestId('latency-link')).toBeNull();
  });
});

describe('the monitor button in a group\'s header', () => {
  it('is on the header of a group that has targets and opens the group comparison', async () => {
    mockUseAuth.mockReturnValue({ isAdmin: true });
    mockGet.mockImplementation(adminApi([node()], groupsWith(11)));
    renderPage();
    const button = await screen.findByTestId('group-latency-11');
    await userEvent.click(button);
    expect(await screen.findByText('group latency page for 11')).toBeInTheDocument();
  });

  it('is only on the groups that have targets', async () => {
    mockUseAuth.mockReturnValue({ isAdmin: true });
    mockGet.mockImplementation(adminApi(
      [node(), node({ group_id: 12, group_name: 'other-group', node_id: 'def456', public_ipv4: '192.0.2.21' })],
      groupsWith(12),
    ));
    renderPage();
    await screen.findByTestId('group-latency-12');
    expect(screen.queryByTestId('group-latency-11')).toBeNull();
  });

  it('is not drawn when no group has targets, nor for a regular user', async () => {
    mockUseAuth.mockReturnValue({ isAdmin: true });
    mockGet.mockImplementation(adminApi([node()], groupsWith()));
    const { unmount } = renderPage();
    await screen.findByText('192.0.2.20');
    expect(screen.queryByTestId('group-latency-11')).toBeNull();
    unmount();

    mockUseAuth.mockReturnValue({ isAdmin: false });
    mockGet.mockImplementation((url: string) =>
      Promise.resolve(url === '/nodes/shared' ? ok([{ ...node(), node_id: 'abc123' }]) : ok([])));
    renderPage();
    await screen.findByText('192.0.2.20');
    expect(screen.queryByTestId('group-latency-11')).toBeNull();
  });
});
