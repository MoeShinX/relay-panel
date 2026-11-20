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

import NodeLatency from './NodeLatency';
import type { NodeLatencyResponse } from '../api/types';

const ok = <T,>(data: T) => ({ code: 0, message: 'ok', data });

const NOW = 1_700_000_000;

function point(ts: number, avg: number | null, loss = 0) {
  return { ts, avg, min: avg == null ? null : avg - 1, max: avg == null ? null : avg + 4, loss };
}

function response(over: Partial<NodeLatencyResponse> = {}): NodeLatencyResponse {
  return {
    node_id: 'n1',
    group_id: 7,
    group_name: 'edge-group',
    node_version: '1.2.12',
    supported: true,
    interval_secs: 10,
    range: '1h',
    granularity: '1m',
    bucket_secs: 60,
    since: NOW - 3600,
    until: NOW,
    series: [
      { target_id: 1, name: 'Beijing Telecom', carrier: 'telecom', kind: 'tcp', points: [point(NOW - 120, 20), point(NOW - 60, 30, 10)] },
      { target_id: 2, name: 'Shanghai Unicom', carrier: 'unicom', kind: 'icmp', points: [point(NOW - 120, 250), point(NOW - 60, null, 100)] },
      { target_id: 3, name: 'Guangzhou Mobile', carrier: 'mobile', kind: 'icmp', points: [] },
    ],
    summary: [
      { target_id: 1, latest: 30, latest_ts: NOW - 60, latest_loss: 10, avg: 25, loss: 5 },
      { target_id: 2, latest: null, latest_ts: NOW - 60, latest_loss: 100, avg: 250, loss: 50 },
      { target_id: 3, latest: null, latest_ts: null, latest_loss: null, avg: null, loss: null },
    ],
    unsupported_targets: [3],
    ...over,
  };
}

function Where() {
  return <div data-testid="where">{useLocation().pathname}</div>;
}

function renderPage() {
  return render(
    <MemoryRouter initialEntries={['/nodes/n1/latency']}>
      <Routes>
        <Route path="/nodes/:nodeId/latency" element={<NodeLatency />} />
        <Route path="*" element={<Where />} />
      </Routes>
    </MemoryRouter>,
  );
}

const flush = (ms = 0) => act(async () => { await vi.advanceTimersByTimeAsync(ms); });

function serve(latency: (url: string) => unknown) {
  mockGet.mockImplementation((url: string) => {
    if (url === '/nodes') {
      return Promise.resolve(ok([{ group_id: 1, group_name: 'edge-group', node_id: 'n1', public_ipv4: '192.0.2.20' }]));
    }
    if (url.startsWith('/admin/nodes/n1/latency')) return Promise.resolve(latency(url));
    return Promise.reject(new Error(`unexpected ${url}`));
  });
}

const latencyCalls = () => mockGet.mock.calls.map((c) => c[0] as string).filter((u) => u.includes('/latency'));

beforeEach(() => {
  mockGet.mockReset();
  vi.useFakeTimers();
});
afterEach(() => {
  vi.runOnlyPendingTimers();
  vi.useRealTimers();
});

describe('NodeLatency empty and gated states', () => {
  it('with no targets in its group it says so and links to the device groups page', async () => {
    serve(() => ok(response({ series: [], summary: [], unsupported_targets: [] })));
    renderPage();
    await flush();
    expect(screen.getByText('latencyNoTargets')).toBeInTheDocument();
    expect(screen.queryByTestId('chart')).toBeNull();
    fireEvent.click(screen.getByText('latencyGoGroups'));
    await flush();
    expect(screen.getByTestId('where')).toHaveTextContent('/groups');
  });

  it('names the node\'s device group and links to that group\'s comparison', async () => {
    serve(() => ok(response()));
    renderPage();
    await flush();
    const link = screen.getByTestId('latency-group-link');
    expect(link).toHaveTextContent('edge-group');
    fireEvent.click(link);
    await flush();
    expect(screen.getByTestId('where')).toHaveTextContent('/groups/7/latency');
  });

  it('a node older than node-v1.2.12 is told to upgrade, not shown an empty chart', async () => {
    serve(() => ok(response({ supported: false, node_version: '1.2.11', series: [], summary: [] })));
    renderPage();
    await flush();
    expect(screen.getByText('latencyNodeTooOld')).toBeInTheDocument();
    expect(screen.getByText(/1\.2\.11/)).toBeInTheDocument();
    expect(screen.queryByTestId('chart')).toBeNull();
  });

  it('an unknown node (404) and a failed load each get their own message', async () => {
    serve(() => ({ code: 404, message: 'x', data: null }));
    const { unmount } = renderPage();
    await flush();
    expect(screen.getByText('latencyNodeNotFound')).toBeInTheDocument();
    unmount();

    serve(() => ({ code: 500, message: 'x', data: null }));
    renderPage();
    await flush();
    expect(screen.getByText('latencyLoadFailed')).toBeInTheDocument();
  });

  it('shows the node name and address in the header', async () => {
    serve(() => ok(response()));
    renderPage();
    await flush();
    expect(screen.getByTestId('latency-title')).toHaveTextContent('edge-group');
    expect(screen.getByText('192.0.2.20')).toBeInTheDocument();
  });
});

describe('NodeLatency monitoring switched off', () => {
  it('shows an info alert above the stored data when the flag is set', async () => {
    serve(() => ok(response({ monitoring_off: true })));
    renderPage();
    await flush();
    expect(screen.getByTestId('latency-monitoring-off')).toHaveTextContent('latencyMonitoringOff');
    // the stored history is still drawn
    expect(screen.getByTestId('latency-cards')).toBeInTheDocument();
    expect(screen.getByTestId('chart')).toBeInTheDocument();
  });

  it('shows no alert when the flag is false or absent', async () => {
    serve(() => ok(response({ monitoring_off: false })));
    const { unmount } = renderPage();
    await flush();
    expect(screen.getByTestId('latency-cards')).toBeInTheDocument();
    expect(screen.queryByTestId('latency-monitoring-off')).toBeNull();
    unmount();

    serve(() => ok(response()));
    renderPage();
    await flush();
    expect(screen.getByTestId('latency-cards')).toBeInTheDocument();
    expect(screen.queryByTestId('latency-monitoring-off')).toBeNull();
  });
});

describe('NodeLatency cards', () => {
  it('draws a card per target, coloured by latency and loss, with unsupported ones marked', async () => {
    serve(() => ok(response()));
    renderPage();
    await flush();
    expect(screen.getByTestId('latency-card-1')).toBeInTheDocument();
    // 30 ms but 10% loss in the newest minute: bad, whatever the latency says
    expect(screen.getByTestId('latency-card-value-1')).toHaveAttribute('data-level', 'bad');
    // the newest minute lost everything
    expect(screen.getByTestId('latency-card-value-2')).toHaveTextContent('latencyAllLost');
    expect(screen.getByTestId('latency-card-value-2')).toHaveAttribute('data-level', 'bad');
    // not "0 ms / good": this node cannot do ICMP at all
    expect(screen.getByTestId('latency-card-value-3')).toHaveAttribute('data-level', 'none');
    expect(within(screen.getByTestId('latency-card-3')).getByText('latencyIcmpUnsupported')).toBeInTheDocument();
  });

  it('clicking a card shows only that target, clicking it again shows all', async () => {
    serve(() => ok(response()));
    renderPage();
    await flush();
    const chart = () => screen.getByTestId('chart');
    expect(screen.getByTestId('latency-current-view')).toHaveTextContent('latencyAllTargets');
    expect(chart().getAttribute('data-keys')).toBe('1,2');

    fireEvent.click(screen.getByTestId('latency-card-2'));
    expect(screen.getByTestId('latency-current-view')).toHaveTextContent('Shanghai Unicom');
    expect(chart().getAttribute('data-keys')).toBe('2');
    expect(screen.getByTestId('latency-card-2')).toHaveAttribute('aria-pressed', 'true');

    // another card switches, it does not add
    fireEvent.click(screen.getByTestId('latency-card-1'));
    expect(chart().getAttribute('data-keys')).toBe('1');

    fireEvent.click(screen.getByTestId('latency-card-1'));
    expect(screen.getByTestId('latency-current-view')).toHaveTextContent('latencyAllTargets');
    expect(chart().getAttribute('data-keys')).toBe('1,2');
  });
});

describe('NodeLatency range and granularity', () => {
  it('asks for the default range and granularity first', async () => {
    serve(() => ok(response()));
    renderPage();
    await flush();
    expect(latencyCalls()).toEqual(['/admin/nodes/n1/latency?range=24h&granularity=1h']);
  });

  it('changing the range asks again', async () => {
    serve(() => ok(response()));
    renderPage();
    await flush();
    fireEvent.click(screen.getByText('latencyRange6h'));
    await flush();
    expect(latencyCalls().at(-1)).toBe('/admin/nodes/n1/latency?range=6h&granularity=1h');
  });

  it('7 and 30 days leave only the hourly granularity (and the default) selectable', async () => {
    serve(() => ok(response()));
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

  it('shorter ranges allow every granularity', async () => {
    serve(() => ok(response()));
    renderPage();
    await flush();
    fireEvent.mouseDown(within(screen.getByTestId('latency-granularity')).getByRole('combobox'));
    await flush();
    for (const label of ['latencyGran1m', 'latencyGran5m', 'latencyGran1h', 'latencyGranDefault']) {
      const el = document.querySelector(`.ant-select-item-option[title="${label}"]`);
      expect(el, label).not.toBeNull();
      expect(el).not.toHaveClass('ant-select-item-option-disabled');
    }
  });

  it('picking a minute granularity and then a long range falls back to the default', async () => {
    serve(() => ok(response()));
    renderPage();
    await flush();
    fireEvent.mouseDown(within(screen.getByTestId('latency-granularity')).getByRole('combobox'));
    await flush();
    fireEvent.click(document.querySelector('.ant-select-item-option[title="latencyGran5m"]') as HTMLElement);
    await flush();
    expect(latencyCalls().at(-1)).toBe('/admin/nodes/n1/latency?range=24h&granularity=5m');

    fireEvent.click(screen.getByText('latencyRange30d'));
    await flush();
    // never the refused combination 30d + 5m
    expect(latencyCalls().at(-1)).toBe('/admin/nodes/n1/latency?range=30d&granularity=1h');
    expect(latencyCalls().some((u) => u.includes('range=30d&granularity=5m'))).toBe(false);
  });
});

describe('NodeLatency auto-refresh', () => {
  it('re-reads every minute for a range up to a day, and not for longer ones', async () => {
    serve(() => ok(response()));
    renderPage();
    await flush();
    expect(latencyCalls()).toHaveLength(1);
    await flush(60_000);
    expect(latencyCalls()).toHaveLength(2);
    await flush(60_000);
    expect(latencyCalls()).toHaveLength(3);

    fireEvent.click(screen.getByText('latencyRange7d'));
    await flush();
    const after = latencyCalls().length;
    await flush(180_000);
    expect(latencyCalls()).toHaveLength(after);
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

describe('NodeLatency status strips', () => {
  const stripLevel = (kind: 'lat' | 'loss', id: number, i: number) =>
    screen.getByTestId(`strip-${kind}-${id}-${i}`).getAttribute('data-level');

  it('draws a row per target with 30 latency and 30 loss slices', async () => {
    serve(() => ok(response()));
    renderPage();
    await flush();
    const strips = screen.getByTestId('latency-strips');
    expect(within(strips).getByTestId('strip-row-1')).toHaveTextContent('Beijing Telecom');
    expect(within(strips).getByTestId('strip-row-2')).toHaveTextContent('Shanghai Unicom');
    expect(within(strips).getByTestId('strip-row-3')).toHaveTextContent('Guangzhou Mobile');
    for (const id of [1, 2]) {
      expect(screen.getByTestId(`strip-lat-${id}`).children).toHaveLength(30);
      expect(screen.getByTestId(`strip-loss-${id}`).children).toHaveLength(30);
    }
  });

  it('uses the cards own now and range-loss figures', async () => {
    serve(() => ok(response()));
    renderPage();
    await flush();
    // 30 ms with 10% loss in the newest minute: bad, exactly as the card is
    expect(screen.getByTestId('strip-current-1')).toHaveTextContent('30.0');
    expect(screen.getByTestId('strip-current-1')).toHaveAttribute('data-level', 'bad');
    expect(screen.getByTestId('strip-loss-value-1')).toHaveTextContent('5.0%');
    expect(screen.getByTestId('strip-loss-value-1')).toHaveAttribute('data-level', 'bad');
    // the newest minute lost everything
    expect(screen.getByTestId('strip-current-2')).toHaveTextContent('latencyAllLost');
    expect(screen.getByTestId('strip-loss-value-2')).toHaveTextContent('50.0%');
  });

  it('colours slices by their own mean latency and loss; empty slices are grey', async () => {
    serve(() => ok(response()));
    renderPage();
    await flush();
    // target 1: 20 ms and 30 ms (10% lost) share the last slice -> 25 ms, 5% loss
    expect(stripLevel('lat', 1, 29)).toBe('good');
    expect(stripLevel('loss', 1, 29)).toBe('bad');
    expect(stripLevel('lat', 1, 0)).toBe('none');
    expect(stripLevel('loss', 1, 0)).toBe('none');
    // target 2: 250 ms and one all-lost point -> the answered one is the mean
    expect(stripLevel('lat', 2, 29)).toBe('bad');
    expect(screen.getByTestId('strip-lat-1-29').getAttribute('title')).toContain('25.0 ms');
  });

  it('a target this node cannot probe gets a note and no slices', async () => {
    serve(() => ok(response()));
    renderPage();
    await flush();
    expect(screen.getByTestId('strip-note-3')).toHaveTextContent('latencyIcmpUnsupported');
    expect(screen.queryByTestId('strip-lat-3')).toBeNull();
    expect(screen.queryByTestId('strip-loss-3')).toBeNull();
  });

  it('selecting a card narrows the chart but keeps every target strip', async () => {
    serve(() => ok(response()));
    renderPage();
    await flush();
    fireEvent.click(screen.getByTestId('latency-card-2'));
    expect(screen.getByTestId('chart').getAttribute('data-keys')).toBe('2');
    expect(screen.getByTestId('strip-row-1')).toBeInTheDocument();
    expect(screen.getByTestId('strip-row-2')).toBeInTheDocument();
    expect(screen.getByTestId('strip-row-3')).toBeInTheDocument();
  });
});


describe('NodeLatency series picker', () => {
  /**
   * Twelve targets T1..T12 (ids 1..12), ranked on the range's mean and loss:
   *   T4 lost everything                      -> worst
   *   bad:  T6 30 ms 10% loss, T3 500 ms, T11 300 ms
   *   warn: T5 20 ms 3% loss, T2 150 ms
   *   good: T10 60 ms, T12 40 ms, T1 20 ms, T9 10 ms
   *   nothing: T8 (no reading), then T7 (this node cannot probe it, whatever the
   *   summary says)
   * so the default ten are 4 6 3 11 5 2 10 12 1 9 and T7, T8 are left out.
   * T8 has points anyway, so a drawn T8 would show.
   */
  const TOP = [4, 6, 3, 11, 5, 2, 10, 12, 1, 9];
  const ALL = Array.from({ length: 12 }, (_, i) => i + 1);
  const inOrder = (xs: number[]) => ALL.filter((i) => xs.includes(i)).map(String);

  const SUMMARY: Record<number, { avg: number | null; loss: number | null }> = {
    1: { avg: 20, loss: 0 },
    2: { avg: 150, loss: 0 },
    3: { avg: 500, loss: 0 },
    4: { avg: null, loss: 100 },
    5: { avg: 20, loss: 3 },
    6: { avg: 30, loss: 10 },
    7: { avg: 999, loss: 50 },
    8: { avg: null, loss: null },
    9: { avg: 10, loss: 0 },
    10: { avg: 60, loss: 0 },
    11: { avg: 300, loss: 0 },
    12: { avg: 40, loss: 0 },
  };

  function many(count = 12): NodeLatencyResponse {
    const ids = ALL.slice(0, count);
    return response({
      series: ids.map((id) => ({
        target_id: id,
        name: `T${id}`,
        carrier: 'telecom',
        kind: 'tcp',
        points: id === 7 ? [] : [point(NOW - 120, 20), point(NOW - 60, 25)],
      })),
      summary: ids.map((id) => ({
        target_id: id,
        latest: 20,
        latest_ts: NOW - 60,
        latest_loss: 0,
        avg: SUMMARY[id].avg,
        loss: SUMMARY[id].loss,
      })),
      unsupported_targets: [7],
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

  it('ten targets or fewer: no picker and the colour domain is every target, as before', async () => {
    serve(() => ok(response()));
    renderPage();
    await flush();
    expect(screen.queryByTestId('series-picker')).toBeNull();
    expect(screen.getByTestId('chart').getAttribute('data-domain')).toBe('1,2,3');
  });

  it('exactly ten targets: still no picker', async () => {
    serve(() => ok(many(10)));
    renderPage();
    await flush();
    expect(screen.queryByTestId('series-picker')).toBeNull();
    expect(domain()).toEqual(inOrder(ALL.slice(0, 10)));
  });

  it('more than ten: the ten that need attention first are drawn', async () => {
    serve(() => ok(many()));
    renderPage();
    await flush();
    expect(picker()).toBeInTheDocument();
    expect(domain()).toEqual(inOrder(TOP));
    expect(drawn()).toEqual(inOrder(TOP));
    expect(drawn()).not.toContain('8');
    // the cards still list every target
    expect(screen.getByTestId('latency-card-8')).toBeInTheDocument();
    expect(screen.getByTestId('latency-card-7')).toBeInTheDocument();
  });

  it('the dropdown selects the ten, disables the others and says so', async () => {
    serve(() => ok(many()));
    renderPage();
    await flush();
    await openPicker();
    for (const id of ALL) {
      const on = TOP.includes(id);
      expect(isSelected(`T${id}`), `T${id} selected`).toBe(on);
      expect(isDisabled(`T${id}`), `T${id} disabled`).toBe(!on);
    }
    expect(screen.getByTestId('series-picker-hint')).toHaveTextContent('seriesPickMax');
    expect(screen.getByTestId('series-picker-auto')).toBeInTheDocument();
  });

  it('picking changes what is drawn; the colour domain is the picked lines in target order', async () => {
    serve(() => ok(many()));
    renderPage();
    await flush();
    await openPicker();
    await click(option('T9'));
    expect(isDisabled('T8')).toBe(false);
    await click(option('T8'));
    const expected = inOrder([...TOP.filter((i) => i !== 9), 8]);
    expect(drawn()).toEqual(expected);
    expect(domain()).toEqual(expected);
    expect(drawn()).toHaveLength(10);
    expect(isDisabled('T9')).toBe(true);
    expect(screen.queryByTestId('series-picker-auto')).toBeNull();
  });

  it('an explicit pick survives a range change and picking a card and then letting it go', async () => {
    serve(() => ok(many()));
    renderPage();
    await flush();
    await openPicker();
    await click(option('T9'));
    await click(option('T8'));
    const expected = inOrder([...TOP.filter((i) => i !== 9), 8]);

    fireEvent.click(screen.getByText('latencyRange7d'));
    await flush();
    expect(latencyCalls().at(-1)).toContain('range=7d');
    expect(drawn()).toEqual(expected);

    // a selected card shows that one target and the picker steps aside
    fireEvent.click(screen.getByTestId('latency-card-5'));
    await flush();
    expect(drawn()).toEqual(['5']);
    expect(screen.queryByTestId('series-picker')).toBeNull();

    fireEvent.click(screen.getByTestId('latency-card-5'));
    await flush();
    expect(drawn()).toEqual(expected);
    expect(picker()).toBeInTheDocument();
  });

  it('targets that disappear drop out of the pick', async () => {
    let count = 12;
    serve(() => ok(many(count)));
    renderPage();
    await flush();
    await openPicker();
    for (const id of TOP.filter((i) => i !== 12 && i !== 1)) await click(option(`T${id}`));
    expect(drawn()).toEqual(['1', '12']);

    // T12 is deleted: eleven are left, T1 stays
    count = 11;
    fireEvent.click(screen.getByText('latencyRange6h'));
    await flush();
    expect(drawn()).toEqual(['1']);
    expect(picker()).toBeInTheDocument();
  });

  it('a pick whose targets are all gone is automatic again', async () => {
    let count = 12;
    serve(() => ok(many(count)));
    renderPage();
    await flush();
    await openPicker();
    for (const id of TOP.filter((i) => i !== 12)) await click(option(`T${id}`));
    expect(domain()).toEqual(['12']);

    count = 11;
    fireEvent.click(screen.getByText('latencyRange6h'));
    await flush();
    // T1..T11 ranked: 4 6 3 11 5 2 10 1 9, then T8 (a reading of nothing) and
    // last T7 (cannot probe at all); ten of them
    const top = [4, 6, 3, 11, 5, 2, 10, 1, 9, 8];
    expect(domain()).toEqual(inOrder(top));
    expect(drawn()).toEqual(inOrder(top));
  });

  it('clearing the select returns to the automatic ten', async () => {
    serve(() => ok(many()));
    renderPage();
    await flush();
    await openPicker();
    await click(option('T9'));
    await click(option('T8'));
    expect(drawn()).toContain('8');

    const clear = picker().querySelector('.ant-select-clear') as HTMLElement;
    fireEvent.mouseDown(clear);
    await flush();
    expect(drawn()).toEqual(inOrder(TOP));
    await openPicker();
    expect(screen.getByTestId('series-picker-auto')).toBeInTheDocument();
  });
});
