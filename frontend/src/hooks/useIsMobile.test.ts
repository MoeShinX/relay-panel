import { afterEach, describe, expect, it, vi } from 'vitest';
import { renderHook } from '@testing-library/react';
import { useIsMobile, useTableScroll } from './useIsMobile';

/** Make matchMedia answer `matches` for every query. */
function viewport(matches: boolean) {
  vi.spyOn(window, 'matchMedia').mockImplementation(
    (query: string) =>
      ({
        matches,
        media: query,
        onchange: null,
        addListener: vi.fn(),
        removeListener: vi.fn(),
        addEventListener: vi.fn(),
        removeEventListener: vi.fn(),
        dispatchEvent: vi.fn(),
      }) as unknown as MediaQueryList,
  );
}

afterEach(() => vi.restoreAllMocks());

describe('useIsMobile', () => {
  it('asks for the width below the breakpoint', () => {
    viewport(true);
    expect(renderHook(() => useIsMobile()).result.current).toBe(true);
    expect(window.matchMedia).toHaveBeenCalledWith('(max-width: 767px)');
  });
});

// v1.2.13: on a phone a wide table keeps its natural width and scrolls inside
// itself. On a desktop it is `x: true` — laid out like a plain table (cells
// wrap to fit), with only what cannot fit in a narrow window scrolling inside
// the table rather than widening the page.
describe('useTableScroll', () => {
  it('keeps a table at its natural width on a phone', () => {
    viewport(true);
    expect(renderHook(() => useTableScroll()).result.current).toEqual({ x: 'max-content' });
  });

  it('fits a desktop table to the window, containing any overflow', () => {
    viewport(false);
    expect(renderHook(() => useTableScroll()).result.current).toEqual({ x: true });
  });
});
