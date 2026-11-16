import { useEffect, useMemo, useState } from 'react';
import { drawnKeys, pruneSelection } from '../utils/seriesPick';
import { MAX_SERIES } from '../utils/trafficBuckets';

const sameList = (a: string[] | null, b: string[] | null) =>
  a === b || (a != null && b != null && a.length === b.length && a.every((k, i) => k === b[i]));

/**
 * Which lines of a multi-line chart to draw.
 *
 * `keys` is the chart's series in its normal (stable) order and `ranked` the
 * same keys, most attention-worthy first. With ten or fewer series everything is
 * drawn and no picker is needed. With more, the pick is `null` (automatic: the
 * top ten of `ranked`) until the viewer chooses; their explicit list then
 * survives data and filter changes, losing only keys that no longer exist, and
 * falls back to automatic when none are left. Both arrays must be memoised.
 */
export function useSeriesPick(keys: string[], ranked: string[]) {
  const [picked, setPicked] = useState<string[] | null>(null);
  const explicit = useMemo(() => pruneSelection(picked, keys), [picked, keys]);

  // Forget keys that are gone for good, so they do not come back with a later
  // load that happens to have them again.
  useEffect(() => {
    if (!sameList(picked, explicit)) setPicked(explicit);
  }, [picked, explicit]);

  const needsPicker = keys.length > MAX_SERIES;
  const shown = useMemo(
    () => (needsPicker ? drawnKeys(keys, explicit, ranked) : keys),
    [needsPicker, keys, explicit, ranked],
  );
  return { needsPicker, shown, automatic: explicit === null, setPicked };
}
