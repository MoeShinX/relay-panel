import { afterEach, describe, expect, it, vi } from 'vitest';
import { act, renderHook } from '@testing-library/react';
import { useLatencySmooth } from './useLatencySmooth';

afterEach(() => {
  vi.restoreAllMocks();
  localStorage.clear();
});

describe('useLatencySmooth', () => {
  it('is off until switched on, and remembers the choice', () => {
    const first = renderHook(() => useLatencySmooth());
    expect(first.result.current[0]).toBe(false);
    act(() => first.result.current[1](true));
    expect(first.result.current[0]).toBe(true);
    first.unmount();
    expect(renderHook(() => useLatencySmooth()).result.current[0]).toBe(true);
  });

  it('works without storage: off by default, and a switch still holds for the page', () => {
    vi.spyOn(Storage.prototype, 'getItem').mockImplementation(() => {
      throw new Error('blocked');
    });
    vi.spyOn(Storage.prototype, 'setItem').mockImplementation(() => {
      throw new Error('blocked');
    });
    const { result } = renderHook(() => useLatencySmooth());
    expect(result.current[0]).toBe(false);
    act(() => result.current[1](true));
    expect(result.current[0]).toBe(true);
  });
});
