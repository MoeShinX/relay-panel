import { describe, expect, it, vi, beforeEach } from 'vitest';
import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { MemoryRouter, useLocation } from 'react-router-dom';

const { mockGet } = vi.hoisted(() => ({ mockGet: vi.fn() }));
vi.mock('../api/client', () => ({
  default: { get: mockGet, post: vi.fn(), put: vi.fn(), delete: vi.fn() },
}));
const { mockUseAuth } = vi.hoisted(() => ({ mockUseAuth: vi.fn() }));
vi.mock('../auth/useAuth', () => ({ useAuth: mockUseAuth }));

import Rules from './Rules';

const ok = <T,>(data: T) => ({ code: 0, message: 'ok', data });

const rule = (over: Record<string, unknown> = {}) => ({
  id: 1, name: 'hk-web', uid: 1, listen_port: 10001, device_group_in: 7,
  target_addr: '1.2.3.4', target_port: 80, protocol: 'tcp', paused: false,
  targets: [{ host: '1.2.3.4', port: 80, enabled: true, position: 1, id: 1, rule_id: 1 }],
  ...over,
});
const group = (over: Record<string, unknown> = {}) => ({
  id: 7, name: 'hk-line', group_type: 'in', token: 'tok', uid: 1,
  connect_host: '1.2.3.4', port_range: '10000-65535', rate: 1.0, hidden: false,
  ...over,
});

/** Surfaces the current query string so URL state can be asserted directly. */
function UrlProbe() {
  const loc = useLocation();
  return <div data-testid="qs">{loc.search}</div>;
}

const renderPage = (initial = '/rules') =>
  render(
    <MemoryRouter initialEntries={[initial]}>
      <Rules />
      <UrlProbe />
    </MemoryRouter>,
  );

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

beforeEach(() => {
  mockGet.mockReset();
  mockUseAuth.mockReset();
  mockUseAuth.mockReturnValue({ isAdmin: true, user: { id: 1, username: 'admin' } });
  // Default: two rules on two different groups, both reads healthy.
  routes({
    rules: ok([rule(), rule({ id: 2, name: 'jp-game', listen_port: 10002, device_group_in: 8 })]),
    groups: ok([group(), group({ id: 8, name: 'jp-line' })]),
  });
});

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

// ── /rules and /groups are two independent reads. They were fetched with
// Promise.all, which rejects on the FIRST rejection and discards the sibling's
// value — so a failed /rules also threw away a good group list, leaving the
// create-rule form with an empty inbound picker for a problem that had nothing
// to do with groups.
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
    await waitFor(() => expect(screen.getByText('hk-web')).toBeInTheDocument());
  });

  it('reports no failure and shows both when each read succeeds', async () => {
    routes({ rules: ok([rule()]), groups: ok([group()]) });
    renderPage();
    await waitFor(() => expect(screen.getByText('hk-web')).toBeInTheDocument());
    expect(screen.queryByText('loadFailed')).toBeNull();
  });
});

// ── v1.2.7: the filters live in the URL. They were component state, so a
// refresh dropped them and the filtered view could not be linked to anyone.
describe('rule filters are URL state', () => {
  it('restores the search term from the query string on first render', async () => {
    renderPage('/rules?q=jp');
    await waitFor(() => expect(screen.getByText('jp-game')).toBeInTheDocument());
    expect(screen.queryByText('hk-web')).toBeNull();
  });

  it('restores the group filter from the query string', async () => {
    renderPage('/rules?group=8');
    await waitFor(() => expect(screen.getByText('jp-game')).toBeInTheDocument());
    expect(screen.queryByText('hk-web')).toBeNull();
  });

  it('writes the search term into the URL as it is typed', async () => {
    const user = userEvent.setup();
    renderPage();
    await waitFor(() => expect(screen.getByText('hk-web')).toBeInTheDocument());
    await user.type(screen.getByLabelText('searchRulePlaceholder'), 'jp');
    await waitFor(() => expect(screen.getByTestId('qs').textContent).toBe('?q=jp'));
  });

  it('drops the key instead of leaving an empty one behind', async () => {
    const user = userEvent.setup();
    renderPage('/rules?q=jp');
    await waitFor(() => expect(screen.getByText('jp-game')).toBeInTheDocument());
    await user.clear(screen.getByLabelText('searchRulePlaceholder'));
    await waitFor(() => expect(screen.getByTestId('qs').textContent).toBe(''));
  });

  it('keeps owner_uid when a filter changes', async () => {
    // owner_uid is how an admin arrives from the users page; losing it would
    // silently switch which account's rules are being managed.
    const user = userEvent.setup();
    renderPage('/rules?owner_uid=5');
    await waitFor(() => expect(screen.getByText('hk-web')).toBeInTheDocument());
    await user.type(screen.getByLabelText('searchRulePlaceholder'), 'jp');
    await waitFor(() => {
      const qs = new URLSearchParams(screen.getByTestId('qs').textContent || '');
      expect(qs.get('owner_uid')).toBe('5');
      expect(qs.get('q')).toBe('jp');
    });
  });
});

// ── Every row's action buttons carry the same visible word ("编辑"), so the
// accessible name has to say which rule, or a screen reader announces an
// undifferentiated list of them — including "delete".
describe('row actions name their rule', () => {
  it('gives each action an accessible name containing the rule name', async () => {
    renderPage();
    await waitFor(() => expect(screen.getByText('hk-web')).toBeInTheDocument());
    for (const action of ['edit', 'copy', 'delete', 'diagnose', 'restart', 'pause']) {
      expect(
        screen.getByRole('button', { name: `${action} hk-web` }),
        `${action} must be addressable on hk-web`,
      ).toBeInTheDocument();
      expect(screen.getByRole('button', { name: `${action} jp-game` })).toBeInTheDocument();
    }
  });

  it('names the resume action on a paused rule', async () => {
    routes({ rules: ok([rule({ paused: true })]), groups: ok([group()]) });
    renderPage();
    await waitFor(() => expect(screen.getByText('hk-web')).toBeInTheDocument());
    expect(screen.getByRole('button', { name: 'resume hk-web' })).toBeInTheDocument();
  });

  it('labels the two filter controls', async () => {
    renderPage();
    await waitFor(() => expect(screen.getByText('hk-web')).toBeInTheDocument());
    expect(screen.getByLabelText('searchRulePlaceholder')).toBeInTheDocument();
    expect(screen.getByLabelText('filterByGroup')).toBeInTheDocument();
  });
});
