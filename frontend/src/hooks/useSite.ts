import { useEffect, useState } from 'react';
import api from '../api/client';
import type { ApiEnvelope, PublicSite } from '../api/types';

/**
 * v1.3.0: the operator's site branding (name + subtitle).
 *
 * Served by the PUBLIC `/site` endpoint, because the login page renders the
 * brand before anyone has a token. The announcement and support contact are
 * deliberately not here — they come from the authenticated `/user/site-notice`.
 *
 * Cached in a module-level promise rather than fetched per component: the brand
 * is rendered by both the login page and the sidebar, and re-requesting it on
 * every mount would put a needless round-trip in front of every navigation.
 *
 * Invalidation notifies live subscribers rather than only clearing the cache.
 * The sidebar is mounted while the admin saves the settings form, so a plain
 * cache clear would leave the old name on screen until a full reload — the one
 * moment the feature is being looked at.
 */
const EMPTY: PublicSite = { site_name: '', subtitle: '' };

let cached: Promise<PublicSite> | null = null;
const subscribers = new Set<(s: PublicSite) => void>();

function fetchSite(): Promise<PublicSite> {
  if (!cached) {
    cached = api
      .get<unknown, ApiEnvelope<PublicSite>>('/site')
      .then((res) => (res.code === 0 && res.data ? res.data : EMPTY))
      // A failed brand lookup must never block the login page — fall back to
      // empty and let the caller use its translated default.
      .catch(() => EMPTY);
  }
  return cached;
}

/** Drop the cache; re-fetch only if something is currently displaying it.
 *  With nothing mounted there is no one to notify and the next mount will
 *  fetch anyway, so an eager request would be pure waste. */
export function invalidateSite() {
  cached = null;
  if (subscribers.size === 0) return;
  fetchSite().then((s) => subscribers.forEach((fn) => fn(s)));
}

export function useSite(): PublicSite {
  const [site, setSite] = useState<PublicSite>(EMPTY);
  useEffect(() => {
    let alive = true;
    const update = (s: PublicSite) => { if (alive) setSite(s); };
    subscribers.add(update);
    fetchSite().then(update);
    return () => {
      alive = false;
      subscribers.delete(update);
    };
  }, []);
  return site;
}
