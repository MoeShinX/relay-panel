import { describe, expect, it, vi, beforeEach } from 'vitest';
import { render, screen, act } from '@testing-library/react';
import userEvent from '@testing-library/user-event';

const { mockGet } = vi.hoisted(() => ({ mockGet: vi.fn() }));

vi.mock('../api/client', () => ({ default: { get: mockGet } }));
vi.mock('react-router-dom', async () => {
  const actual = await vi.importActual<typeof import('react-router-dom')>('react-router-dom');
  return { ...actual, useNavigate: () => vi.fn() };
});
vi.mock('../auth/useAuth', () => ({ useAuth: () => ({ isAdmin: true }) }));

import Users from './Users';
import type { User } from '../api/types';

const ok = <T,>(data: T) => ({ code: 0, message: 'ok', data });

const user = (over: Partial<User>): User => ({
  id: 1, username: 'admin', admin: true, banned: false, suspended: false,
  balance: '0', plan_id: null, all_device_groups: true, max_rules: 5,
  traffic_used: 0, traffic_limit: 0, created_at: '2026-07-24',
  ...over,
} as User);

const users: User[] = [
  user({ id: 1, username: 'admin', admin: true }),
  user({ id: 2, username: 'normaluser', admin: false }),
  user({ id: 42, username: 'alice', admin: false }),
];

beforeEach(() => {
  mockGet.mockReset();
  mockGet.mockImplementation((url: string) => {
    if (url === '/admin/users') return Promise.resolve(ok(users));
    if (url === '/admin/plans') return Promise.resolve(ok([]));
    return Promise.reject(new Error(`unmocked GET ${url}`));
  });
});

const renderPage = async () => { await act(async () => { render(<Users />); }); };

describe('Users search', () => {
  it('filters the table by username substring', async () => {
    const u = userEvent.setup();
    await renderPage();
    expect(screen.getByText('normaluser')).toBeInTheDocument();
    expect(screen.getByText('alice')).toBeInTheDocument();

    await u.type(screen.getByPlaceholderText(/searchUserPlaceholder|search/i), 'normal');

    expect(screen.getByText('normaluser')).toBeInTheDocument();
    expect(screen.queryByText('alice')).not.toBeInTheDocument();
    expect(screen.queryByText('admin')).not.toBeInTheDocument();
  });

  it('matches on id as well as name', async () => {
    const u = userEvent.setup();
    await renderPage();
    await u.type(screen.getByPlaceholderText(/searchUserPlaceholder|search/i), '42');
    expect(screen.getByText('alice')).toBeInTheDocument();
    expect(screen.queryByText('normaluser')).not.toBeInTheDocument();
  });
});
