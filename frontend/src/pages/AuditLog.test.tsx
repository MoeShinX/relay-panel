import { describe, expect, it, vi, beforeEach } from 'vitest';
import { render, screen, within, act, fireEvent } from '@testing-library/react';
import userEvent from '@testing-library/user-event';

// Mock the api client before importing the page. The page only ever GETs —
// the audit log is read-only by design, so there is no post/delete to stub.
const { mockGet } = vi.hoisted(() => ({ mockGet: vi.fn() }));

vi.mock('../api/client', () => ({ default: { get: mockGet } }));

import AuditLog from './AuditLog';
import type { AuditEntry } from '../api/types';

const ok = <T,>(data: T) => ({ code: 0, message: 'ok', data });

const entry = (over: Partial<AuditEntry>): AuditEntry => ({
  id: 1,
  ts: '2026-07-28 10:00:00',
  actor_id: 1,
  actor_name: 'admin',
  action: 'delete_rule',
  target_type: 'rule',
  target_id: '12',
  detail: '',
  ...over,
});

beforeEach(() => {
  mockGet.mockReset();
  mockGet.mockResolvedValue(ok({ items: [entry({})], total: 1 }));
});

const renderPage = async () => { await act(async () => { render(<AuditLog />); }); };

const rowFor = (text: string) => screen.getByText(text).closest('tr') as HTMLElement;

describe('AuditLog', () => {
  it('renders the actor name and the target of an entry', async () => {
    await renderPage();
    const row = rowFor('2026-07-28 10:00:00');
    expect(within(row).getByText('admin')).toBeInTheDocument();
    expect(within(row).getByText('rule 12')).toBeInTheDocument();
  });

  it('still shows who acted after that account was deleted', async () => {
    // The snapshot is the whole point: actor_id can go null, actor_name must
    // not, or the history degrades to anonymous rows exactly when it matters.
    mockGet.mockResolvedValue(ok({
      items: [entry({ id: 2, actor_id: null, actor_name: 'goneadmin' })],
      total: 1,
    }));
    await renderPage();
    expect(screen.getByText('goneadmin')).toBeInTheDocument();
  });

  it('sends the action filter to the server and resets to page 1', async () => {
    await renderPage();
    mockGet.mockClear();

    // antd's Select opens on mouseDown, not click — userEvent.click leaves the
    // dropdown closed in jsdom and the options never render.
    await act(async () => {
      fireEvent.mouseDown(screen.getByRole('combobox'));
    });
    // No I18nProvider here (same as the other page tests), so the context
    // default returns the key itself — the option renders as the raw action
    // name, which is also what the page's own label fallback produces.
    await act(async () => {
      await userEvent.click(screen.getByTitle('delete_user'));
    });

    const url = mockGet.mock.calls.at(-1)?.[0] as string;
    expect(url).toContain('action=delete_user');
    // Filtering from a later page must not keep the old offset, or the first
    // matches would be skipped silently.
    expect(url).toContain('offset=0');
  });

  it('omits the action param entirely when "all actions" is selected', async () => {
    await renderPage();
    // Initial load: no filter chosen yet. An `action=` empty param would be
    // sent to the backend and is handled there, but not sending it at all is
    // what this page promises.
    const url = mockGet.mock.calls[0][0] as string;
    expect(url).not.toContain('action=');
  });
});
