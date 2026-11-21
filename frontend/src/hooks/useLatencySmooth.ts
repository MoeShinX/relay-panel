import { useCallback, useState } from 'react';

const KEY = 'rp.latency.smooth';

function read(): boolean {
  try {
    return localStorage.getItem(KEY) === '1';
  } catch {
    return false;
  }
}

/** The latency trend chart's "smooth" switch, remembered per browser. Off by
 *  default; a browser that blocks storage just forgets it on reload. */
export function useLatencySmooth(): [boolean, (on: boolean) => void] {
  const [on, setOn] = useState<boolean>(read);
  const set = useCallback((next: boolean) => {
    setOn(next);
    try {
      localStorage.setItem(KEY, next ? '1' : '0');
    } catch {
      /* storage blocked: keep the choice for this page only */
    }
  }, []);
  return [on, set];
}
