// UDP forwarding engine with session-based routing.
//
// Architecture per listen port:
//   - One bound UdpSocket `inbound`  (clients send to this)
//   - Per-client source addr, a dedicated UdpSocket `outbound` connected to the
//     chosen target. The outbound socket is used to both send datagrams to the
//     target AND receive the target's replies; replies are then forwarded back
//     to the client through `inbound`.
//
// This yields correct bidirectional UDP for protocols like DNS/QUIC where the
// reply comes from the target. A periodic task expires idle sessions.
//
// Session accounting: each unique (client_addr, rule_id) is one "connection"
// from the panel's point of view. We register/refresh it on every datagram
// via ConnectionTracker::udp_touch, and the tracker expires it after
// UDP_SESSION_TIMEOUT (60s) of inactivity. This makes the panel's
// "connections" column reflect real UDP activity instead of always 0.

use dashmap::mapref::entry::Entry;
use dashmap::DashMap;
use std::future::Future;
use std::net::{Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;
use tokio::net::UdpSocket;
use tokio::sync::oneshot;
use tokio::time;

use super::limiter::RateLimit;
use super::selector::TargetSelector;
use crate::reporter::{ConnectionTracker, TrafficCounter, UDP_SESSION_TIMEOUT};

const UDP_BUF_SIZE: usize = 65535;
/// How often the periodic sweeper runs. Sessions themselves expire on the
/// shared UDP_SESSION_TIMEOUT; this just controls how quickly an idle node
// converges back to 0 in the absence of new datagrams.
const CLEANUP_INTERVAL: Duration = Duration::from_secs(15);

type Sessions = DashMap<SocketAddr, UdpSession>;

struct UdpSession {
    outbound: Arc<UdpSocket>,
    last_active: tokio::time::Instant,
    /// v1.2.12: the session's reply reader waits on the other end. Dropping
    /// this — i.e. the session leaving the map by idle eviction or listener
    /// teardown — stops the reader. Before, eviction only dropped the map
    /// entry: the reader kept its socket and sat in `recv()` forever when the
    /// target never answered again, leaking a task and an fd per idle client.
    _stop: oneshot::Sender<()>,
}

/// v1.2.12: tears down a listener's session state when `serve_udp_listener`
/// returns or is aborted (rule removed / restarted). The sweeper is a detached
/// task that used to outlive its listener forever; clearing the map drops every
/// session's `_stop`, which ends all reply readers.
struct SessionsTeardown {
    sessions: Arc<Sessions>,
    sweeper: tokio::task::AbortHandle,
}

impl Drop for SessionsTeardown {
    fn drop(&mut self) {
        self.sweeper.abort();
        self.sessions.clear();
    }
}

/// v1.0.4: serve an ALREADY-BOUND UDP socket. Binding happens in the manager
/// (synchronously, so errors surface immediately and per-family success is
/// known). This function only runs the receive loop.
#[allow(clippy::too_many_arguments)]
pub async fn serve_udp_listener(
    inbound: Arc<UdpSocket>,
    targets: Vec<String>,
    selector: Arc<TargetSelector>,
    rate_limit: RateLimit,
    counter: Arc<TrafficCounter>,
    connections: Arc<ConnectionTracker>,
    rule_id: i64,
    source_ipv4: Option<Ipv4Addr>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let listen_addr = inbound
        .local_addr()
        .unwrap_or_else(|_| SocketAddr::from(([0, 0, 0, 0], 0)));
    if targets.is_empty() {
        tracing::warn!("UDP listener on {}: no targets configured", listen_addr);
    }
    tracing::info!("UDP listening on {} (rule {})", listen_addr, rule_id);

    let port = listen_addr.port();

    // v1.2.x: targets are resolved LAZILY per new session (see
    // select_udp_target) rather than once here at listener start. The old
    // boot-time resolution pinned a DDNS target to whatever IP it had when the
    // rule was pushed; the IP never refreshed until the rule/node restarted,
    // silently blackholing UDP (WireGuard / game / DNS-forward) traffic after a
    // DDNS update. Session-time resolution goes through the shared 30s DNS cache
    // so new sessions follow IP changes automatically.

    // v1.0.9: sharded concurrent map — per-packet lookups take a per-shard lock
    // (keyed by client addr) instead of one listener-wide mutex, so datagrams
    // from different clients don't serialize on each other.
    let sessions: Arc<Sessions> = Arc::new(DashMap::new());

    // Background cleanup of expired local session entries (outbound sockets).
    // This mirrors the ConnectionTracker's own expiry; together they make sure
    // idle UDP state is reclaimed promptly.
    let sessions_clone = sessions.clone();
    let connections_clone = connections.clone();
    let sweeper = tokio::spawn(async move {
        let mut interval = time::interval(CLEANUP_INTERVAL);
        loop {
            interval.tick().await;
            // Prune the tracker's session table (drops expired (addr,rule)
            // entries, which is what the panel's count ultimately reads).
            connections_clone.udp_prune_expired().await;
            // Drop our local outbound sockets for clients whose local entry is
            // older than the timeout. The tracker already stopped counting
            // them; here we release the socket resources too.
            let before = sessions_clone.len();
            sessions_clone.retain(|_, s| s.last_active.elapsed() < UDP_SESSION_TIMEOUT);
            // saturating: len() is read across shards without a global lock, so a
            // concurrent insert between the two reads must not underflow usize.
            let removed = before.saturating_sub(sessions_clone.len());
            if removed > 0 {
                tracing::debug!(
                    "UDP port {}: cleaned up {} expired outbound sockets",
                    port,
                    removed
                );
            }
        }
    });
    let _teardown = SessionsTeardown {
        sessions: sessions.clone(),
        sweeper: sweeper.abort_handle(),
    };

    let mut buf = vec![0u8; UDP_BUF_SIZE];
    loop {
        // v0.3.6: recv_from resilience. A transient error used to `?`-propagate
        // and kill the listener task, leaving the UDP port dead. Now transient
        // errors back off and retry; only a permanent error ends the task (and
        // the manager's is_finished recovery can restart it).
        let (n, src) = match inbound.recv_from(&mut buf).await {
            Ok(v) => v,
            Err(e) if is_transient_recv_error(&e) => {
                tracing::warn!(
                    "UDP listener on {} (rule {}): transient recv_from error: {}; retrying in 100ms",
                    listen_addr,
                    rule_id,
                    e
                );
                tokio::time::sleep(Duration::from_millis(100)).await;
                continue;
            }
            Err(e) => return Err(Box::new(e) as Box<dyn std::error::Error + Send + Sync>),
        };

        // Register/refresh this client with the tracker on EVERY datagram. The
        // tracker is a sharded DashMap (keyed by client+rule), so this is a cheap
        // per-shard op — not a process-wide lock — and keeps the panel's count
        // accurate without any throttling.
        connections.udp_touch(src, rule_id).await;

        // Fast path: existing session. The session map is a sharded DashMap, so
        // this per-packet lookup takes only a per-shard lock (sync guard, dropped
        // before any .await).
        let existing = sessions.get_mut(&src).map(|mut s| {
            s.last_active = tokio::time::Instant::now();
            s.outbound.clone()
        });

        let outbound_sock = if let Some(sock) = existing {
            sock
        } else {
            // New session: bind an ephemeral outbound socket + pick/connect the
            // target, all WITHOUT holding any map guard.
            //
            // The bind happens ONCE, here, and deliberately not inside the
            // per-target attempt below. Binding is a purely local operation —
            // it fails when OUTBOUND_BIND_IPV4 names an address this host no
            // longer has, which says nothing about any target's health.
            // Attempting it per candidate would report that local fault to the
            // circuit breaker once per target and trip all of them, and would
            // replace this precise diagnostic with a message blaming the
            // targets.
            let outbound = match super::outbound::udp_outbound_socket(source_ipv4).await {
                Ok(s) => s,
                Err(e) => {
                    tracing::warn!("UDP port {}: failed to bind outbound: {}", port, e);
                    continue;
                }
            };
            // Try each target in selector order, resolving lazily: a healthy
            // primary costs one DNS lookup, as before. UDP affinity is still
            // per NEW session, but unlike the former first-result path a
            // connect failure can now fall through to a healthy standby.
            let Some(target) =
                connect_udp_target(&targets, &selector, port, |addr| outbound.connect(addr)).await
            else {
                tracing::warn!("UDP port {}: no reachable target for session", port);
                continue;
            };
            let outbound = Arc::new(outbound);

            // Publish via the entry API (per-shard lock, sync — no .await while
            // the guard is held). Double-check for a concurrent datagram from the
            // same client that won the race while we were connecting: if one did,
            // use the winner and drop ours.
            let now = tokio::time::Instant::now();
            let (chosen, reader_stop) = match sessions.entry(src) {
                Entry::Occupied(mut e) => {
                    e.get_mut().last_active = now;
                    (e.get().outbound.clone(), None)
                }
                Entry::Vacant(e) => {
                    let (stop_tx, stop_rx) = oneshot::channel();
                    e.insert(UdpSession {
                        outbound: outbound.clone(),
                        last_active: now,
                        _stop: stop_tx,
                    });
                    (outbound.clone(), Some(stop_rx))
                }
            };

            if let Some(stop) = reader_stop {
                // The tracker was already refreshed at the top of the loop; just
                // log the new session (the target is known only on this path).
                tracing::debug!(
                    "UDP port {}: new session {} -> {} (rule {})",
                    port,
                    src,
                    target,
                    rule_id
                );
                // Spawn the target -> client reader for OUR socket.
                let relay = ReplyRelay {
                    outbound: outbound.clone(),
                    inbound: inbound.clone(),
                    sessions: sessions.clone(),
                    connections: connections.clone(),
                    counter: counter.clone(),
                    rate_limit: rate_limit.clone(),
                    src,
                    rule_id,
                    port,
                };
                tokio::spawn(relay.run(stop));
            }
            chosen
        };

        // Forward client datagram to target via the connected outbound socket.
        // v0.4.6: throttle client→target (upload) bytes through the shared
        // per-rule limiter BEFORE sending.
        rate_limit.acquire_upload(n as u64).await;
        if let Err(e) = outbound_sock.send(&buf[..n]).await {
            tracing::debug!("UDP port {}: send to target failed: {}", port, e);
        } else {
            counter.add(rule_id, n as u64, 0).await;
        }
    }
}

/// The target -> client half of one UDP session: reads the target's replies on
/// the session's connected `outbound` socket and forwards them to `src`.
struct ReplyRelay {
    outbound: Arc<UdpSocket>,
    inbound: Arc<UdpSocket>,
    sessions: Arc<Sessions>,
    connections: Arc<ConnectionTracker>,
    counter: Arc<TrafficCounter>,
    rate_limit: RateLimit,
    src: SocketAddr,
    rule_id: i64,
    port: u16,
}

impl ReplyRelay {
    /// Runs until `stop` fires (the session left the map) or the socket fails.
    async fn run(self, mut stop: oneshot::Receiver<()>) {
        let mut rbuf = vec![0u8; UDP_BUF_SIZE];
        loop {
            let m = tokio::select! {
                // Evicted or torn down: the entry is already gone (and may by
                // now be a NEWER session for the same client), so there is
                // nothing of ours left to clean up.
                _ = &mut stop => return,
                r = self.outbound.recv(&mut rbuf) => match r {
                    Ok(m) => m,
                    Err(e) => {
                        tracing::debug!("UDP port {}: outbound recv ended: {}", self.port, e);
                        break;
                    }
                },
            };
            // v0.4.6: throttle target→client (download) bytes through the
            // shared per-rule limiter BEFORE forwarding back to the client.
            self.rate_limit.acquire_download(m as u64).await;
            self.counter.add(self.rule_id, 0, m as u64).await;
            // A reply is activity too: refresh the tracker (cheap, sharded) and
            // the session's last_active so a long request/response flow isn't
            // expired.
            self.connections.udp_touch(self.src, self.rule_id).await;
            if self.inbound.send_to(&rbuf[..m], self.src).await.is_err() {
                break;
            }
            if let Some(mut s) = self.sessions.get_mut(&self.src) {
                if Arc::ptr_eq(&s.outbound, &self.outbound) {
                    s.last_active = tokio::time::Instant::now();
                }
            }
        }
        // Our socket failed (target unreachable / error): release this
        // client's session now rather than waiting for the idle timeout. Only
        // if the entry is still OURS — v1.2.12: a plain remove(&src) here used
        // to delete a newer session that had replaced ours for the same client.
        let ours = |_: &SocketAddr, s: &UdpSession| Arc::ptr_eq(&s.outbound, &self.outbound);
        if self.sessions.remove_if(&self.src, ours).is_some() {
            self.connections.udp_close(self.src, self.rule_id).await;
        }
    }
}

/// Resolve the outbound target for a NEW UDP session, honoring the rule's
/// load-balance order and following DNS changes.
///
/// v1.2.x: unlike the pre-split code — which resolved every target ONCE at
/// listener startup and reused those addresses forever — this re-resolves
/// through the shared DNS cache (`resolve_cached`, 30s TTL) at session-open
/// time. A DDNS target whose IP changes is picked up by the next new session
/// within the cache TTL, instead of being pinned to the boot-time IP until the
/// rule or node restarts. (Established sessions keep their socket and age out on
/// the 60s idle timeout, after which new datagrams open a fresh session against
/// the current IP.)
///
/// Connect `outbound` to the first target, in selector order, that both
/// resolves and accepts the connect. Returns the address the socket is now
/// pinned to, or None when every target failed.
///
/// Resolution is LAZY — a target is only looked up when its turn comes. This
/// runs on the listener's receive loop for every new session, so resolving all
/// targets up front would make a healthy primary pay for the DNS of standbys it
/// never uses, and would stall the whole port on a cold cache.
///
/// Failed connects are reported to the shared circuit breaker so a target that
/// keeps refusing drops out of `selector.order()` for the break window; a
/// success clears that target's failure state. Note this only ever observes
/// LOCAL connect results: UDP is connectionless, so `connect` merely fixes the
/// peer and does a route lookup — it cannot tell whether the far side is alive.
async fn connect_udp_target<F, Fut>(
    targets: &[String],
    selector: &TargetSelector,
    port: u16,
    mut connect: F,
) -> Option<SocketAddr>
where
    F: FnMut(SocketAddr) -> Fut,
    Fut: Future<Output = std::io::Result<()>>,
{
    for idx in selector.order() {
        let Some(t) = targets.get(idx) else { continue };
        let addr = match super::outbound::resolve_cached(t).await {
            Ok(addrs) => match addrs.into_iter().next() {
                Some(addr) => addr,
                None => {
                    tracing::debug!("UDP port {}: target {} resolved to no address", port, t);
                    continue;
                }
            },
            Err(e) => {
                tracing::debug!("UDP port {}: failed to resolve target {}: {}", port, t, e);
                continue;
            }
        };
        match connect(addr).await {
            Ok(()) => {
                selector.report(idx, true);
                return Some(addr);
            }
            Err(e) => {
                selector.report(idx, false);
                tracing::debug!("UDP port {}: target {} connect failed: {}", port, addr, e);
            }
        }
    }
    None
}

/// Classify whether a `recv_from` error is worth retrying (mirrors the TCP
/// accept classifier). Transient OS-level resource exhaustion clears on its
/// own; retrying keeps the listener alive. A bad-fd / closed-socket error is
/// permanent and ends the task (the manager can restart it).
fn is_transient_recv_error(e: &std::io::Error) -> bool {
    use std::io::ErrorKind;
    matches!(
        e.kind(),
        ErrorKind::Interrupted
            | ErrorKind::WouldBlock
            | ErrorKind::TimedOut
            | ErrorKind::ResourceBusy
    ) || e.raw_os_error().is_some_and(|c| {
        // EMFILE (24) / ENFILE (23) / ENOBUFS (105) / ENOMEM (12).
        matches!(c, 24 | 23 | 105 | 12)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use relay_shared::protocol::LoadBalanceStrategy;

    // All targets below are IP literals ("ip:port"), which resolve LOCALLY via
    // lookup_host with no DNS query — keeping these tests hermetic (no network).

    /// A connect that always succeeds — used to assert selection order without
    /// touching the network.
    async fn always_ok(_: SocketAddr) -> std::io::Result<()> {
        Ok(())
    }

    /// Failover order picks the first (primary) target and resolves it.
    #[tokio::test]
    async fn connect_udp_target_picks_first_in_order() {
        let targets = vec!["127.0.0.1:9".to_string(), "127.0.0.2:9".to_string()];
        let selector = TargetSelector::new(LoadBalanceStrategy::Failover, 2);
        let got = connect_udp_target(&targets, &selector, 5000, always_ok).await;
        assert_eq!(got, Some("127.0.0.1:9".parse().unwrap()));
    }

    /// Round-robin advances the shared cursor across successive new sessions,
    /// so consecutive sessions pin to different targets.
    #[tokio::test]
    async fn connect_udp_target_follows_round_robin() {
        let targets = vec!["127.0.0.1:9".to_string(), "127.0.0.2:9".to_string()];
        let selector = TargetSelector::new(LoadBalanceStrategy::RoundRobin, 2);
        let a = connect_udp_target(&targets, &selector, 5000, always_ok)
            .await
            .unwrap();
        let b = connect_udp_target(&targets, &selector, 5000, always_ok)
            .await
            .unwrap();
        assert_eq!(a, "127.0.0.1:9".parse().unwrap());
        assert_eq!(b, "127.0.0.2:9".parse().unwrap());
    }

    /// A target that can't be resolved (no port → immediate parse error, no DNS)
    /// is skipped, falling through to the next resolvable target in order.
    #[tokio::test]
    async fn connect_udp_target_skips_unresolvable() {
        let targets = vec!["nocolon-no-port".to_string(), "127.0.0.1:9".to_string()];
        let selector = TargetSelector::new(LoadBalanceStrategy::Failover, 2);
        let got = connect_udp_target(&targets, &selector, 5000, always_ok).await;
        assert_eq!(got, Some("127.0.0.1:9".parse().unwrap()));
    }

    /// A failed connect must fall through to the next target in selector order
    /// instead of dropping the session's first datagram.
    #[tokio::test]
    async fn connect_udp_target_falls_back_after_connect_failure() {
        let targets = vec!["127.0.0.1:9".to_string(), "127.0.0.2:9".to_string()];
        let primary: SocketAddr = "127.0.0.1:9".parse().unwrap();
        let selector = TargetSelector::new(LoadBalanceStrategy::Failover, 2);

        let chosen = connect_udp_target(&targets, &selector, 5000, |target| async move {
            if target == primary {
                Err(std::io::Error::other("primary connect failed"))
            } else {
                Ok(())
            }
        })
        .await;

        assert_eq!(chosen, Some("127.0.0.2:9".parse().unwrap()));
    }

    /// Targets are resolved LAZILY: a healthy primary must not cost a lookup
    /// for standbys it never uses. This runs on the receive loop for every new
    /// session, so eager resolution made every session pay for all of them.
    #[tokio::test]
    async fn connect_udp_target_stops_at_the_first_working_target() {
        let targets = vec!["127.0.0.1:9".to_string(), "127.0.0.2:9".to_string()];
        let selector = TargetSelector::new(LoadBalanceStrategy::Failover, 2);
        let attempted = std::cell::Cell::new(0usize);
        let got = connect_udp_target(&targets, &selector, 5000, |_| {
            attempted.set(attempted.get() + 1);
            async move { Ok(()) }
        })
        .await;
        assert_eq!(got, Some("127.0.0.1:9".parse().unwrap()));
        assert_eq!(
            attempted.get(),
            1,
            "the standby must not be resolved or attempted once the primary connects"
        );
    }

    async fn loopback_udp() -> UdpSocket {
        UdpSocket::bind("127.0.0.1:0").await.unwrap()
    }

    /// An outbound socket connected to a port nobody listens on, with one
    /// datagram already sent: the ICMP port-unreachable it provokes makes the
    /// socket's next `recv` fail, as a dead target does in production.
    async fn outbound_to_dead_port() -> Arc<UdpSocket> {
        let dead = loopback_udp().await;
        let dead_addr = dead.local_addr().unwrap();
        drop(dead);
        let sock = loopback_udp().await;
        sock.connect(dead_addr).await.unwrap();
        sock.send(b"ping").await.unwrap();
        Arc::new(sock)
    }

    fn relay(
        outbound: Arc<UdpSocket>,
        inbound: Arc<UdpSocket>,
        sessions: Arc<Sessions>,
    ) -> ReplyRelay {
        ReplyRelay {
            outbound,
            inbound,
            sessions,
            connections: Arc::new(ConnectionTracker::new()),
            counter: Arc::new(TrafficCounter::new()),
            rate_limit: RateLimit::new(None, None),
            src: "127.0.0.1:40000".parse().unwrap(),
            rule_id: 1,
            port: 5000,
        }
    }

    fn session(outbound: &Arc<UdpSocket>) -> (UdpSession, oneshot::Receiver<()>) {
        let (tx, rx) = oneshot::channel();
        let s = UdpSession {
            outbound: outbound.clone(),
            last_active: tokio::time::Instant::now(),
            _stop: tx,
        };
        (s, rx)
    }

    /// v1.2.12 (H2): evicting an idle session must end its reply reader. The
    /// target here never answers, so a reader that only watched `recv()` would
    /// wait forever, holding its socket — one leaked task + fd per idle client.
    #[tokio::test]
    async fn evicting_a_session_stops_its_reader() {
        let target = loopback_udp().await; // alive, never replies
        let outbound = Arc::new(loopback_udp().await);
        outbound
            .connect(target.local_addr().unwrap())
            .await
            .unwrap();
        let inbound = Arc::new(loopback_udp().await);
        let sessions: Arc<Sessions> = Arc::new(DashMap::new());

        let r = relay(outbound.clone(), inbound, sessions.clone());
        let src = r.src;
        let (s, stop) = session(&outbound);
        sessions.insert(src, s);
        let reader = tokio::spawn(r.run(stop));

        // What the idle sweeper's retain() does.
        sessions.remove(&src);
        tokio::time::timeout(Duration::from_secs(2), reader)
            .await
            .expect("reader must stop once its session is evicted")
            .unwrap();
    }

    /// v1.2.12 (H3): a reader whose socket fails must not remove a NEWER
    /// session that has since replaced its own entry for the same client.
    #[tokio::test]
    async fn a_failing_stale_reader_leaves_the_current_session_alone() {
        let stale = outbound_to_dead_port().await;
        let current = Arc::new(loopback_udp().await);
        let inbound = Arc::new(loopback_udp().await);
        let sessions: Arc<Sessions> = Arc::new(DashMap::new());

        let r = relay(stale.clone(), inbound, sessions.clone());
        let src = r.src;
        // Keep the stale reader's stop sender alive so only the socket error
        // can end it; the map holds the current session for the same client.
        let (_stale_entry, stale_stop) = session(&stale);
        let (s, _current_stop) = session(&current);
        sessions.insert(src, s);

        tokio::time::timeout(Duration::from_secs(5), r.run(stale_stop))
            .await
            .expect("the dead target's error must end the stale reader");
        let entry = sessions.get(&src).expect("current session must survive");
        assert!(Arc::ptr_eq(&entry.outbound, &current));
    }

    /// The error path still cleans up its OWN session right away.
    #[tokio::test]
    async fn a_failing_reader_removes_its_own_session() {
        let outbound = outbound_to_dead_port().await;
        let inbound = Arc::new(loopback_udp().await);
        let sessions: Arc<Sessions> = Arc::new(DashMap::new());

        let r = relay(outbound.clone(), inbound, sessions.clone());
        let src = r.src;
        let (s, stop) = session(&outbound);
        sessions.insert(src, s);

        tokio::time::timeout(Duration::from_secs(5), r.run(stop))
            .await
            .expect("the dead target's error must end the reader");
        assert!(sessions.get(&src).is_none());
    }

    /// No targets → None (the caller drops the datagram and warns).
    #[tokio::test]
    async fn connect_udp_target_none_when_no_target_exists() {
        let targets: Vec<String> = vec![];
        let selector = TargetSelector::new(LoadBalanceStrategy::RoundRobin, 0);
        assert!(connect_udp_target(&targets, &selector, 5000, always_ok)
            .await
            .is_none());
    }
}
