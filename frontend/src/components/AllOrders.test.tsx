import { describe, expect, it, vi, beforeEach } from 'vitest';
import { render, screen, act } from '@testing-library/react';

const { mockGet } = vi.hoisted(() => ({ mockGet: vi.fn() }));
vi.mock('../api/client', () => ({ default: { get: mockGet } }));

import AllOrders from './AllOrders';

const ok = <T,>(data: T) => ({ code: 0, message: 'ok', data });

const rows = [
  { id: 2, user_id: 5, username: 'zhangsan', plan_name: '广港', price: '50.00', created_at: '2026-07-28 10:00:00' },
  { id: 1, user_id: 9, username: null, plan_name: 'Free', price: '9.90', created_at: '2026-07-27 09:00:00' },
];

beforeEach(() => {
  mockGet.mockReset();
  mockGet.mockResolvedValue(ok({ items: rows, total: 2 }));
});

const renderCard = async () => { await act(async () => { render(<AllOrders />); }); };

describe('AllOrders', () => {
  it('reads the admin endpoint, not the per-user one', async () => {
    // /user/orders is scoped to the caller; using it here would silently show
    // the operator only their own purchases.
    await renderCard();
    const url = mockGet.mock.calls[0][0] as string;
    expect(url).toContain('/admin/orders');
    expect(url).not.toContain('/user/orders');
  });

  it('shows the buyer name', async () => {
    await renderCard();
    expect(screen.getByText('zhangsan')).toBeInTheDocument();
    expect(screen.getByText('广港')).toBeInTheDocument();
  });

  it('falls back to the id when the buyer account was deleted', async () => {
    // The order row outlives the account on purpose — it is the money-in
    // record — so a missing name must not blank the row.
    await renderCard();
    expect(screen.getByText('#9')).toBeInTheDocument();
  });

  it('renders an empty state rather than failing when the request errors', async () => {
    mockGet.mockRejectedValue(new Error('network'));
    await renderCard();
    expect(screen.getByText('noOrdersAll')).toBeInTheDocument();
  });
});
