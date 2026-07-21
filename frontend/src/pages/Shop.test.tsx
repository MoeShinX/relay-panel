import { describe, expect, it, vi, beforeEach } from 'vitest';
import { render, screen, act } from '@testing-library/react';
import userEvent from '@testing-library/user-event';

// Mock the api client before importing the page. Shop calls /plans,
// /user/orders and /user/me on mount, and POSTs /user/redeem on top-up.
const { mockGet, mockPost } = vi.hoisted(() => ({ mockGet: vi.fn(), mockPost: vi.fn() }));

vi.mock('../api/client', () => ({
  default: { get: mockGet, post: mockPost },
}));

import Shop from './Shop';

const ok = <T,>(data: T) => ({ code: 0, message: 'ok', data });

const me = (balance: string) => ({
  username: 'u1',
  balance,
  admin: false,
  suspended: false,
  max_rules: 5,
  current_rules: 0,
  traffic_limit: 0,
  traffic_used: 0,
});

beforeEach(() => {
  mockGet.mockReset();
  mockPost.mockReset();
  mockGet.mockImplementation((url: string) => {
    if (url === '/plans') return Promise.resolve(ok([]));
    if (url === '/user/orders') return Promise.resolve(ok([]));
    if (url === '/user/me') return Promise.resolve(ok(me('0')));
    return Promise.reject(new Error(`unmocked GET ${url}`));
  });
});

const renderShop = async () => {
  await act(async () => { render(<Shop />); });
};

/**
 * The shop is where a user discovers they can't afford a plan. Users reported
 * finding "nowhere to top up" when the only redeem entry lived on the account
 * page, so the presence of this control is a product requirement, not styling.
 */
describe('Shop top-up entry', () => {
  it('offers a redeem control next to the balance', async () => {
    await renderShop();
    // No I18nProvider in this harness, so t() yields the raw key — asserting
    // on it keeps the test independent of translation wording.
    expect(screen.getByRole('button', { name: /redeem/ })).toBeInTheDocument();
  });

  it('redeems a code and reflects the new balance without leaving the page', async () => {
    const user = userEvent.setup();
    mockPost.mockResolvedValue(ok({ amount: '25.50', balance: '25.50' }));
    await renderShop();

    await user.click(screen.getByRole('button', { name: /redeem/ }));
    // Deliberately lowercase + dashed: the server normalizes, and the client
    // must not police the format or it would reject codes the server accepts.
    await user.type(screen.getByRole('textbox'), 'test0-test0-test0-t');

    // Balance is re-read from the server rather than patched client-side, so
    // the displayed number can never drift from what the backend recorded.
    mockGet.mockImplementation((url: string) => {
      if (url === '/plans') return Promise.resolve(ok([]));
      if (url === '/user/orders') return Promise.resolve(ok([]));
      if (url === '/user/me') return Promise.resolve(ok(me('25.50')));
      return Promise.reject(new Error(`unmocked GET ${url}`));
    });

    await user.click(screen.getByRole('button', { name: /redeemConfirm/ }));

    expect(mockPost).toHaveBeenCalledWith('/user/redeem', { code: 'test0-test0-test0-t' });
    expect(await screen.findByText('25.50')).toBeInTheDocument();
  });
});
