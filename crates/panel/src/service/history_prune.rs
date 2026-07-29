//! v1.2.0: history retention sweeper (traffic, and since v1.2.4 node metrics
//! and the audit log).
//!
//! Neither table has an FK — rows are never deleted by a parent cascade
//! (deliberate: deleting a rule must not shrink "last 7 days", and removing a
//! node must not erase the history explaining what it did), so this sweeper is
//! the ONLY thing that removes them. Without it the tables grow forever.

use std::time::Duration;

use crate::api::AppState;

/// Keep 35 days: the UI's largest window is 30d, plus margin so a bucket that
/// straddles the boundary mid-query never disappears from under a chart.
const RETENTION_DAYS: i64 = 35;

/// v1.2.4: node metrics keep a week. A report every ~10s is far denser than
/// per-rule traffic, and week-old CPU is rarely what you are looking for —
/// paying 35 days of rows for it would be the wrong trade.
const METRICS_RETENTION_DAYS: i64 = 7;

/// One sweep per hour. Deletion is cheap (indexed range delete) and the
/// granularity of the data is hourly anyway — sweeping faster buys nothing.
const TICK: Duration = Duration::from_secs(3600);

pub fn spawn(state: AppState) {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(TICK);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        tracing::info!(
            "history sweeper started (traffic {}d, node metrics {}d, audit {}d, tick {}s)",
            RETENTION_DAYS,
            METRICS_RETENTION_DAYS,
            crate::service::audit::RETENTION_DAYS,
            TICK.as_secs()
        );
        loop {
            ticker.tick().await;
            let cutoff = (chrono::Utc::now() - chrono::Duration::days(RETENTION_DAYS))
                .format("%Y-%m-%d %H:00:00")
                .to_string();
            match state.db.prune_traffic_history(&cutoff).await {
                Ok(0) => {}
                Ok(n) => tracing::info!("traffic-history: pruned {} rows older than {}", n, cutoff),
                // Transient DB trouble skips the sweep, never kills the loop.
                Err(e) => tracing::error!("traffic-history: prune failed: {}", e),
            }

            // Swept in the same pass, on its own (shorter) cutoff. A failure
            // here is independent — one table failing must not skip the other.
            let metrics_cutoff = (chrono::Utc::now()
                - chrono::Duration::days(METRICS_RETENTION_DAYS))
            .format("%Y-%m-%d %H:00:00")
            .to_string();
            match state.db.prune_node_metrics(&metrics_cutoff).await {
                Ok(0) => {}
                Ok(n) => tracing::info!(
                    "node-metrics: pruned {} rows older than {}",
                    n,
                    metrics_cutoff
                ),
                Err(e) => tracing::error!("node-metrics: prune failed: {}", e),
            }

            // v1.2.4: the audit trail, on its own much longer cutoff. Swept in
            // the same pass but independently — one table failing must not skip
            // the others.
            let audit_cutoff = (chrono::Utc::now()
                - chrono::Duration::days(crate::service::audit::RETENTION_DAYS))
            .format("%Y-%m-%d %H:%M:%S")
            .to_string();
            match state.db.prune_audit_log(&audit_cutoff).await {
                Ok(0) => {}
                Ok(n) => tracing::info!("audit-log: pruned {} rows older than {}", n, audit_cutoff),
                Err(e) => tracing::error!("audit-log: prune failed: {}", e),
            }
        }
    });
}
