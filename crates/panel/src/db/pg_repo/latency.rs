use super::PgRepository;
use crate::db::error::DbError;
use crate::db::repo::*;
use async_trait::async_trait;
use std::collections::BTreeSet;

// ── LatencyRepository ──
//
// Same contract as the SQLite implementation; see `LatencyRepository`.

#[async_trait]
impl LatencyRepository for PgRepository {
    async fn list_latency_targets(&self) -> Result<Vec<LatencyTarget>, DbError> {
        Ok(sqlx::query_as(
            "SELECT id, name, carrier, kind, host, port, sort_order, created_at
             FROM latency_targets ORDER BY sort_order, id",
        )
        .fetch_all(&self.pool)
        .await?)
    }

    async fn replace_latency_targets(
        &self,
        now: &str,
        inputs: &[LatencyTargetInput],
    ) -> Result<Option<Vec<LatencyTarget>>, DbError> {
        let mut tx = self.pool.begin().await?;

        // Lock the rows being edited so two concurrent replaces take turns.
        let existing: Vec<(i64,)> = sqlx::query_as("SELECT id FROM latency_targets FOR UPDATE")
            .fetch_all(&mut *tx)
            .await?;
        let existing: BTreeSet<i64> = existing.into_iter().map(|(id,)| id).collect();
        let kept: BTreeSet<i64> = inputs.iter().filter_map(|i| i.id).collect();
        if kept.iter().any(|id| !existing.contains(id)) {
            // Dropping the transaction rolls it back: nothing was written yet.
            return Ok(None);
        }

        // A deleted target takes its history with it — the admin decided it
        // is gone, and rows nothing can name any more are only clutter.
        for id in existing.difference(&kept) {
            for table in ["latency_minute", "latency_hourly", "latency_targets"] {
                let col = if table == "latency_targets" {
                    "id"
                } else {
                    "target_id"
                };
                sqlx::query(&format!("DELETE FROM {table} WHERE {col} = $1"))
                    .bind(id)
                    .execute(&mut *tx)
                    .await?;
            }
        }

        for (pos, input) in inputs.iter().enumerate() {
            let order = pos as i32;
            match input.id {
                Some(id) => {
                    // Pointing a target somewhere else (another host, port or
                    // kind) starts its history over: the old rows measured a
                    // different thing and must not join the new line. A rename
                    // or a new carrier keeps them.
                    for table in ["latency_minute", "latency_hourly"] {
                        sqlx::query(&format!(
                            "DELETE FROM {table} WHERE target_id = $1 AND EXISTS (
                                 SELECT 1 FROM latency_targets WHERE id = $1
                                 AND (kind IS DISTINCT FROM $2 OR host IS DISTINCT FROM $3
                                      OR port IS DISTINCT FROM $4))"
                        ))
                        .bind(id)
                        .bind(&input.kind)
                        .bind(&input.host)
                        .bind(input.port)
                        .execute(&mut *tx)
                        .await?;
                    }
                    sqlx::query(
                        "UPDATE latency_targets
                         SET name = $1, carrier = $2, kind = $3, host = $4, port = $5,
                             sort_order = $6
                         WHERE id = $7",
                    )
                    .bind(&input.name)
                    .bind(&input.carrier)
                    .bind(&input.kind)
                    .bind(&input.host)
                    .bind(input.port)
                    .bind(order)
                    .bind(id)
                    .execute(&mut *tx)
                    .await?;
                }
                None => {
                    sqlx::query(
                        "INSERT INTO latency_targets
                             (name, carrier, kind, host, port, sort_order, created_at)
                         VALUES ($1, $2, $3, $4, $5, $6, $7)",
                    )
                    .bind(&input.name)
                    .bind(&input.carrier)
                    .bind(&input.kind)
                    .bind(&input.host)
                    .bind(input.port)
                    .bind(order)
                    .bind(now)
                    .execute(&mut *tx)
                    .await?;
                }
            }
        }

        let list: Vec<LatencyTarget> = sqlx::query_as(
            "SELECT id, name, carrier, kind, host, port, sort_order, created_at
             FROM latency_targets ORDER BY sort_order, id",
        )
        .fetch_all(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(Some(list))
    }

    async fn record_latency_minutes(
        &self,
        node_id: &str,
        rows: &[LatencyMinuteRow],
    ) -> Result<(), DbError> {
        if rows.is_empty() {
            return Ok(());
        }
        let mut tx = self.pool.begin().await?;
        let mut hours: BTreeSet<(i64, i64)> = BTreeSet::new();
        for r in rows {
            // The newest report of a minute replaces the stored one.
            sqlx::query(
                "INSERT INTO latency_minute
                     (node_id, target_id, minute_ts, sent, lost, rtt_sum_ms, rtt_min_ms, rtt_max_ms)
                 VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
                 ON CONFLICT (node_id, target_id, minute_ts) DO UPDATE SET
                     sent = EXCLUDED.sent,
                     lost = EXCLUDED.lost,
                     rtt_sum_ms = EXCLUDED.rtt_sum_ms,
                     rtt_min_ms = EXCLUDED.rtt_min_ms,
                     rtt_max_ms = EXCLUDED.rtt_max_ms",
            )
            .bind(node_id)
            .bind(r.target_id)
            .bind(r.minute_ts)
            .bind(r.sent)
            .bind(r.lost)
            .bind(r.rtt_sum_ms)
            .bind(r.rtt_min_ms)
            .bind(r.rtt_max_ms)
            .execute(&mut *tx)
            .await?;
            hours.insert((r.target_id, r.minute_ts.div_euclid(3600) * 3600));
        }
        // The hourly row is rebuilt from the minute rows of its hour rather
        // than added to: re-storing a minute changes the minute row, and the
        // rebuild then gives the hour exactly what its minutes say — counting
        // nothing twice, and keeping the true min / max.
        for (target_id, hour_ts) in hours {
            sqlx::query(
                "INSERT INTO latency_hourly
                     (node_id, target_id, hour_ts, sent, lost, rtt_sum_ms, rtt_min_ms, rtt_max_ms)
                 SELECT node_id, target_id, CAST($1 AS BIGINT),
                        CAST(SUM(sent) AS BIGINT), CAST(SUM(lost) AS BIGINT),
                        SUM(rtt_sum_ms), MIN(rtt_min_ms), MAX(rtt_max_ms)
                 FROM latency_minute
                 WHERE node_id = $2 AND target_id = $3 AND minute_ts >= $1 AND minute_ts < $4
                 GROUP BY node_id, target_id
                 ON CONFLICT (node_id, target_id, hour_ts) DO UPDATE SET
                     sent = EXCLUDED.sent,
                     lost = EXCLUDED.lost,
                     rtt_sum_ms = EXCLUDED.rtt_sum_ms,
                     rtt_min_ms = EXCLUDED.rtt_min_ms,
                     rtt_max_ms = EXCLUDED.rtt_max_ms",
            )
            .bind(hour_ts)
            .bind(node_id)
            .bind(target_id)
            .bind(hour_ts + 3600)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(())
    }

    async fn query_latency_buckets(
        &self,
        node_id: &str,
        granularity: LatencyGranularity,
        since_ts: i64,
        until_ts: i64,
    ) -> Result<Vec<LatencyBucket>, DbError> {
        // Table, time column and bucket expression are compile-time literals.
        // `/` on BIGINT is integer division, so the 5-minute bucket floors.
        let (table, col, bucket) = match granularity {
            LatencyGranularity::Minute1 => ("latency_minute", "minute_ts", "minute_ts"),
            LatencyGranularity::Minute5 => {
                ("latency_minute", "minute_ts", "(minute_ts / 300) * 300")
            }
            LatencyGranularity::Hour1 => ("latency_hourly", "hour_ts", "hour_ts"),
        };
        let sql = format!(
            "SELECT target_id, {bucket} AS ts,
                    CAST(SUM(sent) AS BIGINT) AS sent,
                    CAST(SUM(lost) AS BIGINT) AS lost,
                    SUM(rtt_sum_ms) AS rtt_sum_ms,
                    MIN(rtt_min_ms) AS rtt_min_ms,
                    MAX(rtt_max_ms) AS rtt_max_ms
             FROM {table}
             WHERE node_id = $1 AND {col} >= $2 AND {col} < $3
             GROUP BY target_id, {bucket}
             ORDER BY ts, target_id"
        );
        Ok(sqlx::query_as(&sql)
            .bind(node_id)
            .bind(since_ts)
            .bind(until_ts)
            .fetch_all(&self.pool)
            .await?)
    }

    async fn query_latest_latency(
        &self,
        node_id: &str,
        since_ts: i64,
    ) -> Result<Vec<LatencyMinuteRow>, DbError> {
        Ok(sqlx::query_as(
            "SELECT target_id, minute_ts, sent, lost, rtt_sum_ms, rtt_min_ms, rtt_max_ms
             FROM latency_minute m
             WHERE node_id = $1 AND minute_ts = (
                 SELECT MAX(minute_ts) FROM latency_minute
                 WHERE node_id = m.node_id AND target_id = m.target_id AND minute_ts >= $2)
             ORDER BY target_id",
        )
        .bind(node_id)
        .bind(since_ts)
        .fetch_all(&self.pool)
        .await?)
    }

    async fn prune_latency_history(
        &self,
        minute_before: i64,
        hourly_before: i64,
    ) -> Result<u64, DbError> {
        let minutes = sqlx::query("DELETE FROM latency_minute WHERE minute_ts < $1")
            .bind(minute_before)
            .execute(&self.pool)
            .await?
            .rows_affected();
        let hours = sqlx::query("DELETE FROM latency_hourly WHERE hour_ts < $1")
            .bind(hourly_before)
            .execute(&self.pool)
            .await?
            .rows_affected();
        Ok(minutes + hours)
    }
}
