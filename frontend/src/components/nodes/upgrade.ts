/**
 * Shared node-upgrade eligibility logic — used by BOTH the desktop table and
 * the mobile list so the two views can't drift. Pure function: given a node row
 * and the latest NODE release to compare against (PR5: nodes compare against
 * `latest_node_version`, NOT the panel version), plus a failed-check flag,
 * return the upgrade state.
 *
 * The ladder (mirrors the desktop column exactly):
 * - no node_id            → 'none' (placeholder row; render "-")
 * - node-version check failed → 'checkFailed' (neutral "-", no green check)
 * - protocol incompatible → 'protocolIncompatible' (priority over version)
 * - version unknown       → 'unknown' (render "-", never a green check)
 * - same                  → 'latest' (green check, "up to date")
 * - ahead                 → 'ahead' (green check, "leading build" — never stale)
 * - behind + docker       → 'docker' (amber "update image" hint)
 * - behind + non-systemd  → 'manual' (grey "no supervisor")
 * - behind + systemd      → 'upgradeable' if online, else 'offline'
 * - …but online with no WS → 'noControlChannel' (v1.2.12): the command can
 *   only travel over the WS control channel, so offering the button would
 *   just end in an error after the click
 */
import type { NodeDisplayRow } from '../../api/types';
import { versionRelation } from '../../utils/version';

export type NodeUpgradeState =
  | 'none'
  | 'checkFailed'
  | 'protocolIncompatible'
  | 'unknown'
  | 'latest'
  | 'ahead'
  | 'docker'
  | 'manual'
  | 'upgradeable'
  | 'noControlChannel'
  | 'offline';

export interface NodeUpgrade {
  state: NodeUpgradeState;
}

/**
 * Resolve the upgrade affordance for one node row.
 *
 * `compareVersion` is the latest NODE release (NOT the panel version). When
 * `nodeVersionCheckFailed` is true the lookup failed and we MUST show a neutral
 * state (never a green check or an upgrade button based on a stale/empty value).
 *
 * 'latest' (node == compareVersion) and 'ahead' (node > compareVersion, e.g. a
 * development build) are distinct states so the caller can show different
 * tooltips ("up to date" vs "leading build"), but both render as a green check
 * and neither ever offers a downgrade.
 */
export function resolveNodeUpgrade(
  row: NodeDisplayRow,
  compareVersion: string,
  panelProtocol: number,
  nodeVersionCheckFailed: boolean,
): NodeUpgrade {
  // Placeholder row (no real node) → nothing to render.
  if (!row.node_id) return { state: 'none' };
  // Failed node-version check → neutral, never a green check / button.
  if (nodeVersionCheckFailed) return { state: 'checkFailed' };
  // Protocol-incompatible takes priority over any version status.
  const pv = row.config_protocol_version;
  if (pv != null && panelProtocol > 0 && pv !== panelProtocol) {
    return { state: 'protocolIncompatible' };
  }
  const rel = versionRelation(row.node_version, compareVersion);
  if (rel === 'unknown') return { state: 'unknown' };
  if (rel === 'ahead') return { state: 'ahead' };
  if (rel === 'same') return { state: 'latest' };
  // rel === 'behind' → the offer depends on how the node is installed.
  if (row.install_method === 'docker') return { state: 'docker' };
  if (row.install_method !== 'systemd') return { state: 'manual' };
  if (!row.online) return { state: 'offline' };
  return { state: row.ws_connected === false ? 'noControlChannel' : 'upgradeable' };
}


/**
 * v1.2.16: whether one-click upgrade may be offered for this node — the single
 * yes/no behind the per-node button AND the "upgrade all / upgrade group"
 * buttons. It is exactly `resolveNodeUpgrade(...).state === 'upgradeable'`
 * (online, systemd-managed, behind the latest node release, WS control channel
 * up, protocol compatible, version check succeeded), so the batch can never
 * offer a node the per-node button would not.
 */
export function canUpgradeNode(
  row: NodeDisplayRow,
  latestNodeVersion: string,
  panelProtocol: number,
  nodeVersionCheckFailed: boolean,
): boolean {
  return resolveNodeUpgrade(row, latestNodeVersion, panelProtocol, nodeVersionCheckFailed).state === 'upgradeable';
}

/** The rows of `rows` that `canUpgradeNode` accepts, in their original order. */
export function upgradableNodes(
  rows: NodeDisplayRow[],
  latestNodeVersion: string,
  panelProtocol: number,
  nodeVersionCheckFailed: boolean,
): NodeDisplayRow[] {
  return rows.filter((r) => canUpgradeNode(r, latestNodeVersion, panelProtocol, nodeVersionCheckFailed));
}
