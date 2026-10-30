import { describe, expect, it, vi, beforeEach } from 'vitest';
import { render, screen, waitFor, fireEvent } from '@testing-library/react';
import userEvent from '@testing-library/user-event';

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

describe('naming a node from the node status page', () => {
  it('an admin edits the label: PUT to the node, then the list is read again', async () => {
    mockUseAuth.mockReturnValue({ isAdmin: true });
    mockGet.mockImplementation((url: string) =>
      Promise.resolve(url === '/nodes' ? ok([node()]) : url === '/system/version' ? version : ok([])));
    mockPut.mockResolvedValue(ok('蓝希云 1号'));
    const user = userEvent.setup();
    render(<NodeStatus />);

    await user.click(await screen.findByRole('button', { name: /编辑备注|nodeLabelEdit/ }));
    const box = await screen.findByRole('textbox');
    await user.type(box, '蓝希云 1号');
    fireEvent.keyDown(box, { key: 'Enter', keyCode: 13 });
    fireEvent.keyUp(box, { key: 'Enter', keyCode: 13 });

    await waitFor(() => expect(mockPut).toHaveBeenCalledTimes(1));
    expect(mockPut).toHaveBeenCalledWith('/nodes/11/label/abc123', { label: '蓝希云 1号' });
    await waitFor(() =>
      expect(mockGet.mock.calls.filter((c) => c[0] === '/nodes').length).toBeGreaterThan(1));
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
