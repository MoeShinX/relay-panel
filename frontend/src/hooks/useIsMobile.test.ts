import { afterEach, describe, expect, it, vi } from 'vitest';
import { renderHook } from '@testing-library/react';
import { useIsMobile, usePhoneTableScroll } from './useIsMobile';

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

// v1.2.13: a wide table scrolls inside itself on a phone; on a desktop it gets
// no scroll container at all, so it lays out exactly as before (cells wrap to
// fit the window instead of a scrollbar appearing).
describe('usePhoneTableScroll', () => {
  it('scrolls a table sideways on a phone', () => {
    viewport(true);
    expect(renderHook(() => usePhoneTableScroll()).result.current).toEqual({ x: 'max-content' });
  });

  it('leaves a desktop table alone', () => {
    viewport(false);
    expect(renderHook(() => usePhoneTableScroll()).result.current).toBeUndefined();
  });
});
