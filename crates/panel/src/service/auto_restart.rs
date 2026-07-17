//! v1.2.0: scheduled rule restarts.
//!
//! A rule with `auto_restart_minutes > 0` gets its connections dropped and its
//! listeners rebuilt on that interval. This is the safety valve for rules whose
//! connections accumulate faster than they drain (the `max_connections` cap is
//! the actual fix — this is for when you'd rather shed than refuse).
//!
//! ## Why the schedule is in memory, not in the database
//!
//! The obvious design stores `last_restart_at` per rule. It survives a panel
//! restart, which sounds good until you consider what "survives" means here: on
//! boot, every rule whose interval elapsed while the panel was down comes due at
//! once, and the panel's first act is to drop every connection on every
//! auto-restart rule simultaneously. A panel restart (an upgrade, a container
//! reschedule) would become a fleet-wide disconnect.
//!
//! Keeping the schedule in memory means a panel restart re-bases every rule's
//! timer to "now". The cost is that a rule can go up to one extra interval
//! without a restart across a panel bounce. That is strictly the safer failure:
//! this feature exists to shed load periodically, and skipping one cycle is
//! invisible, while an unscheduled mass disconnect is an incident.
//!
//! For the same reason a rule seen for the FIRST time is only baselined, never
//! restarted on the spot — see `tick`.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use crate::api::restart::{dispatch_restart, NodeRestartStatus};
use crate::api::AppState;

/// How often the scheduler wakes. The finest interval a user can configure is
/// `MIN_AUTO_RESTART_MINUTES`, so a 60s tick is comfortably precise enough and
/// costs one indexed query per minute.
const TICK: Duration = Duration::from_secs(60);

/// Tracks when each rule was last restarted BY THIS SCHEDULER. Keyed by rule_id.
///
/// A manual restart deliberately does NOT reset these timers: the two are
/// independent controls, and letting a manual restart postpone the scheduled one
/// would make the schedule silently drift by an amount that depends on operator
/// behaviour.
type Schedule = HashMap<i64, Instant>;

/// Start the scheduler. Returns immediately; the loop runs until the process
/// exits.
pub fn spawn(state: AppState) {
    tokio::spawn(async move {
        let mut schedule: Schedule = HashMap::new();
        let mut ticker = tokio::time::interval(TICK);
        // Skip missed ticks rather than firing them back-to-back if the loop is
        // ever delayed — a burst of catch-up ticks would restart rules far more
        // often than configured.
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        tracing::info!("auto-restart scheduler started (tick {}s)", TICK.as_secs());
        loop {
            ticker.tick().await;
            tick(&state, &mut schedule, Instant::now()).await;
        }
    });
}

/// One scheduler pass. Split out from the loop so it is testable without
/// waiting on wall-clock time (`now` is injected).
async fn tick(state: &AppState, schedule: &mut Schedule, now: Instant) {
    let rules = match state.db.list_auto_restart_rules().await {
        Ok(r) => r,
        Err(e) => {
            // Transient DB trouble must not kill the scheduler — skip this tick.
            tracing::error!("auto-restart: listing rules failed: {}; skipping tick", e);
            return;
        }
    };

    // Forget rules that no longer auto-restart (disabled, paused, or deleted),
    // so a rule that gets re-enabled later is baselined fresh rather than
    // immediately restarted against a stale timestamp.
    let live: std::collections::HashSet<i64> = rules.iter().map(|(id, _, _)| *id).collect();
    schedule.retain(|rule_id, _| live.contains(rule_id));

    for (rule_id, group_id, minutes) in rules {
        // Defensive: the API validates this, but a hand-edited DB row must not
        // turn into a restart-every-tick loop.
        if minutes < relay_shared::models::MIN_AUTO_RESTART_MINUTES {
            tracing::warn!(
                "auto-restart: rule {} has interval {}min below the {}min floor; skipping",
                rule_id,
                minutes,
                relay_shared::models::MIN_AUTO_RESTART_MINUTES
            );
            continue;
        }

        let due_after = Duration::from_secs(minutes as u64 * 60);
        match schedule.get(&rule_id) {
            // First sight: baseline only. Restarting here would mean every panel
            // start drops every auto-restart rule's connections at once.
            None => {
                schedule.insert(rule_id, now);
                tracing::debug!(
                    "auto-restart: rule {} baselined, first restart in {}min",
                    rule_id,
                    minutes
                );
            }
            Some(&last) if now.duration_since(last) >= due_after => {
                schedule.insert(rule_id, now);
                // Unique per RUN, not per rule. The id exists to tie a panel log
                // line to the matching node log line; a constant id makes every
                // restart of this rule indistinguishable in the logs — exactly
                // when you're reading them to find out which run misbehaved.
                let request_id = format!(
                    "auto-{}-{}",
                    rule_id,
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_millis())
                        .unwrap_or(0)
                );
                match dispatch_restart(state, rule_id, group_id, &request_id).await {
                    Ok(nodes) => {
                        let reached = nodes
                            .iter()
                            .filter(|n| matches!(n, NodeRestartStatus::Restarted { .. }))
                            .count();
                        tracing::info!(
                            "auto-restart: rule {} restarted on {}/{} node(s) (every {}min)",
                            rule_id,
                            reached,
                            nodes.len(),
                            minutes
                        );
                    }
                    Err(e) => {
                        // The timer was already advanced, so a failing rule
                        // retries on its next interval instead of every tick.
                        tracing::error!("auto-restart: rule {} dispatch failed: {}", rule_id, e);
                    }
                }
            }
            Some(_) => {} // not due yet
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The scheduling decision, isolated from AppState/DB/WS. Mirrors the
    /// `match` in `tick`: returns whether the rule is due, given its last-seen
    /// time.
    fn is_due(schedule: &mut Schedule, rule_id: i64, minutes: i32, now: Instant) -> bool {
        let due_after = Duration::from_secs(minutes as u64 * 60);
        match schedule.get(&rule_id) {
            None => {
                schedule.insert(rule_id, now);
                false
            }
            Some(&last) if now.duration_since(last) >= due_after => {
                schedule.insert(rule_id, now);
                true
            }
            Some(_) => false,
        }
    }

    /// A rule the scheduler has never seen is BASELINED, not restarted.
    ///
    /// This is the property that keeps a panel restart from becoming a fleet
    /// disconnect: on boot the schedule is empty, so every auto-restart rule
    /// hits this path at once. If it returned "due", every one of them would
    /// drop its connections simultaneously.
    #[test]
    fn first_sight_baselines_instead_of_restarting() {
        let mut s = Schedule::new();
        let t0 = Instant::now();
        assert!(!is_due(&mut s, 1, 5, t0), "first sight must NOT restart");
        assert_eq!(s.get(&1), Some(&t0), "first sight must record a baseline");
    }

    #[test]
    fn restarts_only_once_the_interval_has_elapsed() {
        let mut s = Schedule::new();
        let t0 = Instant::now();
        is_due(&mut s, 1, 5, t0); // baseline

        assert!(
            !is_due(&mut s, 1, 5, t0 + Duration::from_secs(4 * 60)),
            "4min into a 5min interval is not due"
        );
        assert!(
            is_due(&mut s, 1, 5, t0 + Duration::from_secs(5 * 60)),
            "exactly 5min is due"
        );
        // Firing resets the timer — it must not fire again on the next tick.
        assert!(
            !is_due(&mut s, 1, 5, t0 + Duration::from_secs(6 * 60)),
            "must not re-fire one minute after firing"
        );
        assert!(
            is_due(&mut s, 1, 5, t0 + Duration::from_secs(10 * 60)),
            "due again a full interval after the last fire"
        );
    }

    /// A rule that stops auto-restarting is forgotten, so re-enabling it later
    /// baselines fresh rather than firing immediately off a stale timestamp.
    #[test]
    fn disabled_rule_is_forgotten_and_rebaselined_on_return() {
        let mut s = Schedule::new();
        let t0 = Instant::now();
        is_due(&mut s, 1, 5, t0);

        // Rule disappears from the query (disabled/paused/deleted).
        let live: std::collections::HashSet<i64> = std::collections::HashSet::new();
        s.retain(|id, _| live.contains(id));
        assert!(
            s.is_empty(),
            "a rule that stopped auto-restarting is dropped"
        );

        // It comes back much later: must baseline, not fire instantly.
        let t1 = t0 + Duration::from_secs(60 * 60);
        assert!(
            !is_due(&mut s, 1, 5, t1),
            "a re-enabled rule must baseline, not fire off its stale timestamp"
        );
    }
}
