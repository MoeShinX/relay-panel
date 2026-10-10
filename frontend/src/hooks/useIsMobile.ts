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
const WIDE_TABLE_SCROLL = { x: true } as const;

/** The `scroll` prop for a wide table.
 *
 *  On a phone the table keeps its natural width and scrolls sideways inside
 *  itself, rather than wrapping every cell into a narrow column.
 *
 *  Anywhere wider it is `x: true`: the table is laid out exactly as a plain
 *  one (auto width, at least the window's — cells wrap to fit, no scrollbar
 *  while it fits), and only a table that still cannot fit, in a narrow
 *  window, scrolls inside itself instead of widening the whole page. */
export function useTableScroll(): { x: 'max-content' } | { x: true } {
  return useIsMobile() ? PHONE_TABLE_SCROLL : WIDE_TABLE_SCROLL;
}
