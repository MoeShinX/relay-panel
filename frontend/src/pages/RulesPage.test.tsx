import { describe, expect, it, vi, beforeEach, afterEach } from 'vitest';
import { fireEvent, render, screen, waitFor } from '@testing-library/react';
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

/** Route the independent list reads separately so each can fail alone. */
function routes({ rules, groups, users }: { rules?: unknown; groups?: unknown; users?: unknown }) {
  mockGet.mockImplementation((url: string) => {
    if (url.startsWith('/rules')) {
      return rules instanceof Error ? Promise.reject(rules) : Promise.resolve(rules);
    }
    if (url === '/groups') {
      return groups instanceof Error ? Promise.reject(groups) : Promise.resolve(groups);
    }
    if (url === '/admin/users' && users !== undefined) {
      return users instanceof Error ? Promise.reject(users) : Promise.resolve(users);
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

/** v1.3.3: open one row's "更多" dropdown. */
async function openMore(user: ReturnType<typeof userEvent.setup>, ruleName: string) {
  await user.click(screen.getByRole('button', { name: `ruleMoreActions ${ruleName}` }));
}

// ── v1.3.3: the row used to carry six inline buttons, which forced the table
// past a 1440px window; they fold into "编辑" + a "更多" dropdown now. The
// accessible names stay rule-qualified, so a screen reader still reads
// "删除 hk-web" and not an anonymous "删除" once the menu is open.
describe('row actions name their rule', () => {
  it('gives each action an accessible name containing the rule name', async () => {
    const user = userEvent.setup();
    renderPage();
    await waitFor(() => expect(screen.getByText('hk-web')).toBeInTheDocument());
    // Edit is the one visible action.
    expect(screen.getByRole('button', { name: 'edit hk-web' })).toBeInTheDocument();
    await openMore(user, 'hk-web');
    for (const action of ['copy', 'delete', 'diagnose', 'restart', 'pause']) {
      expect(
        await screen.findByLabelText(`${action} hk-web`),
        `${action} must be addressable on hk-web`,
      ).toBeInTheDocument();
    }
  });

  it('names the resume action on a paused rule', async () => {
    routes({ rules: ok([rule({ paused: true })]), groups: ok([group()]) });
    const user = userEvent.setup();
    renderPage();
    await waitFor(() => expect(screen.getByText('hk-web')).toBeInTheDocument());
    await openMore(user, 'hk-web');
    expect(await screen.findByLabelText('resume hk-web')).toBeInTheDocument();
  });

  // v1.2.14 → v1.3.3: the six inline buttons overflowed a fixed-width column;
  // what matters now is that the table does not insist on an old fixed width.
  it('does not pin the table to the old fixed widths', async () => {
    const { container } = renderPage();
    await waitFor(() => expect(screen.getByText('hk-web')).toBeInTheDocument());
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
    expect(exported).toEqual([{ dest: ['1.2.3.4:80'], listen_port: 10002, name: 'jp-game', protocol: 'tcp' }]);
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

describe('import', () => {
  it('creates each rule with the protocol from the file, defaulting to tcp_udp without one', async () => {
    mockPost.mockReset();
    mockPost.mockResolvedValue(ok(null));
    const user = userEvent.setup();
    renderPage();
    await waitFor(() => expect(screen.getByText('hk-web')).toBeInTheDocument());

    await user.hover(screen.getByRole('button', { name: /exportImport/ }));
    await user.click(await screen.findByText('import'));
    await user.click(await screen.findByLabelText('selectInboundGroup'));
    await user.click(await screen.findByTitle('hk-line (#7)'));
    fireEvent.change(document.querySelector('textarea') as HTMLTextAreaElement, {
      target: {
        value: JSON.stringify([
          { name: 'u', listen_port: 31001, dest: ['1.1.1.1:53'], protocol: 'udp' },
          { name: 't', listen_port: 31002, dest: ['1.1.1.1:80'], protocol: 'tcp' },
          { name: 'old', listen_port: 31003, dest: ['1.1.1.1:81'] },
        ]),
      },
    });
    await user.click(screen.getByRole('button', { name: /^import$/ }));

    await waitFor(() => expect(mockPost).toHaveBeenCalledTimes(3));
    const sent = Object.fromEntries(mockPost.mock.calls.map((c) => [c[1].name, c[1].protocol]));
    expect(sent).toEqual({ u: 'udp', t: 'tcp', old: 'tcp_udp' });
  });
});

describe('editing a rule', () => {
  it('saves a changed port without ever opening the 转发 tab', async () => {
    mockPut.mockReset();
    mockPut.mockResolvedValue(ok(null));
    const user = userEvent.setup();
    renderPage();
    await waitFor(() => expect(screen.getByText('hk-web')).toBeInTheDocument());
    await user.click(screen.getByRole('button', { name: 'edit hk-web' }));

    const port = await screen.findByLabelText('listenPort');
    await user.clear(port);
    await user.type(port, '10555');
    await user.click(screen.getByRole('button', { name: /^save$/ }));

    await waitFor(() => expect(mockPut).toHaveBeenCalledTimes(1));
    expect(mockPut).toHaveBeenCalledWith('/rules/1', { listen_port: 10555 });
  });
});

describe('a save the form refuses', () => {
  // A rule whose scheduled restart is below the floor (the API stores no such
  // value today, but older rows may carry one): the field is on the 转发 tab.
  const oddRule = () => rule({ auto_restart_minutes: 3 });

  it('brings the tab with the problem to the front and says what it is, instead of doing nothing', async () => {
    mockPut.mockReset();
    routes({ rules: ok([oddRule()]), groups: ok([group()]) });
    const user = userEvent.setup();
    renderPage();
    await waitFor(() => expect(screen.getByText('hk-web')).toBeInTheDocument());
    await user.click(screen.getByRole('button', { name: 'edit hk-web' }));

    // Only the port is touched, on the 基础 tab, and that is where the user is.
    expect(await screen.findByRole('tab', { name: 'tabBasic', selected: true })).toBeInTheDocument();
    const port = await screen.findByLabelText('listenPort');
    await user.clear(port);
    await user.type(port, '10555');
    await user.click(screen.getByRole('button', { name: /^save$/ }));

    // The 转发 tab comes forward, with the error showing, and nothing was sent.
    expect(await screen.findByRole('tab', { name: 'tabForward', selected: true })).toBeInTheDocument();
    expect(await screen.findAllByText(/autoRestartTooSmall/)).not.toHaveLength(0);
    expect(mockPut).not.toHaveBeenCalled();
  });

  it('opens on the 基础 tab again for the next rule', async () => {
    routes({ rules: ok([oddRule()]), groups: ok([group()]) });
    const user = userEvent.setup();
    renderPage();
    await waitFor(() => expect(screen.getByText('hk-web')).toBeInTheDocument());
    await user.click(screen.getByRole('button', { name: 'edit hk-web' }));
    await user.click(await screen.findByRole('tab', { name: 'tabForward' }));
    await user.click(screen.getByRole('button', { name: /^cancel$/ }));
    await user.click(screen.getByRole('button', { name: 'edit hk-web' }));
    expect(await screen.findByRole('tab', { name: 'tabBasic', selected: true })).toBeInTheDocument();
  });
});

describe('diagnosis names a node by its label', () => {
  const diagnose = (node_labels?: Record<string, string>) => ok({
    request_id: 'r', rule_id: 1, node_labels,
    nodes: [
      { status: 'control_channel_offline', node_id: 'n-a', group_name: 'hk-line', public_ip: '1.1.1.1' },
      { status: 'timeout', node_id: 'n-b', group_name: 'hk-line', public_ip: '2.2.2.2' },
    ],
  });

  it('puts the admin\'s name first, for the nodes that have one', async () => {
    mockPost.mockReset();
    mockPost.mockResolvedValue(diagnose({ 'n-a': '蓝希云 1号' }));
    const user = userEvent.setup();
    renderPage();
    await waitFor(() => expect(screen.getByText('hk-web')).toBeInTheDocument());
    await openMore(user, 'hk-web');
    await user.click(await screen.findByLabelText('diagnose hk-web'));

    expect(await screen.findByText('蓝希云 1号 · hk-line · 1.1.1.1')).toBeInTheDocument();
    expect(screen.getByText('hk-line · 2.2.2.2')).toBeInTheDocument();
  });

  it('is as before when the panel sent no names (an older panel, or the user setting is off)', async () => {
    mockPost.mockReset();
    mockPost.mockResolvedValue(diagnose(undefined));
    const user = userEvent.setup();
    renderPage();
    await waitFor(() => expect(screen.getByText('hk-web')).toBeInTheDocument());
    await openMore(user, 'hk-web');
    await user.click(await screen.findByLabelText('diagnose hk-web'));
    expect(await screen.findByText('hk-line · 1.1.1.1')).toBeInTheDocument();
  });
});

describe('listen IP column', () => {
  it('reads like targets: the first address, (+N), and the rest on hover', async () => {
    routes({
      rules: ok([rule({ device_group_in: 7 }), rule({ id: 2, name: 'solo', device_group_in: 8 })]),
      groups: ok([
        group({ connect_host: '69.165.73.45,2001:db8::1,1.2.3.4' }),
        group({ id: 8, name: 'one', connect_host: '5.6.7.8' }),
      ]),
    });
    const user = userEvent.setup();
    renderPage();
    const multi = await screen.findByText('69.165.73.45 (+2)');
    expect(screen.getByText('5.6.7.8')).toBeInTheDocument();
    await user.hover(multi);
    expect(await screen.findByText('2001:db8::1')).toBeInTheDocument();
    expect(screen.getByText('1.2.3.4')).toBeInTheDocument();
  });
});

describe('batch move to another group', () => {
  it('moves the selection, skips a port taken in the target group, and says so', async () => {
    mockPut.mockReset();
    mockPut.mockImplementation((url: string) =>
      Promise.resolve(url === '/rules/1' ? { code: 409, message: 'in use', data: null } : ok(null)),
    );
    routes({
      rules: ok([
        rule(),
        rule({ id: 2, name: 'jp-game', listen_port: 10002, device_group_in: 7 }),
      ]),
      groups: ok([group(), group({ id: 8, name: 'jp-line' })]),
    });
    const user = userEvent.setup();
    const { container } = renderPage();
    await waitFor(() => expect(screen.getByText('hk-web')).toBeInTheDocument());

    const boxes = container.querySelectorAll('tr.ant-table-row input[type="checkbox"]');
    await user.click(boxes[0] as HTMLElement);
    await user.click(boxes[1] as HTMLElement);
    await user.click(screen.getByRole('button', { name: /batchMove \(2\)/ }));
    await user.click(await screen.findByLabelText('batchMoveTarget'));
    await user.click(await screen.findByTitle('jp-line (#8)'));
    await user.click(screen.getByRole('button', { name: /batchMoveRun/ }));

    await waitFor(() => expect(mockPut).toHaveBeenCalledTimes(2));
    expect(mockPut).toHaveBeenCalledWith('/rules/1', { device_group_in: 8 });
    expect(mockPut).toHaveBeenCalledWith('/rules/2', { device_group_in: 8 });
    // rule 1 was refused for its port: it is named in the result, rule 2 is counted as moved.
    const dialog = await screen.findByText(/batchMoveResultTitle/);
    expect(dialog).toBeInTheDocument();
    expect(screen.getByText(/#1 hk-web/)).toBeInTheDocument();
    expect(screen.getByText(/batchMoveDone/)).toBeInTheDocument();
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

// ── v1.3.3: the name column. It used to have no width and was squeezed to
// about 48px on a 1440px window, so a long Chinese name wrapped to one
// character per line; the rule's id occupied a whole column of its own.
describe('the name is the primary column and carries the id', () => {
  const longName = '香港-日本-新加坡-多区域混合专线转发规则-用于长名称换行验证-80';

  it('keeps a long name to two lines instead of one character per line', async () => {
    routes({ rules: ok([rule({ name: longName })]), groups: ok([group()]) });
    const { container } = renderPage();
    await waitFor(() => expect(screen.getByText(longName)).toBeInTheDocument());
    const cell = container.querySelector('.rp-rule-name-cell') as HTMLElement;
    expect(cell).not.toBeNull();
    // The clamp is CSS (jsdom cannot lay out), so assert the cell that carries
    // the rule: a single element holding the whole name, not a broken column.
    const nameEl = cell.querySelector('.rp-rule-name') as HTMLElement;
    expect(nameEl.textContent).toBe(longName);
    expect(nameEl.getAttribute('title')).toBe(longName);
  });

  it('gives the desktop name column an explicit width, pins it left, and uses max-content scroll so 1280/1440px windows do not squeeze or scroll it away', async () => {
    const { container } = renderPage();
    await waitFor(() => expect(screen.getByText('hk-web')).toBeInTheDocument());
    const cols = Array.from(container.querySelectorAll('.ant-table colgroup col')) as HTMLElement[];
    // col[0] is the row-selection checkbox; col[1] is the primary name column.
    const nameWidth = parseInt(cols[1]?.style.width || '', 10);
    expect(nameWidth).toBeGreaterThanOrEqual(180);
    expect((container.querySelector('.ant-table table') as HTMLElement).style.width).toBe('max-content');
    // Both the checkbox column and the name column are pinned to the left edge.
    const headerCells = Array.from(container.querySelectorAll('.ant-table-thead th'));
    expect(headerCells[0]?.className).toMatch(/ant-table-cell-fix-(left|start)/);
    expect(headerCells[1]?.className).toMatch(/ant-table-cell-fix-(left|start)/);
  });

  it('shows the rule id under the name instead of in its own column', async () => {
    routes({ rules: ok([rule({ id: 12, name: 'hk-web' })]), groups: ok([group()]) });
    const { container } = renderPage();
    await waitFor(() => expect(screen.getByText('hk-web')).toBeInTheDocument());
    const cell = container.querySelector('.rp-rule-name-cell') as HTMLElement;
    expect(cell.querySelector('.rp-rule-id')?.textContent).toBe('#12');
    // No column titled "ID" any more.
    const headers = Array.from(container.querySelectorAll('.ant-table-thead th')).map((th) => th.textContent?.trim());
    expect(headers).not.toContain('id');
  });
});

// ── v1.3.3: restart and delete moved into the "更多" dropdown, where a
// Popconfirm cannot live (antd closes the menu and unmounts the popup), so the
// confirmation is a Modal that names the rule.
describe('the named confirmations in the more menu', () => {
  it('names the rule in the restart question', async () => {
    const user = userEvent.setup();
    renderPage();
    await waitFor(() => expect(screen.getByText('hk-web')).toBeInTheDocument());
    await openMore(user, 'hk-web');
    await user.click(await screen.findByLabelText('restart hk-web'));
    expect((await screen.findAllByText('restartConfirmNamed')).length).toBeGreaterThan(0);
  });

  it('names the rule in the delete question', async () => {
    const user = userEvent.setup();
    renderPage();
    await waitFor(() => expect(screen.getByText('hk-web')).toBeInTheDocument());
    await openMore(user, 'hk-web');
    await user.click(await screen.findByLabelText('delete hk-web'));
    expect((await screen.findAllByText('deleteRuleConfirmNamed')).length).toBeGreaterThan(0);
  });

  it('disables diagnosis on a UDP rule inside the menu', async () => {
    routes({ rules: ok([rule({ protocol: 'udp', name: 'udp-rule' })]), groups: ok([group()]) });
    const user = userEvent.setup();
    renderPage();
    await waitFor(() => expect(screen.getByText('udp-rule')).toBeInTheDocument());
    await openMore(user, 'udp-rule');
    const item = await screen.findByLabelText('diagnose udp-rule');
    // antd marks a disabled menu item on its list item wrapper.
    expect(item.closest('.ant-dropdown-menu-item')?.classList.contains('ant-dropdown-menu-item-disabled')).toBe(true);
  });
});

// ── v1.3.3: the row checkbox used to be an unqualified "checkbox" on every row.
describe('the row checkbox says which rule it selects', () => {
  it('labels each row checkbox with the rule name', async () => {
    const { container } = renderPage();
    await waitFor(() => expect(screen.getByText('hk-web')).toBeInTheDocument());
    const boxes = Array.from(container.querySelectorAll('tr.ant-table-row input[type="checkbox"]'));
    const labels = boxes.map((b) => b.getAttribute('aria-label'));
    expect(labels).toContain('ruleSelectAria');
    expect(labels).toHaveLength(2);
  });
});

// ── v1.3.3: on a phone the table keeps four columns that fit without a
// sideways scroll: name (with id), status, listen port, and the "更多" menu.
describe('the phone layout', () => {
  const realMatchMedia = window.matchMedia;
  afterEach(() => { window.matchMedia = realMatchMedia; });

  it('shows name, status, port and the more menu without horizontal scroll', async () => {
    window.matchMedia = ((query: string) => ({
      matches: query.includes('max-width: 767px'),
      media: query,
      onchange: null,
      addListener: vi.fn(),
      removeListener: vi.fn(),
      addEventListener: vi.fn(),
      removeEventListener: vi.fn(),
      dispatchEvent: vi.fn(),
    })) as unknown as typeof window.matchMedia;

    routes({
      rules: ok([rule({ paused: true, proxy_protocol_accept: true })]),
      groups: ok([group()]),
    });
    const { container } = renderPage();
    await waitFor(() => expect(screen.getByText('hk-web')).toBeInTheDocument());

    const headers = Array.from(container.querySelectorAll('.ant-table-thead th'))
      .map((th) => th.textContent?.trim())
      .filter((text) => text && text !== '');
    expect(headers).toEqual(['name', 'status', 'listenPort', 'action']);

    // Status and actions are on screen: the paused tag renders, and the more
    // menu is a real button.
    expect(screen.getByText('paused')).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'ruleMoreActions hk-web' })).toBeInTheDocument();

    // The compact action cell holds only the trigger; edit lives in the menu.
    expect(screen.queryByRole('button', { name: 'edit hk-web' })).toBeNull();
  });

  it('expands a phone row on tap to show targets, group, listen IP, owner and traffic', async () => {
    window.matchMedia = ((query: string) => ({
      matches: query.includes('max-width: 767px'),
      media: query,
      onchange: null,
      addListener: vi.fn(),
      removeListener: vi.fn(),
      addEventListener: vi.fn(),
      removeEventListener: vi.fn(),
      dispatchEvent: vi.fn(),
    })) as unknown as typeof window.matchMedia;

    routes({
      rules: ok([rule({
        id: 12,
        name: 'hk-web',
        uid: 2,
        traffic_used: 2048,
        targets: [
          { host: '203.0.113.10', port: 443, enabled: true, position: 1, id: 1, rule_id: 12 },
          { host: '203.0.113.11', port: 443, enabled: true, position: 2, id: 2, rule_id: 12 },
        ],
      })]),
      groups: ok([group({ name: 'hk-line', connect_host: '198.51.100.45' })]),
      users: ok([{ id: 2, username: 'alice' }]),
    });

    const user = userEvent.setup();
    renderPage();
    await waitFor(() => expect(screen.getByText('hk-web')).toBeInTheDocument());

    // Collapsed initially: hidden fields are not rendered yet.
    expect(screen.queryByTestId('rule-mobile-detail-12')).toBeNull();

    // Tapping the name toggle expands the detail panel in place.
    const toggle = screen.getByRole('button', { name: 'ruleExpandDetails' });
    expect(toggle).toHaveAttribute('aria-expanded', 'false');
    await user.click(toggle);

    const detail = await screen.findByTestId('rule-mobile-detail-12');
    expect(detail).toHaveTextContent('203.0.113.10:443');
    expect(detail).toHaveTextContent('203.0.113.11:443');
    expect(detail).toHaveTextContent('hk-line');
    expect(detail).toHaveTextContent('198.51.100.45');
    expect(detail).toHaveTextContent('alice');
    expect(detail).toHaveTextContent('2.00 KB');

    // Tapping again collapses it (antd keeps the row mounted with display:none).
    await user.click(screen.getByRole('button', { name: 'ruleCollapseDetails' }));
    await waitFor(() => {
      expect(screen.getByRole('button', { name: 'ruleExpandDetails' })).toHaveAttribute('aria-expanded', 'false');
      expect(screen.getByTestId('rule-mobile-detail-12')).not.toBeVisible();
    });
  });
});
