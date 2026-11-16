import { describe, expect, it, vi, beforeEach, afterEach } from 'vitest';
import { act, fireEvent, render, screen, within } from '@testing-library/react';
import { MemoryRouter, Route, Routes, useLocation } from 'react-router-dom';

const { mockGet } = vi.hoisted(() => ({ mockGet: vi.fn() }));
vi.mock('../api/client', () => ({ default: { get: mockGet } }));
// The shared test setup stubs the chart as an empty component; here it also
// reports what it was asked to draw, so the page's choices can be asserted.
vi.mock('@ant-design/charts', () => ({
  Column: () => null,
  Line: (props: { data: { key: string }[]; scale?: { color?: { domain?: string[] } } }) => (
    <div
      data-testid="chart"
      data-keys={[...new Set(props.data.map((d) => d.key))].join(',')}
      data-domain={(props.scale?.color?.domain ?? []).join(',')}
      data-n={props.data.length}
    />
  ),
}));

import GroupLatency from './GroupLatency';
import type { GroupLatencyCell, GroupLatencyNode, GroupLatencyResponse, LatencyTarget } from '../api/types';

const ok = <T,>(data: T) => ({ code: 0, message: 'ok', data });

const NOW = 1_700_000_000;

const target = (id: number, name: string, over: Partial<LatencyTarget> = {}): LatencyTarget => ({
  id,
  group_id: 5,
  name,
  carrier: 'telecom',
  kind: 'tcp',
  host: `192.0.2.${id}`,
  port: 443,
  sort_order: id,
  created_at: '',
  ...over,
});

const node = (node_id: string, over: Partial<GroupLatencyNode> = {}): GroupLatencyNode => ({
  node_id,
  label: null,
  public_ip: null,
  node_version: '1.2.12',
  supported: true,
  unsupported_targets: [],
  last_seen: null,
  ...over,
});

const cell = (node_id: string, target_id: number, avg: number | null, loss: number): GroupLatencyCell => ({
  node_id,
  target_id,
  avg,
  loss,
  sent: 360,
});

const point = (ts: number, avg: number | null, loss = 0) => ({
  ts,
  avg,
  min: avg == null ? null : avg - 1,
  max: avg == null ? null : avg + 4,
  loss,
});

function response(over: Partial<GroupLatencyResponse> = {}): GroupLatencyResponse {
  return {
    group_id: 5,
    group_name: 'edge-group',
    interval_secs: 10,
    range: '1h',
    granularity: '1m',
    bucket_secs: 60,
    since: NOW - 3600,
    until: NOW,
    targets: [target(1, 'Beijing Telecom'), target(2, 'Shanghai Unicom', { carrier: 'unicom', kind: 'icmp', port: null })],
    nodes: [
      node('n1', { label: 'HK-1', public_ip: '192.0.2.10', unsupported_targets: [] }),
      node('n2', { public_ip: '192.0.2.11', unsupported_targets: [2] }),
      node('0123456789abcdef', { public_ip: '192.0.2.12' }),
      node('old', { node_version: '1.2.11', supported: false }),
    ],
    matrix: [
      cell('n1', 1, 20, 0),
      cell('n1', 2, 250, 0),
      cell('n2', 1, 30, 10),
      cell('0123456789abcdef', 2, null, 100),
    ],
    target_id: 1,
    series: [
      { node_id: 'n1', points: [point(NOW - 120, 20), point(NOW - 60, 22)] },
      { node_id: 'n2', points: [point(NOW - 120, 30, 10)] },
      { node_id: '0123456789abcdef', points: [] },
      { node_id: 'old', points: [] },
    ],
    ...over,
  };
}

function Where() {
  return <div data-testid="where">{useLocation().pathname}</div>;
}

function renderPage() {
  return render(
    <MemoryRouter initialEntries={['/groups/5/latency']}>
      <Routes>
        <Route path="/groups/:groupId/latency" element={<GroupLatency />} />
        <Route path="*" element={<Where />} />
      </Routes>
    </MemoryRouter>,
  );
}

const flush = (ms = 0) => act(async () => { await vi.advanceTimersByTimeAsync(ms); });

/** The answer follows the target the page asks for, as the server's does. */
function serve(latency: (url: string) => unknown = (url) => {
  const m = /target_id=(\d+)/.exec(url);
  return ok(response(m ? { target_id: Number(m[1]) } : {}));
}) {
  mockGet.mockImplementation((url: string) => {
    if (url.startsWith('/admin/groups/5/latency')) return Promise.resolve(latency(url));
    return Promise.reject(new Error(`unexpected ${url}`));
  });
}

const calls = () => mockGet.mock.calls.map((c) => c[0] as string);
const level = (nodeId: string, targetId: number) =>
  screen.getByTestId(`latency-cell-${nodeId}-${targetId}`).getAttribute('data-level');

beforeEach(() => {
  mockGet.mockReset();
  vi.useFakeTimers();
});
afterEach(() => {
  vi.runOnlyPendingTimers();
  vi.useRealTimers();
});

describe('GroupLatency states', () => {
  it('a group without targets says so and links to the device groups page', async () => {
    serve(() => ok(response({ targets: [], matrix: [], series: [], target_id: null })));
    renderPage();
    await flush();
    expect(screen.getByText('latencyGroupNoTargets')).toBeInTheDocument();
    expect(screen.queryByTestId('chart')).toBeNull();
    expect(screen.queryByTestId('latency-matrix')).toBeNull();
    fireEvent.click(screen.getByText('latencyGoGroups'));
    await flush();
    expect(screen.getByTestId('where')).toHaveTextContent('/groups');
  });

  it('an unknown group (404) and a failed load each get their own message', async () => {
    serve(() => ({ code: 404, message: 'x', data: null }));
    const { unmount } = renderPage();
    await flush();
    expect(screen.getByText('latencyGroupNotFound')).toBeInTheDocument();
    unmount();

    serve(() => ({ code: 500, message: 'x', data: null }));
    renderPage();
    await flush();
    expect(screen.getByText('latencyLoadFailed')).toBeInTheDocument();
  });

  it('shows the group name in the title', async () => {
    serve();
    renderPage();
    await flush();
    expect(screen.getByTestId('group-latency-title')).toHaveTextContent('edge-group');
  });
});

describe('GroupLatency comparison table', () => {
  it('asks for the default range and granularity first, with no target chosen', async () => {
    serve();
    renderPage();
    await flush();
    expect(calls()).toEqual(['/admin/groups/5/latency?range=24h&granularity=1h']);
  });

  it('lists a row per node (name or shortened id, with its address) and a column per target', async () => {
    serve();
    renderPage();
    await flush();
    expect(screen.getByTestId('latency-node-n1')).toHaveTextContent('HK-1');
    expect(screen.getByText('192.0.2.10')).toBeInTheDocument();
    // a node without a name shows its id, shortened when long
    expect(screen.getByTestId('latency-node-n2')).toHaveTextContent('n2');
    expect(screen.getByTestId('latency-node-0123456789abcdef')).toHaveTextContent('01234567…def');
    expect(screen.getByTestId('latency-col-1')).toHaveTextContent('Beijing Telecom');
    expect(screen.getByTestId('latency-col-2')).toHaveTextContent('Shanghai Unicom');
  });

  it('colours each cell by its average and loss, like the single-node cards', async () => {
    serve();
    renderPage();
    await flush();
    expect(level('n1', 1)).toBe('good');
    expect(screen.getByTestId('latency-cell-n1-1')).toHaveTextContent('20.0 ms');
    // 250 ms is poor whatever the loss
    expect(level('n1', 2)).toBe('bad');
    // 30 ms but 10% lost
    expect(level('n2', 1)).toBe('bad');
    // everything lost: no latency to show, and bad, not "no data"
    expect(level('0123456789abcdef', 2)).toBe('bad');
    expect(screen.getByTestId('latency-cell-0123456789abcdef-2')).toHaveTextContent('latencyAllLost');
  });

  it('marks what has no reading: no data, a target the node cannot probe, and an old node', async () => {
    serve();
    renderPage();
    await flush();
    // n2 cannot do ICMP, which target 2 is
    expect(screen.getByTestId('latency-cell-n2-2')).toHaveTextContent('latencyGroupUnsupported');
    expect(level('n2', 2)).toBe('none');
    // a pair the node simply has nothing stored for
    expect(screen.getByTestId('latency-cell-0123456789abcdef-1')).toHaveTextContent('latencyNoData');
    expect(level('0123456789abcdef', 1)).toBe('none');
    // a node older than node-v1.2.12 is asked to upgrade, and its cells are blank
    expect(screen.getByTestId('latency-upgrade-old')).toHaveTextContent('latencyGroupNeedUpgrade');
    expect(screen.queryByTestId('latency-upgrade-n1')).toBeNull();
    expect(screen.getByTestId('latency-cell-old-1')).toHaveTextContent('-');
  });

  it('clicking a node name opens that node\'s own page', async () => {
    serve();
    renderPage();
    await flush();
    fireEvent.click(screen.getByTestId('latency-node-n1'));
    await flush();
    expect(screen.getByTestId('where')).toHaveTextContent('/nodes/n1/latency');
  });
});

describe('GroupLatency chart', () => {
  it('draws a line per node for the first target, and says which target it is', async () => {
    serve();
    renderPage();
    await flush();
    expect(screen.getByTestId('chart').getAttribute('data-keys')).toBe('n1,n2');
    expect(screen.getByTestId('latency-current-view')).toHaveTextContent('Beijing Telecom');
    expect(screen.getByTestId('latency-col-1')).toHaveAttribute('aria-selected', 'true');
  });

  it('clicking a column header switches the chart to that target', async () => {
    serve();
    renderPage();
    await flush();
    fireEvent.click(screen.getByTestId('latency-col-2'));
    await flush();
    expect(calls().at(-1)).toBe('/admin/groups/5/latency?range=24h&granularity=1h&target_id=2');
    expect(screen.getByTestId('latency-current-view')).toHaveTextContent('Shanghai Unicom');
    expect(screen.getByTestId('latency-col-2')).toHaveAttribute('aria-selected', 'true');
    expect(screen.getByTestId('latency-col-1')).toHaveAttribute('aria-selected', 'false');
  });

  it('clicking a cell switches the chart to its target too', async () => {
    serve();
    renderPage();
    await flush();
    fireEvent.click(screen.getByTestId('latency-cell-n1-2'));
    await flush();
    expect(calls().at(-1)).toBe('/admin/groups/5/latency?range=24h&granularity=1h&target_id=2');
  });

  it('keeps the chosen target when the range changes', async () => {
    serve();
    renderPage();
    await flush();
    fireEvent.click(screen.getByTestId('latency-col-2'));
    await flush();
    fireEvent.click(screen.getByText('latencyRange6h'));
    await flush();
    expect(calls().at(-1)).toBe('/admin/groups/5/latency?range=6h&granularity=1h&target_id=2');
  });

  it('a chosen target that was deleted meanwhile falls back to the first', async () => {
    serve();
    renderPage();
    await flush();
    // the server no longer knows the target the click names
    serve((url) => (url.includes('target_id=') ? { code: 400, message: 'x', data: null } : ok(response())));
    fireEvent.click(screen.getByTestId('latency-col-2'));
    await flush();
    expect(calls().at(-1)).toBe('/admin/groups/5/latency?range=24h&granularity=1h');
    expect(screen.getByTestId('chart')).toBeInTheDocument();
  });

  it('with no data in the period the chart says so, the table stays', async () => {
    serve(() => ok(response({ series: response().series.map((s) => ({ ...s, points: [] })) })));
    renderPage();
    await flush();
    expect(screen.queryByTestId('chart')).toBeNull();
    expect(screen.getByText('latencyChartEmpty')).toBeInTheDocument();
    expect(screen.getByTestId('latency-matrix')).toBeInTheDocument();
  });
});

describe('GroupLatency range and refresh', () => {
  it('7 and 30 days leave only the hourly granularity (and the default) selectable', async () => {
    serve();
    renderPage();
    await flush();
    fireEvent.click(screen.getByText('latencyRange7d'));
    await flush();
    fireEvent.mouseDown(within(screen.getByTestId('latency-granularity')).getByRole('combobox'));
    await flush();
    const option = (label: string) =>
      document.querySelector(`.ant-select-item-option[title="${label}"]`) as HTMLElement | null;
    expect(option('latencyGran1m')).toHaveClass('ant-select-item-option-disabled');
    expect(option('latencyGran5m')).toHaveClass('ant-select-item-option-disabled');
    expect(option('latencyGran1h')).not.toHaveClass('ant-select-item-option-disabled');
    expect(option('latencyGranDefault')).not.toHaveClass('ant-select-item-option-disabled');
  });

  it('picking a minute granularity and then a long range falls back to the default', async () => {
    serve();
    renderPage();
    await flush();
    fireEvent.mouseDown(within(screen.getByTestId('latency-granularity')).getByRole('combobox'));
    await flush();
    fireEvent.click(document.querySelector('.ant-select-item-option[title="latencyGran5m"]') as HTMLElement);
    await flush();
    expect(calls().at(-1)).toBe('/admin/groups/5/latency?range=24h&granularity=5m');
    fireEvent.click(screen.getByText('latencyRange30d'));
    await flush();
    expect(calls().at(-1)).toBe('/admin/groups/5/latency?range=30d&granularity=1h');
    expect(calls().some((u) => u.includes('range=30d&granularity=5m'))).toBe(false);
  });

  it('re-reads every minute for a range up to a day, and not for longer ones', async () => {
    serve();
    renderPage();
    await flush();
    expect(calls()).toHaveLength(1);
    await flush(60_000);
    expect(calls()).toHaveLength(2);
    await flush(60_000);
    expect(calls()).toHaveLength(3);

    fireEvent.click(screen.getByText('latencyRange7d'));
    await flush();
    const after = calls().length;
    await flush(180_000);
    expect(calls()).toHaveLength(after);
  });

  it('a refresh that fails keeps the picture on screen', async () => {
    let n = 0;
    serve(() => {
      n += 1;
      return n === 1 ? ok(response()) : { code: 500, message: 'x', data: null };
    });
    renderPage();
    await flush();
    expect(screen.getByTestId('chart')).toBeInTheDocument();
    await flush(60_000);
    expect(screen.getByTestId('chart')).toBeInTheDocument();
    expect(screen.queryByText('latencyLoadFailed')).toBeNull();
  });
});

describe('GroupLatency status strips', () => {
  const stripLevel = (kind: 'lat' | 'loss', id: string, i: number) =>
    screen.getByTestId(`strip-${kind}-${id}-${i}`).getAttribute('data-level');

  it('draws a row per node, named like the table, with 30 slices for each of latency and loss', async () => {
    serve();
    renderPage();
    await flush();
    const strips = screen.getByTestId('latency-strips');
    expect(within(strips).getByTestId('strip-row-n1')).toHaveTextContent('HK-1');
    expect(within(strips).getByTestId('strip-row-n2')).toHaveTextContent('n2');
    expect(within(strips).getByTestId('strip-row-0123456789abcdef')).toHaveTextContent('01234567…def');
    for (const id of ['n1', 'n2', '0123456789abcdef']) {
      expect(screen.getByTestId(`strip-lat-${id}`).children).toHaveLength(30);
      expect(screen.getByTestId(`strip-loss-${id}`).children).toHaveLength(30);
    }
  });

  it('shows the newest reading and the range loss, and colours the slices that hold data', async () => {
    serve();
    renderPage();
    await flush();
    // newest point of n1 is 22 ms; the range loss is the table cell's (0%)
    expect(screen.getByTestId('strip-current-n1')).toHaveTextContent('22.0');
    expect(screen.getByTestId('strip-current-n1')).toHaveAttribute('data-level', 'good');
    expect(screen.getByTestId('strip-loss-value-n1')).toHaveTextContent('0%');
    expect(screen.getByTestId('strip-loss-value-n1')).toHaveAttribute('data-level', 'good');
    // n2: 30 ms but the table says 10% lost
    expect(screen.getByTestId('strip-loss-value-n2')).toHaveTextContent('10.0%');
    expect(screen.getByTestId('strip-loss-value-n2')).toHaveAttribute('data-level', 'bad');
    // both points sit in the last slice of the hour; the rest has no data
    expect(stripLevel('lat', 'n1', 29)).toBe('good');
    expect(stripLevel('lat', 'n1', 0)).toBe('none');
    expect(stripLevel('loss', 'n1', 29)).toBe('good');
    expect(stripLevel('loss', 'n2', 29)).toBe('bad');
    expect(stripLevel('loss', 'n2', 3)).toBe('none');
    // a node with no points: grey strip, no reading
    expect(stripLevel('lat', '0123456789abcdef', 29)).toBe('none');
    expect(screen.getByTestId('strip-current-0123456789abcdef')).toHaveTextContent('latencyNoData');
  });

  it('names the time span, average and loss of a slice on hover', async () => {
    serve();
    renderPage();
    await flush();
    const slice = screen.getByTestId('strip-lat-n1-29');
    // the default range is a day, so the span carries the date too
    expect(slice.getAttribute('title')).toMatch(/\d\d-\d\d \d\d:\d\d - \d\d-\d\d \d\d:\d\d/);
    expect(slice.getAttribute('title')).toContain('21.0 ms');
    expect(screen.getByTestId('strip-lat-n1-0').getAttribute('title')).toContain('latencyNoData');
  });

  it('a node that is too old gets a note and no slices', async () => {
    serve();
    renderPage();
    await flush();
    expect(screen.getByTestId('strip-note-old')).toHaveTextContent('latencyGroupNeedUpgrade');
    expect(screen.queryByTestId('strip-lat-old')).toBeNull();
    expect(screen.queryByTestId('strip-loss-old')).toBeNull();
  });

  it('follows the chosen target: rows and slices change when another column is picked', async () => {
    serve((url) => {
      const m = /target_id=(\d+)/.exec(url);
      const id = m ? Number(m[1]) : 1;
      if (id === 1) return ok(response());
      return ok(
        response({
          target_id: 2,
          series: [
            { node_id: 'n1', points: [point(NOW - 120, 250)] },
            { node_id: 'n2', points: [] },
            { node_id: '0123456789abcdef', points: [point(NOW - 60, null, 100)] },
            { node_id: 'old', points: [] },
          ],
        }),
      );
    });
    renderPage();
    await flush();
    expect(screen.getByTestId('strip-current-n1')).toHaveTextContent('22.0');

    fireEvent.click(screen.getByTestId('latency-col-2'));
    await flush();
    expect(screen.getByTestId('strip-current-n1')).toHaveTextContent('250');
    expect(screen.getByTestId('strip-current-n1')).toHaveAttribute('data-level', 'bad');
    expect(stripLevel('lat', 'n1', 29)).toBe('bad');
    // the node that cannot probe target 2 is told so
    expect(screen.getByTestId('strip-note-n2')).toHaveTextContent('latencyGroupUnsupported');
    expect(screen.queryByTestId('strip-lat-n2')).toBeNull();
    // everything lost: red, not grey
    expect(screen.getByTestId('strip-current-0123456789abcdef')).toHaveTextContent('latencyAllLost');
    expect(stripLevel('lat', '0123456789abcdef', 29)).toBe('bad');
    expect(stripLevel('loss', '0123456789abcdef', 29)).toBe('bad');
    expect(screen.getByTestId('strip-loss-value-0123456789abcdef')).toHaveTextContent('100.0%');
  });
});


describe('GroupLatency series picker', () => {
  /**
   * Thirteen nodes n0..n12. For target 1 (levels by latencyLevel):
   *   n3 lost everything                      -> worst
   *   bad:  n5 30 ms 10% loss, n2 500 ms, n11 300 ms
   *   warn: n4 20 ms 3% loss, n1 150 ms
   *   good: n10 60 ms, n12 40 ms, n0 20 ms, n9 10 ms
   *   no reading: n6 (no cell), n7 (too old), n8 (cannot probe target 1)
   * so the default ten are n3 n5 n2 n11 n4 n1 n10 n12 n0 n9, and n6 n7 n8 are
   * left out. (n6 has points anyway, so a drawn n6 would show.) For target 2
   * only n0 (900 ms) and n1 (400 ms) have cells: they lead, then everyone else
   * (no reading, a tie) in the table's order: n0..n9.
   */
  const ids = Array.from({ length: 13 }, (_, i) => `n${i}`);
  const TOP1 = ['n3', 'n5', 'n2', 'n11', 'n4', 'n1', 'n10', 'n12', 'n0', 'n9'];
  const inOrder = (keys: string[]) => ids.filter((k) => keys.includes(k));
  const TOP1_DRAWN = inOrder(TOP1);
  const TOP2 = ids.slice(0, 10);
  const matrix1 = [
    cell('n0', 1, 20, 0), cell('n1', 1, 150, 0), cell('n2', 1, 500, 0), cell('n3', 1, null, 100),
    cell('n4', 1, 20, 3), cell('n5', 1, 30, 10), cell('n9', 1, 10, 0), cell('n10', 1, 60, 0),
    cell('n11', 1, 300, 0), cell('n12', 1, 40, 0),
    cell('n0', 2, 900, 0), cell('n1', 2, 400, 0),
  ];

  const manyNodes = (count: number) =>
    ids.slice(0, count).map((id) => {
      if (id === 'n7') return node(id, { supported: false, node_version: '1.2.11' });
      if (id === 'n8') return node(id, { unsupported_targets: [1] });
      return node(id);
    });

  function serveMany(count = 13) {
    serve((url) => {
      const m = /target_id=(\d+)/.exec(url);
      const tid = m ? Number(m[1]) : 1;
      const nodes = manyNodes(count);
      return ok(
        response({
          nodes,
          matrix: matrix1.filter((c) => nodes.some((n) => n.node_id === c.node_id)),
          target_id: tid,
          series: nodes.map((n) => ({
            node_id: n.node_id,
            points: n.node_id === 'n7' || n.node_id === 'n8' ? [] : [point(NOW - 120, 20), point(NOW - 60, 25)],
          })),
        }),
      );
    });
  }

  const drawn = () => (screen.getByTestId('chart').getAttribute('data-keys') ?? '').split(',').filter(Boolean);
  const domain = () => (screen.getByTestId('chart').getAttribute('data-domain') ?? '').split(',').filter(Boolean);
  const picker = () => screen.getByTestId('series-picker');
  const option = (label: string) =>
    document.querySelector(`.ant-select-item-option[title="${label}"]`) as HTMLElement;
  const isDisabled = (label: string) => option(label).classList.contains('ant-select-item-option-disabled');
  const isSelected = (label: string) => option(label).classList.contains('ant-select-item-option-selected');
  const openPicker = async () => {
    fireEvent.mouseDown(within(picker()).getByRole('combobox'));
    await flush();
  };
  const click = async (el: HTMLElement) => {
    fireEvent.click(el);
    await flush();
  };

  it('ten nodes or fewer: no picker and the colour domain is every node, as before', async () => {
    serve();
    renderPage();
    await flush();
    expect(screen.queryByTestId('series-picker')).toBeNull();
    expect(screen.getByTestId('chart').getAttribute('data-domain')).toBe('n1,n2,0123456789abcdef,old');
  });

  it('exactly ten nodes: still no picker', async () => {
    serveMany(10);
    renderPage();
    await flush();
    expect(screen.queryByTestId('series-picker')).toBeNull();
    expect(domain()).toEqual(ids.slice(0, 10));
  });

  it('more than ten: the ten that need attention first are drawn, the rest are not', async () => {
    serveMany();
    renderPage();
    await flush();
    expect(picker()).toBeInTheDocument();
    // the closed control states drawn/total rather than squeezing in long names
    expect(picker()).toHaveTextContent('seriesPickCount');
    expect(domain()).toEqual(TOP1_DRAWN);
    expect(drawn()).toEqual(TOP1_DRAWN);
    expect(drawn()).not.toContain('n6');
    // the comparison table still lists every node
    expect(screen.getByTestId('latency-node-n6')).toBeInTheDocument();
    expect(screen.getByTestId('latency-node-n12')).toBeInTheDocument();
  });

  it('the default follows the chosen target', async () => {
    serveMany();
    renderPage();
    await flush();
    fireEvent.click(screen.getByTestId('latency-col-2'));
    await flush();
    expect(domain()).toEqual(TOP2);
    // n7 and n8 are in the ten but have nothing to draw
    expect(drawn()).toEqual(TOP2.filter((k) => k !== 'n7' && k !== 'n8'));
  });

  it('the dropdown selects the ten, disables the others and says so', async () => {
    serveMany();
    renderPage();
    await flush();
    await openPicker();
    for (const id of ids) {
      const on = TOP1.includes(id);
      expect(isSelected(id), `${id} selected`).toBe(on);
      expect(isDisabled(id), `${id} disabled`).toBe(!on);
    }
    expect(screen.getByTestId('series-picker-hint')).toHaveTextContent('seriesPickMax');
    expect(screen.getByTestId('series-picker-auto')).toBeInTheDocument();
  });

  it('picking changes what is drawn; the colour domain is the picked lines in node order', async () => {
    serveMany();
    renderPage();
    await flush();
    await openPicker();
    await click(option('n9'));
    expect(isDisabled('n6')).toBe(false);
    await click(option('n6'));
    const expected = inOrder([...TOP1.filter((k) => k !== 'n9'), 'n6']);
    expect(drawn()).toEqual(expected);
    expect(domain()).toEqual(expected);
    expect(drawn()).toHaveLength(10);
    expect(isDisabled('n9')).toBe(true);
    expect(screen.queryByTestId('series-picker-auto')).toBeNull();
  });

  it('an explicit pick survives a target change and a range change', async () => {
    serveMany();
    renderPage();
    await flush();
    await openPicker();
    await click(option('n9'));
    await click(option('n6'));
    const expected = inOrder([...TOP1.filter((k) => k !== 'n9'), 'n6']);

    fireEvent.click(screen.getByTestId('latency-col-2'));
    await flush();
    expect(calls().at(-1)).toContain('target_id=2');
    expect(drawn()).toEqual(expected);

    fireEvent.click(screen.getByText('latencyRange7d'));
    await flush();
    expect(calls().at(-1)).toContain('range=7d');
    expect(drawn()).toEqual(expected);
  });

  /** Serve a group whose size the test can change between loads. */
  function serveShrinking(size: { count: number }) {
    serve((url) => {
      const nodes = manyNodes(size.count);
      const m = /target_id=(\d+)/.exec(url);
      return ok(
        response({
          nodes,
          matrix: matrix1.filter((c) => nodes.some((n) => n.node_id === c.node_id)),
          target_id: m ? Number(m[1]) : 1,
          series: nodes.map((n) => ({ node_id: n.node_id, points: [point(NOW - 60, 20)] })),
        }),
      );
    });
  }

  it('nodes that left the group drop out of the pick', async () => {
    const size = { count: 13 };
    serveShrinking(size);
    renderPage();
    await flush();
    await openPicker();
    // keep only n12 and n0
    for (const id of TOP1.filter((k) => k !== 'n12' && k !== 'n0')) await click(option(id));
    expect(drawn()).toEqual(['n0', 'n12']);

    // the group shrinks to twelve nodes: n12 is gone, n0 stays
    size.count = 12;
    fireEvent.click(screen.getByTestId('latency-col-2'));
    await flush();
    expect(drawn()).toEqual(['n0']);
    expect(picker()).toBeInTheDocument();
  });

  it('a pick whose nodes are all gone is automatic again', async () => {
    const size = { count: 13 };
    serveShrinking(size);
    renderPage();
    await flush();
    await openPicker();
    for (const id of TOP1.filter((k) => k !== 'n12')) await click(option(id));
    expect(drawn()).toEqual(['n12']);

    size.count = 12;
    fireEvent.click(screen.getByTestId('latency-col-1'));
    await flush();
    // n0..n11 for target 1, worst first, ten of them: n6 (no cell) is the first
    // of the readingless and takes the last place
    expect(drawn()).toEqual(inOrder(['n3', 'n5', 'n2', 'n11', 'n4', 'n1', 'n10', 'n0', 'n9', 'n6']));
  });

  it('clearing the select returns to the automatic ten', async () => {
    serveMany();
    renderPage();
    await flush();
    await openPicker();
    await click(option('n9'));
    await click(option('n6'));
    expect(drawn()).toContain('n6');

    const clear = picker().querySelector('.ant-select-clear') as HTMLElement;
    fireEvent.mouseDown(clear);
    await flush();
    expect(drawn()).toEqual(TOP1_DRAWN);
    await openPicker();
    expect(screen.getByTestId('series-picker-auto')).toBeInTheDocument();
  });
});
