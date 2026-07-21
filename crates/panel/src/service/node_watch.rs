//! v1.2.0: node offline/recovery detection and alerting.
//!
//! Scans the `node_status:*` kvs rows on a timer and notifies when a node has
//! been silent for longer than the configured threshold, then again when it
//! comes back.
//!
//! ## Why this isn't just "online == false"
//!
//! The UI paints a node offline after `NODE_ONLINE_WINDOW_SECS` (30s), which is
//! the right call for a status dot — it should react fast. It is the wrong
//! trigger for an alert: a node that misses two status reports on a flaky link
//! is briefly "offline" and perfectly healthy, and paging on that trains the
//! operator to ignore the channel. Alerting therefore has its OWN, longer
//! threshold (default 180s ≈ six missed reports).
//!
//! ## Why state is in memory
//!
//! Same reasoning as the auto-restart scheduler: persisting "was offline" would
//! mean a panel restart replays alerts for everything that happened while it
//! was down, so an upgrade would open with a burst of stale pages. Keeping it
//! in memory re-baselines on boot — nodes are observed fresh, and only
//! transitions seen by THIS process are announced.
//!
//! The cost is that a node which goes down exactly during a panel restart is
//! first observed as already-offline. That case is handled explicitly below
//! (see `first_seen_offline`) rather than being silently dropped.

use std::collections::HashMap;
use std::time::Duration;

use crate::api::stats::status_last_seen;
use crate::api::AppState;
use crate::service::notify::{self, NotifyConfig};

/// How often to scan. The finest alert threshold is 60s, so 30s keeps the
/// reported delay within half a threshold while costing one kvs scan a minute.
const TICK: Duration = Duration::from_secs(30);

/// What the watcher believes about one node.
///
/// Only two states: the "offline but not yet past the threshold" case does NOT
/// need one, because `tick` folds the threshold into `is_offline` before asking
/// — a node silent for 10s of a 180s threshold is simply not offline yet, and
/// treating it as Online is exactly right (it has nothing to recover FROM, so
/// coming back stays silent).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NodeState {
    /// Healthy, or silent for less than the alert threshold.
    Online,
    /// Past the threshold and already announced — do not announce again.
    OfflineAlerted,
}

/// node key ("group_id:node_id") → last known state.
type Watch = HashMap<String, NodeState>;

/// What a transition should announce.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Announce {
    Nothing,
    Offline,
    Recovery,
}

/// The whole state machine, as one pure function.
///
/// `tick` calls this and the tests call this — deliberately not two copies of
/// the same `match`, which would drift the moment one side is edited.
fn decide(previous: Option<NodeState>, is_offline: bool) -> (NodeState, Announce) {
    match (previous, is_offline) {
        // Healthy, staying healthy. Also covers a node silent for less than the
        // threshold — a blip is entirely silent, which is what the long
        // threshold is FOR.
        (None | Some(NodeState::Online), false) => (NodeState::Online, Announce::Nothing),

        // Crossed the threshold into a real outage.
        (Some(NodeState::Online), true) => (NodeState::OfflineAlerted, Announce::Offline),

        // First sight of a node that is ALREADY past the threshold: it died
        // while the panel wasn't watching (a restart/upgrade). Alert once —
        // silently baselining it would mean an outage that began during an
        // upgrade is never reported, exactly when the operator needs to know.
        (None, true) => (NodeState::OfflineAlerted, Announce::Offline),

        // Ongoing announced outage: stay quiet. Re-alerting every tick is how
        // an alert channel gets muted, and a muted channel is worse than none.
        (Some(NodeState::OfflineAlerted), true) => (NodeState::OfflineAlerted, Announce::Nothing),

        // Came back from an announced outage.
        (Some(NodeState::OfflineAlerted), false) => (NodeState::Online, Announce::Recovery),
    }
}

pub fn spawn(state: AppState) {
    tokio::spawn(async move {
        let mut watch: Watch = HashMap::new();
        let mut ticker = tokio::time::interval(TICK);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        tracing::info!("node-watch started (tick {}s)", TICK.as_secs());
        loop {
            ticker.tick().await;
            tick(&state, &mut watch, chrono::Utc::now()).await;
        }
    });
}

/// One scan. `now` is injected so the decision logic can be tested without
/// sleeping.
async fn tick(state: &AppState, watch: &mut Watch, now: chrono::DateTime<chrono::Utc>) {
    let raw = state.db.get(notify::NOTIFY_CONFIG_KEY).await.ok().flatten();
    let cfg = NotifyConfig::from_json(raw.as_deref());

    let rows = match state.db.scan_prefix("node_status:").await {
        Ok(r) => r,
        Err(e) => {
            // Transient DB trouble skips this tick rather than killing the loop.
            tracing::error!("node-watch: scanning node status failed: {}", e);
            return;
        }
    };

    let threshold = cfg.alert_after();
    let mut seen: Vec<String> = Vec::with_capacity(rows.len());

    for (key, value) in &rows {
        let node_key = key.trim_start_matches("node_status:").to_string();
        seen.push(node_key.clone());

        // A row with no parseable last_seen counts as silent since forever;
        // treating it as online would hide a genuinely broken node.
        let offline_secs = status_last_seen(value)
            .map(|t| (now - t).num_seconds())
            .unwrap_or(i64::MAX);
        let is_offline = offline_secs > threshold;

        let (next, announce) = decide(watch.get(&node_key).copied(), is_offline);
        match announce {
            Announce::Offline => announce_offline(&cfg, &node_key, value, offline_secs).await,
            // The recovery toggle is applied here rather than inside `decide`
            // so the state machine stays about STATE and the config only gates
            // delivery.
            Announce::Recovery if cfg.notify_recovery => {
                announce_recovery(&cfg, &node_key, value).await
            }
            _ => {}
        }
        watch.insert(node_key, next);
    }

    // Forget nodes whose status row is gone (deleted group / cleared status),
    // so a node re-added later is observed fresh instead of inheriting a stale
    // "was offline" and firing a bogus recovery.
    watch.retain(|k, _| seen.iter().any(|s| s == k));
}

/// Pull display fields out of a node_status JSON blob for the message body.
fn describe(node_key: &str, status_json: &str) -> String {
    let v: serde_json::Value = serde_json::from_str(status_json).unwrap_or_default();
    let ip = v
        .get("public_ipv4")
        .or_else(|| v.get("public_ip"))
        .and_then(|s| s.as_str())
        .unwrap_or("-");
    format!("{node_key} (IP: {ip})")
}

async fn announce_offline(
    cfg: &NotifyConfig,
    node_key: &str,
    status_json: &str,
    offline_secs: i64,
) {
    // Offline detection runs regardless of the notification config so the state
    // machine stays accurate; only DELIVERY is gated. That way enabling alerts
    // doesn't immediately fire for outages that started earlier.
    if !cfg.any_channel_enabled() {
        tracing::info!(
            "node-watch: {} offline (alerts disabled, not sending)",
            node_key
        );
        return;
    }
    let mins = (offline_secs / 60).max(1);
    let text = format!(
        "🔴 节点离线\n\n{}\n已离线约 {} 分钟。\n\n该节点上的转发规则可能已经中断。",
        describe(node_key, status_json),
        mins
    );
    let report = notify::send_all(cfg, "RelayPanel 节点离线告警", &text).await;
    log_report(node_key, "offline", &report);
}

async fn announce_recovery(cfg: &NotifyConfig, node_key: &str, status_json: &str) {
    if !cfg.any_channel_enabled() {
        tracing::info!("node-watch: {} recovered (alerts disabled)", node_key);
        return;
    }
    let text = format!(
        "🟢 节点已恢复\n\n{}\n已重新上报状态。",
        describe(node_key, status_json)
    );
    let report = notify::send_all(cfg, "RelayPanel 节点恢复", &text).await;
    log_report(node_key, "recovery", &report);
}

/// A failed notification is logged, never propagated: the alert loop must keep
/// running whether or not Telegram/SMTP is reachable.
fn log_report(node_key: &str, kind: &str, report: &notify::SendReport) {
    if let Some(Err(e)) = &report.telegram {
        tracing::error!("node-watch: {} {} telegram failed: {}", node_key, kind, e);
    }
    if let Some(Err(e)) = &report.email {
        tracing::error!("node-watch: {} {} email failed: {}", node_key, kind, e);
    }
    if matches!(report.telegram, Some(Ok(()))) || matches!(report.email, Some(Ok(()))) {
        tracing::info!("node-watch: {} {} alert sent", node_key, kind);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A healthy node never generates traffic on the alert channel.
    #[test]
    fn healthy_node_never_alerts() {
        assert_eq!(decide(None, false), (NodeState::Online, Announce::Nothing));
        assert_eq!(
            decide(Some(NodeState::Online), false),
            (NodeState::Online, Announce::Nothing)
        );
    }

    /// An ongoing outage alerts EXACTLY once, no matter how many ticks pass.
    /// Re-alerting every 30s is how an alert channel gets muted, and a muted
    /// channel is worse than no channel.
    #[test]
    fn outage_alerts_once_not_every_tick() {
        let (mut state, announce) = decide(Some(NodeState::Online), true);
        assert_eq!(state, NodeState::OfflineAlerted);
        assert_eq!(announce, Announce::Offline, "the transition alerts");

        for tick in 0..20 {
            let (next, announce) = decide(Some(state), true);
            assert_eq!(
                announce,
                Announce::Nothing,
                "tick {tick}: an ongoing outage must stay silent"
            );
            state = next;
        }
    }

    /// Recovery is announced exactly once, on the transition back.
    #[test]
    fn recovery_alerts_once() {
        let (state, announce) = decide(Some(NodeState::OfflineAlerted), false);
        assert_eq!(state, NodeState::Online);
        assert_eq!(announce, Announce::Recovery, "coming back is announced");

        // Steady online afterwards — no repeat.
        assert_eq!(
            decide(Some(state), false),
            (NodeState::Online, Announce::Nothing)
        );
    }

    /// A node first seen ALREADY offline (it died during a panel restart) must
    /// still alert. Silently baselining it would mean an outage that began
    /// during an upgrade is never reported — exactly when the operator most
    /// needs to know.
    #[test]
    fn node_first_seen_offline_still_alerts() {
        assert_eq!(
            decide(None, true),
            (NodeState::OfflineAlerted, Announce::Offline)
        );
    }

    /// A node silent for LESS than the threshold is not offline as far as this
    /// machine is concerned, so a blip is entirely silent — no outage alert on
    /// the way down, and no recovery alert on the way back up. That second half
    /// matters: a recovery notice for an outage nobody was told about is pure
    /// confusion.
    #[test]
    fn blip_below_threshold_is_silent_in_both_directions() {
        // `is_offline` is false for a sub-threshold gap (tick folds the
        // threshold in before calling decide).
        let (state, announce) = decide(Some(NodeState::Online), false);
        assert_eq!(state, NodeState::Online);
        assert_eq!(announce, Announce::Nothing, "going quiet briefly is silent");

        let (_, announce) = decide(Some(state), false);
        assert_eq!(announce, Announce::Nothing, "and so is coming back");
    }
}
