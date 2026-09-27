import { describe, expect, it, vi, beforeEach, afterEach } from 'vitest';
import { render, screen, act, fireEvent } from '@testing-library/react';

const { mockGet, mockPost } = vi.hoisted(() => ({ mockGet: vi.fn(), mockPost: vi.fn() }));
vi.mock('../api/client', () => ({ default: { get: mockGet, post: mockPost } }));

import { PanelUpdateButton, type PanelUpdateStatus } from './PanelUpdateButton';

const ok = <T,>(data: T) => ({ code: 0, message: 'ok', data });
const flush = (ms = 0) => act(async () => { await vi.advanceTimersByTimeAsync(ms); });

const status = (over: Partial<PanelUpdateStatus>): PanelUpdateStatus => ({
  available: true, state: 'idle', from_version: '', to_version: '',
  started_at: 0, finished_at: 0, message: '', log_tail: '', ...over,
});

const reload = vi.fn();

beforeEach(() => {
  mockGet.mockReset();
  mockPost.mockReset();
  reload.mockReset();
  vi.useFakeTimers();
  Object.defineProperty(window, 'location', { configurable: true, value: { ...window.location, reload } });
});
afterEach(() => {
  vi.runOnlyPendingTimers();
  vi.useRealTimers();
});

const renderButton = () =>
  render(<PanelUpdateButton currentVersion="1.2.10" targetVersion="v1.2.11" manualUrl="https://docs.example/update" />);

/** Click the button, then OK in the confirmation. */
async function clickUpdateAndConfirm() {
  fireEvent.click(screen.getByRole('button'));
  await flush();
  const okButtons = screen.getAllByRole('button').filter((b) => b.closest('.ant-modal-confirm'));
  fireEvent.click(okButtons[okButtons.length - 1]);
  await flush();
}

describe('PanelUpdateButton', () => {
  it('without the host updater, links to the manual steps instead of pretending', async () => {
    mockGet.mockResolvedValue(ok(status({ available: false })));
    renderButton();
    await flush();
    expect(screen.getByRole('link')).toHaveAttribute('href', 'https://docs.example/update');
    expect(mockPost).not.toHaveBeenCalled();
  });

  it('requests the update and reloads once the panel is back on a new version', async () => {
    let version = '1.2.10';
    mockGet.mockImplementation((url: string) => {
      if (url === '/system/panel-update') return Promise.resolve(ok(status({})));
      if (url === '/health') return Promise.resolve({ status: 'ok', version });
      return Promise.reject(new Error(url));
    });
    mockPost.mockResolvedValue(ok({ target: 'v1.2.11' }));

    renderButton();
    await flush();
    await clickUpdateAndConfirm();
    expect(mockPost).toHaveBeenCalledWith('/system/panel-update');

    // While the panel restarts, health keeps answering the old version (or not
    // at all) — nothing may be declared yet.
    await flush(3000);
    expect(reload).not.toHaveBeenCalled();

    version = '1.2.11';
    await flush(3000);
    await flush(1500);
    expect(reload).toHaveBeenCalled();
  });

  /** The status file still holds the last run's result until the host starts
   *  this one. Reading that as this run's failure would report a failure for an
   *  update that has not even begun. */
  it("ignores the previous run's result, and reports this run's", async () => {
    let hostStatus = status({ state: 'failed', started_at: 100, message: 'old failure' });
    mockGet.mockImplementation((url: string) => {
      if (url === '/system/panel-update') return Promise.resolve(ok(hostStatus));
      if (url === '/health') return Promise.resolve({ status: 'ok', version: '1.2.10' });
      return Promise.reject(new Error(url));
    });
    mockPost.mockResolvedValue(ok({ target: 'v1.2.11' }));

    renderButton();
    await flush();
    await clickUpdateAndConfirm();
    await flush(3000);
    expect(screen.queryByText('old failure')).not.toBeInTheDocument();

    hostStatus = status({ state: 'failed', started_at: 200, message: 'git pull failed', log_tail: 'fatal: local changes' });
    await flush(3000);
    expect(screen.getByText('git pull failed')).toBeInTheDocument();
    expect(reload).not.toHaveBeenCalled();
  });

  it('a request the panel refuses is reported and nothing is watched', async () => {
    mockGet.mockResolvedValue(ok(status({})));
    mockPost.mockResolvedValue({ code: 409, message: 'an update is already in progress', data: null });
    renderButton();
    await flush();
    await clickUpdateAndConfirm();
    await flush(3000);
    expect(mockGet.mock.calls.filter((c) => c[0] === '/health')).toHaveLength(0);
  });
});
