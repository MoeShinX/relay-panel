use super::SqliteRepository;
use crate::db::error::DbError;
use crate::db::repo::*;
use async_trait::async_trait;
use relay_shared::protocol::TrafficEntry;

// ── TrafficRepository ──
//
// Atomicity + security contract (v0.4.9 hardened):
//   - whole batch is one transaction (deferred BEGIN; SQLite serialises writers)
//   - rule NOT available to this node (missing OR foreign-group): ABORT +
//     rollback the entire batch, return Ok(vec![Unavailable]). The caller maps
//     that to a uniform 403 with a generic message. There is deliberately NO
//     distinction between "missing" and "foreign" — that distinction was a
//     rule-id existence oracle (a node could enumerate ids and tell from the
//     response whether an id exists in another group). The real reason is
//     logged server-side only, never returned to the node.
//   - overflow (per-entry, per-rule cumulative, per-user cumulative, or
//     existing value + delta): ABORT + rollback, return Ok(vec![Overflow]).
//     The caller maps that to a uniform 400.
//   - duplicate rule_ids in one batch are aggregated (summed) first, so the
//     overflow check sees the true batch delta and each distinct rule gets one
//     UPDATE.
//   - any UPDATE failure: ABORT + rollback, return Err(DbError).
//   - only after COMMIT succeeds do we return Ok(vec![Ok]).
#[async_trait]
impl TrafficRepository for SqliteRepository {
    async fn apply_traffic_batch(
        &self,
        group_id: i64,
        entries: &[TrafficEntry],
    ) -> Result<Vec<TrafficEntryResult>, DbError> {
        let mut tx = self.pool.begin().await?;

        // ── v1.0.8: read this group's billing rate once for the whole batch
        // (every entry in a batch is for the SAME group_id — the node reports
        // per-group). rate is stored on device_groups; users are CHARGED
        // real * rate (rounded), while forward_rules keeps real bytes. A group
        // missing here is treated as rate=1.0 (defensive: a deleted group mid-
        // batch shouldn't crash accounting — the per-rule ownership check below
        // will reject its rules as Unavailable anyway). ──
        let rate: f64 = sqlx::query_scalar("SELECT rate FROM device_groups WHERE id = ?")
            .bind(group_id)
            .fetch_optional(&mut *tx)
            .await?
            // Group deleted mid-batch → treat as rate=1.0 (its rules will be
            // rejected as Unavailable in Pass 2 anyway; don't crash accounting).
            .flatten()
            .unwrap_or(1.0);
        if !(0.1..=100.0).contains(&rate) {
            // Out-of-range rate is a data integrity bug; refuse the batch
            // rather than silently billing a wrong amount.
            let _ = tx.rollback().await;
            tracing::error!(
                "traffic_batch: group {} has out-of-range rate {} (expected 0.1..=100)",
                group_id,
                rate
            );
            return Ok(vec![TrafficEntryResult::Overflow]);
        }

        // ── Pass 1: validate u64→i64 per entry (a single entry's upload or
        // download alone can exceed i64::MAX; reject before any DB read). ──
        // Aggregate duplicate rule_ids INTO ONE delta first so the per-rule
        // overflow check below sees the true batch total, not a per-row slice.
        // (Rule keyed by id; we resolve owner/uid in pass 2.)
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
        // SINGLE query per distinct rule_id: id+uid gated by device_group_in.
        // A miss means "not available to this node" (missing OR foreign) — we
        // do NOT run a second "does this id exist elsewhere?" query; that was
        // the rule-id existence oracle. The reason is logged, not returned.
        struct Resolved {
            rule_id: i64,
            uid: i64,
            delta_up: u64,
            delta_down: u64,
            /// v1.0.8: billed bytes charged to the USER = round((up+down) * rate).
            /// Kept separate from delta_up/delta_down (real bytes for the rule).
            billed_delta: i64,
        }
        let mut resolved: Vec<Resolved> = Vec::with_capacity(rule_delta.len());
        // Track the per-USER aggregate delta (a user may own several rules in
        // this batch) for the cumulative overflow check.
        let mut user_delta: std::collections::HashMap<i64, i64> = std::collections::HashMap::new();
        for (rule_id, (dup, ddown)) in &rule_delta {
            // The rule's own delta must fit in i64 (upload+download summed).
            let rule_delta_sum = match dup.checked_add(*ddown) {
                Some(v) if v <= i64::MAX as u64 => v as i64,
                _ => {
                    let _ = tx.rollback().await;
                    return Ok(vec![TrafficEntryResult::Overflow]);
                }
            };
            let row: Option<(i64, i64, i64, i64)> = sqlx::query_as(
                "SELECT fr.id, fr.uid, fr.traffic_used, u.traffic_used \
                 FROM forward_rules fr \
                 JOIN users u ON u.id = fr.uid \
                 WHERE fr.id = ? AND fr.device_group_in = ?",
            )
            .bind(rule_id)
            .bind(group_id)
            .fetch_optional(&mut *tx)
            .await?;
            let Some((rid, uid, rule_used, user_used)) = row else {
                // Not available: missing OR foreign. Log the id (server-side
                // only) and roll the whole batch back with a uniform 403.
                tracing::warn!(
                    "traffic_batch: rule {} not available to group {} \
                     (missing or foreign) — rejecting batch",
                    rule_id,
                    group_id
                );
                let _ = tx.rollback().await;
                return Ok(vec![TrafficEntryResult::Unavailable]);
            };
            // Per-rule cumulative overflow: existing + this batch's delta.
            // Rule statistics are REAL bytes (rate does not apply here).
            if rule_used.checked_add(rule_delta_sum).is_none() {
                let _ = tx.rollback().await;
                return Ok(vec![TrafficEntryResult::Overflow]);
            }
            // v1.0.8: billed delta charged to the user = round(real * rate).
            // f64 mul can't overflow (rule_delta_sum ≤ i64::MAX, rate ≤ 100),
            // but the rounded result must fit in i64 — guard it.
            let billed_raw = (rule_delta_sum as f64) * rate;
            let billed_delta = if billed_raw.is_finite() && billed_raw <= i64::MAX as f64 {
                billed_raw.round() as i64
            } else {
                let _ = tx.rollback().await;
                return Ok(vec![TrafficEntryResult::Overflow]);
            };
            // Per-user cumulative: existing user total + (running user delta),
            // charged in BILLED bytes (rate applied).
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

        // ── Pass 3: apply writes for every resolved rule. We resolved against
        // the SAME tx, so a concurrent DELETE between passes still produces a
        // 0-rows-affected UPDATE (not an error). Duplicate rule_ids are already
        // aggregated, so each distinct rule gets ONE UPDATE (fewer SQL round
        // trips + no double-counting).
        // v1.0.8: forward_rules += REAL bytes (up+down); users += BILLED bytes
        // (billed_delta = round((up+down) * rate)). ──
        // v1.2.0: one hour bucket for the whole batch. Nodes report every ~10s,
        // so history MUST accumulate into (rule, hour) rows — an insert per
        // report would be ~8.6k rows/rule/day. Computed once so every rule in
        // the batch lands in the same bucket even across an hour rollover
        // mid-loop.
        let hour_ts = chrono::Utc::now().format("%Y-%m-%d %H:00:00").to_string();

        for r in &resolved {
            let up = r.delta_up as i64;
            let down = r.delta_down as i64;
            sqlx::query(
                "UPDATE forward_rules SET traffic_used = traffic_used + ? + ? WHERE id = ?",
            )
            .bind(up)
            .bind(down)
            .bind(r.rule_id)
            .execute(&mut *tx)
            .await?;
            sqlx::query("UPDATE users SET traffic_used = traffic_used + ? WHERE id = ?")
                .bind(r.billed_delta)
                .bind(r.uid)
                .execute(&mut *tx)
                .await?;
            // v1.2.0: history accumulates the SAME billed_delta charged to the
            // user above — inside the same tx, so the chart and the quota can
            // never disagree, not even by a crashed half-batch.
            sqlx::query(
                "INSERT INTO traffic_history \
                   (rule_id, uid, group_id, hour_ts, real_upload, real_download, billed_total) \
                 VALUES (?, ?, ?, ?, ?, ?, ?) \
                 ON CONFLICT(rule_id, hour_ts) DO UPDATE SET \
                   real_upload = real_upload + excluded.real_upload, \
                   real_download = real_download + excluded.real_download, \
                   billed_total = billed_total + excluded.billed_total, \
                   group_id = excluded.group_id",
            )
            .bind(r.rule_id)
            .bind(r.uid)
            // v1.2.0: the batch's group. A rule belongs to exactly one inbound
            // group, and pass 2 already verified ownership against THIS group,
            // so it is the rule's group by construction. Refreshed on conflict
            // so a row the backfill left at 0 self-heals on the next report.
            .bind(group_id)
            .bind(&hour_ts)
            .bind(up)
            .bind(down)
            .bind(r.billed_delta)
            .execute(&mut *tx)
            .await?;
        }

        tx.commit().await?;
        Ok(vec![TrafficEntryResult::Ok])
    }

    async fn query_traffic_history(
        &self,
        uid: Option<i64>,
        rule_id: Option<i64>,
        since: &str,
        daily: bool,
    ) -> Result<Vec<TrafficHistoryBucket>, DbError> {
        // Two statements rather than a parameterised bucket expression — the
        // GROUP BY column can't be a bind parameter. COALESCE folds the two
        // optional filters into one prepared statement each (the statistics
        // query's trick).
        //
        // v1.2.0: grouped by (bucket, line). The LEFT JOIN resolves the name
        // for the legend but never gates the row — a deleted group must still
        // show its history, labelled "#id", rather than vanishing from the
        // chart (which is the whole reason group_id is a stored snapshot).
        let sql = if daily {
            "SELECT substr(th.hour_ts, 1, 10) AS bucket, \
                    th.group_id AS group_id, \
                    COALESCE(dg.name, '#' || th.group_id) AS group_name, \
                    SUM(th.real_upload) AS real_upload, \
                    SUM(th.real_download) AS real_download, \
                    SUM(th.billed_total) AS billed_total \
             FROM traffic_history th \
             LEFT JOIN device_groups dg ON dg.id = th.group_id \
             WHERE th.hour_ts >= ? AND th.uid = COALESCE(?, th.uid) \
               AND th.rule_id = COALESCE(?, th.rule_id) \
             GROUP BY bucket, th.group_id, group_name ORDER BY bucket, th.group_id"
        } else {
            "SELECT th.hour_ts AS bucket, \
                    th.group_id AS group_id, \
                    COALESCE(dg.name, '#' || th.group_id) AS group_name, \
                    SUM(th.real_upload) AS real_upload, \
                    SUM(th.real_download) AS real_download, \
                    SUM(th.billed_total) AS billed_total \
             FROM traffic_history th \
             LEFT JOIN device_groups dg ON dg.id = th.group_id \
             WHERE th.hour_ts >= ? AND th.uid = COALESCE(?, th.uid) \
               AND th.rule_id = COALESCE(?, th.rule_id) \
             GROUP BY bucket, th.group_id, group_name ORDER BY bucket, th.group_id"
        };
        Ok(sqlx::query_as(sql)
            .bind(since)
            .bind(uid)
            .bind(rule_id)
            .fetch_all(&self.pool)
            .await?)
    }

    async fn prune_traffic_history(&self, cutoff: &str) -> Result<u64, DbError> {
        Ok(sqlx::query("DELETE FROM traffic_history WHERE hour_ts < ?")
            .bind(cutoff)
            .execute(&self.pool)
            .await?
            .rows_affected())
    }
}
