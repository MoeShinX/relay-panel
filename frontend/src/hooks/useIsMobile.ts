import { useEffect, useState } from 'react';

/** Is the viewport phone-width (below antd's `md`, 768px by default)?
 *
 *  Read synchronously on the first render, so a phone never paints the desktop
 *  layout first, and kept current as the window crosses the breakpoint
 *  (rotation, a resized window). Shared by the main layout and the node-status
 *  page so both switch at the same width. */
export function useIsMobile(breakpoint = 768): boolean {
  const query = `(max-width: ${breakpoint - 1}px)`;
  const [mobile, setMobile] = useState(() => window.matchMedia(query).matches);
  useEffect(() => {
    const mq = window.matchMedia(query);
    const sync = () => setMobile(mq.matches);
    sync();
    mq.addEventListener('change', sync);
    return () => mq.removeEventListener('change', sync);
  }, [query]);
  return mobile;
}

const PHONE_TABLE_SCROLL = { x: 'max-content' } as const;

/** The `scroll` prop for a wide table. On a phone the table scrolls sideways
 *  inside itself instead of widening the whole page. Anywhere wider it is
 *  undefined — no scroll container at all — so a desktop table lays out
 *  exactly as it did before: cells wrap to fit the window, no scrollbar. */
export function usePhoneTableScroll(): { x: 'max-content' } | undefined {
  return useIsMobile() ? PHONE_TABLE_SCROLL : undefined;
}
