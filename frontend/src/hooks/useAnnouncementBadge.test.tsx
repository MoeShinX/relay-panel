import { describe, expect, it, vi, beforeEach } from 'vitest';
import { renderHook, act, waitFor } from '@testing-library/react';

const { mockGet } = vi.hoisted(() => ({ mockGet: vi.fn() }));
vi.mock('../api/client', () => ({ default: { get: mockGet } }));

import { useAnnouncementBadge, resetAnnouncementBadgeCache } from './useAnnouncementBadge';

const ok = (latest_id: number) => ({ code: 0, message: 'ok', data: { latest_id } });

beforeEach(() => {
  mockGet.mockReset();
  localStorage.clear();
  resetAnnouncementBadgeCache();
});

describe('useAnnouncementBadge', () => {
  it('shows the dot when there is an announcement nobody on this account has opened', async () => {
    mockGet.mockResolvedValue(ok(7));
    const { result } = renderHook(() => useAnnouncementBadge(1));
    await waitFor(() => expect(result.current.latestId).toBe(7));
    expect(result.current.unread).toBe(true);
  });

  it('clears the dot once marked seen, and keeps it clear', async () => {
    mockGet.mockResolvedValue(ok(7));
    const { result } = renderHook(() => useAnnouncementBadge(1));
    await waitFor(() => expect(result.current.latestId).toBe(7));

    act(() => result.current.markSeen());
    expect(result.current.unread).toBe(false);

    // A fresh mount for the same account stays clear — the marker persisted.
    const second = renderHook(() => useAnnouncementBadge(1));
    await waitFor(() => expect(second.result.current.latestId).toBe(7));
    expect(second.result.current.unread).toBe(false);
  });

  it('clears the dot on OTHER mounted instances, not just the one that marked', async () => {
    // The header bell and the archive page each hold an instance, and the PAGE
    // is what marks seen. With per-instance state the bell kept its stale copy
    // and the dot stayed lit until a full reload — which is exactly when
    // someone is looking at it. This is the regression that caught me.
    mockGet.mockResolvedValue(ok(7));
    const bell = renderHook(() => useAnnouncementBadge(1));
    const page = renderHook(() => useAnnouncementBadge(1));
    await waitFor(() => expect(bell.result.current.latestId).toBe(7));
    await waitFor(() => expect(page.result.current.latestId).toBe(7));
    expect(bell.result.current.unread).toBe(true);

    act(() => page.result.current.markSeen());

    expect(page.result.current.unread).toBe(false);
    expect(bell.result.current.unread).toBe(false);
  });

  it('lights up again when a newer announcement appears', async () => {
    mockGet.mockResolvedValue(ok(7));
    const { result } = renderHook(() => useAnnouncementBadge(1));
    await waitFor(() => expect(result.current.latestId).toBe(7));
    act(() => result.current.markSeen());

    mockGet.mockResolvedValue(ok(8));
    const next = renderHook(() => useAnnouncementBadge(1));
    await waitFor(() => expect(next.result.current.latestId).toBe(8));
    expect(next.result.current.unread).toBe(true);
  });

  it('keeps the marker per account', async () => {
    // A shared browser must not let one account's "seen" suppress another's
    // dot — the marker is keyed by user id for exactly this.
    mockGet.mockResolvedValue(ok(7));
    const first = renderHook(() => useAnnouncementBadge(1));
    await waitFor(() => expect(first.result.current.latestId).toBe(7));
    act(() => first.result.current.markSeen());

    const other = renderHook(() => useAnnouncementBadge(2));
    await waitFor(() => expect(other.result.current.latestId).toBe(7));
    expect(other.result.current.unread).toBe(true);
  });

  it('shows no dot on an install with no announcements', async () => {
    // latest_id 0 must not read as "something newer than 0 exists".
    mockGet.mockResolvedValue(ok(0));
    const { result } = renderHook(() => useAnnouncementBadge(1));
    await waitFor(() => expect(result.current.latestId).toBe(0));
    expect(result.current.unread).toBe(false);
  });

  it('shows no dot when the request fails', async () => {
    // A bell that lights up because a request failed trains people to ignore it.
    mockGet.mockRejectedValue(new Error('network'));
    const { result } = renderHook(() => useAnnouncementBadge(1));
    await waitFor(() => expect(mockGet).toHaveBeenCalled());
    expect(result.current.unread).toBe(false);
  });

  it('does not call the endpoint before the account is known', async () => {
    renderHook(() => useAnnouncementBadge(null));
    expect(mockGet).not.toHaveBeenCalled();
  });
});
