import { useCallback, useEffect, useState } from 'react';
import { AUTO_REFRESH_MS } from '../utils/latency';

/**
 * The browser's clock in unix seconds, re-read every `periodMs` (a minute, like
 * the pages' auto-refresh) whether or not any request succeeds, so a "current"
 * reading goes stale on its own when the data stops coming. `refresh` re-reads
 * it at once (after a load). The value only changes on a tick or a refresh, so
 * it is safe in memo dependencies.
 */
export function useNowSecs(periodMs: number = AUTO_REFRESH_MS): [number, () => void] {
  const [now, setNow] = useState(() => Date.now() / 1000);
  const refresh = useCallback(() => setNow(Date.now() / 1000), []);
  useEffect(() => {
    const ti = setInterval(refresh, periodMs);
    return () => clearInterval(ti);
  }, [periodMs, refresh]);
  return [now, refresh];
}
