import { useEffect, useState } from 'react';
import api from '../api/client';
import type { ApiEnvelope, Announcement } from '../api/types';

/**
 * v1.2.4: the one announcement the banner shows, from the authenticated
 * `/user/announcements/active`.
 *
 * `null` means there is nothing live — no pinned notice, and everything else
 * either absent or past its expiry. The banner renders nothing at all in that
 * case rather than an empty box.
 *
 * Cached and invalidatable like the other site hooks: an admin who posts a
 * notice should see the banner appear without a full reload.
 */
let cached: Promise<Announcement | null> | null = null;
const subscribers = new Set<(a: Announcement | null) => void>();

function fetchActive(): Promise<Announcement | null> {
  if (!cached) {
    cached = api
      .get<unknown, ApiEnvelope<Announcement | null>>('/user/announcements/active')
      .then((res) => (res.code === 0 ? (res.data ?? null) : null))
      // An unreachable banner is not worth an error toast on every page load.
      .catch(() => null);
  }
  return cached;
}

/** Drop the cache; re-fetch only if something is currently displaying it. */
export function invalidateActiveAnnouncement() {
  cached = null;
  if (subscribers.size === 0) return;
  fetchActive().then((a) => subscribers.forEach((fn) => fn(a)));
}

export function useActiveAnnouncement(): Announcement | null {
  const [active, setActive] = useState<Announcement | null>(null);
  useEffect(() => {
    let alive = true;
    const update = (a: Announcement | null) => { if (alive) setActive(a); };
    subscribers.add(update);
    fetchActive().then(update);
    return () => {
      alive = false;
      subscribers.delete(update);
    };
  }, []);
  return active;
}
