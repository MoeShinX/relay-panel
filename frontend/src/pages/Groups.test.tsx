import { describe, expect, it, vi, beforeEach } from 'vitest';
import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';

const { mockGet, mockPost, mockPut } = vi.hoisted(() => ({
  mockGet: vi.fn(), mockPost: vi.fn(), mockPut: vi.fn(),
}));
vi.mock('../api/client', () => ({
  default: { get: mockGet, post: mockPost, put: mockPut, delete: vi.fn() },
}));
const { mockUseAuth } = vi.hoisted(() => ({ mockUseAuth: vi.fn() }));
vi.mock('../auth/useAuth', () => ({ useAuth: mockUseAuth }));

import Groups from './Groups';

const ok = <T,>(data: T) => ({ code: 0, message: 'ok', data });

function group(over: Record<string, unknown> = {}) {
  return {
    id: 1, name: 'g1', group_type: 'in', token: 'tok-'.padEnd(36, 'x'), uid: 1,
    connect_host: '1.2.3.4', port_range: '10000-65535', rate: 1.0, hidden: false,
    ...over,
  };
}

beforeEach(() => {
  mockGet.mockReset();
  mockPost.mockReset();
  mockPut.mockReset();
  mockUseAuth.mockReset();
  mockUseAuth.mockReturnValue({ isAdmin: true });
  mockGet.mockImplementation((url: string) => {
    if (url === '/groups') return Promise.resolve(ok([group()]));
    return Promise.resolve(ok([]));
  });
});

// ── v1.2.5: a monitor-only group forwards nothing and never reaches a regular
// user, so connect host / port range / rate / hidden are inert on it. The forms
// stop asking for them, and nothing inert gets written to the DB.
describe('monitor-only groups hide the forwarding fields', () => {
  it('drops the forwarding fields from the create form when the type is monitor', async () => {
    const user = userEvent.setup();
    render(<Groups />);
    await user.click(screen.getByRole('button', { name: /addGroup/i }));

    // Inbound is the default: the fields are there to begin with.
    await waitFor(() => expect(screen.getByLabelText('connectHost')).toBeInTheDocument());
    expect(screen.getByLabelText('portRange')).toBeInTheDocument();

    // Switch to monitor-only.
    await user.click(screen.getByLabelText('type'));
    await user.click(await screen.findByTitle('typeMonitor'));

    await waitFor(() => expect(screen.queryByLabelText('connectHost')).toBeNull());
    expect(screen.queryByLabelText('portRange')).toBeNull();
    expect(screen.queryByLabelText('rate')).toBeNull();
    expect(screen.getByText('monitorOnlyNoForwardTitle')).toBeInTheDocument();
  });

  it('sends empty forwarding fields rather than omitting them', async () => {
    // Hiding the Form.Items unregisters them, so they vanish from the submitted
    // values — and CreateGroupRequest declares connect_host/port_range as plain
    // String with no serde default, which turns an omission into a 422. The
    // explicit empty string is what makes creating a monitor group work at all;
    // it also guarantees a host typed before the switch is not carried over.
    const user = userEvent.setup();
    mockPost.mockResolvedValue(ok(group({ group_type: 'monitor' })));
    render(<Groups />);
    await user.click(screen.getByRole('button', { name: /addGroup/i }));

    await user.type(await screen.findByLabelText('name'), 'watch-only');
    await user.type(screen.getByLabelText('connectHost'), '9.9.9.9');

    await user.click(screen.getByLabelText('type'));
    await user.click(await screen.findByTitle('typeMonitor'));
    await user.click(screen.getByRole('button', { name: /^create$/i }));

    await waitFor(() => expect(mockPost).toHaveBeenCalled());
    const [url, body] = mockPost.mock.calls[0];
    expect(url).toBe('/groups');
    expect(body).toMatchObject({
      group_type: 'monitor', connect_host: '', port_range: '', rate: 1.0, hidden: false,
    });
  });

  it('leaves the stored forwarding fields alone when converting a group to monitor', async () => {
    // Wiping them would destroy what you need to convert the group back, and
    // flipping the type to look at something has to be a safe round trip.
    const user = userEvent.setup();
    mockPut.mockResolvedValue(ok(null));
    render(<Groups />);

    await user.click(await screen.findByRole('button', { name: /edit/i }));
    await user.click(await screen.findByLabelText('type'));
    await user.click(await screen.findByTitle('typeMonitor'));
    await user.click(screen.getByRole('button', { name: /^save$/i }));

    await waitFor(() => expect(mockPut).toHaveBeenCalled());
    const [, body] = mockPut.mock.calls[0];
    expect(body).toEqual({ group_type: 'monitor' });
    expect(body).not.toHaveProperty('connect_host');
    expect(body).not.toHaveProperty('port_range');
  });

  it('shows the type as a readable label, not the wire value', async () => {
    // The column rendered `group_type.toUpperCase()`, so an otherwise Chinese
    // page showed "IN" / "MONITOR". It now reuses the picker's own strings.
    mockGet.mockImplementation((url: string) => {
      if (url === '/groups') {
        return Promise.resolve(ok([group({ id: 1 }), group({ id: 4, name: 'm', group_type: 'monitor' })]));
      }
      return Promise.resolve(ok([]));
    });
    render(<Groups />);
    await waitFor(() => expect(screen.getByText('inboundListener')).toBeInTheDocument());
    expect(screen.getByText('typeMonitor')).toBeInTheDocument();
    expect(screen.queryByText('IN')).toBeNull();
    expect(screen.queryByText('MONITOR')).toBeNull();
  });

  it('falls back to the raw type when no label exists for it', async () => {
    // A legacy or newly added type must still read, rather than rendering an
    // empty tag.
    mockGet.mockImplementation((url: string) => {
      if (url === '/groups') return Promise.resolve(ok([group({ group_type: 'chained_outbound' })]));
      return Promise.resolve(ok([]));
    });
    render(<Groups />);
    await waitFor(() => expect(screen.getByText('chained_outbound')).toBeInTheDocument());
  });

  it('shows a dash instead of an inert port range on a monitor row', async () => {
    mockGet.mockImplementation((url: string) => {
      if (url === '/groups') {
        return Promise.resolve(ok([group({ group_type: 'monitor', port_range: '10000-65535', connect_host: '1.2.3.4' })]));
      }
      return Promise.resolve(ok([]));
    });
    render(<Groups />);
    await waitFor(() => expect(screen.getByText('g1')).toBeInTheDocument());
    expect(screen.queryByText('10000-65535')).toBeNull();
    expect(screen.queryByText('1.2.3.4')).toBeNull();
  });
});
