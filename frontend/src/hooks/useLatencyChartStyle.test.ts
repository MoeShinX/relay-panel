import { afterEach, describe, expect, it, vi } from 'vitest';
import { act, renderHook } from '@testing-library/react';
import { useLatencyChartStyle } from './useLatencyChartStyle';

afterEach(() => {
  vi.restoreAllMocks();
  localStorage.clear();
});

describe('useLatencyChartStyle', () => {
  it('is modern until switched, and remembers the choice', () => {
    const first = renderHook(() => useLatencyChartStyle());
    expect(first.result.current[0]).toBe('modern');
    act(() => first.result.current[1]('classic'));
    expect(first.result.current[0]).toBe('classic');
    first.unmount();
    expect(renderHook(() => useLatencyChartStyle()).result.current[0]).toBe('classic');
  });

  it('ignores a stored value it does not know', () => {
    localStorage.setItem('rp.latency.chartStyle', 'neon');
    expect(renderHook(() => useLatencyChartStyle()).result.current[0]).toBe('modern');
  });

  it('works without storage', () => {
    vi.spyOn(Storage.prototype, 'getItem').mockImplementation(() => {
      throw new Error('blocked');
    });
    vi.spyOn(Storage.prototype, 'setItem').mockImplementation(() => {
      throw new Error('blocked');
    });
    const { result } = renderHook(() => useLatencyChartStyle());
    expect(result.current[0]).toBe('modern');
    act(() => result.current[1]('classic'));
    expect(result.current[0]).toBe('classic');
  });
});
