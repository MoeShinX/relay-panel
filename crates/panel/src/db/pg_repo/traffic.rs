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
        // v1.2.0: one hour bucket for the whole batch (see the SQLite impl for
        // why history accumulates per (rule, hour) rather than per report).
        let hour_ts = chrono::Utc::now().format("%Y-%m-%d %H:00:00").to_string();
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
            // v1.2.0: same billed_delta as the user charge above, in the same
            // tx — the history chart can never disagree with the quota.
            sqlx::query(
                "INSERT INTO traffic_history \
                   (rule_id, uid, group_id, hour_ts, real_upload, real_download, billed_total) \
                 VALUES ($1, $2, $3, $4, $5, $6, $7) \
                 ON CONFLICT (rule_id, hour_ts) DO UPDATE SET \
                   real_upload = traffic_history.real_upload + EXCLUDED.real_upload, \
                   real_download = traffic_history.real_download + EXCLUDED.real_download, \
                   billed_total = traffic_history.billed_total + EXCLUDED.billed_total, \
                   group_id = EXCLUDED.group_id",
            )
            .bind(r.rule_id)
            .bind(r.uid)
            // v1.2.0: the batch's group — the rule's group by construction
            // (pass 2 verified ownership against it). Refreshed on conflict so
            // a row the backfill left at 0 self-heals on the next report.
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
        // SUM(bigint) is NUMERIC in PG — cast back to BIGINT or sqlx can't
        // decode into the i64 fields of TrafficHistoryBucket.
        //
        // v1.2.0: grouped by (bucket, line). The LEFT JOIN resolves the legend
        // name but never gates the row — a deleted group must keep showing its
        // history as "#id" rather than disappearing from the chart.
        let sql = if daily {
            "SELECT substr(th.hour_ts, 1, 10) AS bucket, \
                    th.group_id AS group_id, \
                    COALESCE(dg.name, '#' || th.group_id) AS group_name, \
                    SUM(th.real_upload)::BIGINT AS real_upload, \
                    SUM(th.real_download)::BIGINT AS real_download, \
                    SUM(th.billed_total)::BIGINT AS billed_total \
             FROM traffic_history th \
             LEFT JOIN device_groups dg ON dg.id = th.group_id \
             WHERE th.hour_ts >= $1 AND th.uid = COALESCE($2, th.uid) \
               AND th.rule_id = COALESCE($3, th.rule_id) \
             GROUP BY bucket, th.group_id, dg.name ORDER BY bucket, th.group_id"
        } else {
            "SELECT th.hour_ts AS bucket, \
                    th.group_id AS group_id, \
                    COALESCE(dg.name, '#' || th.group_id) AS group_name, \
                    SUM(th.real_upload)::BIGINT AS real_upload, \
                    SUM(th.real_download)::BIGINT AS real_download, \
                    SUM(th.billed_total)::BIGINT AS billed_total \
             FROM traffic_history th \
             LEFT JOIN device_groups dg ON dg.id = th.group_id \
             WHERE th.hour_ts >= $1 AND th.uid = COALESCE($2, th.uid) \
               AND th.rule_id = COALESCE($3, th.rule_id) \
             GROUP BY bucket, th.group_id, dg.name ORDER BY bucket, th.group_id"
        };
        Ok(sqlx::query_as(sql)
            .bind(since)
            .bind(uid)
            .bind(rule_id)
            .fetch_all(&self.pool)
            .await?)
    }

    async fn prune_traffic_history(&self, cutoff: &str) -> Result<u64, DbError> {
        Ok(
            sqlx::query("DELETE FROM traffic_history WHERE hour_ts < $1")
                .bind(cutoff)
                .execute(&self.pool)
                .await?
                .rows_affected(),
        )
    }

    async fn record_node_metrics(&self, m: &NodeMetricSample) -> Result<(), DbError> {
        // Mirrors the SQLite UPSERT (see there for why sums + maxima are both
        // kept). PG spells the pairwise maximum GREATEST, not MAX — MAX is the
        // aggregate here and would be a type error in this position.
        sqlx::query(
            "INSERT INTO node_metrics_history                  (node_id, group_id, hour_ts, samples, cpu_sum, cpu_max,                   mem_sum, mem_max, conn_sum, conn_max)              VALUES ($1, $2, $3, 1, $4, $5, $6, $7, $8, $9)              ON CONFLICT (node_id, hour_ts) DO UPDATE SET                  group_id = EXCLUDED.group_id,                  samples  = node_metrics_history.samples + 1,                  cpu_sum  = node_metrics_history.cpu_sum + EXCLUDED.cpu_sum,                  cpu_max  = GREATEST(node_metrics_history.cpu_max, EXCLUDED.cpu_max),                  mem_sum  = node_metrics_history.mem_sum + EXCLUDED.mem_sum,                  mem_max  = GREATEST(node_metrics_history.mem_max, EXCLUDED.mem_max),                  conn_sum = node_metrics_history.conn_sum + EXCLUDED.conn_sum,                  conn_max = GREATEST(node_metrics_history.conn_max, EXCLUDED.conn_max)",
        )
        .bind(&m.node_id)
        .bind(m.group_id)
        .bind(&m.hour_ts)
        .bind(m.cpu)
        .bind(m.cpu)
        .bind(m.mem)
        .bind(m.mem)
        .bind(m.connections)
        .bind(m.connections)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn query_node_metrics(
        &self,
        since: &str,
        daily: bool,
    ) -> Result<Vec<NodeMetricBucket>, DbError> {
        // See the SQLite version for why the averages are derived from
        // sum/samples rather than stored. GREATEST(..., 1) guards the divide;
        // samples can only be 0 for a row that was never updated, but a
        // division by zero here would take down the whole chart.
        let sql = if daily {
            "SELECT LEFT(nm.hour_ts, 10) AS bucket,                     nm.node_id AS node_id,                     nm.group_id AS group_id,                     COALESCE(dg.name, '#' || nm.group_id::text) AS group_name,                     SUM(nm.cpu_sum) / GREATEST(SUM(nm.samples), 1) AS cpu_avg,                     MAX(nm.cpu_max) AS cpu_max,                     SUM(nm.mem_sum) / GREATEST(SUM(nm.samples), 1) AS mem_avg,                     MAX(nm.mem_max) AS mem_max,                     SUM(nm.conn_sum)::double precision                         / GREATEST(SUM(nm.samples), 1) AS conn_avg,                     MAX(nm.conn_max) AS conn_max              FROM node_metrics_history nm              LEFT JOIN device_groups dg ON dg.id = nm.group_id              WHERE nm.hour_ts >= $1              GROUP BY bucket, nm.node_id, nm.group_id, group_name              ORDER BY bucket, nm.node_id"
        } else {
            "SELECT nm.hour_ts AS bucket,                     nm.node_id AS node_id,                     nm.group_id AS group_id,                     COALESCE(dg.name, '#' || nm.group_id::text) AS group_name,                     nm.cpu_sum / GREATEST(nm.samples, 1) AS cpu_avg,                     nm.cpu_max AS cpu_max,                     nm.mem_sum / GREATEST(nm.samples, 1) AS mem_avg,                     nm.mem_max AS mem_max,                     nm.conn_sum::double precision                         / GREATEST(nm.samples, 1) AS conn_avg,                     nm.conn_max AS conn_max              FROM node_metrics_history nm              LEFT JOIN device_groups dg ON dg.id = nm.group_id              WHERE nm.hour_ts >= $1              ORDER BY nm.hour_ts, nm.node_id"
        };
        Ok(sqlx::query_as(sql)
            .bind(since)
            .fetch_all(&self.pool)
            .await?)
    }

    async fn prune_node_metrics(&self, cutoff: &str) -> Result<u64, DbError> {
        Ok(
            sqlx::query("DELETE FROM node_metrics_history WHERE hour_ts < $1")
                .bind(cutoff)
                .execute(&self.pool)
                .await?
                .rows_affected(),
        )
    }

    async fn record_audit(&self, e: &NewAuditEntry) -> Result<(), DbError> {
        sqlx::query(
            "INSERT INTO audit_log                  (ts, actor_id, actor_name, action, target_type, target_id, detail)              VALUES ($1, $2, $3, $4, $5, $6, $7)",
        )
        .bind(&e.ts)
        .bind(e.actor_id)
        .bind(&e.actor_name)
        .bind(&e.action)
        .bind(&e.target_type)
        .bind(&e.target_id)
        .bind(&e.detail)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn query_audit_log(
        &self,
        action: Option<&str>,
        limit: i64,
        offset: i64,
    ) -> Result<Vec<AuditEntry>, DbError> {
        // Ordered by id, not ts: several entries can share a second, and only
        // the id gives a stable total order for pagination.
        Ok(sqlx::query_as(
            "SELECT id, ts, actor_id, actor_name, action, target_type, target_id, detail              FROM audit_log              WHERE action = COALESCE($1, action)              ORDER BY id DESC LIMIT $2 OFFSET $3",
        )
        .bind(action)
        .bind(limit)
        .bind(offset)
        .fetch_all(&self.pool)
        .await?)
    }

    async fn count_audit_log(&self, action: Option<&str>) -> Result<i64, DbError> {
        let (n,): (i64,) =
            sqlx::query_as("SELECT COUNT(*) FROM audit_log WHERE action = COALESCE($1, action)")
                .bind(action)
                .fetch_one(&self.pool)
                .await?;
        Ok(n)
    }

    async fn prune_audit_log(&self, cutoff: &str) -> Result<u64, DbError> {
        Ok(sqlx::query("DELETE FROM audit_log WHERE ts < $1")
            .bind(cutoff)
            .execute(&self.pool)
            .await?
            .rows_affected())
    }
}
