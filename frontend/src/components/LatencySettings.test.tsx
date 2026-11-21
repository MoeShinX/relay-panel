import { describe, expect, it, vi, beforeEach } from 'vitest';
import { fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';

const { mockGet, mockPut, mockDelete } = vi.hoisted(() => ({ mockGet: vi.fn(), mockPut: vi.fn(), mockDelete: vi.fn() }));
vi.mock('../api/client', () => ({ default: { get: mockGet, put: mockPut, delete: mockDelete } }));

import LatencySettings from './LatencySettings';
import type { LatencyConfig, LatencyTarget } from '../api/types';
import { I18nContext } from '../i18n/context';
import { zhCN } from '../i18n/zh-CN';

const ok = <T,>(data: T) => ({ code: 0, message: 'ok', data });

const realT = {
  t: (k: keyof typeof zhCN) => zhCN[k],
  lang: 'zh-CN' as const,
  setLang: () => {},
};

function target(id: number, over: Partial<LatencyTarget> = {}): LatencyTarget {
  return {
    id,
    group_id: 5,
    name: `target ${id}`,
    carrier: 'telecom',
    kind: 'tcp',
    host: `192.0.2.${id}`,
    port: 443,
    sort_order: id,
    created_at: '2026-01-01 00:00:00',
    ...over,
  };
}

function config(targets: LatencyTarget[], interval = 10): LatencyConfig {
  return { interval_secs: interval, targets };
}

const onSaved = vi.fn();
const onCleared = vi.fn();

async function renderSettings(cfg: LatencyConfig) {
  mockGet.mockResolvedValue(ok(cfg));
  const view = render(
    <I18nContext.Provider value={realT}>
      <LatencySettings groupId={5} onSaved={onSaved} onCleared={onCleared} />
    </I18nContext.Provider>,
  );
  await screen.findByTestId('latency-row-0').catch(() => screen.findByText(zhCN.latencyNoTargetsYet));
  return view;
}

const rowFields = (i: number) => {
  const row = screen.getByTestId(`latency-row-${i}`);
  return {
    row,
    name: within(row).getByLabelText(zhCN.name) as HTMLInputElement,
    host: within(row).getByLabelText(zhCN.latencyHost) as HTMLInputElement,
    port: within(row).getByLabelText(zhCN.port) as HTMLInputElement,
  };
};

const clickSave = () => userEvent.click(screen.getByTestId("latency-save"));

beforeEach(() => {
  mockGet.mockReset();
  mockPut.mockReset();
  mockDelete.mockReset();
  onSaved.mockReset();
  onCleared.mockReset();
  mockPut.mockImplementation((_url: string, body: { interval_secs: number; targets: unknown[] }) =>
    Promise.resolve(ok(config(
      body.targets.map((t, i) => ({ ...target(i + 1), ...(t as object) }) as LatencyTarget),
      body.interval_secs,
    ))));
});

// Saving shows antd's message toasts; src/test/setup.ts closes them and lets
// React's scheduler drain after every test, so nothing runs after jsdom is gone.

describe('LatencySettings clear history', () => {
  /** The confirmation's own button: it carries the same label as the one that opened it. */
  const confirmButton = async () => {
    const text = await screen.findByText(zhCN.latencyClearHistoryConfirm);
    const popover = text.closest('.ant-popover') as HTMLElement;
    return within(popover).getByRole('button', { name: new RegExp(zhCN.latencyClearHistory) });
  };

  it('asks first, then sends the DELETE for this group and says how many rows went', async () => {
    mockDelete.mockResolvedValue(ok({ deleted: 12 }));
    await renderSettings(config([target(1)]));
    await userEvent.click(screen.getByTestId('latency-clear-history'));
    // nothing is sent until the question is answered
    expect(await screen.findByText(zhCN.latencyClearHistoryConfirm)).toBeInTheDocument();
    expect(mockDelete).not.toHaveBeenCalled();

    await userEvent.click(await confirmButton());
    await waitFor(() => expect(mockDelete).toHaveBeenCalledTimes(1));
    expect(mockDelete).toHaveBeenCalledWith('/admin/groups/5/latency/history');
    expect(await screen.findByText(zhCN.latencyHistoryClearedN.replace('{n}', '12'))).toBeInTheDocument();
    expect(onCleared).toHaveBeenCalledTimes(1);
    // clearing history is not saving the configuration
    expect(mockPut).not.toHaveBeenCalled();
    await waitFor(() => expect(screen.getByTestId('latency-clear-history')).not.toBeDisabled());
  });

  it('cancelling sends nothing', async () => {
    await renderSettings(config([target(1)]));
    await userEvent.click(screen.getByTestId('latency-clear-history'));
    const text = await screen.findByText(zhCN.latencyClearHistoryConfirm);
    // antd spaces out a two-character label, so take the first button of the pair
    const cancel = (text.closest('.ant-popover') as HTMLElement).querySelector('.ant-popconfirm-buttons button') as HTMLElement;
    await userEvent.click(cancel);
    expect(mockDelete).not.toHaveBeenCalled();
    expect(onCleared).not.toHaveBeenCalled();
  });

  it('the button is disabled while the request runs', async () => {
    let finish: (v: unknown) => void = () => {};
    mockDelete.mockReturnValue(new Promise((resolve) => { finish = resolve; }));
    await renderSettings(config([target(1)]));
    await userEvent.click(screen.getByTestId('latency-clear-history'));
    await userEvent.click(await confirmButton());
    await waitFor(() => expect(screen.getByTestId('latency-clear-history')).toBeDisabled());
    await act(async () => {
      finish(ok({ deleted: 0 }));
    });
    await waitFor(() => expect(screen.getByTestId('latency-clear-history')).not.toBeDisabled());
    expect(mockDelete).toHaveBeenCalledTimes(1);
  });

  it('a refused or failed request says so and does not report success', async () => {
    mockDelete.mockResolvedValueOnce({ code: 404, message: '分组不存在', data: null });
    await renderSettings(config([target(1)]));
    await userEvent.click(screen.getByTestId('latency-clear-history'));
    await userEvent.click(await confirmButton());
    expect(await screen.findByText('分组不存在')).toBeInTheDocument();
    expect(onCleared).not.toHaveBeenCalled();

    mockDelete.mockRejectedValueOnce(new Error('network'));
    await userEvent.click(screen.getByTestId('latency-clear-history'));
    await userEvent.click(await confirmButton());
    expect(await screen.findByText(zhCN.latencyClearFailed)).toBeInTheDocument();
    expect(onCleared).not.toHaveBeenCalled();
  });
});

describe('LatencySettings', () => {
  it('reads and writes the configuration of the group it is given', async () => {
    await renderSettings(config([target(1)]));
    expect(mockGet).toHaveBeenCalledWith('/admin/groups/5/latency/config');
    await clickSave();
    await waitFor(() => expect(mockPut).toHaveBeenCalledTimes(1));
    expect(mockPut.mock.calls[0][0]).toBe('/admin/groups/5/latency/config');
    // the page that opened it learns how many targets the group now has
    expect(onSaved).toHaveBeenCalledWith(1);
  });

  it('starts empty with the default interval and nothing to save but the interval', async () => {
    await renderSettings(config([]));
    expect(screen.getByText(zhCN.latencyNoTargetsYet)).toBeInTheDocument();
    expect(screen.getByLabelText(zhCN.latencyInterval)).toHaveValue('10');
  });

  it('shows the saved targets in order', async () => {
    await renderSettings(config([target(1, { name: 'A' }), target(2, { name: 'B', kind: 'icmp', port: null })]));
    expect(rowFields(0).name.value).toBe('A');
    expect(rowFields(1).name.value).toBe('B');
    // ICMP has no port to fill in
    expect(rowFields(1).port).toBeDisabled();
    expect(rowFields(0).port).not.toBeDisabled();
  });

  it('saves a new target without an id and keeps the id of an existing one', async () => {
    await renderSettings(config([target(7, { name: 'Old' })]));
    await userEvent.click(screen.getByRole('button', { name: new RegExp(zhCN.latencyAddTarget) }));
    const second = rowFields(1);
    await userEvent.type(second.name, '  Beijing  ');
    await userEvent.type(second.host, ' probe.example.com ');
    await clickSave();

    await waitFor(() => expect(mockPut).toHaveBeenCalledTimes(1));
    const [url, body] = mockPut.mock.calls[0];
    expect(url).toBe('/admin/groups/5/latency/config');
    expect(body.interval_secs).toBe(10);
    expect(body.targets).toEqual([
      { id: 7, name: 'Old', carrier: 'telecom', kind: 'tcp', host: '192.0.2.7', port: 443 },
      // trimmed, no id, the default TCP port
      { name: 'Beijing', carrier: 'telecom', kind: 'tcp', host: 'probe.example.com', port: 443 },
    ]);
  });

  it('an ICMP target is sent without a port', async () => {
    await renderSettings(config([target(1, { kind: 'icmp', port: null })]));
    await clickSave();
    await waitFor(() => expect(mockPut).toHaveBeenCalled());
    expect(mockPut.mock.calls[0][1].targets[0]).toMatchObject({ kind: 'icmp', port: null });
  });

  it('refuses what the server would refuse, and sends nothing', async () => {
    await renderSettings(config([target(1)]));

    // empty name
    fireEvent.change(rowFields(0).name, { target: { value: '   ' } });
    await clickSave();
    expect(mockPut).not.toHaveBeenCalled();
    expect(await screen.findAllByText(zhCN.latencyErrName)).not.toHaveLength(0);
    fireEvent.change(rowFields(0).name, { target: { value: 'ok' } });

    // name over 32 characters
    fireEvent.change(rowFields(0).name, { target: { value: 'x'.repeat(33) } });
    await clickSave();
    expect(mockPut).not.toHaveBeenCalled();
    fireEvent.change(rowFields(0).name, { target: { value: 'x'.repeat(32) } });

    // not a host
    for (const bad of ['http://example.com', 'a b', '999.1.1.1', '']) {
      fireEvent.change(rowFields(0).host, { target: { value: bad } });
      await clickSave();
      expect(mockPut, bad).not.toHaveBeenCalled();
    }
    fireEvent.change(rowFields(0).host, { target: { value: 'example.com' } });

    // TCP without a port
    fireEvent.change(rowFields(0).port, { target: { value: '' } });
    await clickSave();
    expect(mockPut).not.toHaveBeenCalled();
    fireEvent.change(rowFields(0).port, { target: { value: '8443' } });

    // no interval at all (the field itself clamps what is typed to 5-60)
    fireEvent.change(screen.getByLabelText(zhCN.latencyInterval), { target: { value: '' } });
    fireEvent.blur(screen.getByLabelText(zhCN.latencyInterval));
    await clickSave();
    expect(mockPut).not.toHaveBeenCalled();
    fireEvent.change(screen.getByLabelText(zhCN.latencyInterval), { target: { value: '30' } });
    fireEvent.blur(screen.getByLabelText(zhCN.latencyInterval));

    // all fixed: it goes through
    await clickSave();
    await waitFor(() => expect(mockPut).toHaveBeenCalledTimes(1));
    expect(mockPut.mock.calls[0][1].interval_secs).toBe(30);
    expect(mockPut.mock.calls[0][1].targets[0]).toMatchObject({ host: 'example.com', port: 8443 });
  });

  it('allows at most 16 targets', async () => {
    const sixteen = Array.from({ length: 16 }, (_, i) => target(i + 1));
    await renderSettings(config(sixteen));
    const add = screen.getByRole('button', { name: new RegExp(zhCN.latencyAddTarget) });
    expect(add).toBeDisabled();
    expect(screen.getByText(zhCN.latencyTargetLimit)).toBeInTheDocument();
    expect(screen.queryByTestId('latency-row-16')).toBeNull();
  });

  it('adding up to the limit disables the button at 16', async () => {
    await renderSettings(config(Array.from({ length: 15 }, (_, i) => target(i + 1))));
    const add = screen.getByRole('button', { name: new RegExp(zhCN.latencyAddTarget) });
    expect(add).not.toBeDisabled();
    await userEvent.click(add);
    expect(screen.getByTestId('latency-row-15')).toBeInTheDocument();
    expect(add).toBeDisabled();
  });

  it('removing a saved target warns that its history goes with it', async () => {
    await renderSettings(config([target(1), target(2)]));
    expect(screen.queryByText(zhCN.latencyDeleteWarning)).toBeNull();
    await userEvent.click(within(screen.getByTestId('latency-row-0')).getByRole('button', { name: zhCN.latencyRemoveTarget }));
    expect(screen.getByText(zhCN.latencyDeleteWarning)).toBeInTheDocument();
    expect(screen.queryByTestId('latency-row-1')).toBeNull();
    await clickSave();
    await waitFor(() => expect(mockPut).toHaveBeenCalled());
    expect(mockPut.mock.calls[0][1].targets).toHaveLength(1);
    expect(mockPut.mock.calls[0][1].targets[0].id).toBe(2);
  });

  it('removing a target that was never saved does not warn', async () => {
    await renderSettings(config([]));
    await userEvent.click(screen.getByRole('button', { name: new RegExp(zhCN.latencyAddTarget) }));
    await userEvent.click(within(screen.getByTestId('latency-row-0')).getByRole('button', { name: zhCN.latencyRemoveTarget }));
    expect(screen.queryByText(zhCN.latencyDeleteWarning)).toBeNull();
  });

  it('shows a retry when the configuration cannot be loaded', async () => {
    mockGet.mockResolvedValue({ code: 500, message: 'x', data: null });
    render(
      <I18nContext.Provider value={realT}>
        <LatencySettings groupId={5} />
      </I18nContext.Provider>,
    );
    expect(await screen.findByText(zhCN.settingsLoadFailed)).toBeInTheDocument();
    expect(screen.getByTestId("latency-save")).toBeDisabled();
  });

  it('keeps the page on the server\'s answer after a save', async () => {
    await renderSettings(config([target(1)]));
    fireEvent.change(rowFields(0).name, { target: { value: 'Renamed' } });
    await clickSave();
    await waitFor(() => expect(mockPut).toHaveBeenCalled());
    await waitFor(() => expect(rowFields(0).name.value).toBe('Renamed'));
  });
});
