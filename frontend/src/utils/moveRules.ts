/**
 * Batch "move rules to another inbound group".
 *
 * There is no bulk endpoint: each rule goes through `PUT /rules/{id}` with just
 * `device_group_in`, the same call the edit form makes, so ownership, group
 * authorization and protocol checks all stay server-side. A listen port is
 * unique per inbound group, so a rule whose port is already taken in the target
 * group comes back 409 — that one is skipped (left where it was) and reported,
 * and the rest carry on.
 *
 * Rules are moved one after another, not in parallel: two selected rules that
 * want the same port in the target group then resolve the same way every time
 * (the first in the list gets it) instead of racing.
 */
import type { ApiEnvelope, ForwardRule } from '../api/types';

export interface MoveFailure {
  rule: ForwardRule;
  message: string;
}

export interface MoveResult {
  moved: ForwardRule[];
  /** Listen port already in use in the target group; rule was left untouched. */
  occupied: ForwardRule[];
  /** Already in the target group; nothing to do. */
  alreadyThere: ForwardRule[];
  /** Any other refusal (not authorized, not found, ...). */
  failed: MoveFailure[];
}

type Put = (url: string, body: unknown) => Promise<ApiEnvelope<null>>;

export async function moveRules(
  rules: ForwardRule[],
  targetGroupId: number,
  put: Put,
): Promise<MoveResult> {
  const result: MoveResult = { moved: [], occupied: [], alreadyThere: [], failed: [] };
  for (const rule of rules) {
    if (rule.device_group_in === targetGroupId) {
      result.alreadyThere.push(rule);
      continue;
    }
    try {
      const res = await put(`/rules/${rule.id}`, { device_group_in: targetGroupId });
      if (res.code === 0) result.moved.push(rule);
      else if (res.code === 409) result.occupied.push(rule);
      else result.failed.push({ rule, message: res.message });
    } catch (e) {
      result.failed.push({ rule, message: e instanceof Error ? e.message : String(e) });
    }
  }
  return result;
}
