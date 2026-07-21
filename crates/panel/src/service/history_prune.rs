//! v1.2.0: traffic-history retention sweeper.
//!
//! `traffic_history` has no FK — rows are never deleted by a parent cascade
//! (deliberate: deleting a rule must not shrink "last 7 days"), so this sweeper
//! is the ONLY thing that removes them. Without it the table grows forever.

use std::time::Duration;

use crate::api::AppState;

/// Keep 35 days: the UI's largest window is 30d, plus margin so a bucket that
/// straddles the boundary mid-query never disappears from under a chart.
const RETENTION_DAYS: i64 = 35;

/// One sweep per hour. Deletion is cheap (indexed range delete) and the
/// granularity of the data is hourly anyway — sweeping faster buys nothing.
const TICK: Duration = Duration::from_secs(3600);

pub fn spawn(state: AppState) {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(TICK);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        tracing::info!(
            "traffic-history sweeper started (retention {}d, tick {}s)",
            RETENTION_DAYS,
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
        }
    });
}
