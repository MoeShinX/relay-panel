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
use std::net::{Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;
use tokio::net::UdpSocket;
use tokio::time;

use super::limiter::RateLimit;
use super::selector::TargetSelector;
use crate::reporter::{ConnectionTracker, TrafficCounter, UDP_SESSION_TIMEOUT};

const UDP_BUF_SIZE: usize = 65535;
/// How often the periodic sweeper runs. Sessions themselves expire on the
/// shared UDP_SESSION_TIMEOUT; this just controls how quickly an idle node
// converges back to 0 in the absence of new datagrams.
const CLEANUP_INTERVAL: Duration = Duration::from_secs(15);

struct UdpSession {
    outbound: Arc<UdpSocket>,
    last_active: tokio::time::Instant,
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
    let sessions: Arc<DashMap<SocketAddr, UdpSession>> = Arc::new(DashMap::new());

    // Background cleanup of expired local session entries (outbound sockets).
    // This mirrors the ConnectionTracker's own expiry; together they make sure
    // idle UDP state is reclaimed promptly.
    let sessions_clone = sessions.clone();
    let connections_clone = connections.clone();
    tokio::spawn(async move {
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
            let outbound = match super::outbound::udp_outbound_socket(source_ipv4).await {
                Ok(s) => s,
                Err(e) => {
                    tracing::warn!("UDP port {}: failed to bind outbound: {}", port, e);
                    continue;
                }
            };
            // Pick a target per the rule's load-balance strategy AND resolve it
            // now, through the 30s DNS cache, so a DDNS target follows IP
            // changes (see select_udp_target). UDP affinity: this happens once
            // per NEW session, so all datagrams from the same client stay pinned
            // to the IP chosen here until the session idles out.
            let target = match select_udp_target(&targets, &selector, port).await {
                Some(t) => t,
                None => {
                    tracing::warn!("UDP port {}: no resolvable target for session", port);
                    continue;
                }
            };
            if let Err(e) = outbound.connect(target).await {
                tracing::warn!(
                    "UDP port {}: failed to connect to target {}: {}",
                    port,
                    target,
                    e
                );
                continue;
            }
            let outbound = Arc::new(outbound);

            // Publish via the entry API (per-shard lock, sync — no .await while
            // the guard is held). Double-check for a concurrent datagram from the
            // same client that won the race while we were connecting: if one did,
            // use the winner and drop ours.
            let now = tokio::time::Instant::now();
            let (chosen, we_won) = match sessions.entry(src) {
                Entry::Occupied(mut e) => {
                    e.get_mut().last_active = now;
                    (e.get().outbound.clone(), false)
                }
                Entry::Vacant(e) => {
                    e.insert(UdpSession {
                        outbound: outbound.clone(),
                        last_active: now,
                    });
                    (outbound.clone(), true)
                }
            };

            if we_won {
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
                let inbound_c = inbound.clone();
                let sessions_c = sessions.clone();
                let connections_c = connections.clone();
                let counter_c = counter.clone();
                let rl_c = rate_limit.clone();
                let src_c = src;
                let outbound_c = outbound.clone();
                let port_c = port;
                tokio::spawn(async move {
                    let mut rbuf = vec![0u8; UDP_BUF_SIZE];
                    loop {
                        match outbound_c.recv(&mut rbuf).await {
                            Ok(m) => {
                                // v0.4.6: throttle target→client (download) bytes
                                // through the shared per-rule limiter BEFORE
                                // forwarding back to the client.
                                rl_c.acquire_download(m as u64).await;
                                counter_c.add(rule_id, 0, m as u64).await;
                                // A reply is activity too: refresh the tracker
                                // (cheap, sharded) and the session's last_active
                                // so a long request/response flow isn't expired.
                                connections_c.udp_touch(src_c, rule_id).await;
                                if inbound_c.send_to(&rbuf[..m], src_c).await.is_err() {
                                    break;
                                }
                                if let Some(mut s) = sessions_c.get_mut(&src_c) {
                                    s.last_active = tokio::time::Instant::now();
                                }
                            }
                            Err(e) => {
                                tracing::debug!("UDP port {}: outbound recv ended: {}", port_c, e);
                                break;
                            }
                        }
                    }
                    // Outbound side ended (target closed / error): release this
                    // client's session immediately rather than waiting for timeout.
                    sessions_c.remove(&src_c);
                    connections_c.udp_close(src_c, rule_id).await;
                });
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
/// Returns the first target, in selector order, that resolves to at least one
/// address; None when none resolve.
async fn select_udp_target(
    targets: &[String],
    selector: &TargetSelector,
    port: u16,
) -> Option<SocketAddr> {
    for idx in selector.order() {
        let Some(t) = targets.get(idx) else { continue };
        match super::outbound::resolve_cached(t).await {
            Ok(addrs) => {
                if let Some(addr) = addrs.into_iter().next() {
                    return Some(addr);
                }
                tracing::debug!("UDP port {}: target {} resolved to no address", port, t);
            }
            Err(e) => {
                tracing::debug!("UDP port {}: failed to resolve target {}: {}", port, t, e);
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

    /// Failover order picks the first (primary) target and resolves it.
    #[tokio::test]
    async fn select_udp_target_picks_first_in_order() {
        let targets = vec!["127.0.0.1:9".to_string(), "127.0.0.2:9".to_string()];
        let selector = TargetSelector::new(LoadBalanceStrategy::Failover, 2);
        let got = select_udp_target(&targets, &selector, 5000).await;
        assert_eq!(got, Some("127.0.0.1:9".parse().unwrap()));
    }

    /// Round-robin advances the shared cursor across successive new sessions,
    /// so consecutive sessions pin to different targets.
    #[tokio::test]
    async fn select_udp_target_follows_round_robin() {
        let targets = vec!["127.0.0.1:9".to_string(), "127.0.0.2:9".to_string()];
        let selector = TargetSelector::new(LoadBalanceStrategy::RoundRobin, 2);
        let a = select_udp_target(&targets, &selector, 5000).await.unwrap();
        let b = select_udp_target(&targets, &selector, 5000).await.unwrap();
        assert_eq!(a, "127.0.0.1:9".parse().unwrap());
        assert_eq!(b, "127.0.0.2:9".parse().unwrap());
    }

    /// A target that can't be resolved (no port → immediate parse error, no DNS)
    /// is skipped, falling through to the next resolvable target in order.
    #[tokio::test]
    async fn select_udp_target_skips_unresolvable() {
        let targets = vec!["nocolon-no-port".to_string(), "127.0.0.1:9".to_string()];
        let selector = TargetSelector::new(LoadBalanceStrategy::Failover, 2);
        let got = select_udp_target(&targets, &selector, 5000).await;
        assert_eq!(got, Some("127.0.0.1:9".parse().unwrap()));
    }

    /// No targets → None (the caller drops the datagram and warns).
    #[tokio::test]
    async fn select_udp_target_none_when_empty() {
        let targets: Vec<String> = vec![];
        let selector = TargetSelector::new(LoadBalanceStrategy::RoundRobin, 0);
        assert!(select_udp_target(&targets, &selector, 5000).await.is_none());
    }
}
