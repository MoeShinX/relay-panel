import { useCallback, useState } from 'react';
import type { ChartVariant } from '../components/LatencyChart';

const KEY = 'rp.latency.chartStyle';

function read(): ChartVariant {
  try {
    return localStorage.getItem(KEY) === 'classic' ? 'classic' : 'modern';
  } catch {
    return 'modern';
  }
}

/** Which look the latency trend chart has, remembered per browser. Modern until
 *  switched; a browser that blocks storage just forgets it on reload. */
export function useLatencyChartStyle(): [ChartVariant, (v: ChartVariant) => void] {
  const [variant, setVariant] = useState<ChartVariant>(read);
  const set = useCallback((next: ChartVariant) => {
    setVariant(next);
    try {
      localStorage.setItem(KEY, next);
    } catch {
      /* storage blocked: keep the choice for this page only */
    }
  }, []);
  return [variant, set];
}
