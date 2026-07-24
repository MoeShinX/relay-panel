import { describe, expect, it, vi, beforeEach } from 'vitest';
import { render, screen, within, act } from '@testing-library/react';
import userEvent from '@testing-library/user-event';

// Mock the api client before importing the page. The page GETs the list and
// DELETEs by id; POST is only used for generate/void.
const { mockGet, mockPost, mockDelete } = vi.hoisted(() => ({
  mockGet: vi.fn(),
  mockPost: vi.fn(),
  mockDelete: vi.fn(),
}));

vi.mock('../api/client', () => ({
  default: { get: mockGet, post: mockPost, delete: mockDelete },
}));

import RedeemCodes from './RedeemCodes';
import type { RedeemCode } from '../api/types';

const ok = <T,>(data: T) => ({ code: 0, message: 'ok', data });

const code = (over: Partial<RedeemCode>): RedeemCode => ({
  id: 1,
  code: 'AAAA-AAAA-AAAA-AAAA',
  amount: '100.00',
  status: 'unused',
  used_by: null,
  used_by_username: null,
  used_at: null,
  expires_at: null,
  batch_id: 'B1',
  remark: '',
  created_at: '2026-07-24 00:00:00',
  ...over,
});

const rows: RedeemCode[] = [
  code({ id: 2, code: 'USED-USED-USED-USED', status: 'used', used_by: 2, used_by_username: 'normaluser', used_at: '2026-07-24 06:00:01' }),
  code({ id: 3, code: 'UNUS-0000-0000-000A', status: 'unused' }),
  code({ id: 5, code: 'VOID-VOID-VOID-VOID', status: 'void' }),
];

beforeEach(() => {
  mockGet.mockReset();
  mockPost.mockReset();
  mockDelete.mockReset();
  mockGet.mockResolvedValue(ok({ items: rows, total: rows.length }));
});

const renderPage = async () => { await act(async () => { render(<RedeemCodes />); }); };

const rowFor = (text: string) => screen.getByText(text).closest('tr') as HTMLElement;

describe('RedeemCodes', () => {
  it('shows the redeemer username, not a bare #id, for a used code', async () => {
    await renderPage();
    const row = rowFor('USED-USED-USED-USED');
    expect(within(row).getByText('normaluser')).toBeInTheDocument();
    // The raw #id must not leak when a name resolved.
    expect(within(row).queryByText('#2')).not.toBeInTheDocument();
  });

  it('falls back to #id when the username did not resolve (deleted account keeps used_by)', async () => {
    mockGet.mockResolvedValue(ok({
      items: [code({ id: 9, code: 'GONE-GONE-GONE-GONE', status: 'used', used_by: 7, used_by_username: null })],
      total: 1,
    }));
    await renderPage();
    expect(within(rowFor('GONE-GONE-GONE-GONE')).getByText('#7')).toBeInTheDocument();
  });

  it('shows only VOID per row (delete is a toolbar action), and nothing on a used code', async () => {
    await renderPage();
    // used → the money-in record: no per-row action.
    const used = rowFor('USED-USED-USED-USED');
    expect(within(used).queryByRole('button', { name: /void/i })).not.toBeInTheDocument();
    // unused → void only.
    const unused = rowFor('UNUS-0000-0000-000A');
    expect(within(unused).getByRole('button', { name: /void/i })).toBeInTheDocument();
    expect(within(unused).queryByRole('button', { name: /delete/i })).not.toBeInTheDocument();
    // void → no per-row action (can't re-void; delete is the toolbar button).
    const voided = rowFor('VOID-VOID-VOID-VOID');
    expect(within(voided).queryByRole('button', { name: /void/i })).not.toBeInTheDocument();
  });

  it('keeps a delete button in the toolbar, disabled until rows are ticked', async () => {
    const user = userEvent.setup();
    mockDelete.mockResolvedValue(ok(2));
    await renderPage();

    // Always present so the affordance is discoverable, but disabled with no
    // selection — the discoverability fix that prompted this. (The accessible
    // name is "delete delete": the DeleteOutlined icon's aria-label plus the
    // button text, both the raw i18n key in this provider-less harness.)
    const delBtn = screen.getByRole('button', { name: /^delete delete$/i });
    expect(delBtn).toBeDisabled();

    // Tick two rows via their row checkboxes, then the button enables.
    const unused = rowFor('UNUS-0000-0000-000A');
    const voided = rowFor('VOID-VOID-VOID-VOID');
    await user.click(within(unused).getByRole('checkbox'));
    await user.click(within(voided).getByRole('checkbox'));

    const enabled = screen.getByRole('button', { name: /delete.*\(2\)/i });
    expect(enabled).toBeEnabled();
    await user.click(enabled);
    const confirm = await screen.findByRole('button', { name: /^OK$/i });
    await user.click(confirm);

    expect(mockDelete).toHaveBeenCalledWith('/admin/redeem-codes', { data: { ids: [3, 5] } });
  });
});
