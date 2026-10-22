import { describe, expect, it, vi, beforeEach } from 'vitest';
import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { MemoryRouter, useLocation } from 'react-router-dom';

const { mockGet, mockPost, mockPut } = vi.hoisted(() => ({ mockGet: vi.fn(), mockPost: vi.fn(), mockPut: vi.fn() }));
vi.mock('../api/client', () => ({
  default: { get: mockGet, post: mockPost, put: mockPut, delete: vi.fn() },
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

  // v1.2.14: six buttons on one line in a fixed 260px column overflowed it —
  // at 1440px the owner column was squeezed to a letter per line and delete
  // went off screen. The buttons fold now, and the table no longer insists
  // on 1200px.
  it('lets the six actions fold instead of widening the table', async () => {
    const { container } = renderPage();
    await waitFor(() => expect(screen.getByText('hk-web')).toBeInTheDocument());
    const space = screen.getByRole('button', { name: 'edit hk-web' }).closest('.ant-space') as HTMLElement;
    expect(space.style.flexWrap).toBe('wrap');
    // no column is pinned to the old 260px (col order shifts with responsive columns)
    const widths = Array.from(container.querySelectorAll('.ant-table colgroup col')).map((c) => (c as HTMLElement).style.width);
    expect(widths).not.toContain('260px');
    expect((container.querySelector('.ant-table table') as HTMLElement).style.width).not.toBe('1200px');
  });

  it('labels the two filter controls', async () => {
    renderPage();
    await waitFor(() => expect(screen.getByText('hk-web')).toBeInTheDocument());
    expect(screen.getByLabelText('searchRulePlaceholder')).toBeInTheDocument();
    expect(screen.getByLabelText('filterByGroup')).toBeInTheDocument();
  });
});

// ── v1.2.12: export one group's rules (one machine's, where each machine has
// its own group) — "export all" mixed every machine's targets into one file.
describe('exporting the filtered rules', () => {
  /** Capture what a download writes: its file name and text. */
  function captureDownload() {
    const saved: { name?: string; blob?: Blob } = {};
    const create = vi.fn((b: Blob) => { saved.blob = b; return 'blob:x'; });
    Object.assign(URL, { createObjectURL: create, revokeObjectURL: vi.fn() });
    const click = vi.spyOn(HTMLAnchorElement.prototype, 'click').mockImplementation(function (this: HTMLAnchorElement) {
      saved.name = this.download;
    });
    return { saved, restore: () => click.mockRestore() };
  }

  /** jsdom's Blob has no text(); FileReader reads it. */
  const readBlob = (b: Blob) =>
    new Promise<string>((resolve) => {
      const r = new FileReader();
      r.onload = () => resolve(String(r.result));
      r.readAsText(b);
    });

  it('exports only the selected group, named after it', async () => {
    const user = userEvent.setup();
    const { saved, restore } = captureDownload();
    renderPage('/rules?group=8');
    await waitFor(() => expect(screen.getByText('jp-game')).toBeInTheDocument());
    await user.hover(screen.getByRole('button', { name: /exportImport/ }));
    await user.click(await screen.findByText('exportFiltered'));

    await waitFor(() => expect(saved.name).toBeDefined());
    expect(saved.name).toMatch(/^relaypanel-rules-jp-line-\d{4}-\d{2}-\d{2}\.json$/);
    const exported = JSON.parse(await readBlob(saved.blob!));
    expect(exported).toEqual([{ dest: ['1.2.3.4:80'], listen_port: 10002, name: 'jp-game' }]);
    restore();
  });

  it('is not offered while nothing is filtered', async () => {
    const user = userEvent.setup();
    renderPage();
    await waitFor(() => expect(screen.getByText('hk-web')).toBeInTheDocument());
    await user.hover(screen.getByRole('button', { name: /exportImport/ }));
    expect(await screen.findByText('exportAll')).toBeInTheDocument();
    expect(screen.queryByText('exportFiltered')).toBeNull();
  });
});

// ── v1.2.15: user feedback — page size, sorting, batch traffic reset,
// host:port targets and bulk-added targets.
describe('rules table: page size and sorting', () => {
  const many = (n: number) =>
    Array.from({ length: n }, (_, i) => rule({ id: i + 1, name: `r${i + 1}`, listen_port: 10001 + i, traffic_used: (i % 7) * 1000 }));

  beforeEach(() => {
    try { localStorage.removeItem('relaypanel_rules_page_size'); } catch { /* jsdom */ }
  });

  it('shows the remembered page size instead of a fixed 20', async () => {
    localStorage.setItem('relaypanel_rules_page_size', '50');
    routes({ rules: ok(many(30)), groups: ok([group()]) });
    const { container } = renderPage();
    await waitFor(() => expect(screen.getByText('r1')).toBeInTheDocument());
    expect(container.querySelectorAll('tr.ant-table-row')).toHaveLength(30);
  });

  it('changes the page size from the size picker, up to 500, and remembers it', async () => {
    routes({ rules: ok(many(30)), groups: ok([group()]) });
    const user = userEvent.setup();
    const { container } = renderPage();
    await waitFor(() => expect(screen.getByText('r1')).toBeInTheDocument());
    expect(container.querySelectorAll('tr.ant-table-row')).toHaveLength(20);

    await user.click(container.querySelector('.ant-pagination-options .ant-select') as HTMLElement);
    expect(await screen.findByTitle(/500/)).toBeInTheDocument();
    await user.click(await screen.findByTitle(/100/));
    await waitFor(() => expect(container.querySelectorAll('tr.ant-table-row')).toHaveLength(30));
    expect(localStorage.getItem('relaypanel_rules_page_size')).toBe('100');
  });

  it('sorts by traffic across every page, not just the visible one', async () => {
    // r26 is on page 2 until sorted
    const rules = many(30).map((r) => (r.id === 26 ? { ...r, traffic_used: 9_000_000 } : r));
    routes({ rules: ok(rules), groups: ok([group()]) });
    const user = userEvent.setup();
    const { container } = renderPage();
    await waitFor(() => expect(screen.getByText('r1')).toBeInTheDocument());
    const trafficHeader = Array.from(container.querySelectorAll('.ant-table-thead th'))
      .find((th) => th.textContent?.trim() === 'traffic') as HTMLElement;
    await user.click(trafficHeader); // ascending
    await user.click(trafficHeader); // descending
    const firstRow = container.querySelector('tr.ant-table-row') as HTMLElement;
    expect(firstRow.textContent).toContain('r26');
  });
});

describe('batch traffic reset', () => {
  it('resets each selected rule once, then reloads the list', async () => {
    mockPost.mockReset();
    mockPost.mockResolvedValue(ok(null));
    const user = userEvent.setup();
    const { container } = renderPage();
    await waitFor(() => expect(screen.getByText('hk-web')).toBeInTheDocument());
    const getsBefore = mockGet.mock.calls.filter((c) => String(c[0]).startsWith('/rules')).length;

    const boxes = container.querySelectorAll('tr.ant-table-row input[type="checkbox"]');
    await user.click(boxes[0] as HTMLElement);
    await user.click(boxes[1] as HTMLElement);
    await user.click(screen.getByRole('button', { name: /batchResetTraffic/ }));
    await user.click(await screen.findByRole('button', { name: /OK|确/ }));

    await waitFor(() => expect(mockPost).toHaveBeenCalledTimes(2));
    expect(mockPost).toHaveBeenCalledWith('/rules/1/reset-traffic', {});
    expect(mockPost).toHaveBeenCalledWith('/rules/2/reset-traffic', {});
    await waitFor(() =>
      expect(mockGet.mock.calls.filter((c) => String(c[0]).startsWith('/rules')).length).toBeGreaterThan(getsBefore),
    );
  });
});

describe('target address entry', () => {
  const openCreate = async () => {
    const user = userEvent.setup();
    renderPage();
    await user.click(await screen.findByRole('button', { name: /addRule/ }));
    await user.click(await screen.findByRole('tab', { name: 'tabForward' }));
    return user;
  };
  const hostInputs = () => screen.getAllByPlaceholderText('targetAddressOrDest') as HTMLInputElement[];
  const portInputs = () => screen.getAllByPlaceholderText('targetPort') as HTMLInputElement[];

  it('splits address:port typed into the host box', async () => {
    const user = await openCreate();
    await user.type(hostInputs()[0], 'example.com:8443');
    await user.tab();
    await waitFor(() => expect(hostInputs()[0].value).toBe('example.com'));
    expect(portInputs()[0].value).toBe('8443');
  });

  it('leaves a bare IPv6 address alone', async () => {
    const user = await openCreate();
    await user.type(hostInputs()[0], '2001:db8::1');
    await user.tab();
    expect(hostInputs()[0].value).toBe('2001:db8::1');
    expect(portInputs()[0].value).toBe('');
  });

  it('adds many targets from one paste', async () => {
    const user = await openCreate();
    await user.click(screen.getByRole('button', { name: /bulkAddTargets/ }));
    const box = await screen.findByLabelText('bulkAddTargets');
    await user.click(box);
    await user.paste('1.2.3.4:443\nexample.com:8443\n[2001:db8::1]:53');
    await user.click(screen.getByRole('button', { name: 'add' }));
    await waitFor(() => expect(hostInputs()).toHaveLength(3));
    expect(hostInputs().map((i) => i.value)).toEqual(['1.2.3.4', 'example.com', '2001:db8::1']);
    expect(portInputs().map((i) => i.value)).toEqual(['443', '8443', '53']);
  });
});

// ── v1.2.15: PROXY protocol on a rule ──
describe('PROXY protocol', () => {
  const openEdit = async (name: string) => {
    const user = userEvent.setup();
    renderPage();
    await user.click(await screen.findByRole('button', { name: `edit ${name}` }));
    await user.click(await screen.findByRole('tab', { name: 'tabForward' }));
    return user;
  };

  it('tags the rules that send or accept it', async () => {
    routes({
      rules: ok([rule({ proxy_protocol_send: 'v2', proxy_protocol_accept: true }), rule({ id: 2, name: 'plain', listen_port: 10002 })]),
      groups: ok([group()]),
    });
    renderPage();
    await waitFor(() => expect(screen.getByText('hk-web')).toBeInTheDocument());
    expect(screen.getAllByText('ppTagSend')).toHaveLength(1);
    expect(screen.getAllByText('ppTagAccept')).toHaveLength(1);
  });

  it('sends only what changed when it is turned on in an edit', async () => {
    mockPut.mockReset();
    mockPut.mockResolvedValue(ok(null));
    const user = await openEdit('hk-web');
    await user.click(screen.getByLabelText('ppSend'));
    await user.click(await screen.findByTitle('ppV1'));
    await user.click(screen.getByRole('button', { name: /save/ }));
    await waitFor(() => expect(mockPut).toHaveBeenCalled());
    expect(mockPut).toHaveBeenCalledWith('/rules/1', { proxy_protocol_send: 'v1' });
  });

  it('asks for trusted sources only once accepting is on', async () => {
    const user = await openEdit('hk-web');
    expect(screen.queryByPlaceholderText('203.0.113.7, 10.0.0.0/8')).toBeNull();
    await user.click(screen.getByRole('switch', { name: 'ppAccept' }));
    expect(await screen.findByPlaceholderText('203.0.113.7, 10.0.0.0/8')).toBeInTheDocument();
  });

  it('is disabled on a UDP rule, and says why', async () => {
    routes({ rules: ok([rule({ protocol: 'udp' })]), groups: ok([group()]) });
    await openEdit('hk-web');
    expect(screen.getByRole('switch', { name: 'ppAccept' })).toBeDisabled();
    expect(screen.getByText('ppUdpUnsupported')).toBeInTheDocument();
  });
});
