//! Node traffic + status-key service logic.
//!
//! Extracted from `api/node.rs` so the business rules (traffic-report overflow
//! pre-check + batch-result interpretation, legacy status-key cleanup, stale
//! status sweep) live behind the `Repository` trait and are unit-testable
//! without the HTTP layer. The handlers keep only the node-compat wire format
//! (business `code` in the JSON body, GeoIP fire-and-forget, status JSON
//! assembly).

use crate::db::error::DbError;
use crate::db::repo::TrafficEntryResult;
use crate::db::Repository;
use relay_shared::protocol::TrafficEntry;

/// How long a node's status row outlives its last report.
///
/// v1.2.5: raised from 2 minutes to 24 hours. Two minutes meant a node that
/// went down vanished from the list entirely, so the panel could not answer
/// "which of my nodes is offline right now" — the row an operator needs to see
/// during an outage is exactly the one that got deleted. It also made the
/// offline state nearly unobservable: the node is painted offline after 30s of
/// silence and gone 90s later.
///
/// A day is long enough to cover an outage and a night's sleep. Past that the
/// row is swept, which is what makes a decommissioned node disappear on its own
/// — and an admin who does not want to wait can remove it by hand from the node
/// list.
const STALE_STATUS_THRESHOLD_SECS: i64 = 24 * 60 * 60;

/// Outcome of applying a node traffic report. The handler maps each variant to
/// the node-compat wire response (business `code` in the JSON body).
#[derive(Debug)]
pub enum TrafficReportError {
    /// One or more entries (or their cumulative effect) overflow i64. Maps to a
    /// uniform 400 — never echoes a rule_id.
    Overflow,
    /// One or more rules are missing OR belong to another group. Deliberately
    /// indistinguishable (closes the rule-id existence oracle). Maps to a
    /// uniform 403.
    Unavailable,
    Database(DbError),
}

/// Apply a node's traffic report atomically.
///
/// Business rules (all preserved from the original handler):
///   - Pre-validate obvious overflow before opening a transaction (cheap path;
///     the DB layer re-checks the cumulative case).
///   - The whole batch is one atomic transaction inside `apply_traffic_batch`;
///     any non-Ok entry rolls back the whole batch.
///   - A rule missing OR foreign → [`TrafficReportError::Unavailable`] (uniform,
///     no oracle). Overflow → [`TrafficReportError::Overflow`].
///
/// Heavy lifting (ownership check, per-rule/per-user cumulative overflow,
/// duplicate rule_id aggregation) lives in `Repository::apply_traffic_batch`.
pub async fn apply_traffic_report(
    db: &dyn Repository,
    group_id: i64,
    reports: &[TrafficEntry],
) -> Result<(), TrafficReportError> {
    // Pre-validate obvious overflow before starting a transaction. The message
    // the handler emits must stay generic — it's the node's OWN reported id, so
    // it leaks nothing, but we keep it uniform for consistency.
    for entry in reports {
        let sum = entry
            .upload
            .checked_add(entry.download)
            .ok_or(TrafficReportError::Overflow)?;
        if sum > i64::MAX as u64 {
            return Err(TrafficReportError::Overflow);
        }
    }

    // apply_traffic_batch returns Ok(vec![result]) even on rejection; the
    // element(s) tell us which uniform response to send.
    let results = db
        .apply_traffic_batch(group_id, reports)
        .await
        .map_err(TrafficReportError::Database)?;

    // Any non-Ok result is a whole-batch rejection (rolled back inside).
    if results
        .iter()
        .any(|r| matches!(r, TrafficEntryResult::Unavailable))
    {
        return Err(TrafficReportError::Unavailable);
    }
    if results
        .iter()
        .any(|r| matches!(r, TrafficEntryResult::Overflow))
    {
        return Err(TrafficReportError::Overflow);
    }
    Ok(())
}

/// Delete the legacy per-group status key for `group_id` if its stored
/// public_ip matches `ip`. This cleans up the ghost entry left when a node
/// upgrades from pre-v0.3.1 (no node_id) to v0.3.1+ (per-node key).
///
/// Safety: only matches on public_ip, so a DIFFERENT physical node still
/// running an old version (different IP, same group) is NOT deleted — it keeps
/// reporting into its legacy key until it too upgrades.
pub async fn cleanup_legacy_status(db: &dyn Repository, group_id: i64, ip: &str) {
    let legacy_key = format!("node_status:{}", group_id);
    // Read the legacy value; only delete if its public_ip matches this node.
    let value = match db.get(&legacy_key).await {
        Ok(Some(v)) => v,
        Ok(None) => return,
        Err(e) => {
            tracing::warn!("cleanup_legacy_status: kvs get failed: {}", e);
            return;
        }
    };
    let matches_ip = serde_json::from_str::<serde_json::Value>(&value)
        .ok()
        .and_then(|v| v.get("public_ip").and_then(|p| p.as_str()).map(|p| p == ip))
        .unwrap_or(false);
    if matches_ip {
        let _ = db
            .delete(&legacy_key)
            .await
            .map_err(|e| tracing::warn!("cleanup_legacy_status: kvs delete failed: {}", e));
        tracing::info!(
            "cleaned up legacy node_status:{} (upgraded node_id, same public_ip {})",
            group_id,
            ip
        );
    }
}

/// Remove node_status entries older than [`STALE_STATUS_THRESHOLD_SECS`].
/// Parses last_seen from the stored JSON; entries without last_seen are left
/// alone (conservative — don't delete what we can't age).
///
/// Also runs on READ (get_node_status), so ghost rows get cleaned even when no
/// node in the group is still reporting.
pub async fn sweep_stale_status(db: &dyn Repository) -> Result<(), DbError> {
    let rows: Vec<(String, String)> = db.scan_prefix("node_status:").await?;
    let now = chrono::Utc::now();
    for (key, value) in &rows {
        let stale = serde_json::from_str::<serde_json::Value>(value)
            .ok()
            .and_then(|v| {
                v.get("last_seen")
                    .and_then(|s| s.as_str())
                    .map(|s| s.to_string())
            })
            .and_then(|s| chrono::DateTime::parse_from_rfc3339(&s).ok())
            .map(|t| {
                (now - t.with_timezone(&chrono::Utc)).num_seconds() > STALE_STATUS_THRESHOLD_SECS
            })
            .unwrap_or(false);
        if stale {
            let _ = db
                .delete(key)
                .await
                .map_err(|e| tracing::warn!("sweep_stale_status: kvs delete failed: {}", e));
            tracing::debug!("swept stale node status {}", key);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::sqlite_repo::SqliteRepository;
    use sqlx::sqlite::{SqlitePool, SqlitePoolOptions};

    /// Minimal in-memory DB with just the kvs table (all the status functions
    /// touch). Faster than full fresh_pool and isolates these tests from the
    /// schema migrations.
    async fn kvs_pool() -> SqlitePool {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::query("CREATE TABLE kvs (key TEXT PRIMARY KEY, value TEXT NOT NULL)")
            .execute(&pool)
            .await
            .unwrap();
        pool
    }

    /// Wrap a kvs-only pool in a SqliteRepository so the service-layer helpers
    /// (cleanup_legacy_status / sweep_stale_status) can be exercised via the
    /// Repository trait the same way the production code does.
    fn repo(pool: &SqlitePool) -> SqliteRepository {
        SqliteRepository::new(pool.clone())
    }

    /// Helper: insert a status row with the given key + public_ip + last_seen.
    async fn put_status(pool: &SqlitePool, key: &str, public_ip: Option<&str>, last_seen: &str) {
        let v = serde_json::json!({
            "public_ip": public_ip,
            "last_seen": last_seen,
            "cpu": 10.0,
        });
        sqlx::query("INSERT OR REPLACE INTO kvs (key, value) VALUES (?, ?)")
            .bind(key)
            .bind(v.to_string())
            .execute(pool)
            .await
            .unwrap();
    }
    async fn exists(pool: &SqlitePool, key: &str) -> bool {
        let (n,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM kvs WHERE key=?")
            .bind(key)
            .fetch_one(pool)
            .await
            .unwrap();
        n > 0
    }

    /// Legacy key with the SAME public_ip is cleaned up when the upgraded node
    /// reports. This is the core fix: a node that moved from
    /// node_status:{gid} to node_status:{gid}:{node_id} must take its old
    /// ghost entry with it.
    #[tokio::test]
    async fn legacy_status_same_ip_is_cleaned() {
        let pool = kvs_pool().await;
        put_status(
            &pool,
            "node_status:5",
            Some("1.2.3.4"),
            "2026-01-01T00:00:00Z",
        )
        .await;
        cleanup_legacy_status(&repo(&pool), 5, "1.2.3.4").await;
        assert!(
            !exists(&pool, "node_status:5").await,
            "legacy entry with matching IP must be deleted"
        );
    }

    /// Legacy key with a DIFFERENT public_ip must NOT be deleted — it belongs
    /// to a different physical node still running an old version.
    #[tokio::test]
    async fn legacy_status_different_ip_is_kept() {
        let pool = kvs_pool().await;
        put_status(
            &pool,
            "node_status:5",
            Some("9.9.9.9"),
            "2026-01-01T00:00:00Z",
        )
        .await;
        cleanup_legacy_status(&repo(&pool), 5, "1.2.3.4").await;
        assert!(
            exists(&pool, "node_status:5").await,
            "legacy entry with a DIFFERENT IP must be kept (another node)"
        );
    }

    /// No legacy key at all → cleanup is a no-op (no error, nothing deleted).
    #[tokio::test]
    async fn legacy_cleanup_noop_when_absent() {
        let pool = kvs_pool().await;
        cleanup_legacy_status(&repo(&pool), 5, "1.2.3.4").await;
        // Just must not panic / error.
    }

    /// Entries older than the threshold are swept; fresh ones are kept.
    #[tokio::test]
    async fn stale_status_is_swept_fresh_kept() {
        let pool = kvs_pool().await;
        // Comfortably past the 24h threshold; 15 minutes used to qualify when
        // the threshold was 2 minutes.
        let old = (chrono::Utc::now() - chrono::Duration::hours(30)).to_rfc3339();
        let fresh = chrono::Utc::now().to_rfc3339();
        put_status(&pool, "node_status:1:old", Some("1.1.1.1"), &old).await;
        put_status(&pool, "node_status:1:new", Some("2.2.2.2"), &fresh).await;
        sweep_stale_status(&repo(&pool)).await.unwrap();
        assert!(
            !exists(&pool, "node_status:1:old").await,
            "old entry must be swept"
        );
        assert!(
            exists(&pool, "node_status:1:new").await,
            "fresh entry must be kept"
        );
    }

    /// A node down for most of a day is still listed. This is the whole point of
    /// the v1.2.5 change: during an outage the offline row is the one an
    /// operator needs to see, and the old 2-minute threshold deleted it before
    /// anyone could look.
    #[tokio::test]
    async fn node_offline_for_hours_is_still_listed() {
        let pool = kvs_pool().await;
        let hours_ago = (chrono::Utc::now() - chrono::Duration::hours(20)).to_rfc3339();
        put_status(&pool, "node_status:1:down", Some("1.1.1.1"), &hours_ago).await;
        sweep_stale_status(&repo(&pool)).await.unwrap();
        assert!(
            exists(&pool, "node_status:1:down").await,
            "a node offline for 20h must still be visible"
        );
    }

    /// Entries without last_seen are NOT swept (conservative — can't age what
    /// we can't timestamp; better to leave a ghost than delete a live node
    /// whose clock format we didn't understand).
    #[tokio::test]
    async fn status_without_last_seen_is_kept() {
        let pool = kvs_pool().await;
        sqlx::query("INSERT INTO kvs (key, value) VALUES ('node_status:2', '{}')")
            .execute(&pool)
            .await
            .unwrap();
        sweep_stale_status(&repo(&pool)).await.unwrap();
        assert!(
            exists(&pool, "node_status:2").await,
            "entry without last_seen must not be swept (conservative)"
        );
    }

    /// The overflow pre-check rejects an entry whose upload+download overflows
    /// u64 BEFORE touching the DB (the kvs-only pool has no forward_rules table,
    /// so reaching apply_traffic_batch would error — proving the early return).
    #[tokio::test]
    async fn apply_traffic_report_rejects_overflow_before_db() {
        let pool = kvs_pool().await;
        let err = apply_traffic_report(
            &repo(&pool),
            10,
            &[TrafficEntry {
                rule_id: 100,
                upload: u64::MAX,
                download: 1,
            }],
        )
        .await
        .expect_err("overflow must be rejected");
        assert!(matches!(err, TrafficReportError::Overflow));
    }

    /// A sum that exceeds i64::MAX (but does not overflow u64) is also rejected.
    #[tokio::test]
    async fn apply_traffic_report_rejects_above_i64_max() {
        let pool = kvs_pool().await;
        let err = apply_traffic_report(
            &repo(&pool),
            10,
            &[TrafficEntry {
                rule_id: 100,
                upload: i64::MAX as u64,
                download: 1,
            }],
        )
        .await
        .expect_err("sum above i64::MAX must be rejected");
        assert!(matches!(err, TrafficReportError::Overflow));
    }
}
