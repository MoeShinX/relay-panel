//! v1.3.0: admin audit trail.
//!
//! Destructive operations already emitted a `tracing` line with
//! `action = "..."`, but the process log rotates, dies with the container, and
//! is invisible from the panel. "Who deleted my rule" needs an answer the
//! operator can look up.
//!
//! The tracing calls are kept as-is: they are useful when tailing logs, and
//! removing them would make an ops regression out of a product feature.

use crate::api::AppState;
use crate::db::repo::NewAuditEntry;

/// UTC 'YYYY-MM-DD HH:MM:SS' — the format every other timestamp column uses,
/// and the one whose lexicographic order is chronological (retention compares
/// it as TEXT).
fn now_utc() -> String {
    chrono::Utc::now().format("%Y-%m-%d %H:%M:%S").to_string()
}

/// Append one entry.
///
/// **Best-effort on purpose.** This is called AFTER the operation succeeded, so
/// a failure here means the action happened but was not recorded. That is a
/// real gap, and it is still better than the alternative: returning an error
/// would make a full audit table (or a transient DB blip) start rejecting user
/// deletions and token rotations. The failure is logged at error level so it is
/// visible rather than silent.
///
/// `detail` MUST NOT contain secrets. Record that a token was rotated, never
/// the new token; that notification settings changed, never the credentials.
pub async fn record(
    state: &AppState,
    actor_id: Option<i64>,
    action: &str,
    target_type: &str,
    target_id: impl std::fmt::Display,
    detail: &str,
) {
    // The actor's name is resolved HERE and stored, not joined at read time, so
    // the record survives deletion of the account that made it.
    //
    // It costs one extra lookup per audited action. That is affordable because
    // audited actions are destructive ones — a few per day — and the
    // alternatives were worse: adding the name to AuthUser would put a DB query
    // on every authenticated request, and putting it in the JWT would leave
    // already-issued tokens without it.
    //
    // A failed lookup falls back to "#id" rather than dropping the entry: who
    // it was matters less than that it happened.
    let actor_name = match actor_id {
        Some(id) => {
            match crate::db::repo::UserRepository::find_by_id(state.db.as_ref(), id).await {
                Ok(Some(u)) => u.username,
                _ => format!("#{id}"),
            }
        }
        None => "system".to_string(),
    };

    let entry = NewAuditEntry {
        ts: now_utc(),
        actor_id,
        actor_name,
        action: action.to_string(),
        target_type: target_type.to_string(),
        target_id: target_id.to_string(),
        detail: detail.to_string(),
    };
    if let Err(e) = state.db.record_audit(&entry).await {
        tracing::error!(
            "audit: failed to record {} on {} {}: {}",
            action,
            target_type,
            entry.target_id,
            e
        );
    }
}

/// Retention for the audit trail.
///
/// Far longer than the metric history (7 days) — the whole point is answering a
/// question asked weeks later — but still bounded, because an unbounded table
/// is its own operational problem. Admin actions are low volume (tens per day
/// at most), so a year is only tens of thousands of small rows.
pub const RETENTION_DAYS: i64 = 365;
