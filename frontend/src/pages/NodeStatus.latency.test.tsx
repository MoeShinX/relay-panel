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
const withTargets = ok({ interval_secs: 10, targets: [{ id: 1, name: 'A', carrier: 'telecom', kind: 'tcp', host: '192.0.2.1', port: 443, sort_order: 0, created_at: '' }] });
const noTargets = ok({ interval_secs: 10, targets: [] });
const adminApi = (rows: unknown[], latency: unknown) => (url: string) =>
  Promise.resolve(url === '/nodes' ? ok(rows) : url === '/system/version' ? version : url === '/admin/latency/config' ? latency : ok([]));

function LatencyStub() {
  const { nodeId } = useParams();
  return <div>latency page for {nodeId}</div>;
}

function renderPage() {
  return render(
    <MemoryRouter initialEntries={['/nodes']}>
      <Routes>
        <Route path="/nodes" element={<NodeStatus />} />
        <Route path="/nodes/:nodeId/latency" element={<LatencyStub />} />
      </Routes>
    </MemoryRouter>,
  );
}

beforeEach(() => {
  mockGet.mockReset();
  mockUseAuth.mockReset();
});

describe('the latency link on the node status page', () => {
  it('an admin gets a monitor icon beside the IP, and it opens that node\'s latency page', async () => {
    mockUseAuth.mockReturnValue({ isAdmin: true });
    mockGet.mockImplementation(adminApi([node()], withTargets));
    renderPage();
    await screen.findByText('192.0.2.20');
    const link = await screen.findByTestId('latency-link');
    await userEvent.click(link);
    expect(await screen.findByText('latency page for abc123')).toBeInTheDocument();
  });

  it('a regular user gets no such icon', async () => {
    mockUseAuth.mockReturnValue({ isAdmin: false });
    mockGet.mockImplementation((url: string) =>
      Promise.resolve(url === '/nodes/shared' ? ok([{ ...node(), node_id: 'abc123' }]) : ok([])));
    renderPage();
    await screen.findByText('192.0.2.20');
    expect(screen.queryByTestId('latency-link')).toBeNull();
  });

  it('a node row with no node id (legacy status record) has no icon either', async () => {
    mockUseAuth.mockReturnValue({ isAdmin: true });
    mockGet.mockImplementation(adminApi([node({ node_id: null })], withTargets));
    renderPage();
    await screen.findByText('edge-group');
    expect(screen.queryByTestId('latency-link')).toBeNull();
  });

  it('an admin sees no icon until latency targets are configured', async () => {
    mockUseAuth.mockReturnValue({ isAdmin: true });
    mockGet.mockImplementation(adminApi([node()], noTargets));
    renderPage();
    await screen.findByText('192.0.2.20');
    expect(mockGet).toHaveBeenCalledWith('/admin/latency/config');
    expect(screen.queryByTestId('latency-link')).toBeNull();
  });
});
