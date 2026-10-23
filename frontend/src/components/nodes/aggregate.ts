/**
 * v0.4.17: dashboard node-status aggregation.
 *
 * The dashboard used to render every relay-node row individually, so a group
 * with several servers repeated its group id/name and wasted screen space.
 * This module collapses the raw /nodes payload into one summary per group,
 * keeping the per-node detail on the Node Status page.
 *
 * Pure functions, no React, so it can be unit-tested directly and imported
 * by the page without tripping react-refresh/only-export-components.
 */
import type { NodeStatus } from '../../api/types';

/** Roll-up status for one group, derived from its member nodes. */
export type GroupStatus = 'online' | 'partial' | 'offline';

export interface NodeGroupSummary {
  group_id: number;
  group_name: string;
  online_nodes: number;
  total_nodes: number;
  /** Sum of connections across ONLINE nodes only. */
  connections: number;
  /** Sum of live upload/download rates across ONLINE nodes only. */
  upload_bps: number;
  download_bps: number;
  /** Sum of cumulative traffic across ALL nodes still holding a status record
   *  (online or offline). A node whose status the backend cleared drops out. */
  upload_bytes: number;
  download_bytes: number;
  status: GroupStatus;
}

/**
 * Coerce any value into a finite non-negative number, or 0. Guards the
 * aggregations so a missing/NaN/Infinity/negative field never poisons a sum
 * into NaN. Accepts `unknown` because NodeStatus numeric fields are typed
 * number but older payloads can carry anything.
 */
export function safeNumber(value: unknown): number {
  return typeof value === 'number' && Number.isFinite(value) && value >= 0 ? value : 0;
}

/** online_nodes/total → roll-up status. Exposed for tests + reuse. */
export function groupStatus(online: number, total: number): GroupStatus {
  if (total <= 0) return 'offline';
  if (online === 0) return 'offline';
  if (online >= total) return 'online';
  return 'partial';
}

/**
 * Collapse raw node rows into one summary per group.
 *
 * Rules (per the v0.4.17 spec):
 *  - Group by group_id; output groups sorted by group_id ascending.
 *  - total_nodes = every row in the group; online_nodes = rows with
 *    online === true.
 *  - status = online | partial | offline from groupStatus().
 *  - connections / upload_bps / download_bps: summed over ONLINE nodes only.
 *  - upload_bytes / download_bytes: summed over ALL rows in the group (an
 *    offline node that still has a status record contributes its last
 *    reported cumulative bytes).
 *  - Every numeric field flows through safeNumber so the sums can never be NaN.
 */
export function aggregateNodesByGroup(rows: NodeStatus[]): NodeGroupSummary[] {
  const groups = new Map<number, NodeStatus[]>();
  for (const r of rows) {
    const arr = groups.get(r.group_id) ?? [];
    arr.push(r);
    groups.set(r.group_id, arr);
  }

  const out: NodeGroupSummary[] = [];
  const orderOf = new Map<number, number>();
  for (const [gid, members] of groups) {
    orderOf.set(gid, members[0]?.group_sort_order ?? Number.POSITIVE_INFINITY);
    const total = members.length;
    const onlineMembers = members.filter((m) => m.online === true);
    const online = onlineMembers.length;

    out.push({
      group_id: gid,
      group_name: members[0]?.group_name ?? '',
      online_nodes: online,
      total_nodes: total,
      connections: onlineMembers.reduce((s, m) => s + safeNumber(m.connections), 0),
      upload_bps: onlineMembers.reduce((s, m) => s + safeNumber(m.upload_bps), 0),
      download_bps: onlineMembers.reduce((s, m) => s + safeNumber(m.download_bps), 0),
      upload_bytes: members.reduce((s, m) => s + safeNumber(m.boot_upload_bytes), 0),
      download_bytes: members.reduce((s, m) => s + safeNumber(m.boot_download_bytes), 0),
      status: groupStatus(online, total),
    });
  }

  // The panel-wide group order (what the admin arranged); ties and groups
  // with no known order fall back to group_id.
  out.sort((a, b) => {
    const oa = orderOf.get(a.group_id)!;
    const ob = orderOf.get(b.group_id)!;
    if (oa !== ob) return oa < ob ? -1 : 1;
    return a.group_id - b.group_id;
  });
  return out;
}
