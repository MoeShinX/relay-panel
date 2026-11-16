import { useMemo, useState } from 'react';
import { drawnKeys, pruneSelection } from '../utils/seriesPick';
import { MAX_SERIES } from '../utils/trafficBuckets';

/**
 * Which lines of a multi-line chart to draw.
 *
 * `keys` is the chart's series in its normal (stable) order and `ranked` the
 * same keys, most attention-worthy first. With ten or fewer series everything is
 * drawn and no picker is needed. With more, the pick is `null` (automatic: the
 * top ten of `ranked`) until the viewer chooses; their explicit list then
 * survives data and filter changes. Keys missing from the current data are
 * skipped, not forgotten: a node with no data in one range or metric is drawn
 * again once it has some. With none of the pick present it is automatic for
 * that view. Both arrays must be memoised.
 */
export function useSeriesPick(keys: string[], ranked: string[]) {
  const [picked, setPicked] = useState<string[] | null>(null);
  const explicit = useMemo(() => pruneSelection(picked, keys), [picked, keys]);

  const needsPicker = keys.length > MAX_SERIES;
  const shown = useMemo(
    () => (needsPicker ? drawnKeys(keys, explicit, ranked) : keys),
    [needsPicker, keys, explicit, ranked],
  );
  return { needsPicker, shown, automatic: explicit === null, setPicked };
}
