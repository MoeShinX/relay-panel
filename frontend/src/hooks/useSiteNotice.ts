import { useEffect, useState } from 'react';
import api from '../api/client';
import type { ApiEnvelope, SiteNotice } from '../api/types';

/**
 * v1.3.0: the signed-in half of the site config — announcement + support
 * contact, from the AUTHENTICATED `/user/site-notice`.
 *
 * Separate from `useSite` (public branding) because the two have different auth
 * requirements, not just different fields: the login page reads branding with
 * no token, and these must not be served to unauthenticated callers.
 *
 * Same cache + subscriber shape as useSite, for the same reason — an admin who
 * edits the announcement should see the banner change, not a stale copy.
 */
const EMPTY: SiteNotice = { announcement: '', contact: '' };

let cached: Promise<SiteNotice> | null = null;
const subscribers = new Set<(n: SiteNotice) => void>();

function fetchNotice(): Promise<SiteNotice> {
  if (!cached) {
    cached = api
      .get<unknown, ApiEnvelope<SiteNotice>>('/user/site-notice')
      .then((res) => (res.code === 0 && res.data ? res.data : EMPTY))
      // An unreachable announcement is not worth an error toast on every page.
      .catch(() => EMPTY);
  }
  return cached;
}

/** Drop the cache; re-fetch only if something is currently displaying it.
 *  With nothing mounted there is no one to notify and the next mount will
 *  fetch anyway, so an eager request would be pure waste. */
export function invalidateSiteNotice() {
  cached = null;
  if (subscribers.size === 0) return;
  fetchNotice().then((n) => subscribers.forEach((fn) => fn(n)));
}

export function useSiteNotice(): SiteNotice {
  const [notice, setNotice] = useState<SiteNotice>(EMPTY);
  useEffect(() => {
    let alive = true;
    const update = (n: SiteNotice) => { if (alive) setNotice(n); };
    subscribers.add(update);
    fetchNotice().then(update);
    return () => {
      alive = false;
      subscribers.delete(update);
    };
  }, []);
  return notice;
}
