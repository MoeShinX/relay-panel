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
#[derive(Debug, Clone, PartialEq, Eq)]
enum NodeState {
    /// Healthy, or silent for less than the alert threshold.
    Online,
    /// Past the threshold and already announced — do not announce again.
    OfflineAlerted {
        last_alert_at: chrono::DateTime<chrono::Utc>,
    },
}

/// In-memory state for availability and node-version alerts. Version targets
/// are kept separate: a newer GitHub release intentionally produces one fresh
/// reminder for nodes that are still behind it.
#[derive(Default)]
struct Watch {
    nodes: HashMap<String, NodeState>,
    version_alerted_target: HashMap<String, String>,
}

/// What a transition should announce.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Announce {
    Nothing,
    Offline,
    OfflineReminder,
    Recovery,
}

/// The whole state machine, as one pure function.
///
/// `tick` calls this and the tests call this — deliberately not two copies of
/// the same `match`, which would drift the moment one side is edited.
fn decide(
    previous: Option<NodeState>,
    is_offline: bool,
    now: chrono::DateTime<chrono::Utc>,
    repeat_alert_minutes: i64,
) -> (NodeState, Announce) {
    match (previous, is_offline) {
        // Healthy, staying healthy. Also covers a node silent for less than the
        // threshold — a blip is entirely silent, which is what the long
        // threshold is FOR.
        (None | Some(NodeState::Online), false) => (NodeState::Online, Announce::Nothing),

        // Crossed the threshold into a real outage.
        (Some(NodeState::Online), true) => (
            NodeState::OfflineAlerted { last_alert_at: now },
            Announce::Offline,
        ),

        // First sight of a node that is ALREADY past the threshold: it died
        // while the panel wasn't watching (a restart/upgrade). Alert once —
        // silently baselining it would mean an outage that began during an
        // upgrade is never reported, exactly when the operator needs to know.
        (None, true) => (
            NodeState::OfflineAlerted { last_alert_at: now },
            Announce::Offline,
        ),

        // Ongoing announced outage: stay quiet. Re-alerting every tick is how
        // an alert channel gets muted, and a muted channel is worse than none.
        (Some(NodeState::OfflineAlerted { last_alert_at }), true)
            if repeat_alert_minutes > 0
                && now - last_alert_at >= chrono::Duration::minutes(repeat_alert_minutes) =>
        {
            (
                NodeState::OfflineAlerted { last_alert_at: now },
                Announce::OfflineReminder,
            )
        }

        (Some(NodeState::OfflineAlerted { last_alert_at }), true) => (
            NodeState::OfflineAlerted { last_alert_at },
            Announce::Nothing,
        ),

        // Came back from an announced outage.
        (Some(NodeState::OfflineAlerted { .. }), false) => (NodeState::Online, Announce::Recovery),
    }
}

/// A bad or missing version is not evidence of staleness. Only emit an alert
/// when both sides parse and the reported node binary is strictly behind.
fn is_version_outdated(node_version: Option<&str>, latest_version: Option<&str>) -> bool {
    let parse = |version: Option<&str>| {
        version.and_then(|v| semver::Version::parse(v.trim().trim_start_matches('v')).ok())
    };
    matches!((parse(node_version), parse(latest_version)), (Some(node), Some(latest)) if node < latest)
}

pub fn spawn(state: AppState) {
    tokio::spawn(async move {
        let mut watch = Watch::default();
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
    let latest_node_version = if cfg.notify_version_outdated {
        match state.release_cache.resolve_latest_node_version().await {
            Ok(version) => version,
            Err(e) => {
                tracing::warn!("node-watch: version check skipped: {e}");
                None
            }
        }
    } else {
        None
    };
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

        let (next, announce) = decide(
            watch.nodes.get(&node_key).cloned(),
            is_offline,
            now,
            cfg.repeat_alert_minutes.max(0),
        );
        match announce {
            Announce::Offline if cfg.notify_offline => {
                announce_offline(state, &cfg, &node_key, value, offline_secs, false).await
            }
            Announce::OfflineReminder if cfg.notify_offline => {
                announce_offline(state, &cfg, &node_key, value, offline_secs, true).await
            }
            // The recovery toggle is applied here rather than inside `decide`
            // so the state machine stays about STATE and the config only gates
            // delivery.
            Announce::Recovery if cfg.notify_recovery => {
                announce_recovery(state, &cfg, &node_key, value).await
            }
            _ => {}
        }
        watch.nodes.insert(node_key.clone(), next);

        let reported_version = serde_json::from_str::<serde_json::Value>(value)
            .ok()
            .and_then(|v| {
                v.get("node_version")
                    .and_then(|version| version.as_str())
                    .map(str::to_owned)
            });
        if is_version_outdated(reported_version.as_deref(), latest_node_version.as_deref()) {
            let latest = latest_node_version.as_deref().unwrap_or_default();
            if watch
                .version_alerted_target
                .get(&node_key)
                .map(String::as_str)
                != Some(latest)
            {
                announce_version_outdated(state, &cfg, &node_key, value, latest).await;
                watch
                    .version_alerted_target
                    .insert(node_key.clone(), latest.to_string());
            }
        } else {
            watch.version_alerted_target.remove(&node_key);
        }
    }

    // Forget nodes whose status row is gone (deleted group / cleared status),
    // so a node re-added later is observed fresh instead of inheriting a stale
    // "was offline" and firing a bogus recovery.
    watch.nodes.retain(|k, _| seen.iter().any(|s| s == k));
    watch
        .version_alerted_target
        .retain(|k, _| seen.iter().any(|s| s == k));
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
    state: &AppState,
    cfg: &NotifyConfig,
    node_key: &str,
    status_json: &str,
    offline_secs: i64,
    reminder: bool,
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
    let title = if reminder {
        "🔴 节点仍离线"
    } else {
        "🔴 节点离线"
    };
    let text = format!(
        "{title}\n\n{}\n已离线约 {} 分钟。\n\n该节点上的转发规则可能已经中断。",
        describe(node_key, status_json),
        mins
    );
    let event = if reminder {
        "offline_reminder"
    } else {
        "offline"
    };
    let report = notify::send_all(cfg, "RelayPanel 节点离线告警", &text).await;
    notify::record_report(state.db.as_ref(), event, Some(node_key), &report).await;
    log_report(node_key, event, &report);
}

async fn announce_recovery(
    state: &AppState,
    cfg: &NotifyConfig,
    node_key: &str,
    status_json: &str,
) {
    if !cfg.any_channel_enabled() {
        tracing::info!("node-watch: {} recovered (alerts disabled)", node_key);
        return;
    }
    let text = format!(
        "🟢 节点已恢复\n\n{}\n已重新上报状态。",
        describe(node_key, status_json)
    );
    let report = notify::send_all(cfg, "RelayPanel 节点恢复", &text).await;
    notify::record_report(state.db.as_ref(), "recovery", Some(node_key), &report).await;
    log_report(node_key, "recovery", &report);
}

async fn announce_version_outdated(
    state: &AppState,
    cfg: &NotifyConfig,
    node_key: &str,
    status_json: &str,
    latest_version: &str,
) {
    if !cfg.any_channel_enabled() {
        return;
    }
    let text = format!(
        "🟠 节点版本过旧\n\n{}\n当前节点版本低于最新版本 {}，建议在节点状态页面安排升级。",
        describe(node_key, status_json),
        latest_version,
    );
    let report = notify::send_all(cfg, "RelayPanel 节点版本过旧", &text).await;
    notify::record_report(
        state.db.as_ref(),
        "version_outdated",
        Some(node_key),
        &report,
    )
    .await;
    log_report(node_key, "version_outdated", &report);
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

    fn test_now() -> chrono::DateTime<chrono::Utc> {
        chrono::DateTime::parse_from_rfc3339("2026-08-07T10:00:00Z")
            .unwrap()
            .with_timezone(&chrono::Utc)
    }

    #[test]
    fn outage_repeats_only_after_the_configured_interval() {
        let first = chrono::DateTime::parse_from_rfc3339("2026-08-07T10:00:00Z")
            .unwrap()
            .with_timezone(&chrono::Utc);
        let (state, announce) = decide(None, true, first, 30);
        assert_eq!(announce, Announce::Offline);

        let before_repeat = first + chrono::Duration::minutes(29);
        let (_, announce) = decide(Some(state.clone()), true, before_repeat, 30);
        assert_eq!(announce, Announce::Nothing);

        let at_repeat = first + chrono::Duration::minutes(30);
        let (_, announce) = decide(Some(state), true, at_repeat, 30);
        assert_eq!(announce, Announce::OfflineReminder);
    }

    #[test]
    fn version_alerts_only_for_parseable_versions_behind_the_latest_release() {
        assert!(is_version_outdated(Some("v1.0.0"), Some("1.1.0")));
        assert!(!is_version_outdated(Some("1.1.0"), Some("1.1.0")));
        assert!(!is_version_outdated(Some("not-a-version"), Some("1.1.0")));
        assert!(!is_version_outdated(Some("1.0.0"), None));
    }

    /// A healthy node never generates traffic on the alert channel.
    #[test]
    fn healthy_node_never_alerts() {
        assert_eq!(
            decide(None, false, test_now(), 0),
            (NodeState::Online, Announce::Nothing)
        );
        assert_eq!(
            decide(Some(NodeState::Online), false, test_now(), 0),
            (NodeState::Online, Announce::Nothing)
        );
    }

    /// An ongoing outage alerts EXACTLY once, no matter how many ticks pass.
    /// Re-alerting every 30s is how an alert channel gets muted, and a muted
    /// channel is worse than no channel.
    #[test]
    fn outage_alerts_once_not_every_tick() {
        let now = test_now();
        let (mut state, announce) = decide(Some(NodeState::Online), true, now, 0);
        assert!(matches!(state, NodeState::OfflineAlerted { .. }));
        assert_eq!(announce, Announce::Offline, "the transition alerts");

        for tick in 0..20 {
            let (next, announce) = decide(Some(state), true, now, 0);
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
        let now = test_now();
        let (state, announce) = decide(
            Some(NodeState::OfflineAlerted { last_alert_at: now }),
            false,
            now,
            0,
        );
        assert_eq!(state, NodeState::Online);
        assert_eq!(announce, Announce::Recovery, "coming back is announced");

        // Steady online afterwards — no repeat.
        assert_eq!(
            decide(Some(state), false, now, 0),
            (NodeState::Online, Announce::Nothing)
        );
    }

    /// A node first seen ALREADY offline (it died during a panel restart) must
    /// still alert. Silently baselining it would mean an outage that began
    /// during an upgrade is never reported — exactly when the operator most
    /// needs to know.
    #[test]
    fn node_first_seen_offline_still_alerts() {
        let now = test_now();
        assert_eq!(
            decide(None, true, now, 0),
            (
                NodeState::OfflineAlerted { last_alert_at: now },
                Announce::Offline
            )
        );
    }

    /// A node silent for LESS than the threshold is not offline as far as this
    /// machine is concerned, so a blip is entirely silent — no outage alert on
    /// the way down, and no recovery alert on the way back up. That second half
    /// matters: a recovery notice for an outage nobody was told about is pure
    /// confusion.
    #[test]
    fn blip_below_threshold_is_silent_in_both_directions() {
        let now = test_now();
        // `is_offline` is false for a sub-threshold gap (tick folds the
        // threshold in before calling decide).
        let (state, announce) = decide(Some(NodeState::Online), false, now, 0);
        assert_eq!(state, NodeState::Online);
        assert_eq!(announce, Announce::Nothing, "going quiet briefly is silent");

        let (_, announce) = decide(Some(state), false, now, 0);
        assert_eq!(announce, Announce::Nothing, "and so is coming back");
    }
}
