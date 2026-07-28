import { describe, expect, it, vi, beforeEach } from 'vitest';
import { render, screen, act } from '@testing-library/react';

const { mockGet } = vi.hoisted(() => ({ mockGet: vi.fn() }));
vi.mock('../api/client', () => ({ default: { get: mockGet } }));

import RedeemHistory from './RedeemHistory';

const ok = <T,>(data: T) => ({ code: 0, message: 'ok', data });

const rows = [
  { id: 1, code: '****-****-****-CDEF', amount: '100.00', used_at: '2026-07-28 10:00:00' },
];

beforeEach(() => {
  mockGet.mockReset();
  mockGet.mockResolvedValue(ok(rows));
});

const renderCard = async () => { await act(async () => { render(<RedeemHistory />); }); };

describe('RedeemHistory', () => {
  it('reads from the user-scoped endpoint, with no id in the URL', async () => {
    await renderCard();
    const urls = mockGet.mock.calls.map((c) => c[0] as string);
    expect(urls).toEqual(['/user/redeem-records']);
    // The server pins this to the token; a page that passed an id would be the
    // wrong shape for this data.
    expect(urls[0]).not.toMatch(/\d/);
  });

  it('does not fetch order history — that lives on the shop page', async () => {
    await renderCard();
    const urls = mockGet.mock.calls.map((c) => c[0] as string);
    expect(urls.some((u) => u.includes('orders'))).toBe(false);
  });

  it('shows the masked code and the amount', async () => {
    await renderCard();
    expect(screen.getByText('****-****-****-CDEF')).toBeInTheDocument();
    expect(screen.getByText('100.00')).toBeInTheDocument();
  });

  it('renders an empty state for an account that never topped up', async () => {
    // The default state for a brand-new account, and the one a first-time user
    // is most likely to hit.
    mockGet.mockResolvedValue(ok([]));
    await renderCard();
    expect(screen.getByText('noRecharges')).toBeInTheDocument();
  });

  it('stays quiet when the request fails', async () => {
    mockGet.mockRejectedValue(new Error('network'));
    await renderCard();
    // Still renders its shell rather than blanking the account page.
    expect(screen.getByText('rechargeHistory')).toBeInTheDocument();
  });
});
