import { act, renderHook } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { useSuppressClickAfterDrag } from './useSuppressClickAfterDrag';

describe('useSuppressClickAfterDrag', () => {
  beforeEach(() => { vi.useFakeTimers(); });
  afterEach(() => { vi.useRealTimers(); });

  it('lets an ordinary click through', () => {
    const { result } = renderHook(({ d }) => useSuppressClickAfterDrag(d), { initialProps: { d: false } });
    expect(result.current()).toBe(false);
  });

  it('swallows the click that follows a drop, once', () => {
    const { result, rerender } = renderHook(({ d }) => useSuppressClickAfterDrag(d), { initialProps: { d: false } });
    rerender({ d: true });
    expect(result.current()).toBe(false); // still dragging: nothing to swallow yet
    rerender({ d: false });
    expect(result.current()).toBe(true); // the click that ends the drag
    expect(result.current()).toBe(false); // the next real click folds as usual
  });

  it('stops swallowing after a moment, so a later click is not lost', () => {
    const { result, rerender } = renderHook(({ d }) => useSuppressClickAfterDrag(d), { initialProps: { d: false } });
    rerender({ d: true });
    rerender({ d: false });
    act(() => { vi.advanceTimersByTime(500); });
    expect(result.current()).toBe(false);
  });
});
