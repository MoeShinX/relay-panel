use super::PgRepository;
use crate::db::error::DbError;
use crate::db::repo::*;
use async_trait::async_trait;
use relay_shared::protocol::TrafficEntry;

// ── TrafficRepository ──
//
// Same atomicity contract as SQLite (see sqlite_repo.rs). PG defaults to READ
// COMMITTED, so the ownership check + write on the same tx handle is the
// guarantee: a concurrent tx can't make our UPDATE see a different row than
// our SELECT did because both run on the same snapshot within this tx.
#[async_trait]
impl TrafficRepository for PgRepository {
    async fn apply_traffic_batch(
        &self,
        group_id: i64,
        entries: &[TrafficEntry],
    ) -> Result<Vec<TrafficEntryResult>, DbError> {
        let mut tx = self.pool.begin().await?;

        // ── v1.0.8: read this group's billing rate once for the whole batch
        // (every entry in a batch is for the SAME group_id). rate lives on
        // device_groups; users are CHARGED real * rate (rounded) while
        // forward_rules keeps real bytes. Missing group → rate=1.0 (defensive;
        // its rules will be rejected as Unavailable below anyway). ──
        let rate: f64 = sqlx::query_scalar("SELECT rate FROM device_groups WHERE id = $1")
            .bind(group_id)
            .fetch_optional(&mut *tx)
            .await?
            .flatten()
            .unwrap_or(1.0);
        if !(0.1..=100.0).contains(&rate) {
            let _ = tx.rollback().await;
            tracing::error!(
                "traffic_batch: group {} has out-of-range rate {} (expected 0.1..=100)",
                group_id,
                rate
            );
            return Ok(vec![TrafficEntryResult::Overflow]);
        }

        // ── Pass 1: validate u64→i64 per entry + aggregate duplicate rule_ids
        // into one per-rule delta (so the cumulative overflow check sees the
        // true batch total, not a per-row slice). ──
        let mut rule_delta: std::collections::HashMap<i64, (u64, u64)> =
            std::collections::HashMap::new();
        for entry in entries {
            if entry.upload > i64::MAX as u64 || entry.download > i64::MAX as u64 {
                let _ = tx.rollback().await;
                return Ok(vec![TrafficEntryResult::Overflow]);
            }
            let e = rule_delta.entry(entry.rule_id).or_insert((0, 0));
            e.0 = match e.0.checked_add(entry.upload) {
                Some(v) => v,
                None => {
                    let _ = tx.rollback().await;
                    return Ok(vec![TrafficEntryResult::Overflow]);
                }
            };
            e.1 = match e.1.checked_add(entry.download) {
                Some(v) => v,
                None => {
                    let _ = tx.rollback().await;
                    return Ok(vec![TrafficEntryResult::Overflow]);
                }
            };
        }

        // ── Pass 2: ownership + existing-value resolution.
        // SINGLE query per distinct rule_id, gated by device_group_in. A miss =
        // "not available" (missing OR foreign); NO second existence query (that
        // was the rule-id oracle). Reason logged server-side only.
        struct Resolved {
            rule_id: i64,
            uid: i64,
            delta_up: u64,
            delta_down: u64,
            /// v1.0.8: billed bytes charged to the USER = round((up+down) * rate).
            /// Separate from delta_up/delta_down (real bytes for the rule).
            billed_delta: i64,
        }
        let mut resolved: Vec<Resolved> = Vec::with_capacity(rule_delta.len());
        let mut user_delta: std::collections::HashMap<i64, i64> = std::collections::HashMap::new();
        for (rule_id, (dup, ddown)) in &rule_delta {
            let rule_delta_sum = match dup.checked_add(*ddown) {
                Some(v) if v <= i64::MAX as u64 => v as i64,
                _ => {
                    let _ = tx.rollback().await;
                    return Ok(vec![TrafficEntryResult::Overflow]);
                }
            };
            // JOIN users to fetch both the rule's and the user's current totals
            // in one round trip (same as the SQLite path).
            let row: Option<(i64, i64, i64, i64)> = sqlx::query_as(
                "SELECT fr.id, fr.uid, fr.traffic_used, u.traffic_used \
                 FROM forward_rules fr \
                 JOIN users u ON u.id = fr.uid \
                 WHERE fr.id = $1 AND fr.device_group_in = $2",
            )
            .bind(rule_id)
            .bind(group_id)
            .fetch_optional(&mut *tx)
            .await?;
            let Some((rid, uid, rule_used, user_used)) = row else {
                tracing::warn!(
                    "traffic_batch: rule {} not available to group {} \
                     (missing or foreign) — rejecting batch",
                    rule_id,
                    group_id
                );
                let _ = tx.rollback().await;
                return Ok(vec![TrafficEntryResult::Unavailable]);
            };
            // Per-rule cumulative overflow (REAL bytes — rate does not apply).
            if rule_used.checked_add(rule_delta_sum).is_none() {
                let _ = tx.rollback().await;
                return Ok(vec![TrafficEntryResult::Overflow]);
            }
            // v1.0.8: billed delta charged to the user = round(real * rate).
            let billed_raw = (rule_delta_sum as f64) * rate;
            let billed_delta = if billed_raw.is_finite() && billed_raw <= i64::MAX as f64 {
                billed_raw.round() as i64
            } else {
                let _ = tx.rollback().await;
                return Ok(vec![TrafficEntryResult::Overflow]);
            };
            // Per-user cumulative overflow: existing total + running batch delta
            // (BILLED bytes).
            let cur_user_delta = *user_delta.get(&uid).unwrap_or(&0);
            let new_user_delta = match cur_user_delta.checked_add(billed_delta) {
                Some(v) => v,
                None => {
                    let _ = tx.rollback().await;
                    return Ok(vec![TrafficEntryResult::Overflow]);
                }
            };
            if user_used.checked_add(new_user_delta).is_none() {
                let _ = tx.rollback().await;
                return Ok(vec![TrafficEntryResult::Overflow]);
            }
            user_delta.insert(uid, new_user_delta);
            resolved.push(Resolved {
                rule_id: rid,
                uid,
                delta_up: *dup,
                delta_down: *ddown,
                billed_delta,
            });
        }

        // ── Pass 3: apply writes (one UPDATE per distinct rule + its user).
        // v1.0.8: forward_rules += REAL bytes; users += BILLED bytes. ──
        for r in &resolved {
            let up = r.delta_up as i64;
            let down = r.delta_down as i64;
            sqlx::query(
                "UPDATE forward_rules SET traffic_used = traffic_used + $1 + $2 WHERE id = $3",
            )
            .bind(up)
            .bind(down)
            .bind(r.rule_id)
            .execute(&mut *tx)
            .await?;
            sqlx::query("UPDATE users SET traffic_used = traffic_used + $1 WHERE id = $2")
                .bind(r.billed_delta)
                .bind(r.uid)
                .execute(&mut *tx)
                .await?;
        }

        tx.commit().await?;
        Ok(vec![TrafficEntryResult::Ok])
    }
}
