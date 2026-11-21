use super::SqliteRepository;
use crate::db::error::DbError;
use crate::db::repo::*;
use async_trait::async_trait;
use sqlx::SqliteConnection;
use std::collections::BTreeSet;

// ── LatencyRepository ──

/// Columns of a stored minute / hourly row, shared by both tables.
const SELECT_ROW_COLS: &str =
    "target_id, minute_ts, sent, lost, rtt_sum_ms, rtt_min_ms, rtt_max_ms, retrans, retrans_sum_ms";

/// The retransmit sums of a query over `table` (the minute table carries the
/// answered-probe base implicitly: it is `sent - lost` of the rows that have the
/// field; the hourly table stores it). A NULL stays NULL when no row of the
/// group carried the field, because SQL `SUM` ignores NULLs and is NULL over
/// nothing but NULLs.
fn retrans_sums(table: &str) -> &'static str {
    if table == "latency_hourly" {
        "SUM(retrans) AS retrans,
                    SUM(retrans_sum_ms) AS retrans_sum_ms,
                    SUM(retrans_base) AS retrans_base"
    } else {
        "SUM(retrans) AS retrans,
                    SUM(retrans_sum_ms) AS retrans_sum_ms,
                    SUM(CASE WHEN retrans IS NOT NULL THEN sent - lost END) AS retrans_base"
    }
}

/// Remove the latency targets matching `target_filter` (a condition on
/// `latency_targets.group_id` with one `?`) together with their minute and
/// hourly history. Runs inside the caller's transaction, so a group and what
/// it measured go together or not at all.
async fn purge_latency_where(
    conn: &mut SqliteConnection,
    target_filter: &str,
    bind: i64,
) -> Result<(), sqlx::Error> {
    for table in ["latency_minute", "latency_hourly"] {
        sqlx::query(&format!(
            "DELETE FROM {table} WHERE target_id IN
                 (SELECT id FROM latency_targets WHERE {target_filter})"
        ))
        .bind(bind)
        .execute(&mut *conn)
        .await?;
    }
    sqlx::query(&format!(
        "DELETE FROM latency_targets WHERE {target_filter}"
    ))
    .bind(bind)
    .execute(&mut *conn)
    .await?;
    Ok(())
}

/// A device group is going away: its targets and their history go with it.
pub(super) async fn purge_group_latency(
    conn: &mut SqliteConnection,
    group_id: i64,
) -> Result<(), sqlx::Error> {
    purge_latency_where(conn, "group_id = ?", group_id).await
}

/// Every group of a user is going away (account deletion). Must run before the
/// groups are deleted: it finds them through `device_groups`.
pub(super) async fn purge_owner_latency(
    conn: &mut SqliteConnection,
    uid: i64,
) -> Result<(), sqlx::Error> {
    purge_latency_where(
        conn,
        "group_id IN (SELECT id FROM device_groups WHERE uid = ?)",
        uid,
    )
    .await
}

#[async_trait]
impl LatencyRepository for SqliteRepository {
    async fn list_latency_targets(&self, group_id: i64) -> Result<Vec<LatencyTarget>, DbError> {
        Ok(sqlx::query_as(
            "SELECT id, group_id, name, carrier, kind, host, port, sort_order, created_at
             FROM latency_targets WHERE group_id = ? ORDER BY sort_order, id",
        )
        .bind(group_id)
        .fetch_all(&self.pool)
        .await?)
    }

    async fn count_latency_targets_by_group(&self) -> Result<Vec<LatencyGroupCount>, DbError> {
        Ok(sqlx::query_as(
            "SELECT group_id, COUNT(*) AS targets FROM latency_targets
             GROUP BY group_id ORDER BY group_id",
        )
        .fetch_all(&self.pool)
        .await?)
    }

    async fn replace_latency_targets(
        &self,
        group_id: i64,
        now: &str,
        inputs: &[LatencyTargetInput],
    ) -> Result<Option<Vec<LatencyTarget>>, DbError> {
        let mut tx = self.begin_write().await?;

        // Only this group's targets count as existing: an id that belongs to
        // another group is as unknown here as one that was deleted.
        let existing: Vec<(i64,)> =
            sqlx::query_as("SELECT id FROM latency_targets WHERE group_id = ?")
                .bind(group_id)
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
                sqlx::query(&format!("DELETE FROM {table} WHERE {col} = ?"))
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
                            "DELETE FROM {table} WHERE target_id = ?1 AND EXISTS (
                                 SELECT 1 FROM latency_targets WHERE id = ?1
                                 AND (kind IS NOT ?2 OR host IS NOT ?3 OR port IS NOT ?4))"
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
                         SET name = ?, carrier = ?, kind = ?, host = ?, port = ?, sort_order = ?
                         WHERE id = ?",
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
                             (group_id, name, carrier, kind, host, port, sort_order, created_at)
                         VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
                    )
                    .bind(group_id)
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
            "SELECT id, group_id, name, carrier, kind, host, port, sort_order, created_at
             FROM latency_targets WHERE group_id = ? ORDER BY sort_order, id",
        )
        .bind(group_id)
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
        let mut tx = self.begin_write().await?;
        let mut hours: BTreeSet<(i64, i64)> = BTreeSet::new();
        for r in rows {
            // The newest report of a minute replaces the stored one.
            sqlx::query(
                "INSERT INTO latency_minute
                     (node_id, target_id, minute_ts, sent, lost, rtt_sum_ms, rtt_min_ms, rtt_max_ms,
                      retrans, retrans_sum_ms)
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
                 ON CONFLICT(node_id, target_id, minute_ts) DO UPDATE SET
                     sent = excluded.sent,
                     lost = excluded.lost,
                     rtt_sum_ms = excluded.rtt_sum_ms,
                     rtt_min_ms = excluded.rtt_min_ms,
                     rtt_max_ms = excluded.rtt_max_ms,
                     retrans = excluded.retrans,
                     retrans_sum_ms = excluded.retrans_sum_ms",
            )
            .bind(node_id)
            .bind(r.target_id)
            .bind(r.minute_ts)
            .bind(r.sent)
            .bind(r.lost)
            .bind(r.rtt_sum_ms)
            .bind(r.rtt_min_ms)
            .bind(r.rtt_max_ms)
            .bind(r.retrans)
            .bind(r.retrans_sum_ms)
            .execute(&mut *tx)
            .await?;
            hours.insert((r.target_id, r.minute_ts.div_euclid(3600) * 3600));
        }
        // The hourly row is rebuilt from the minute rows of its hour rather
        // than added to: re-storing a minute changes the minute row, and the
        // rebuild then gives the hour exactly what its minutes say — counting
        // nothing twice, and keeping the true min / max.
        //
        // The retransmit columns are summed over the minutes that carry them
        // (SUM skips NULLs and stays NULL when no minute carried the field);
        // `retrans_base` is the answered probes of exactly those minutes, so a
        // rate over an hour that mixes old and new reports divides correctly.
        for (target_id, hour_ts) in hours {
            sqlx::query(
                "INSERT INTO latency_hourly
                     (node_id, target_id, hour_ts, sent, lost, rtt_sum_ms, rtt_min_ms, rtt_max_ms,
                      retrans, retrans_sum_ms, retrans_base)
                 SELECT node_id, target_id, ?, SUM(sent), SUM(lost), SUM(rtt_sum_ms),
                        MIN(rtt_min_ms), MAX(rtt_max_ms),
                        SUM(retrans), SUM(retrans_sum_ms),
                        SUM(CASE WHEN retrans IS NOT NULL THEN sent - lost END)
                 FROM latency_minute
                 WHERE node_id = ? AND target_id = ? AND minute_ts >= ? AND minute_ts < ?
                 GROUP BY node_id, target_id
                 ON CONFLICT(node_id, target_id, hour_ts) DO UPDATE SET
                     sent = excluded.sent,
                     lost = excluded.lost,
                     rtt_sum_ms = excluded.rtt_sum_ms,
                     rtt_min_ms = excluded.rtt_min_ms,
                     rtt_max_ms = excluded.rtt_max_ms,
                     retrans = excluded.retrans,
                     retrans_sum_ms = excluded.retrans_sum_ms,
                     retrans_base = excluded.retrans_base",
            )
            .bind(hour_ts)
            .bind(node_id)
            .bind(target_id)
            .bind(hour_ts)
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
        let (table, col, bucket) = match granularity {
            LatencyGranularity::Minute1 => ("latency_minute", "minute_ts", "minute_ts"),
            LatencyGranularity::Minute5 => {
                ("latency_minute", "minute_ts", "(minute_ts / 300) * 300")
            }
            LatencyGranularity::Hour1 => ("latency_hourly", "hour_ts", "hour_ts"),
        };
        let retrans = retrans_sums(table);
        let sql = format!(
            "SELECT target_id, {bucket} AS ts,
                    CAST(SUM(sent) AS INTEGER) AS sent,
                    CAST(SUM(lost) AS INTEGER) AS lost,
                    SUM(rtt_sum_ms) AS rtt_sum_ms,
                    MIN(rtt_min_ms) AS rtt_min_ms,
                    MAX(rtt_max_ms) AS rtt_max_ms,
                    {retrans}
             FROM {table}
             WHERE node_id = ? AND {col} >= ? AND {col} < ?
             GROUP BY target_id, ts
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
        let sql = format!(
            "SELECT {SELECT_ROW_COLS} FROM latency_minute m
             WHERE node_id = ?1 AND minute_ts = (
                 SELECT MAX(minute_ts) FROM latency_minute
                 WHERE node_id = m.node_id AND target_id = m.target_id AND minute_ts >= ?2)
             ORDER BY target_id"
        );
        Ok(sqlx::query_as(&sql)
            .bind(node_id)
            .bind(since_ts)
            .fetch_all(&self.pool)
            .await?)
    }

    async fn query_latency_totals(
        &self,
        node_ids: &[String],
        target_ids: &[i64],
        hourly: bool,
        since_ts: i64,
        until_ts: i64,
    ) -> Result<Vec<LatencyNodeTotal>, DbError> {
        if node_ids.is_empty() || target_ids.is_empty() {
            return Ok(Vec::new());
        }
        let (table, col) = if hourly {
            ("latency_hourly", "hour_ts")
        } else {
            ("latency_minute", "minute_ts")
        };
        let marks = |n: usize| vec!["?"; n].join(", ");
        let retrans = retrans_sums(table);
        let sql = format!(
            "SELECT node_id, target_id,
                    CAST(SUM(sent) AS INTEGER) AS sent,
                    CAST(SUM(lost) AS INTEGER) AS lost,
                    SUM(rtt_sum_ms) AS rtt_sum_ms,
                    {retrans}
             FROM {table}
             WHERE node_id IN ({}) AND target_id IN ({}) AND {col} >= ? AND {col} < ?
             GROUP BY node_id, target_id
             ORDER BY node_id, target_id",
            marks(node_ids.len()),
            marks(target_ids.len())
        );
        let mut q = sqlx::query_as(&sql);
        for n in node_ids {
            q = q.bind(n);
        }
        for t in target_ids {
            q = q.bind(t);
        }
        Ok(q.bind(since_ts)
            .bind(until_ts)
            .fetch_all(&self.pool)
            .await?)
    }

    async fn query_latency_target_series(
        &self,
        node_ids: &[String],
        target_id: i64,
        granularity: LatencyGranularity,
        since_ts: i64,
        until_ts: i64,
    ) -> Result<Vec<LatencyNodeBucket>, DbError> {
        if node_ids.is_empty() {
            return Ok(Vec::new());
        }
        let (table, col, bucket) = match granularity {
            LatencyGranularity::Minute1 => ("latency_minute", "minute_ts", "minute_ts"),
            LatencyGranularity::Minute5 => {
                ("latency_minute", "minute_ts", "(minute_ts / 300) * 300")
            }
            LatencyGranularity::Hour1 => ("latency_hourly", "hour_ts", "hour_ts"),
        };
        let marks = vec!["?"; node_ids.len()].join(", ");
        let retrans = retrans_sums(table);
        let sql = format!(
            "SELECT node_id, {bucket} AS ts,
                    CAST(SUM(sent) AS INTEGER) AS sent,
                    CAST(SUM(lost) AS INTEGER) AS lost,
                    SUM(rtt_sum_ms) AS rtt_sum_ms,
                    MIN(rtt_min_ms) AS rtt_min_ms,
                    MAX(rtt_max_ms) AS rtt_max_ms,
                    {retrans}
             FROM {table}
             WHERE node_id IN ({marks}) AND target_id = ? AND {col} >= ? AND {col} < ?
             GROUP BY node_id, ts
             ORDER BY ts, node_id"
        );
        let mut q = sqlx::query_as(&sql);
        for n in node_ids {
            q = q.bind(n);
        }
        Ok(q.bind(target_id)
            .bind(since_ts)
            .bind(until_ts)
            .fetch_all(&self.pool)
            .await?)
    }

    async fn prune_latency_history(
        &self,
        minute_before: i64,
        hourly_before: i64,
    ) -> Result<u64, DbError> {
        let minutes = sqlx::query("DELETE FROM latency_minute WHERE minute_ts < ?")
            .bind(minute_before)
            .execute(&self.pool)
            .await?
            .rows_affected();
        let hours = sqlx::query("DELETE FROM latency_hourly WHERE hour_ts < ?")
            .bind(hourly_before)
            .execute(&self.pool)
            .await?
            .rows_affected();
        Ok(minutes + hours)
    }
}
