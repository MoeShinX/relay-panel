//! v1.2.11: one-click panel update — the panel's half.
//!
//! A node updates itself: it is a binary under systemd, so it swaps its own
//! file and exits. The panel cannot do that. It runs in a container, and a
//! container cannot replace its own image; the only way to do it from inside is
//! mounting the Docker socket, which hands the panel root on the host. The panel
//! is a public web app with self-registration — if it is ever breached, that
//! would turn a breach of the panel into a breach of the server.
//!
//! So the panel only ASKS. It drops a request file into a directory shared with
//! the host (`./run` on the host, `/app/run` here), and a systemd path unit
//! installed by deploy.sh runs `scripts/panel-updater.sh`, which does what an
//! operator would: `git pull` + `./deploy.sh`. The host reports back through a
//! status file in the same directory.
//!
//! What a compromised panel can do through this is exactly what the button
//! does: ask for an update to the official latest release. It cannot choose the
//! version — the host ignores the request file's contents and pulls the official
//! repository — and it cannot run anything on the host.

use crate::api::middleware::AdminOnly;
use crate::api::AppState;
use axum::{extract::State, Json};
use relay_shared::protocol::ApiResponse;
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// Written by deploy.sh once the systemd units are installed. Without it the
/// button would request an update that nothing on the host is waiting for.
const READY_FILE: &str = "updater-ready";
/// Dropped by the panel; its existence is what triggers the host.
const REQUEST_FILE: &str = "update-request";
/// Written by the host updater.
const STATUS_FILE: &str = "update-status.json";

/// A run the host last marked "running" longer ago than this is treated as
/// dead (the updater crashed or the host rebooted mid-update) so the button is
/// not locked forever. deploy.sh pulls images and waits up to a minute for the
/// panel; half an hour is far past any real run.
const STALE_RUN_SECS: u64 = 30 * 60;

/// Status files are small; anything past this is not ours to parse.
const MAX_STATUS_BYTES: u64 = 64 * 1024;

fn updater_dir() -> PathBuf {
    std::env::var("UPDATER_DIR")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| "/app/run".into())
        .into()
}

fn now_epoch() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// What the host wrote about its last run. Every field is optional: the file
/// may be from an older updater, or absent altogether.
#[derive(Debug, Clone, Default, Deserialize)]
struct HostStatus {
    #[serde(default)]
    state: String,
    #[serde(default)]
    from_version: String,
    #[serde(default)]
    to_version: String,
    #[serde(default)]
    started_at: u64,
    #[serde(default)]
    finished_at: u64,
    #[serde(default)]
    message: String,
    #[serde(default)]
    log_tail: String,
}

/// GET /system/panel-update
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct PanelUpdateStatus {
    /// The host updater is installed. False → the page keeps pointing at the
    /// manual steps, which also install it.
    pub available: bool,
    /// "idle" | "requested" | "running" | "succeeded" | "failed" | "up_to_date"
    /// | "pinned". "requested" = the panel dropped a request and the host has
    /// not picked it up yet.
    pub state: String,
    pub from_version: String,
    pub to_version: String,
    pub started_at: u64,
    pub finished_at: u64,
    pub message: String,
    pub log_tail: String,
}

fn read_host_status(dir: &Path) -> Option<HostStatus> {
    let path = dir.join(STATUS_FILE);
    let meta = std::fs::metadata(&path).ok()?;
    if !meta.is_file() || meta.len() > MAX_STATUS_BYTES {
        return None;
    }
    let raw = std::fs::read(&path).ok()?;
    serde_json::from_slice(&raw).ok()
}

/// Is an update underway (so a second one must not be started)?
fn in_progress(dir: &Path, status: Option<&HostStatus>, now: u64) -> bool {
    if dir.join(REQUEST_FILE).exists() {
        return true;
    }
    matches!(status, Some(s) if s.state == "running" && now.saturating_sub(s.started_at) < STALE_RUN_SECS)
}

fn current_status(dir: &Path, now: u64) -> PanelUpdateStatus {
    let available = dir.join(READY_FILE).is_file();
    let host = read_host_status(dir);
    let requested = dir.join(REQUEST_FILE).exists();
    let h = host.clone().unwrap_or_default();

    let state = if requested {
        "requested".to_string()
    } else if h.state == "running" && now.saturating_sub(h.started_at) >= STALE_RUN_SECS {
        // Never finished: report it as a failure rather than "running" forever.
        "failed".to_string()
    } else if h.state.is_empty() {
        "idle".to_string()
    } else {
        h.state.clone()
    };
    let message = if state == "failed" && h.state == "running" {
        "The updater stopped without reporting a result. Check /var/log/relaypanel-updater.log on the server."
            .to_string()
    } else {
        h.message
    };

    PanelUpdateStatus {
        available,
        state,
        from_version: h.from_version,
        to_version: h.to_version,
        started_at: h.started_at,
        finished_at: h.finished_at,
        message,
        log_tail: h.log_tail,
    }
}

#[derive(Debug, PartialEq, Eq)]
enum RequestError {
    NotInstalled,
    InProgress,
    Io(String),
}

/// Drop the request file. `create_new` makes the "is one already pending?"
/// check and the write a single step, so two clicks cannot both succeed.
fn write_request(
    dir: &Path,
    target: &str,
    requested_by: &str,
    now: u64,
) -> Result<(), RequestError> {
    if !dir.join(READY_FILE).is_file() {
        return Err(RequestError::NotInstalled);
    }
    if in_progress(dir, read_host_status(dir).as_ref(), now) {
        return Err(RequestError::InProgress);
    }
    let mut f = match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(dir.join(REQUEST_FILE))
    {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            return Err(RequestError::InProgress)
        }
        Err(e) => return Err(RequestError::Io(e.to_string())),
    };
    // Informational only: the host does not read it. It exists so an operator
    // looking at the file can see who asked and for what.
    let body = serde_json::json!({
        "target": target,
        "requested_by": requested_by,
        "requested_at": now,
    });
    f.write_all(body.to_string().as_bytes())
        .map_err(|e| RequestError::Io(e.to_string()))
}

pub async fn get_panel_update(_admin: AdminOnly) -> Json<ApiResponse<PanelUpdateStatus>> {
    Json(ApiResponse::success(current_status(
        &updater_dir(),
        now_epoch(),
    )))
}

#[derive(Debug, Serialize)]
pub struct PanelUpdateStarted {
    pub target: String,
}

/// POST /system/panel-update — ask the host to update the panel.
pub async fn start_panel_update(
    admin: AdminOnly,
    State(state): State<AppState>,
) -> Json<ApiResponse<PanelUpdateStarted>> {
    let err = |code: i32, message: &str| {
        Json(ApiResponse {
            code,
            message: message.into(),
            data: None,
        })
    };

    // Only ever towards a newer official release. The host would pull the
    // official repository anyway; checking here keeps a click on an up-to-date
    // panel from restarting it for nothing.
    let target = match state.release_cache.resolve_latest_panel_version().await {
        Ok(Some(tag)) => tag,
        Ok(None) => return err(400, "no panel release found"),
        Err(e) => {
            tracing::warn!("panel update: version check failed: {}", e);
            return err(502, "could not check for the latest release");
        }
    };
    let current = crate::config::app_version();
    match (
        crate::api::system::parse_version(current),
        crate::api::system::parse_version(&target),
    ) {
        (Some(c), Some(t)) if t > c => {}
        _ => return err(400, "the panel is already up to date"),
    }

    let requested_by =
        match crate::db::repo::UserRepository::find_by_id(state.db.as_ref(), admin.user_id).await {
            Ok(Some(u)) => u.username,
            _ => format!("#{}", admin.user_id),
        };

    match write_request(&updater_dir(), &target, &requested_by, now_epoch()) {
        Ok(()) => {}
        Err(RequestError::NotInstalled) => {
            return err(
                409,
                "the updater is not installed on this server; update manually once to install it",
            )
        }
        Err(RequestError::InProgress) => return err(409, "an update is already in progress"),
        Err(RequestError::Io(e)) => {
            tracing::error!("panel update: writing the request failed: {}", e);
            return err(500, "could not write the update request");
        }
    }

    // Recorded as REQUESTED, the same honesty as the node upgrade: the outcome
    // is decided on the host, afterwards, and shows as the version this panel
    // reports once it is back.
    crate::service::audit::record(
        &state,
        Some(admin.user_id),
        "upgrade_panel",
        "panel",
        current,
        &format!("已请求 {current} → {target}（结果以面板重启后的版本为准）"),
    )
    .await;

    Json(ApiResponse::success(PanelUpdateStarted { target }))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fresh directory per test: pid + clock + a counter, so parallel tests
    /// never share one.
    fn tmp() -> PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};
        static N: AtomicU64 = AtomicU64::new(0);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let dir = std::env::temp_dir().join(format!(
            "rp-panel-update-{}-{}-{}",
            std::process::id(),
            nanos,
            N.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn ready(dir: &Path) {
        std::fs::write(dir.join(READY_FILE), "{\"version\":1}").unwrap();
    }

    fn host_status(dir: &Path, json: &str) {
        std::fs::write(dir.join(STATUS_FILE), json).unwrap();
    }

    /// Without the host updater the button must not pretend to work: a request
    /// file with nothing watching it would sit there forever.
    #[test]
    fn a_request_needs_the_updater_to_be_installed() {
        let dir = tmp();
        assert_eq!(
            write_request(&dir, "v9.9.9", "admin", 100),
            Err(RequestError::NotInstalled)
        );
        assert!(!dir.join(REQUEST_FILE).exists());
        assert!(!current_status(&dir, 100).available);
    }

    #[test]
    fn a_request_is_written_and_shows_as_requested() {
        let dir = tmp();
        ready(&dir);
        write_request(&dir, "v9.9.9", "admin", 100).unwrap();
        assert!(dir.join(REQUEST_FILE).is_file());
        let st = current_status(&dir, 101);
        assert!(st.available);
        assert_eq!(st.state, "requested");
    }

    /// Two clicks (or two admins) must not queue two updates.
    #[test]
    fn a_second_request_is_refused_while_one_is_pending_or_running() {
        let dir = tmp();
        ready(&dir);
        write_request(&dir, "v9.9.9", "admin", 100).unwrap();
        assert_eq!(
            write_request(&dir, "v9.9.9", "admin", 101),
            Err(RequestError::InProgress)
        );

        // The host consumed the request and is working on it.
        std::fs::remove_file(dir.join(REQUEST_FILE)).unwrap();
        host_status(&dir, r#"{"state":"running","started_at":100}"#);
        assert_eq!(
            write_request(&dir, "v9.9.9", "admin", 200),
            Err(RequestError::InProgress)
        );
    }

    /// An updater that died mid-run must not lock the button forever, and the
    /// page must say it failed rather than show "running" indefinitely.
    #[test]
    fn a_run_that_never_finished_turns_into_a_failure() {
        let dir = tmp();
        ready(&dir);
        host_status(&dir, r#"{"state":"running","started_at":100}"#);
        let later = 100 + STALE_RUN_SECS + 1;
        let st = current_status(&dir, later);
        assert_eq!(st.state, "failed");
        assert!(st.message.contains("relaypanel-updater.log"));
        write_request(&dir, "v9.9.9", "admin", later)
            .expect("a stale run must not block a new request");
    }

    #[test]
    fn the_host_result_is_passed_through() {
        let dir = tmp();
        ready(&dir);
        host_status(
            &dir,
            r#"{"state":"failed","from_version":"1.2.10","to_version":"","started_at":5,"finished_at":9,"message":"git pull failed","log_tail":"fatal: local changes"}"#,
        );
        let st = current_status(&dir, 10);
        assert_eq!(st.state, "failed");
        assert_eq!(st.from_version, "1.2.10");
        assert_eq!(st.message, "git pull failed");
        assert_eq!(st.log_tail, "fatal: local changes");
        // A finished run does not block the next attempt.
        write_request(&dir, "v9.9.9", "admin", 11).unwrap();
    }

    /// The status file sits in a directory the host writes; anything malformed
    /// or oversized must read as "no information", never as an error page.
    #[test]
    fn a_garbled_or_huge_status_file_is_ignored() {
        let dir = tmp();
        ready(&dir);
        host_status(&dir, "not json");
        assert_eq!(current_status(&dir, 1).state, "idle");
        host_status(&dir, &"x".repeat((MAX_STATUS_BYTES + 1) as usize));
        assert_eq!(current_status(&dir, 1).state, "idle");
    }
}
