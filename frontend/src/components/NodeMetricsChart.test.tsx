import { describe, expect, it, vi, beforeEach } from 'vitest';
import { act, fireEvent, render, screen, waitFor, within } from '@testing-library/react';

const { mockGet } = vi.hoisted(() => ({ mockGet: vi.fn() }));
vi.mock('../api/client', () => ({ default: { get: mockGet } }));
// The shared test setup stubs the chart as an empty component; here it reports
// what it was asked to draw (the lines present in the data, and the colour
// domain), so the component's choices can be asserted.
vi.mock('@ant-design/charts', () => ({
  Column: () => null,
  Line: (props: { data: { node: string }[]; scale?: { color?: { domain?: string[] } } }) => (
    <div
      data-testid="chart"
      data-keys={[...new Set(props.data.map((d) => d.node))].join(',')}
      data-domain={(props.scale?.color?.domain ?? []).join(',')}
    />
  ),
}));

import NodeMetricsChart from './NodeMetricsChart';
import type { NodeMetricBucket, NodeMetricsResponse } from '../api/types';

const ok = <T,>(data: T) => ({ code: 0, message: 'ok', data });

/**
 * Node i is the group "g<i>". Means over the range, chosen so each metric
 * leaves out a different pair:
 *   cpu  = (i * 7) % 12   -> lowest are g0 (0) and g7 (1)
 *   mem  = 11 - i         -> lowest are g11 (0) and g10 (1)
 *   conn = i              -> lowest are g0 (0) and g1 (1)
 */
const cpuOf = (i: number) => (i * 7) % 12;
const memOf = (i: number) => 11 - i;
const connOf = (i: number) => i;

function bucketsFor(ids: number[]): NodeMetricBucket[] {
  const out: NodeMetricBucket[] = [];
  for (const hour of ['2026-11-15 08:00:00', '2026-11-15 09:00:00']) {
    for (const i of ids) {
      out.push({
        bucket: hour,
        node_id: `node-${i}`,
        group_id: i + 1,
        group_name: `g${i}`,
        cpu_avg: cpuOf(i),
        cpu_max: cpuOf(i) + 5,
        mem_avg: memOf(i),
        mem_max: memOf(i) + 5,
        conn_avg: connOf(i),
        conn_max: connOf(i) + 5,
      });
    }
  }
  return out;
}

const range = (from: number, to: number) => Array.from({ length: to - from }, (_, i) => from + i);
const names = (ids: number[]) => ids.map((i) => `g${i}`);
const without = (ids: number[], drop: number[]) => ids.filter((i) => !drop.includes(i));

/** `ids` per range, as the server would answer. */
function serve(byRange: Record<string, number[]>) {
  mockGet.mockImplementation((url: string) => {
    const m = /^\/stats\/node-metrics\?range=(\w+)$/.exec(url);
    if (!m) return Promise.reject(new Error(`unexpected ${url}`));
    const res: NodeMetricsResponse = { granularity: '1h', since: '', buckets: bucketsFor(byRange[m[1]] ?? []) };
    return Promise.resolve(ok(res));
  });
}

const flush = () => act(async () => { await Promise.resolve(); });

async function renderChart() {
  render(<NodeMetricsChart />);
  // the chart is there while loading, with nothing to draw yet
  await waitFor(() => expect(drawn().length).toBeGreaterThan(0));
}

const drawn = () => (screen.getByTestId('chart').getAttribute('data-keys') ?? '').split(',').filter(Boolean);
const domain = () => (screen.getByTestId('chart').getAttribute('data-domain') ?? '').split(',').filter(Boolean);
const picker = () => screen.getByTestId('series-picker');
const option = (label: string) =>
  document.querySelector(`.ant-select-item-option[title="${label}"]`) as HTMLElement;
const isDisabled = (label: string) => option(label).classList.contains('ant-select-item-option-disabled');
const isSelected = (label: string) => option(label).classList.contains('ant-select-item-option-selected');

async function openPicker() {
  fireEvent.mouseDown(within(picker()).getByRole('combobox'));
  await flush();
}
const click = async (el: HTMLElement) => {
  fireEvent.click(el);
  await flush();
};

beforeEach(() => {
  mockGet.mockReset();
});

describe('NodeMetricsChart series picker', () => {
  it('ten nodes or fewer: no picker, every line drawn, nothing else changed', async () => {
    serve({ '1d': range(0, 10) });
    await renderChart();
    expect(screen.queryByTestId('series-picker')).toBeNull();
    expect(drawn()).toEqual(names(range(0, 10)));
    // no colour domain is forced at this size
    expect(domain()).toEqual([]);
  });

  it('eleven nodes: the picker appears and the ten with the highest mean CPU are drawn', async () => {
    serve({ '1d': range(0, 11) });
    await renderChart();
    expect(picker()).toBeInTheDocument();
    // g0 has the lowest CPU of the eleven
    const expected = names(without(range(0, 11), [0]));
    expect(drawn()).toEqual(expected);
    expect(domain()).toEqual(expected);
  });

  it('twelve nodes: the default top ten follows the metric switch', async () => {
    serve({ '1d': range(0, 12) });
    await renderChart();
    const all = range(0, 12);
    expect(drawn()).toEqual(names(without(all, [0, 7])));

    fireEvent.click(screen.getByText('metricMem'));
    await flush();
    expect(drawn()).toEqual(names(without(all, [10, 11])));
    expect(domain()).toEqual(names(without(all, [10, 11])));

    fireEvent.click(screen.getByText('metricConn'));
    await flush();
    expect(drawn()).toEqual(names(without(all, [0, 1])));
  });

  it('the dropdown shows the ten as selected, disables the rest and says so', async () => {
    serve({ '1d': range(0, 12) });
    await renderChart();
    await openPicker();
    for (const i of range(0, 12)) {
      const picked = ![0, 7].includes(i);
      expect(isSelected(`g${i}`), `g${i} selected`).toBe(picked);
      expect(isDisabled(`g${i}`), `g${i} disabled`).toBe(!picked);
    }
    expect(screen.getByTestId('series-picker-hint')).toHaveTextContent('seriesPickMax');
    expect(screen.getByTestId('series-picker-auto')).toHaveTextContent('seriesPickAuto');
  });

  it('picking changes what is drawn, and the colours follow the chart order', async () => {
    serve({ '1d': range(0, 12) });
    await renderChart();
    await openPicker();
    // free a slot, then take g0
    await click(option('g11'));
    expect(isDisabled('g0')).toBe(false);
    await click(option('g0'));
    const expected = names(without(range(0, 12), [7, 11]));
    expect(drawn()).toEqual(expected);
    expect(drawn()).toHaveLength(10);
    // g0 is drawn first although it was picked last: the normal order, so the
    // ten lines get ten distinct colours of the palette
    expect(domain()).toEqual(expected);
    // full again: everything else is disabled
    expect(isDisabled('g7')).toBe(true);
    expect(isDisabled('g11')).toBe(true);
    // no longer the automatic choice
    expect(screen.queryByTestId('series-picker-auto')).toBeNull();
  });

  it('an explicit pick survives a metric change and a range change', async () => {
    serve({ '1d': range(0, 12), '7d': range(0, 12) });
    await renderChart();
    await openPicker();
    await click(option('g11'));
    await click(option('g0'));
    const expected = names(without(range(0, 12), [7, 11]));

    fireEvent.click(screen.getByText('metricMem'));
    await flush();
    expect(drawn()).toEqual(expected);

    fireEvent.click(screen.getByText('range7d'));
    await waitFor(() => expect(mockGet).toHaveBeenLastCalledWith('/stats/node-metrics?range=7d'));
    await flush();
    expect(drawn()).toEqual(expected);
    expect(picker()).toBeInTheDocument();
  });

  it('keys that no longer exist drop out of the pick; with none left it is automatic again', async () => {
    // The 7-day answer no longer has g0 and g1.
    serve({ '1d': range(0, 12), '7d': range(2, 14) });
    await renderChart();
    await openPicker();
    // narrow the pick down to g1 and g5 by clearing the other eight
    for (const i of [2, 3, 4, 6, 8, 9, 10, 11]) await click(option(`g${i}`));
    expect(drawn()).toEqual(['g1', 'g5']);

    fireEvent.click(screen.getByText('range7d'));
    await waitFor(() => expect(mockGet).toHaveBeenLastCalledWith('/stats/node-metrics?range=7d'));
    await flush();
    // g1 is gone from the data, g5 stays: still an explicit pick of one line
    await waitFor(() => expect(drawn()).toEqual(['g5']));
    expect(picker()).toBeInTheDocument();
  });

  it('a pick whose keys all vanish goes back to the automatic top ten', async () => {
    serve({ '1d': range(0, 12), '7d': range(2, 14) });
    await renderChart();
    await openPicker();
    for (const i of [2, 3, 4, 5, 6, 8, 9, 10, 11]) await click(option(`g${i}`));
    expect(drawn()).toEqual(['g1']);

    fireEvent.click(screen.getByText('range7d'));
    await waitFor(() => expect(mockGet).toHaveBeenLastCalledWith('/stats/node-metrics?range=7d'));
    await flush();
    // g2..g13, CPU means (i * 7) % 12: the two lowest are g7 (1) and g2 (2)
    const all = range(2, 14).map((i) => ({ i, cpu: cpuOf(i) }));
    const expected = all
      .sort((a, b) => a.cpu - b.cpu)
      .slice(2)
      .map((x) => x.i)
      .sort((a, b) => a - b);
    await waitFor(() => expect(drawn()).toEqual(names(expected)));
    expect(drawn()).toHaveLength(10);
  });

  it('clearing the select returns to the automatic top ten', async () => {
    serve({ '1d': range(0, 12) });
    await renderChart();
    await openPicker();
    await click(option('g11'));
    await click(option('g0'));
    expect(drawn()).toContain('g0');

    const clear = picker().querySelector('.ant-select-clear') as HTMLElement;
    expect(clear).not.toBeNull();
    fireEvent.mouseDown(clear);
    await flush();
    expect(drawn()).toEqual(names(without(range(0, 12), [0, 7])));
    expect(picker()).toBeInTheDocument();
    await openPicker();
    expect(screen.getByTestId('series-picker-auto')).toBeInTheDocument();
  });
});
