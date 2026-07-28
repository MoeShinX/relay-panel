import { describe, expect, it, vi, beforeEach } from 'vitest';
import { render, screen, act } from '@testing-library/react';

const { mockGet } = vi.hoisted(() => ({ mockGet: vi.fn() }));
vi.mock('../api/client', () => ({ default: { get: mockGet } }));

import AccountRecords from './AccountRecords';

const ok = <T,>(data: T) => ({ code: 0, message: 'ok', data });

const redeems = [
  { id: 1, code: '****-****-****-CDEF', amount: '100.00', used_at: '2026-07-28 10:00:00' },
];
const orders = [
  { id: 7, user_id: 2, plan_id: 3, plan_name: '基础套餐', price: '50.00', created_at: '2026-07-27 09:00:00' },
];

beforeEach(() => {
  mockGet.mockReset();
  mockGet.mockImplementation((url: string) =>
    Promise.resolve(url.includes('redeem-records') ? ok(redeems) : ok(orders)),
  );
});

const renderCard = async () => { await act(async () => { render(<AccountRecords />); }); };

describe('AccountRecords', () => {
  it('reads both histories from the user-scoped endpoints', async () => {
    await renderCard();
    const urls = mockGet.mock.calls.map((c) => c[0] as string);
    // Neither URL carries a user id — the server pins both to the token, and a
    // page that passed an id would be the wrong shape for this data.
    expect(urls).toContain('/user/redeem-records');
    expect(urls).toContain('/user/orders');
    expect(urls.every((u) => !/\d/.test(u))).toBe(true);
  });

  it('shows the top-up amount and the masked code', async () => {
    await renderCard();
    expect(screen.getByText('****-****-****-CDEF')).toBeInTheDocument();
    expect(screen.getByText('100.00')).toBeInTheDocument();
  });

  it('renders without crashing when both histories are empty', async () => {
    // The default state for a brand-new account, and the one most likely to be
    // hit by a first-time user.
    mockGet.mockResolvedValue(ok([]));
    await renderCard();
    expect(screen.getByText('noRecharges')).toBeInTheDocument();
  });

  it('stays quiet when the requests fail', async () => {
    mockGet.mockRejectedValue(new Error('network'));
    await renderCard();
    // Still renders its shell rather than blanking the account page.
    expect(screen.getByText('rechargeHistory')).toBeInTheDocument();
  });
});
