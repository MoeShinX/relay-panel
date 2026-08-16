import { describe, expect, it, vi, beforeEach } from 'vitest';
import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { MemoryRouter } from 'react-router-dom';

const { mockGet } = vi.hoisted(() => ({ mockGet: vi.fn() }));
vi.mock('../api/client', () => ({
  default: { get: mockGet, post: vi.fn(), put: vi.fn(), delete: vi.fn() },
}));
const { mockUseAuth } = vi.hoisted(() => ({ mockUseAuth: vi.fn() }));
vi.mock('../auth/useAuth', () => ({ useAuth: mockUseAuth }));

import Rules from './Rules';

const ok = <T,>(data: T) => ({ code: 0, message: 'ok', data });

const rule = (over: Record<string, unknown> = {}) => ({
  id: 1, name: 'r1', uid: 1, listen_port: 10001, device_group_in: 7,
  target_addr: '1.2.3.4', target_port: 80, protocol: 'tcp', paused: false,
  targets: [{ host: '1.2.3.4', port: 80, enabled: true, position: 1, id: 1, rule_id: 1 }],
  ...over,
});
const group = (over: Record<string, unknown> = {}) => ({
  id: 7, name: 'hk-line', group_type: 'in', token: 'tok', uid: 1,
  connect_host: '1.2.3.4', port_range: '10000-65535', rate: 1.0, hidden: false,
  ...over,
});

const renderPage = () => render(<MemoryRouter><Rules /></MemoryRouter>);

beforeEach(() => {
  mockGet.mockReset();
  mockUseAuth.mockReset();
  mockUseAuth.mockReturnValue({ isAdmin: true, user: { id: 1, username: 'admin' } });
});

/** Route the two independent list reads separately so each can fail alone. */
function routes({ rules, groups }: { rules?: unknown; groups?: unknown }) {
  mockGet.mockImplementation((url: string) => {
    if (url.startsWith('/rules')) {
      return rules instanceof Error ? Promise.reject(rules) : Promise.resolve(rules);
    }
    if (url === '/groups') {
      return groups instanceof Error ? Promise.reject(groups) : Promise.resolve(groups);
    }
    return Promise.resolve(ok([]));
  });
}

// ── /rules and /groups are two independent reads. They were fetched with
// Promise.all, which rejects on the FIRST rejection and discards the sibling's
// value — so a failed /rules also threw away a good group list, leaving the
// create-rule form with an empty inbound picker for a problem that had nothing
// to do with groups.
/**
 * Assert the loaded groups reached the create-rule form's inbound picker.
 *
 * This is the consequence that matters: the picker is the only place `groups`
 * surfaces when the rules table is empty, and an admin who cannot choose an
 * inbound group cannot create a rule at all.
 */
async function inboundPickerOffers(name: string) {
  const user = userEvent.setup();
  await user.click(await screen.findByRole('button', { name: /addRule/ }));
  const picker = await screen.findByLabelText('inboundGroup');
  await user.click(picker);
  await waitFor(() => expect(screen.getByTitle(name)).toBeInTheDocument());
}

describe('a failed list read does not discard the other list', () => {
  it('keeps the groups when only /rules fails', async () => {
    routes({
      rules: { code: 500, message: '数据库错误', data: null },
      groups: ok([group()]),
    });
    renderPage();
    // The failure is reported...
    await waitFor(() => expect(screen.getByText('loadFailed')).toBeInTheDocument());
    // ...and the group list still made it through.
    await inboundPickerOffers('hk-line');
  });

  it('keeps the groups when the /rules request itself rejects', async () => {
    routes({ rules: new Error('network down'), groups: ok([group()]) });
    renderPage();
    await waitFor(() => expect(screen.getByText('loadFailed')).toBeInTheDocument());
    await inboundPickerOffers('hk-line');
  });

  it('keeps the rules when only /groups fails', async () => {
    routes({ rules: ok([rule()]), groups: { code: 500, message: '数据库错误', data: null } });
    renderPage();
    await waitFor(() => expect(screen.getByText('loadFailed')).toBeInTheDocument());
    await waitFor(() => expect(screen.getByText('r1')).toBeInTheDocument());
  });

  it('reports no failure and shows both when each read succeeds', async () => {
    routes({ rules: ok([rule()]), groups: ok([group()]) });
    renderPage();
    await waitFor(() => expect(screen.getByText('r1')).toBeInTheDocument());
    expect(screen.queryByText('loadFailed')).toBeNull();
  });
});
