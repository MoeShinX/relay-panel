import { describe, expect, it, vi, beforeEach, afterEach } from 'vitest';
import { act, fireEvent, render, screen, within } from '@testing-library/react';
import { MemoryRouter, Route, Routes, useLocation } from 'react-router-dom';

const { mockGet } = vi.hoisted(() => ({ mockGet: vi.fn() }));
vi.mock('../api/client', () => ({ default: { get: mockGet } }));
// The shared test setup stubs the chart as an empty component; here it also
// reports what it was asked to draw, so the page's choices can be asserted.
vi.mock('@ant-design/charts', () => ({
  Column: () => null,
  Line: (props: { data: { key: string }[] }) => (
    <div data-testid="chart" data-keys={[...new Set(props.data.map((d) => d.key))].join(',')} data-n={props.data.length} />
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
