import { formatPercent } from './format';

/** The metrics the node history chart can display. */
export type Metric = 'cpu' | 'mem' | 'conn';

/**
 * Which group names have more than one node reporting.
 *
 * The short node id is a disambiguator, not information: it is the first six
 * hex characters of a random id the node generates on first boot, so it means
 * nothing to a human reading the legend. Appending it to a single-node group is
 * pure noise — and single-node groups are the common case.
 */
export function multiNodeGroupNames(
  buckets: { group_name: string; node_id: string }[],
): Set<string> {
  const seen = new Map<string, Set<string>>();
  for (const b of buckets) {
    if (!seen.has(b.group_name)) seen.set(b.group_name, new Set());
    seen.get(b.group_name)!.add(b.node_id);
  }
  return new Set(
    Array.from(seen.entries())
      .filter(([, ids]) => ids.size > 1)
      .map(([name]) => name),
  );
}

/** The legend entry for one bucket, given the set from multiNodeGroupNames.
 *  Within a group of several nodes a node is told apart by the name its admin
 *  gave it (`labels`, keyed `{group_id}:{node_id}`), or failing that by the
 *  head of its id. */
export function seriesName(
  b: { group_name: string; node_id: string; group_id?: number },
  multi: Set<string>,
  labels?: Record<string, string>,
): string {
  if (!multi.has(b.group_name)) return b.group_name;
  const label = b.group_id != null ? labels?.[`${b.group_id}:${b.node_id}`] : undefined;
  return `${b.group_name}/${label || b.node_id.slice(0, 6)}`;
}

/**
 * Axis and tooltip formatting.
 *
 * CPU and memory arrive as PERCENTAGES (0..100), not fractions — the node
 * reports `global_cpu_usage()` and a `used/total*100` memory figure, and the
 * panel stores both unscaled. Multiplying by 100 here is what produced
 * "2026.7%" readings; `formatPercent` is the same helper the node tables and
 * detail drawer use, so all four surfaces now agree by construction.
 */
export function formatMetric(metric: Metric, v: number): string {
  return metric === 'conn' ? String(Math.round(v)) : formatPercent(v);
}
