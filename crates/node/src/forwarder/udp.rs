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

use std::collections::HashMap;
use std::net::{Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;
use tokio::net::UdpSocket;
use tokio::sync::Mutex;
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

#[allow(clippy::too_many_arguments)]
pub async fn start_udp_listener(
    listen_addr: SocketAddr,
    targets: Vec<String>,
    selector: Arc<TargetSelector>,
    rate_limit: RateLimit,
    counter: Arc<TrafficCounter>,
    connections: Arc<ConnectionTracker>,
    rule_id: i64,
    source_ipv4: Option<Ipv4Addr>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    if targets.is_empty() {
        tracing::warn!("UDP listener on {}: no targets configured", listen_addr);
    }

    let inbound = Arc::new(UdpSocket::bind(listen_addr).await?);
    tracing::info!("UDP listening on {} (rule {})", listen_addr, rule_id);

    let port = listen_addr.port();

    // v0.4.6: resolve targets keeping index alignment with the `targets` list so
    // the load-balance selector (which returns target INDICES) maps to the right
    // address. Each target string resolves to its FIRST address; unresolvable
    // targets become None and are skipped during selection. Async DNS is used so
    // a slow resolver can't block a runtime worker.
    let mut resolved: Vec<Option<SocketAddr>> = Vec::with_capacity(targets.len());
    for t in &targets {
        if let Ok(addr) = t.parse::<SocketAddr>() {
            resolved.push(Some(addr));
            continue;
        }
        match tokio::net::lookup_host(t).await {
            Ok(mut addrs) => resolved.push(addrs.next()),
            Err(e) => {
                tracing::warn!(
                    "UDP listener on {}: failed to resolve {}: {}",
                    listen_addr,
                    t,
                    e
                );
                resolved.push(None);
            }
        }
    }
    if resolved.iter().all(Option::is_none) {
        tracing::error!(
            "UDP listener on {}: failed to resolve any target",
            listen_addr
        );
        return Err("no resolvable target".into());
    }

    let sessions: Arc<Mutex<HashMap<SocketAddr, UdpSession>>> =
        Arc::new(Mutex::new(HashMap::new()));

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
            let mut sessions = sessions_clone.lock().await;
            let before = sessions.len();
            sessions.retain(|_, s| s.last_active.elapsed() < UDP_SESSION_TIMEOUT);
            let removed = before - sessions.len();
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

        // Register/refresh this client as an active UDP session so the panel
        // counts it. Returns true on the FIRST datagram from this client.
        let opened = connections.udp_touch(src, rule_id).await;

        // Pick or create a local session (with its own outbound socket) for
        // this client.
        let sessions_arc = sessions.clone();
        let outbound_sock = {
            let mut map = sessions_arc.lock().await;
            if let Some(s) = map.get_mut(&src) {
                s.last_active = tokio::time::Instant::now();
                s.outbound.clone()
            } else {
                // New session: bind an ephemeral outbound socket and connect to target
                let outbound = match super::outbound::udp_outbound_socket(source_ipv4).await {
                    Ok(s) => s,
                    Err(e) => {
                        tracing::warn!("UDP port {}: failed to bind outbound: {}", port, e);
                        continue;
                    }
                };
                // v0.4.6: pick a target per the rule's load-balance strategy.
                // The selector yields target indices in priority order; we use
                // the first one that resolved to an address. UDP affinity: this
                // pick happens once per NEW session, so all datagrams from the
                // same client stay pinned to the chosen target.
                let target = match selector
                    .order()
                    .into_iter()
                    .find_map(|idx| resolved.get(idx).copied().flatten())
                {
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

                // Spawn a forwarder: outbound -> inbound (target replies back to client)
                let inbound_c = inbound.clone();
                let sessions_c = sessions_arc.clone();
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
                                // A reply from the target counts as activity
                                // too — refresh the session so a long-lived
                                // request/response flow isn't expired mid-flight.
                                connections_c.udp_touch(src_c, rule_id).await;
                                if inbound_c.send_to(&rbuf[..m], src_c).await.is_err() {
                                    break;
                                }
                                if let Some(s) = sessions_c.lock().await.get_mut(&src_c) {
                                    s.last_active = tokio::time::Instant::now();
                                }
                            }
                            Err(e) => {
                                tracing::debug!("UDP port {}: outbound recv ended: {}", port_c, e);
                                break;
                            }
                        }
                    }
                    // Outbound side ended (target closed / error): release
                    // this client's session immediately rather than waiting
                    // for timeout.
                    sessions_c.lock().await.remove(&src_c);
                    connections_c.udp_close(src_c, rule_id).await;
                });

                map.insert(
                    src,
                    UdpSession {
                        outbound: outbound.clone(),
                        last_active: tokio::time::Instant::now(),
                    },
                );
                if opened {
                    tracing::debug!(
                        "UDP port {}: new session {} -> {} (rule {})",
                        port,
                        src,
                        target,
                        rule_id
                    );
                }
                outbound.clone()
            }
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
