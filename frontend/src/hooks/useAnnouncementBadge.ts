import { useCallback, useEffect, useState, useSyncExternalStore } from 'react';
import api from '../api/client';
import type { ApiEnvelope } from '../api/types';

/**
 * v1.3.0: unread state for the header announcement bell.
 *
 * "Unread" is `latest id > the id this account last looked at`. The id, not a
 * timestamp: editing an old notice must not re-notify everyone, and only a new
 * row raises the maximum.
 *
 * The seen marker is per account and lives in localStorage — the server keeps
 * no per-user read state, which would be a table and a write on every page view
 * for a dot. The cost is that the dot returns on a new browser, which is the
 * right way for this to be wrong.
 *
 * The marker is exposed through useSyncExternalStore rather than per-instance
 * component state. The header bell and the archive page each hold their own
 * instance of this hook and the PAGE is what marks things seen; with local
 * state the bell kept its stale copy and the dot stayed lit until a full
 * reload — exactly when someone is looking at it.
 */
const KEY_PREFIX = 'relaypanel_ann_seen_';

const listeners = new Set<() => void>();

/** In-memory mirror so getSnapshot is cheap and returns a stable number. */
const cache = new Map<string, number>();

function seenKey(userId: number | null): string | null {
  return userId == null ? null : `${KEY_PREFIX}${userId}`;
}

function getSeen(userId: number | null): number {
  const k = seenKey(userId);
  if (!k) return 0;
  const hit = cache.get(k);
  if (hit !== undefined) return hit;
  const raw = localStorage.getItem(k);
  const n = raw ? Number(raw) : 0;
  const val = Number.isFinite(n) ? n : 0;
  cache.set(k, val);
  return val;
}

function setSeen(userId: number | null, id: number) {
  const k = seenKey(userId);
  if (!k) return;
  localStorage.setItem(k, String(id));
  cache.set(k, id);
  listeners.forEach((fn) => fn());
}

function subscribe(cb: () => void): () => void {
  listeners.add(cb);
  return () => { listeners.delete(cb); };
}

export interface AnnouncementBadge {
  latestId: number;
  unread: boolean;
  /** Call when the archive has actually been shown. */
  markSeen: () => void;
}

export function useAnnouncementBadge(userId: number | null): AnnouncementBadge {
  const [latestId, setLatestId] = useState(0);
  const seen = useSyncExternalStore(subscribe, () => getSeen(userId));

  useEffect(() => {
    if (userId == null) return;
    let alive = true;
    api
      .get<unknown, ApiEnvelope<{ latest_id: number }>>('/user/announcements/latest-id')
      .then((res) => {
        if (alive && res.code === 0 && res.data) setLatestId(res.data.latest_id);
      })
      // No dot is the right failure mode: a bell that lights up because a
      // request failed would train people to ignore it.
      .catch(() => {});
    return () => { alive = false; };
  }, [userId]);

  const markSeen = useCallback(() => {
    if (userId == null || latestId <= 0) return;
    setSeen(userId, latestId);
  }, [userId, latestId]);

  return { latestId, unread: latestId > seen, markSeen };
}

/** Test helper: drop the in-memory mirror so a cleared localStorage is re-read. */
export function resetAnnouncementBadgeCache() {
  cache.clear();
  listeners.forEach((fn) => fn());
}
