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
 * - same / ahead          → 'latest' (green check)
 * - behind + docker       → 'docker' (amber "update image" hint)
 * - behind + non-systemd  → 'manual' (grey "no supervisor")
 * - behind + systemd      → 'upgradeable' if online, else 'offline'
 */
import type { NodeDisplayRow } from '../../api/types';
import { versionRelation } from '../../utils/version';

export type NodeUpgradeState =
  | 'none'
  | 'checkFailed'
  | 'protocolIncompatible'
  | 'unknown'
  | 'latest'
  | 'docker'
  | 'manual'
  | 'upgradeable'
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
  if (rel !== 'behind') return { state: 'latest' };
  // Behind → the offer depends on how the node is installed.
  if (row.install_method === 'docker') return { state: 'docker' };
  if (row.install_method !== 'systemd') return { state: 'manual' };
  return { state: row.online ? 'upgradeable' : 'offline' };
}
