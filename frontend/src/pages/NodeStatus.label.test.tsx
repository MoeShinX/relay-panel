import { describe, expect, it, vi, beforeEach } from 'vitest';
import { render, screen } from '@testing-library/react';

const { mockGet, mockPut } = vi.hoisted(() => ({ mockGet: vi.fn(), mockPut: vi.fn() }));
const { mockUseAuth } = vi.hoisted(() => ({ mockUseAuth: vi.fn() }));
vi.mock('../api/client', () => ({
  default: { get: mockGet, put: mockPut, delete: vi.fn() },
}));
vi.mock('../auth/useAuth', () => ({ useAuth: mockUseAuth }));

import NodeStatus from './NodeStatus';

const ok = <T,>(data: T) => ({ code: 0, message: 'ok', data });
const node = (over: Record<string, unknown> = {}) => ({
  group_id: 11, group_name: 'JTTI-HK', node_id: 'abc123', online: true,
  public_ipv4: '177.201.231.20', cpu: 5, mem: 5, connections: 0, uptime: 100,
  last_seen: new Date().toISOString(), ...over,
});
const version = { current_version: '1.2.17', config_protocol_version: 4, latest_node_version: '1.2.10', node_version_check_failed: false };

beforeEach(() => {
  mockGet.mockReset();
  mockPut.mockReset();
  mockUseAuth.mockReset();
});

describe('node names on the node status page', () => {
  it('an admin sees no editing here: naming a machine is done on the device-group page', async () => {
    mockUseAuth.mockReturnValue({ isAdmin: true });
    mockGet.mockImplementation((url: string) =>
      Promise.resolve(url === '/nodes' ? ok([node()]) : url === '/system/version' ? version : ok([])));
    render(<NodeStatus />);
    await screen.findByText('JTTI-HK');
    expect(screen.queryByRole('button', { name: /编辑备注|nodeLabelEdit/ })).toBeNull();
    expect(screen.queryByText(/添加备注|nodeLabelAdd/)).toBeNull();
    expect(mockPut).not.toHaveBeenCalled();
  });

  it('shows the saved name', async () => {
    mockUseAuth.mockReturnValue({ isAdmin: true });
    mockGet.mockImplementation((url: string) =>
      Promise.resolve(url === '/nodes' ? ok([node({ label: 'HK-1 备用' })]) : url === '/system/version' ? version : ok([])));
    render(<NodeStatus />);
    expect(await screen.findByText('HK-1 备用')).toBeInTheDocument();
  });

  it('a user gets no editing, and sees a name only when the panel sent one', async () => {
    mockUseAuth.mockReturnValue({ isAdmin: false });
    mockGet.mockImplementation((url: string) =>
      Promise.resolve(url === '/nodes/shared' ? ok([node({ label: 'HK-1' })]) : ok([])));
    render(<NodeStatus />);
    expect(await screen.findByText('HK-1')).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: /编辑备注|nodeLabelEdit/ })).toBeNull();
  });
});
