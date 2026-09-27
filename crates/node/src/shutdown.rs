//! v1.2.5: graceful shutdown, on SIGTERM / SIGINT or after a self-upgrade.
//!
//! What this buys, precisely — and what it does not:
//!
//! - **No lost billing.** Traffic that was counted but not yet reported is sent
//!   to the panel before the process exits. Until now every restart threw away
//!   up to one report interval (10 s by default) of counted bytes: traffic that
//!   was forwarded and never charged.
//! - **In-flight connections get a short window to finish** — up to
//!   `SHUTDOWN_DRAIN_SECS` (default 5). The listeners close first, so nothing
//!   new starts during the window, and the node exits the moment the last TCP
//!   connection ends.
//!
//! It does NOT keep a long-lived connection alive across a restart. A tunnel or
//! VPN that is still open when the window ends is cut, exactly as before; this
//! process is the one holding it, and it is going away. Handing live sockets to
//! the next process is a different and much larger change.
//!
//! The window is not free either: new connections are refused for its whole
//! length, on top of systemd's RestartSec. That is why it is short, why it ends
//! early when nothing is open, and why 0 turns it off.

use crate::config::NodeConfig;
use crate::forwarder::ForwarderManager;
use crate::reporter::{self, ConnectionTracker, TrafficCounter};
use std::sync::{Arc, OnceLock};
use std::time::Duration;
use tokio::sync::{Mutex, Notify};

/// How long one traffic flush may take. `report_traffic` has no timeout of its
/// own, so against a panel that stops answering it would otherwise hang until
/// systemd's stop timeout SIGKILLs the process — mid-flush. Two flushes plus
/// the longest drain (60 s) stay under systemd's default 90 s.
const FLUSH_TIMEOUT: Duration = Duration::from_secs(10);

/// How often the drain checks whether the last connection has closed.
const DRAIN_POLL: Duration = Duration::from_millis(200);

fn in_process_trigger() -> &'static Notify {
    static TRIGGER: OnceLock<Notify> = OnceLock::new();
    TRIGGER.get_or_init(Notify::new)
}

/// Ask for a graceful shutdown from inside the process. The self-upgrade calls
/// this after swapping the binary, instead of exiting on the spot. Safe to call
/// before anything is waiting: `notify_one` keeps a permit.
pub fn request() {
    in_process_trigger().notify_one();
}

/// Resolves on the first SIGTERM (systemctl stop/restart), SIGINT (Ctrl+C), or
/// [`request`].
pub async fn triggered() {
    tokio::select! {
        _ = os_signal() => tracing::warn!("shutdown: signal received"),
        _ = in_process_trigger().notified() => tracing::warn!("shutdown: requested (self-upgrade)"),
    }
}

#[cfg(unix)]
async fn os_signal() {
    use tokio::signal::unix::{signal, SignalKind};
    match signal(SignalKind::terminate()) {
        Ok(mut term) => {
            tokio::select! {
                _ = term.recv() => {}
                _ = tokio::signal::ctrl_c() => {}
            }
        }
        Err(e) => {
            // Without a SIGTERM handler the default action still terminates the
            // process — just not gracefully. Keep listening for Ctrl+C.
            tracing::warn!("shutdown: cannot install SIGTERM handler: {}", e);
            let _ = tokio::signal::ctrl_c().await;
        }
    }
}

#[cfg(not(unix))]
async fn os_signal() {
    let _ = tokio::signal::ctrl_c().await;
}

/// Run the shutdown sequence. The caller exits the process afterwards.
pub async fn run(
    config: &NodeConfig,
    manager: &Arc<Mutex<ForwarderManager>>,
    counter: &TrafficCounter,
    connections: &ConnectionTracker,
) {
    let drain = Duration::from_secs(config.shutdown_drain_secs);

    let closed = manager.lock().await.stop_accepting().await;
    let open = connections.tcp_active();
    tracing::warn!(
        "shutdown: closed {} listener(s); {} TCP connection(s) open, draining up to {}s",
        closed,
        open,
        drain.as_secs()
    );

    // Flush what is counted so far BEFORE waiting. If the drain is then cut
    // short — a second signal, or systemd's SIGKILL — the bulk is already billed
    // and only the drain window's own bytes are at risk.
    flush(config, counter, "before drain").await;

    if open > 0 && !drain.is_zero() {
        tokio::select! {
            _ = drain_until_idle(connections, drain) => {}
            // A second signal means "stop waiting", not "skip the billing": the
            // final flush below still runs.
            _ = os_signal() => tracing::warn!("shutdown: second signal, cutting the drain short"),
        }
    }

    let left = connections.tcp_active();
    if left > 0 {
        tracing::warn!(
            "shutdown: {} connection(s) still open; they close with the process",
            left
        );
    }
    flush(config, counter, "final").await;
}

/// Wait until no TCP connection is open, or `window` has passed.
async fn drain_until_idle(connections: &ConnectionTracker, window: Duration) {
    let deadline = tokio::time::Instant::now() + window;
    while connections.tcp_active() > 0 && tokio::time::Instant::now() < deadline {
        tokio::time::sleep(DRAIN_POLL).await;
    }
}

async fn flush(config: &NodeConfig, counter: &TrafficCounter, stage: &str) {
    if tokio::time::timeout(FLUSH_TIMEOUT, reporter::report_traffic(config, counter))
        .await
        .is_err()
    {
        tracing::warn!(
            "shutdown: traffic flush ({}) got no answer within {}s; bytes counted since the last report are lost",
            stage,
            FLUSH_TIMEOUT.as_secs()
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The drain must end the moment the last connection closes — a node with
    /// nothing open should not sit out the whole window refusing connections.
    #[tokio::test]
    async fn the_drain_ends_as_soon_as_the_last_connection_closes() {
        let conns = ConnectionTracker::new();
        let guard = conns.tcp_handle();
        let started = tokio::time::Instant::now();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(300)).await;
            drop(guard);
        });
        drain_until_idle(&conns, Duration::from_secs(30)).await;
        let took = started.elapsed();
        assert!(
            took < Duration::from_secs(2),
            "should stop right after the connection closes, took {:?}",
            took
        );
    }

    /// A connection that never closes is cut when the window ends — the drain
    /// is bounded, whatever the traffic is doing.
    #[tokio::test]
    async fn the_drain_gives_up_when_the_window_ends() {
        let conns = ConnectionTracker::new();
        let _held = conns.tcp_handle();
        let started = tokio::time::Instant::now();
        drain_until_idle(&conns, Duration::from_millis(500)).await;
        let took = started.elapsed();
        assert!(
            took >= Duration::from_millis(500),
            "must wait the window out, took {:?}",
            took
        );
        assert!(
            took < Duration::from_secs(2),
            "must not overrun the window, took {:?}",
            took
        );
    }

    /// A request made before anyone is waiting must not be lost — the
    /// self-upgrade can finish before the shutdown task first polls.
    #[tokio::test]
    async fn a_request_made_early_is_not_lost() {
        request();
        tokio::time::timeout(Duration::from_secs(2), triggered())
            .await
            .expect("an earlier request() must still trigger shutdown");
    }
}
