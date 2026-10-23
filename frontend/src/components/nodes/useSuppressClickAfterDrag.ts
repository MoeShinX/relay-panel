import { useCallback, useEffect, useRef } from 'react';

/** How long after a drop a click still counts as the tail of that drag. */
const WINDOW_MS = 150;

/**
 * Dropping a dragged group fires a click on the header it was picked up from,
 * which would fold or unfold the group. `consume()` says whether the click it
 * is asked about is that tail (true: swallow it), once per drag.
 */
export function useSuppressClickAfterDrag(isDragging: boolean): () => boolean {
  const wasDragging = useRef(false);
  const suppress = useRef(false);

  useEffect(() => {
    if (isDragging) {
      wasDragging.current = true;
      return undefined;
    }
    if (!wasDragging.current) return undefined;
    wasDragging.current = false;
    suppress.current = true;
    const timer = setTimeout(() => { suppress.current = false; }, WINDOW_MS);
    return () => clearTimeout(timer);
  }, [isDragging]);

  return useCallback(() => {
    if (!suppress.current) return false;
    suppress.current = false;
    return true;
  }, []);
}
