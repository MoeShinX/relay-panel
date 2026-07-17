//! v1.2.0: per-rule runtime controls — the concurrent-connection cap and the
//! restart cancellation channel.
//!
//! ## Why this state lives per RULE, not per listener
//!
//! A single TCP rule binds up to TWO listeners (IPv4 and IPv6, see the
//! `(Protocol::Tcp, NodeTransport::Raw)` arm in `manager::apply_config`). If the
//! live-connection counter were owned by a listener, a dual-stack rule would
//! admit `max_connections` on v4 PLUS `max_connections` on v6 — silently double
//! the configured cap. The counter therefore hangs off the rule and both
//! listeners share one `Arc`, exactly like the rule's rate limiter.
//!
//! ## Why a cancellation channel is needed at all
//!
//! Each accepted connection is driven by a DETACHED `tokio::spawn` task.
//! Aborting the accept-loop task does NOT cascade to those children: the
//! listener stops accepting, but every established connection keeps forwarding
//! (verified empirically — a post-abort read/write round-trips fine). So
//! "restart a rule" cannot be implemented as abort-and-rebind; that would only
//! re-bind the port and shed nothing, which is the entire point of the feature.
//!
//! Instead every connection task selects on `RuleGate::cancel`. Bumping the
//! generation via `RuleRuntime::cancel_all` wakes them all, they drop their
//! sockets, and the peers see the connection close.
//!
//! ## Why cancellation is NOT wired into `apply_config`
//!
//! A fingerprint change (new targets, new rate cap) tears the listener down and
//! rebuilds it, but deliberately leaves live connections alone — that has been
//! the behaviour since v0.3.6 and users rely on editing a rule without kicking
//! everyone off. Only an EXPLICIT restart cancels. The two paths are separate on
//! purpose: `apply_config` never touches the generation.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::sync::watch;

/// The half of a rule's runtime state handed to its listeners. Cheap to clone;
/// a rule's v4 and v6 listeners each hold one and they share the same counter
/// and cancellation channel.
#[derive(Clone)]
pub struct RuleGate {
    /// Concurrent TCP connections allowed for this rule ON THIS NODE. `None` =
    /// unlimited (the default, and every pre-v1.2 rule after migration).
    pub max_connections: Option<u32>,
    /// Live TCP connections for this rule right now. Incremented by
    /// [`RuleGate::admit`], decremented when the returned guard drops.
    live: Arc<AtomicU64>,
    /// Restart signal. The value is a generation counter; any change means
    /// "drop the connection you are driving".
    cancel: watch::Receiver<u64>,
}

impl RuleGate {
    /// Try to admit one new connection.
    ///
    /// Returns `None` when the rule is at its cap — the caller must drop the
    /// accepted socket immediately. Returns a guard otherwise; hold it for the
    /// connection's lifetime, and the count decrements however the task ends
    /// (clean close, error, panic, or restart cancellation).
    ///
    /// MUST be called from the accept loop rather than from inside the spawned
    /// connection task. The accept loop is sequential, so check-then-increment
    /// here is atomic with respect to other accepts on the same listener; doing
    /// it inside the task would let an unbounded number of accepts slip through
    /// before the first increment lands — precisely the connection-flood case
    /// this cap exists for. (The v4 and v6 accept loops do race each other, but
    /// `fetch_add`-then-compare below makes the counter exact regardless.)
    pub fn admit(&self) -> Option<ConnGuard> {
        let guard = ConnGuard {
            live: self.live.clone(),
        };
        // Increment FIRST, then compare, so two accept loops (v4 + v6) racing on
        // the same rule can never both observe "one slot left" and both take it.
        // The guard is already constructed, so an over-cap increment is undone
        // by dropping it on the reject path.
        let now = self.live.fetch_add(1, Ordering::AcqRel) + 1;
        match self.max_connections {
            Some(cap) if now > cap as u64 => None, // `guard` drops → decrements
            _ => Some(guard),
        }
    }

    /// Live connection count for this rule on this node.
    pub fn live(&self) -> u64 {
        self.live.load(Ordering::Relaxed)
    }

    /// Resolves when this rule is restarted. Connection tasks select on it and
    /// drop their sockets when it fires.
    ///
    /// Also resolves if the sender is gone (the rule was deleted from the
    /// node's config), which is the right outcome: a deleted rule's traffic can
    /// no longer be attributed or billed, so its connections must not outlive
    /// it.
    pub async fn cancelled(&mut self) {
        // `changed()` errors only when the sender dropped; treat that as a
        // cancel rather than parking forever on a dead channel.
        let _ = self.cancel.changed().await;
    }
}

/// A rule's runtime state as the manager owns it. Dropping this cancels every
/// connection task belonging to the rule (the `watch::Sender` goes away and each
/// task's `cancelled()` resolves).
pub struct RuleRuntime {
    live: Arc<AtomicU64>,
    cancel: watch::Sender<u64>,
}

impl RuleRuntime {
    pub fn new() -> Self {
        let (cancel, _) = watch::channel(0);
        Self {
            live: Arc::new(AtomicU64::new(0)),
            cancel,
        }
    }

    /// Build the listener-side handle. `max_connections` is passed per-spawn
    /// rather than stored, because it comes from the config being applied and
    /// changing it restarts the listener anyway (it is part of the fingerprint).
    pub fn gate(&self, max_connections: Option<u32>) -> RuleGate {
        RuleGate {
            max_connections,
            live: self.live.clone(),
            cancel: self.cancel.subscribe(),
        }
    }

    /// Drop every in-flight connection of this rule. Returns the number of
    /// connections that were live at the moment of cancellation — they tear
    /// down asynchronously, so the counter drains shortly after this returns
    /// rather than being 0 immediately.
    pub fn cancel_all(&self) -> u64 {
        let live = self.live.load(Ordering::Relaxed);
        // send_modify notifies every receiver even with no listeners attached,
        // and bumping the generation guarantees a value CHANGE (send_if_modified
        // with an equal value would not wake anyone).
        self.cancel.send_modify(|generation| *generation += 1);
        live
    }
}

impl Default for RuleRuntime {
    fn default() -> Self {
        Self::new()
    }
}

/// RAII: decrements the rule's live-connection count when the connection task
/// ends, however it ends.
pub struct ConnGuard {
    live: Arc<AtomicU64>,
}

impl Drop for ConnGuard {
    fn drop(&mut self) {
        self.live.fetch_sub(1, Ordering::AcqRel);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The cap admits exactly `max_connections` and rejects the next one; a
    /// closed connection frees its slot.
    #[test]
    fn admit_enforces_cap_and_guard_frees_slot() {
        let rt = RuleRuntime::new();
        let gate = rt.gate(Some(2));

        let g1 = gate.admit().expect("1st under cap");
        let g2 = gate.admit().expect("2nd hits cap exactly, still admitted");
        assert_eq!(gate.live(), 2);

        assert!(gate.admit().is_none(), "3rd must be rejected at cap 2");
        // The rejected admit must NOT leak a slot — the count is still 2, not 3.
        assert_eq!(
            gate.live(),
            2,
            "a rejected admit must not inflate the count"
        );

        drop(g1);
        assert_eq!(gate.live(), 1);
        let _g3 = gate.admit().expect("slot freed by the closed connection");
        assert_eq!(gate.live(), 2);
        drop(g2);
    }

    /// `None` = unlimited. This is the migration default for every existing
    /// rule, so getting it wrong would cap the whole fleet at zero on upgrade.
    #[test]
    fn no_cap_admits_freely() {
        let rt = RuleRuntime::new();
        let gate = rt.gate(None);
        let guards: Vec<_> = (0..1000)
            .map(|_| gate.admit().expect("unlimited"))
            .collect();
        assert_eq!(gate.live(), 1000);
        drop(guards);
        assert_eq!(gate.live(), 0);
    }

    /// The v4 and v6 listeners of one rule share a counter, so a dual-stack
    /// rule admits `max_connections` IN TOTAL — not that many per family.
    #[test]
    fn dual_stack_listeners_share_one_budget() {
        let rt = RuleRuntime::new();
        let v4 = rt.gate(Some(3));
        let v6 = rt.gate(Some(3));

        let _a = v4.admit().expect("v4 #1");
        let _b = v6.admit().expect("v6 #1");
        let _c = v4.admit().expect("v4 #2 — 3 total");
        assert!(
            v6.admit().is_none(),
            "the 4th connection must be rejected regardless of family"
        );
        assert_eq!(v4.live(), 3);
    }

    /// cancel_all wakes a task parked on `cancelled()`. This is what makes a
    /// restart actually shed connections instead of only rebinding the port.
    #[tokio::test]
    async fn cancel_all_wakes_connection_tasks() {
        let rt = RuleRuntime::new();
        let mut gate = rt.gate(None);
        let _g = gate.admit().unwrap();

        let waiter = tokio::spawn(async move {
            gate.cancelled().await;
            "cancelled"
        });

        // Nothing has cancelled yet — the task must still be parked.
        tokio::task::yield_now().await;
        assert!(!waiter.is_finished(), "must not fire before cancel_all");

        assert_eq!(rt.cancel_all(), 1, "reports the connections it is dropping");
        assert_eq!(
            tokio::time::timeout(std::time::Duration::from_secs(1), waiter)
                .await
                .expect("cancelled() must resolve promptly")
                .unwrap(),
            "cancelled"
        );
    }

    /// A deleted rule (manager drops its RuleRuntime) must also drop its
    /// connections, not leave them forwarding for a rule that no longer exists.
    #[tokio::test]
    async fn dropping_runtime_cancels_connections() {
        let rt = RuleRuntime::new();
        let mut gate = rt.gate(None);

        let waiter = tokio::spawn(async move {
            gate.cancelled().await;
        });
        tokio::task::yield_now().await;
        assert!(!waiter.is_finished());

        drop(rt);
        tokio::time::timeout(std::time::Duration::from_secs(1), waiter)
            .await
            .expect("a dropped RuleRuntime must cancel its connections")
            .unwrap();
    }
}
