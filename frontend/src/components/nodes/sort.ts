/**
 * v0.4.16 PR3: stable, order-independent node-row sorting for the node-status
 * board.
 *
 * Problem: the panel groups come from a KVS scan whose return order is not
 * guaranteed, and within a group multiple nodes can arrive in any order. The
 * old `groupByGroupId` preserved first-seen order, so the board could reorder
 * itself on every 5s refresh — visually flickering and making it hard to read.
 *
 * Fix: these pure functions impose a deterministic order that depends only on
 * the row DATA, never on arrival order. The same input set always yields the
 * same output. Kept in a logic-only module (no React) so it can be unit-tested
 * directly and imported by the page without tripping the
 * react-refresh/only-export-components rule.
 */
import { arrayMove } from '@dnd-kit/sortable';
import type { NodeDisplayRow } from '../../api/types';

/**
 * Compare two node rows for STABLE within-group ordering. Sort keys (first
 * difference wins):
 *   0. the admin's label, when the node has one — "香港 1号" before "香港 2号",
 *      "HK-2" before "HK-10" (digits compare as numbers) — and labelled nodes
 *      before unlabelled ones: a name the admin chose is the order they want
 *   1. public_ipv4 (falling back to legacy public_ip)
 *   2. public_ipv6
 *   3. node_id
 * Missing/empty values sort LAST (so a node that hasn't reported its IP yet
 * doesn't jump around above named nodes). Past the label the comparison is a
 * plain string compare — it just needs to be deterministic, not
 * human-meaningful.
 */
export function compareNodeRows<T extends NodeDisplayRow>(a: T, b: T): number {
  const al = a.label ?? '';
  const bl = b.label ?? '';
  if (al !== bl) {
    if (!al || !bl) return blankLast(al, bl);
    const byName = LABEL_COLLATOR.compare(al, bl);
    if (byName !== 0) return byName;
  }
  const av4 = a.public_ipv4 ?? a.public_ip ?? '';
  const bv4 = b.public_ipv4 ?? b.public_ip ?? '';
  if (av4 !== bv4) return blankLast(av4, bv4);
  const av6 = a.public_ipv6 ?? '';
  const bv6 = b.public_ipv6 ?? '';
  if (av6 !== bv6) return blankLast(av6, bv6);
  const aid = a.node_id ?? '';
  const bid = b.node_id ?? '';
  return blankLast(aid, bid);
}

/** Labels compare the way a person reads them: digits as numbers, case ignored. */
const LABEL_COLLATOR = new Intl.Collator(undefined, { numeric: true, sensitivity: 'base' });

/** Empty string sorts after non-empty (blank-last). Otherwise lexical. */
function blankLast(a: string, b: string): number {
  if (!a && !b) return 0;
  if (!a) return 1;
  if (!b) return -1;
  return a < b ? -1 : a > b ? 1 : 0;
}

/**
 * Stable, order-independent view of the node rows. Returns groups in the
 * panel-wide display order the server gives each row (`group_sort_order`, which
 * the admin sets by dragging; ties and missing values fall back to group_id,
 * and a group whose record outlived it — no order — sorts after the rest), each
 * group's rows sorted by compareNodeRows. Does NOT depend on the order the
 * API/KVS returned rows in.
 */
export function stableGroupedRows<T extends NodeDisplayRow>(
  rows: T[],
): Array<[number, T[]]> {
  const m = new Map<number, T[]>();
  for (const r of rows) {
    const arr = m.get(r.group_id) ?? [];
    arr.push(r);
    m.set(r.group_id, arr);
  }
  // Sort each group's rows stably, then emit groups by display order.
  for (const arr of m.values()) arr.sort(compareNodeRows);
  const orderOf = (rows: T[]) => rows[0]?.group_sort_order ?? Number.POSITIVE_INFINITY;
  return Array.from(m.entries()).sort((x, y) => {
    const ox = orderOf(x[1]);
    const oy = orderOf(y[1]);
    // Infinity - Infinity is NaN, so compare explicitly.
    if (ox !== oy) return ox < oy ? -1 : 1;
    return x[0] - y[0];
  });
}

/** Is this group's display order known (i.e. does the group still exist)? Only
 *  those can be reordered; a leftover record of a deleted group cannot. */
export function hasGroupOrder(rows: NodeDisplayRow[]): boolean {
  return rows[0]?.group_sort_order != null;
}

/**
 * Move the group `activeId` to where `overId` is: an INSERT, not a swap. The
 * others between the two positions shift by one — [A,B,C,D,E] with D dropped on
 * B gives [A,D,B,C,E]. Dragging down is the mirror image. Returns the same
 * array when nothing moves (either id missing, or dropped on itself).
 */
export function moveGroup(ids: number[], activeId: number, overId: number): number[] {
  const from = ids.indexOf(activeId);
  const to = ids.indexOf(overId);
  if (from < 0 || to < 0 || from === to) return ids;
  return arrayMove(ids, from, to);
}

/**
 * Turn the order of the groups the page shows into the order of ALL groups.
 * The page lists only groups that have a node record, but the server orders
 * every group, so the shown ones take over the slots the shown ones held and
 * the hidden ones stay exactly where they were. Ids in `shown` that `full` does
 * not know (deleted meanwhile) are dropped.
 */
export function mergeVisibleOrder(full: number[], shown: number[]): number[] {
  const known = new Set(full);
  const queue = shown.filter((id) => known.has(id));
  const slots = new Set(queue);
  let next = 0;
  return full.map((id) => (slots.has(id) ? queue[next++] : id));
}

/** The rows with the given groups' display order replaced by their position in
 *  `ids` (1-based), so the page shows a new order at once, before the server
 *  has answered. Rows of other groups are untouched. */
export function withGroupOrder<T extends NodeDisplayRow>(rows: T[], ids: number[]): T[] {
  const pos = new Map(ids.map((id, i) => [id, i + 1]));
  return rows.map((r) => {
    const p = pos.get(r.group_id);
    return p === undefined ? r : { ...r, group_sort_order: p };
  });
}
