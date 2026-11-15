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
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Duration;
use tokio::net::UdpSocket;
use tokio::sync::oneshot;
use tokio::time;

use super::limiter::RateLimit;
use super::outbound::OutboundError;
use super::selector::TargetSelector;
use crate::reporter::{ConnectionTracker, TrafficCounter, UDP_SESSION_TIMEOUT};

const UDP_BUF_SIZE: usize = 65535;
/// How often the periodic sweeper runs. Sessions themselves expire on the
/// shared UDP_SESSION_TIMEOUT; this just controls how quickly an idle node
// converges back to 0 in the absence of new datagrams.
const CLEANUP_INTERVAL: Duration = Duration::from_secs(15);

type Sessions = DashMap<SocketAddr, UdpSession>;

/// v1.2.12: live UDP sessions across ALL rules on this node. Each one holds a
/// real socket (an fd) plus a reader task until it has been idle for
/// UDP_SESSION_TIMEOUT, and a new session is opened for every new source
/// address — including spoofed ones. Unbounded, a flood of forged sources
/// used up the process's file descriptors, and then every rule stopped (TCP
/// included) while the node still looked online. New sessions beyond
/// `udp_session_cap()` are refused; existing ones carry on.
static UDP_SESSIONS: AtomicUsize = AtomicUsize::new(0);

/// The node-wide session cap: `UDP_MAX_SESSIONS` if set, otherwise half the
/// file-descriptor limit (so the other half stays for TCP and the node
/// itself) — 32768 under the installer's LimitNOFILE=65536. Read on first use,
/// after main() has raised the soft limit.
fn udp_session_cap() -> usize {
    static CAP: OnceLock<usize> = OnceLock::new();
    *CAP.get_or_init(|| {
        let cap = session_cap_from(std::env::var("UDP_MAX_SESSIONS").ok(), nofile_limit());
        tracing::info!("UDP: at most {} concurrent sessions node-wide", cap);
        cap
    })
}

fn session_cap_from(env: Option<String>, nofile: Option<u64>) -> usize {
    if let Some(n) = env
        .and_then(|v| v.trim().parse::<usize>().ok())
        .filter(|n| *n > 0)
    {
        return n;
    }
    match nofile {
        Some(n) => (n / 2).clamp(256, 524_288) as usize,
        None => 4096,
    }
}

#[cfg(unix)]
// rlim_t is u64 on the targets we ship (Linux x86_64/aarch64), which makes the
// cast a no-op there, but it is not u64 on every unix.
#[allow(clippy::unnecessary_cast)]
fn nofile_limit() -> Option<u64> {
    let mut lim = libc::rlimit {
        rlim_cur: 0,
        rlim_max: 0,
    };
    // SAFETY: getrlimit writes into a local rlimit we own.
    (unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, &mut lim) } == 0).then_some(lim.rlim_cur as u64)
}

#[cfg(not(unix))]
fn nofile_limit() -> Option<u64> {
    None
}

/// One counted place in `counter`, given back on drop. Held by the session's
/// socket (`SessionSocket`), so it counts sockets that are really open.
struct SessionSlot(&'static AtomicUsize);

impl SessionSlot {
    fn acquire(counter: &'static AtomicUsize, cap: usize) -> Option<Self> {
        counter
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |n| {
                (n < cap).then_some(n + 1)
            })
            .ok()
            .map(|_| SessionSlot(counter))
    }
}

impl Drop for SessionSlot {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

/// Default for how many sessions one source address may hold at once.
const DEFAULT_SESSIONS_PER_IP: usize = 1024;

/// The per-source limit: `UDP_MAX_SESSIONS_PER_IP` if it is a positive
/// number, otherwise `DEFAULT_SESSIONS_PER_IP`. Read once, on first use.
fn udp_sessions_per_ip_cap() -> usize {
    static CAP: OnceLock<usize> = OnceLock::new();
    *CAP.get_or_init(|| {
        let cap = per_ip_cap_from(std::env::var("UDP_MAX_SESSIONS_PER_IP").ok());
        tracing::info!(
            "UDP: at most {} concurrent sessions per source address",
            cap
        );
        cap
    })
}

fn per_ip_cap_from(env: Option<String>) -> usize {
    env.and_then(|v| v.trim().parse::<usize>().ok())
        .filter(|n| *n > 0)
        .unwrap_or(DEFAULT_SESSIONS_PER_IP)
}

/// Whom a session is counted against: one IPv4 address, or one IPv6 /64 (a
/// single subscriber or host is routinely handed a whole /64, so counting
/// full IPv6 addresses would let it dodge the limit by rotating them).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum SourceKey {
    V4(Ipv4Addr),
    V6Prefix64(u64),
}

impl SourceKey {
    fn of(ip: IpAddr) -> Self {
        match ip {
            IpAddr::V4(v4) => SourceKey::V4(v4),
            // `::ffff:a.b.c.d` is how a dual-stack socket shows an IPv4
            // client: the same host as a.b.c.d, not an IPv6 network.
            IpAddr::V6(v6) => match v6.to_ipv4_mapped() {
                Some(v4) => SourceKey::V4(v4),
                None => SourceKey::V6Prefix64((u128::from(v6) >> 64) as u64),
            },
        }
    }
}

/// Live UDP sessions per source, across all rules on this node. A source with
/// no sessions left has no entry, so the table only holds sources that are
/// currently active.
struct SourceTable {
    counts: OnceLock<DashMap<SourceKey, usize>>,
}

impl SourceTable {
    const fn new() -> Self {
        Self {
            counts: OnceLock::new(),
        }
    }

    fn map(&self) -> &DashMap<SourceKey, usize> {
        self.counts.get_or_init(DashMap::new)
    }
}

static UDP_SOURCES: SourceTable = SourceTable::new();

/// One counted session for a source in `table`, given back on drop. Like
/// `SessionSlot` it lives in the session's socket, so every way a session can
/// end (idle eviction, reader failure, rule removal, listener teardown)
/// releases it exactly once, when the socket really closes.
struct SourceSlot {
    table: &'static SourceTable,
    key: SourceKey,
}

impl SourceSlot {
    fn acquire(table: &'static SourceTable, key: SourceKey, cap: usize) -> Option<Self> {
        // The shard lock held by `entry` makes check-and-increment atomic.
        match table.map().entry(key) {
            Entry::Occupied(mut e) => {
                if *e.get() >= cap {
                    return None;
                }
                *e.get_mut() += 1;
            }
            Entry::Vacant(e) => {
                if cap == 0 {
                    return None;
                }
                e.insert(1);
            }
        }
        Some(SourceSlot { table, key })
    }
}

impl Drop for SourceSlot {
    fn drop(&mut self) {
        if let Entry::Occupied(mut e) = self.table.map().entry(self.key) {
            if *e.get() <= 1 {
                e.remove();
            } else {
                *e.get_mut() -= 1;
            }
        }
    }
}

/// Why a new session was not admitted.
#[derive(Debug, PartialEq, Eq)]
enum Refusal {
    /// This source already holds its share of sessions.
    PerSource,
    /// The node-wide cap is reached.
    NodeWide,
}

/// Claim the places a new session from `ip` needs: one under its source's
/// limit, then one under the node-wide cap. The source is checked first so a
/// refused source never takes a node-wide place, even briefly. Dropping the
/// returned slots (or the first one, when the second is refused) gives them
/// back.
fn acquire_session_slots(
    ip: IpAddr,
    sources: &'static SourceTable,
    per_ip_cap: usize,
    node: &'static AtomicUsize,
    node_cap: usize,
) -> Result<(SessionSlot, SourceSlot), Refusal> {
    let source =
        SourceSlot::acquire(sources, SourceKey::of(ip), per_ip_cap).ok_or(Refusal::PerSource)?;
    let slot = SessionSlot::acquire(node, node_cap).ok_or(Refusal::NodeWide)?;
    Ok((slot, source))
}

/// Refused-per-source count since the last warning, and when that was.
static REFUSED_PER_SOURCE: AtomicU64 = AtomicU64::new(0);
static LAST_PER_SOURCE_WARNING: AtomicU64 = AtomicU64::new(0);
const PER_SOURCE_WARNING_INTERVAL_SECS: u64 = 10;

/// True (and records `now`) when at least `interval` seconds have passed since
/// the last time this returned true for `last`.
fn log_due(last: &AtomicU64, now: u64, interval: u64) -> bool {
    let prev = last.load(Ordering::Relaxed);
    now.saturating_sub(prev) >= interval
        && last
            .compare_exchange(prev, now, Ordering::Relaxed, Ordering::Relaxed)
            .is_ok()
}

/// Count a session refused by the per-source limit; warn at most once per
/// `PER_SOURCE_WARNING_INTERVAL_SECS` node-wide, so a flood cannot flood the
/// log either.
fn note_source_refused(ip: IpAddr, port: u16) {
    REFUSED_PER_SOURCE.fetch_add(1, Ordering::Relaxed);
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    if log_due(
        &LAST_PER_SOURCE_WARNING,
        now,
        PER_SOURCE_WARNING_INTERVAL_SECS,
    ) {
        tracing::warn!(
            "UDP per-source session limit reached ({} sessions per address, IPv6 counted per /64); \
             refused {} new session(s) since the last warning (latest from {} on port {}). \
             Existing sessions are unaffected. If this is real traffic (many clients behind one \
             address), raise UDP_MAX_SESSIONS_PER_IP.",
            udp_sessions_per_ip_cap(),
            REFUSED_PER_SOURCE.swap(0, Ordering::Relaxed),
            ip,
            port
        );
    }
}

/// A session's outbound socket together with its places under the node-wide
/// cap and its source's limit. The places are given back only when the socket
/// itself closes — once the map entry, the reply reader and any in-flight send
/// have all let go of it — so the limits bound the sockets actually open, not
/// just the map entries.
struct SessionSocket {
    sock: UdpSocket,
    _slot: SessionSlot,
    _source: SourceSlot,
}

impl std::ops::Deref for SessionSocket {
    type Target = UdpSocket;

    fn deref(&self) -> &UdpSocket {
        &self.sock
    }
}

/// Refused new sessions since the last warning, and when that was (unix secs).
static REFUSED_SESSIONS: AtomicU64 = AtomicU64::new(0);
static LAST_CAP_WARNING: AtomicU64 = AtomicU64::new(0);

/// Count a refused session; warn at most once a minute so a flood cannot
/// flood the log too.
fn note_session_refused(port: u16) {
    REFUSED_SESSIONS.fetch_add(1, Ordering::Relaxed);
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let last = LAST_CAP_WARNING.load(Ordering::Relaxed);
    if now.saturating_sub(last) >= 60
        && LAST_CAP_WARNING
            .compare_exchange(last, now, Ordering::Relaxed, Ordering::Relaxed)
            .is_ok()
    {
        tracing::warn!(
            "UDP session limit reached ({} sessions node-wide); refused {} new session(s) \
             since the last warning (latest on port {}). Existing sessions are unaffected. \
             If this is real traffic, raise UDP_MAX_SESSIONS (and LimitNOFILE).",
            udp_session_cap(),
            REFUSED_SESSIONS.swap(0, Ordering::Relaxed),
            port
        );
    }
}

struct UdpSession {
    /// v1.2.12: carries this session's place under the node-wide cap.
    outbound: Arc<SessionSocket>,
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

/// Drop the sessions idle for longer than the timeout and return how many went.
/// Dropping a session stops its reader; its socket (and with it its places
/// under the limits) closes once the reader lets go.
fn evict_expired(sessions: &Sessions) -> usize {
    let before = sessions.len();
    sessions.retain(|_, s| s.last_active.elapsed() < UDP_SESSION_TIMEOUT);
    // saturating: len() is read across shards without a global lock, so a
    // concurrent insert between the two reads must not underflow usize.
    before.saturating_sub(sessions.len())
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
            let removed = evict_expired(&sessions_clone);
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
            // v1.2.12: a place under the node-wide cap first, before any
            // socket is opened. Refused: drop the datagram.
            // The source's own limit comes first: one address cannot fill the
            // node-wide cap and lock every other client out.
            let (slot, source) = match acquire_session_slots(
                src.ip(),
                &UDP_SOURCES,
                udp_sessions_per_ip_cap(),
                &UDP_SESSIONS,
                udp_session_cap(),
            ) {
                Ok(slots) => slots,
                Err(Refusal::PerSource) => {
                    note_source_refused(src.ip(), port);
                    continue;
                }
                Err(Refusal::NodeWide) => {
                    note_session_refused(port);
                    continue;
                }
            };
            // New session: pick a target and give the session an ephemeral
            // outbound socket connected to it, all WITHOUT holding any map
            // guard. Targets are tried in selector order, resolving lazily: a
            // healthy primary costs one DNS lookup. UDP affinity is still per
            // NEW session, and a connect failure falls through to a healthy
            // standby.
            //
            // node-v1.2.8: the socket is bound per attempt, in the family of
            // the address the target resolved to. It used to be one IPv4
            // socket bound up front, which could never reach an IPv6 target.
            //
            // A bind for an IPv4 target fails only when OUTBOUND_BIND_IPV4
            // names an address this host no longer has: a local fault that
            // says nothing about any target's health. It ends the attempt
            // unreported (see AttemptError::Local) — reported per target it
            // would trip all of them, and the log would blame the targets. An
            // IPv6 socket that cannot be had means this host has no IPv6, so
            // that target really is unreachable from here; reporting it lets
            // the rule fail over to an IPv4 standby.
            let Some((target, sock)) =
                connect_udp_target(&targets, &selector, port, |addr| async move {
                    let sock = super::outbound::udp_outbound_socket(addr, source_ipv4)
                        .await
                        .map_err(|e| match addr {
                            SocketAddr::V4(_) => AttemptError::Local(e),
                            SocketAddr::V6(_) => {
                                AttemptError::Target(std::io::Error::other(e.to_string()))
                            }
                        })?;
                    sock.connect(addr).await?;
                    Ok(sock)
                })
                .await
            else {
                continue;
            };
            let outbound = Arc::new(SessionSocket {
                sock,
                _slot: slot,
                _source: source,
            });

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
                // Log the new session (the target is known only on this path).
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

        // Register/refresh this client with the tracker on every datagram that
        // has a session. The tracker is a sharded DashMap (keyed by
        // client+rule), so this is a cheap per-shard op and keeps the panel's
        // count accurate. v1.2.12: only after the session exists — a datagram
        // refused by the cap (or with no reachable target) used to add a
        // tracker entry per spoofed source all the same.
        connections.udp_touch(src, rule_id).await;

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
    outbound: Arc<SessionSocket>,
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
            // Evicted or torn down: the entry is already gone (and may by now
            // be a NEWER session for the same client), so there is nothing of
            // ours left to clean up. `biased` checks stop first, so a reader
            // with more datagrams queued still stops at once.
            let m = tokio::select! {
                biased;
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
            // v1.2.12: this wait can be long on a slow rule with many sessions
            // backed up, so it gives way to stop too — otherwise an evicted
            // session's reader kept its socket until the wait ended and then
            // still forwarded the queued datagram. (The limiter charges before
            // it sleeps, so leaving mid-wait does not disturb its accounting.)
            tokio::select! {
                biased;
                _ = &mut stop => return,
                _ = self.rate_limit.acquire_download(m as u64) => {}
            }
            self.counter.add(self.rule_id, 0, m as u64).await;
            // A reply is activity too: refresh the tracker (cheap, sharded) and
            // the session's last_active so a long request/response flow isn't
            // expired.
            self.connections.udp_touch(self.src, self.rule_id).await;
            let sent = tokio::select! {
                biased;
                _ = &mut stop => return,
                r = self.inbound.send_to(&rbuf[..m], self.src) => r,
            };
            if sent.is_err() {
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
/// Connect to the first target, in selector order, that both resolves and
/// accepts the connect: `connect` opens a socket for the resolved address and
/// connects it. Returns that address and the socket, or None — logged here —
/// when every target failed or a local fault stopped the attempt.
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
async fn connect_udp_target<F, Fut, S>(
    targets: &[String],
    selector: &TargetSelector,
    port: u16,
    mut connect: F,
) -> Option<(SocketAddr, S)>
where
    F: FnMut(SocketAddr) -> Fut,
    Fut: Future<Output = Result<S, AttemptError>>,
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
            Ok(sock) => {
                selector.report(idx, true);
                return Some((addr, sock));
            }
            Err(AttemptError::Target(e)) => {
                selector.report(idx, false);
                tracing::debug!("UDP port {}: target {} connect failed: {}", port, addr, e);
            }
            Err(AttemptError::Local(e)) => {
                tracing::warn!("UDP port {}: failed to bind outbound: {}", port, e);
                return None;
            }
        }
    }
    tracing::warn!("UDP port {}: no reachable target for session", port);
    None
}

/// Why one target attempt gave a new session no socket.
enum AttemptError {
    /// This target cannot be reached from here: reported to the circuit
    /// breaker, and the next target is tried.
    Target(std::io::Error),
    /// A fault on this host that every target would hit alike. Not the
    /// target's: not reported, and no other target is tried.
    Local(OutboundError),
}

impl From<std::io::Error> for AttemptError {
    fn from(e: std::io::Error) -> Self {
        Self::Target(e)
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

#[cfg(test)]
mod tests {
    use super::*;
    use relay_shared::protocol::LoadBalanceStrategy;

    // All targets below are IP literals ("ip:port"), which resolve LOCALLY via
    // lookup_host with no DNS query — keeping these tests hermetic (no network).

    /// A connect that always succeeds — used to assert selection order without
    /// touching the network.
    async fn always_ok(_: SocketAddr) -> Result<(), AttemptError> {
        Ok(())
    }

    /// The chosen address alone, for tests whose sockets are `()`.
    fn addr_of(got: Option<(SocketAddr, ())>) -> Option<SocketAddr> {
        got.map(|(addr, ())| addr)
    }

    /// Failover order picks the first (primary) target and resolves it.
    #[tokio::test]
    async fn connect_udp_target_picks_first_in_order() {
        let targets = vec!["127.0.0.1:9".to_string(), "127.0.0.2:9".to_string()];
        let selector = TargetSelector::new(LoadBalanceStrategy::Failover, 2);
        let got = connect_udp_target(&targets, &selector, 5000, always_ok).await;
        assert_eq!(addr_of(got), Some("127.0.0.1:9".parse().unwrap()));
    }

    /// Round-robin advances the shared cursor across successive new sessions,
    /// so consecutive sessions pin to different targets.
    #[tokio::test]
    async fn connect_udp_target_follows_round_robin() {
        let targets = vec!["127.0.0.1:9".to_string(), "127.0.0.2:9".to_string()];
        let selector = TargetSelector::new(LoadBalanceStrategy::RoundRobin, 2);
        let (a, ()) = connect_udp_target(&targets, &selector, 5000, always_ok)
            .await
            .unwrap();
        let (b, ()) = connect_udp_target(&targets, &selector, 5000, always_ok)
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
        assert_eq!(addr_of(got), Some("127.0.0.1:9".parse().unwrap()));
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
                Err(std::io::Error::other("primary connect failed").into())
            } else {
                Ok(())
            }
        })
        .await;

        assert_eq!(addr_of(chosen), Some("127.0.0.2:9".parse().unwrap()));
    }

    /// A local fault (the IPv4 source address is gone) is not the target's:
    /// the attempt stops there, untried standbys are left alone, and the
    /// breaker hears nothing — so the primary is still first next time.
    #[tokio::test]
    async fn connect_udp_target_stops_on_a_local_fault() {
        let targets = vec!["127.0.0.1:9".to_string(), "127.0.0.2:9".to_string()];
        let selector = TargetSelector::new(LoadBalanceStrategy::Failover, 2);
        let attempted = std::cell::Cell::new(0usize);
        for _ in 0..10 {
            let got = connect_udp_target(&targets, &selector, 5000, |_| {
                attempted.set(attempted.get() + 1);
                async move {
                    Err::<(), _>(AttemptError::Local(OutboundError::Bind(
                        std::io::Error::other("source address gone"),
                    )))
                }
            })
            .await;
            assert!(got.is_none());
        }
        assert_eq!(attempted.get(), 10, "the standby must not be attempted");
        assert_eq!(
            selector.order().first(),
            Some(&0),
            "the primary must not be tripped"
        );
    }

    /// node-v1.2.8: a session to an IPv6 target, end to end through the real
    /// connect closure's socket choice — the datagram arrives and the reply
    /// comes back on the same socket.
    #[tokio::test]
    async fn connect_udp_target_reaches_an_ipv6_target() {
        let Ok(echo) = UdpSocket::bind("[::1]:0").await else {
            eprintln!("skipped: no IPv6 loopback on this host");
            return;
        };
        let targets = vec![echo.local_addr().unwrap().to_string()];
        let selector = TargetSelector::new(LoadBalanceStrategy::Failover, 1);
        let (addr, sock) = connect_udp_target(&targets, &selector, 5000, |addr| async move {
            let sock = super::super::outbound::udp_outbound_socket(addr, Some(Ipv4Addr::LOCALHOST))
                .await
                .map_err(|e| AttemptError::Target(std::io::Error::other(e.to_string())))?;
            sock.connect(addr).await?;
            Ok(sock)
        })
        .await
        .expect("the IPv6 target must be reachable");
        assert!(addr.is_ipv6());
        sock.send(b"ping").await.unwrap();
        let mut buf = [0u8; 16];
        let (n, from) = echo.recv_from(&mut buf).await.unwrap();
        assert_eq!(&buf[..n], b"ping");
        echo.send_to(b"pong", from).await.unwrap();
        let n = sock.recv(&mut buf).await.unwrap();
        assert_eq!(&buf[..n], b"pong");
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
        assert_eq!(addr_of(got), Some("127.0.0.1:9".parse().unwrap()));
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
    async fn outbound_to_dead_port() -> Arc<SessionSocket> {
        let sock = loopback_udp().await;
        point_at_dead_port(&sock).await;
        tracked(sock)
    }

    /// Connect `sock` to a port nobody listens on and send one datagram.
    async fn point_at_dead_port(sock: &UdpSocket) {
        let dead = loopback_udp().await;
        let dead_addr = dead.local_addr().unwrap();
        drop(dead);
        sock.connect(dead_addr).await.unwrap();
        sock.send(b"ping").await.unwrap();
    }

    /// Slots for sessions built by these tests, apart from the node-wide count.
    static TEST_SLOTS: AtomicUsize = AtomicUsize::new(0);
    static TEST_SOURCES: SourceTable = SourceTable::new();

    /// A session socket counted under TEST_SLOTS and TEST_SOURCES.
    fn tracked(sock: UdpSocket) -> Arc<SessionSocket> {
        Arc::new(SessionSocket {
            sock,
            _slot: SessionSlot::acquire(&TEST_SLOTS, usize::MAX).unwrap(),
            _source: SourceSlot::acquire(
                &TEST_SOURCES,
                SourceKey::of(IpAddr::V4(Ipv4Addr::LOCALHOST)),
                usize::MAX,
            )
            .unwrap(),
        })
    }

    fn relay(
        outbound: Arc<SessionSocket>,
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

    fn session(outbound: &Arc<SessionSocket>) -> (UdpSession, oneshot::Receiver<()>) {
        let (tx, rx) = oneshot::channel();
        let s = UdpSession {
            outbound: outbound.clone(),
            last_active: tokio::time::Instant::now(),
            _stop: tx,
        };
        (s, rx)
    }

    /// v1.2.12: the node-wide UDP session cap — explicit setting first, else
    /// half the fd limit (bounded), else a fixed fallback.
    #[test]
    fn session_cap_follows_setting_then_fd_limit() {
        assert_eq!(session_cap_from(Some("5000".into()), Some(65_536)), 5000);
        assert_eq!(session_cap_from(None, Some(65_536)), 32_768);
        // A bad or zero setting falls back to the fd-based cap.
        assert_eq!(session_cap_from(Some("lots".into()), Some(65_536)), 32_768);
        assert_eq!(session_cap_from(Some("0".into()), Some(65_536)), 32_768);
        // A tiny fd limit still leaves some sessions; a huge one is bounded.
        assert_eq!(session_cap_from(None, Some(100)), 256);
        assert_eq!(session_cap_from(None, Some(u64::MAX)), 524_288);
        assert_eq!(session_cap_from(None, None), 4096);
    }

    /// v1.2.12: past the cap a new session gets no slot, and a slot comes back
    /// when its holder is dropped.
    #[test]
    fn session_slots_stop_at_the_cap_and_come_back() {
        static SLOTS: AtomicUsize = AtomicUsize::new(0);
        let a = SessionSlot::acquire(&SLOTS, 2).expect("first");
        let _b = SessionSlot::acquire(&SLOTS, 2).expect("second");
        assert!(SessionSlot::acquire(&SLOTS, 2).is_none(), "cap reached");
        drop(a);
        assert!(
            SessionSlot::acquire(&SLOTS, 2).is_some(),
            "a freed slot is reusable"
        );
    }

    /// v1.2.12: a session's place comes back when its socket closes — when
    /// the map entry AND everyone else holding the socket (the reply reader,
    /// an in-flight send) have let go. Freeing it with the map entry alone let
    /// more sockets stay open than the cap allows.
    #[tokio::test]
    async fn a_slot_is_held_until_the_socket_closes() {
        static SLOTS: AtomicUsize = AtomicUsize::new(0);
        static SOURCES: SourceTable = SourceTable::new();
        let ip = IpAddr::V4(Ipv4Addr::LOCALHOST);
        let outbound = Arc::new(SessionSocket {
            sock: loopback_udp().await,
            _slot: SessionSlot::acquire(&SLOTS, 1).unwrap(),
            _source: SourceSlot::acquire(&SOURCES, SourceKey::of(ip), 1).unwrap(),
        });
        let reader_copy = outbound.clone(); // what a still-running reader holds
        let sessions: Arc<Sessions> = Arc::new(DashMap::new());
        let src: SocketAddr = "127.0.0.1:40001".parse().unwrap();
        let (tx, _rx) = oneshot::channel();
        sessions.insert(
            src,
            UdpSession {
                outbound,
                last_active: tokio::time::Instant::now(),
                _stop: tx,
            },
        );
        assert!(SessionSlot::acquire(&SLOTS, 1).is_none());
        sessions.remove(&src);
        assert!(
            SessionSlot::acquire(&SLOTS, 1).is_none(),
            "the socket is still open, so its place is still taken"
        );
        assert!(
            SourceSlot::acquire(&SOURCES, SourceKey::of(ip), 1).is_none(),
            "and so is its place under its source's limit"
        );
        drop(reader_copy);
        assert!(SessionSlot::acquire(&SLOTS, 1).is_some());
        assert!(SourceSlot::acquire(&SOURCES, SourceKey::of(ip), 1).is_some());
    }

    /// `UDP_MAX_SESSIONS_PER_IP`: a positive number, else the default.
    #[test]
    fn per_ip_cap_follows_setting_else_default() {
        assert_eq!(per_ip_cap_from(Some("50".into())), 50);
        assert_eq!(per_ip_cap_from(Some(" 7 ".into())), 7);
        assert_eq!(per_ip_cap_from(None), 1024);
        assert_eq!(per_ip_cap_from(Some("0".into())), 1024);
        assert_eq!(per_ip_cap_from(Some("-3".into())), 1024);
        assert_eq!(per_ip_cap_from(Some("many".into())), 1024);
        assert_eq!(per_ip_cap_from(Some(String::new())), 1024);
    }

    fn key(s: &str) -> SourceKey {
        SourceKey::of(s.parse().unwrap())
    }

    /// IPv4 is counted per address, IPv6 per /64, and an IPv4-mapped IPv6
    /// address is the IPv4 host it names.
    #[test]
    fn sources_are_grouped_by_address_or_ipv6_64() {
        assert_eq!(key("192.0.2.1"), key("192.0.2.1"));
        assert_ne!(key("192.0.2.1"), key("192.0.2.2"));
        // Same /64, different interface identifiers.
        assert_eq!(
            key("2001:db8:1:2::1"),
            key("2001:db8:1:2:ffff:ffff:ffff:ffff")
        );
        // Different /64 (last bit of the prefix differs, or a different prefix).
        assert_ne!(key("2001:db8:1:2::1"), key("2001:db8:1:3::1"));
        assert_ne!(key("2001:db8:1:2::1"), key("2001:db8:2:2::1"));
        // IPv4-mapped IPv6 collapses onto the IPv4 key.
        assert_eq!(key("::ffff:192.0.2.1"), key("192.0.2.1"));
        assert_ne!(key("::ffff:192.0.2.1"), key("::ffff:192.0.2.2"));
        // Other low-valued IPv6 addresses are not mistaken for IPv4.
        assert_ne!(key("::1"), key("0.0.0.1"));
        assert!(matches!(key("::1"), SourceKey::V6Prefix64(0)));
    }

    /// One place per session for a source, up to its limit; another source is
    /// unaffected; a source with nothing left has no entry in the table.
    #[test]
    fn source_slots_stop_at_the_limit_and_leave_no_entry_behind() {
        static SOURCES: SourceTable = SourceTable::new();
        let a = key("192.0.2.1");
        let b = key("2001:db8::1");
        let a1 = SourceSlot::acquire(&SOURCES, a, 2).expect("first");
        let a2 = SourceSlot::acquire(&SOURCES, a, 2).expect("second");
        assert!(
            SourceSlot::acquire(&SOURCES, a, 2).is_none(),
            "limit reached"
        );
        assert!(
            SourceSlot::acquire(&SOURCES, b, 2).is_some(),
            "another source has its own share"
        );
        assert_eq!(SOURCES.map().len(), 1, "b's slot was dropped right away");
        drop(a1);
        let a3 = SourceSlot::acquire(&SOURCES, a, 2).expect("a freed place is reusable");
        drop(a2);
        drop(a3);
        assert!(SOURCES.map().is_empty());
    }

    /// The warning is allowed once per interval, however often it is asked.
    #[test]
    fn log_due_allows_one_per_interval() {
        let last = AtomicU64::new(0);
        assert!(log_due(&last, 100, 10));
        assert!(!log_due(&last, 100, 10));
        assert!(!log_due(&last, 109, 10));
        assert!(log_due(&last, 110, 10));
        assert!(!log_due(&last, 111, 10));
        // A clock stepping back never panics and never opens the gate early.
        assert!(!log_due(&last, 50, 10));
    }

    /// A refused source takes no node-wide place; a node-wide refusal gives
    /// the source's place back.
    #[tokio::test]
    async fn refusals_hold_no_places() {
        static SOURCES: SourceTable = SourceTable::new();
        static NODE: AtomicUsize = AtomicUsize::new(0);
        let a: IpAddr = "192.0.2.1".parse().unwrap();
        let b: IpAddr = "192.0.2.2".parse().unwrap();

        let first = acquire_session_slots(a, &SOURCES, 1, &NODE, 2).unwrap();
        assert!(matches!(
            acquire_session_slots(a, &SOURCES, 1, &NODE, 2),
            Err(Refusal::PerSource)
        ));
        assert_eq!(NODE.load(Ordering::Acquire), 1);
        let second = acquire_session_slots(b, &SOURCES, 1, &NODE, 2).unwrap();
        // The node is full: c is refused for that, and keeps no place of its own.
        let c: IpAddr = "192.0.2.3".parse().unwrap();
        assert!(matches!(
            acquire_session_slots(c, &SOURCES, 1, &NODE, 2),
            Err(Refusal::NodeWide)
        ));
        assert!(SOURCES.map().get(&SourceKey::of(c)).is_none());
        drop((first, second));
        assert_eq!(NODE.load(Ordering::Acquire), 0);
        assert!(SOURCES.map().is_empty());
    }

    /// Admit a session from `ip`, with a real socket, the way the receive loop does.
    async fn open_session(
        ip: IpAddr,
        sources: &'static SourceTable,
        per_ip_cap: usize,
        node: &'static AtomicUsize,
    ) -> Result<Arc<SessionSocket>, Refusal> {
        let (slot, source) = acquire_session_slots(ip, sources, per_ip_cap, node, usize::MAX)?;
        Ok(Arc::new(SessionSocket {
            sock: loopback_udp().await,
            _slot: slot,
            _source: source,
        }))
    }

    /// The per-source count is the number of sockets really open, whichever
    /// way a session ends: idle eviction, a failing reader, or the listener
    /// being torn down (rule removed or restarted). Refused sessions leave
    /// nothing behind, and another source is never affected.
    #[tokio::test]
    async fn source_counts_return_to_zero_however_a_session_ends() {
        static SOURCES: SourceTable = SourceTable::new();
        static NODE: AtomicUsize = AtomicUsize::new(0);
        const CAP: usize = 3;
        let a: IpAddr = "192.0.2.1".parse().unwrap();
        let b: IpAddr = "2001:db8::1".parse().unwrap();
        let count = |ip: IpAddr| SOURCES.map().get(&SourceKey::of(ip)).map_or(0, |c| *c);

        let sessions: Arc<Sessions> = Arc::new(DashMap::new());
        let inbound = Arc::new(loopback_udp().await);
        let at = |port: u16| SocketAddr::from(([127, 0, 0, 1], port));

        let evictable = open_session(a, &SOURCES, CAP, &NODE).await.unwrap();
        let failing = open_session(a, &SOURCES, CAP, &NODE).await.unwrap();
        let torn_down_a = open_session(a, &SOURCES, CAP, &NODE).await.unwrap();
        let torn_down_b = open_session(b, &SOURCES, CAP, &NODE).await.unwrap();
        assert!(
            matches!(
                open_session(a, &SOURCES, CAP, &NODE).await,
                Err(Refusal::PerSource)
            ),
            "a fourth session from one source is refused"
        );
        assert_eq!((count(a), count(b)), (3, 1));
        assert_eq!(NODE.load(Ordering::Acquire), 4);
        // A full source blocks nobody else, and another address in b's /64
        // is counted as b.
        let b_neighbour: IpAddr = "2001:db8::2".parse().unwrap();
        let b_second = open_session(b_neighbour, &SOURCES, CAP, &NODE)
            .await
            .unwrap();
        assert_eq!(count(b), 2);
        drop(b_second);
        assert_eq!(count(b), 1);

        // 1. Idle eviction: the sweeper's own function drops the entry.
        let (mut s, _stop) = session(&evictable);
        s.last_active = tokio::time::Instant::now()
            .checked_sub(UDP_SESSION_TIMEOUT + Duration::from_secs(1))
            .expect("the clock has run longer than the session timeout");
        sessions.insert(at(40001), s);
        drop(evictable);
        assert_eq!(count(a), 3, "still counted while the entry is in the map");
        assert_eq!(evict_expired(&sessions), 1);
        assert_eq!(count(a), 2);

        // 2. A reader whose socket fails removes its own session.
        point_at_dead_port(&failing).await;
        let mut r = relay(failing.clone(), inbound.clone(), sessions.clone());
        r.src = at(40002);
        let (s, stop) = session(&failing);
        sessions.insert(r.src, s);
        drop(failing);
        tokio::time::timeout(Duration::from_secs(5), r.run(stop))
            .await
            .expect("the dead target's error must end the reader");
        assert!(sessions.get(&at(40002)).is_none());
        assert_eq!(count(a), 1);

        // 3. Listener teardown (rule removed or restarted) clears every
        // session; their readers stop and the sockets close.
        let mut readers = Vec::new();
        for (port, outbound) in [(40003, torn_down_a), (40004, torn_down_b)] {
            let mut r = relay(outbound.clone(), inbound.clone(), sessions.clone());
            r.src = at(port);
            let (s, stop) = session(&outbound);
            sessions.insert(r.src, s);
            drop(outbound);
            readers.push(tokio::spawn(r.run(stop)));
        }
        drop(SessionsTeardown {
            sessions: sessions.clone(),
            sweeper: tokio::spawn(std::future::pending::<()>()).abort_handle(),
        });
        for reader in readers {
            tokio::time::timeout(Duration::from_secs(2), reader)
                .await
                .expect("teardown must stop the readers")
                .unwrap();
        }

        assert_eq!((count(a), count(b)), (0, 0));
        assert!(SOURCES.map().is_empty(), "no entry is left behind");
        assert_eq!(NODE.load(Ordering::Acquire), 0);
        // The freed places are usable again.
        assert!(open_session(a, &SOURCES, CAP, &NODE).await.is_ok());
    }

    /// v1.2.12: a reader waiting on the rate limit stops as soon as its
    /// session is evicted, and does not forward the datagram it was holding.
    #[tokio::test]
    async fn a_throttled_reader_stops_when_its_session_is_evicted() {
        let target = loopback_udp().await;
        let outbound = tracked(loopback_udp().await);
        outbound
            .connect(target.local_addr().unwrap())
            .await
            .unwrap();
        let client = loopback_udp().await;
        let sessions: Arc<Sessions> = Arc::new(DashMap::new());

        let mut r = relay(
            outbound.clone(),
            Arc::new(loopback_udp().await),
            sessions.clone(),
        );
        // 1000 B/s with a 1 s burst: a 5000-byte reply owes ~4 s of waiting.
        r.rate_limit = RateLimit::new(None, Some(1000));
        r.src = client.local_addr().unwrap();
        let src = r.src;
        let (s, stop) = session(&outbound);
        sessions.insert(src, s);
        drop(outbound);
        let reader = tokio::spawn(r.run(stop));

        target
            .send_to(
                &[7u8; 5000],
                sessions.get(&src).unwrap().outbound.local_addr().unwrap(),
            )
            .await
            .unwrap();
        // Let the reader take the datagram and start waiting on the limiter.
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert!(!reader.is_finished());

        sessions.remove(&src);
        tokio::time::timeout(Duration::from_secs(1), reader)
            .await
            .expect("an evicted reader must not sit out the rate-limit wait")
            .unwrap();
        let mut buf = [0u8; 8192];
        assert!(
            tokio::time::timeout(Duration::from_millis(300), client.recv_from(&mut buf))
                .await
                .is_err(),
            "the queued datagram must not be forwarded after eviction"
        );
    }

    /// v1.2.12 (H2): evicting an idle session must end its reply reader. The
    /// target here never answers, so a reader that only watched `recv()` would
    /// wait forever, holding its socket — one leaked task + fd per idle client.
    #[tokio::test]
    async fn evicting_a_session_stops_its_reader() {
        let target = loopback_udp().await; // alive, never replies
        let outbound = tracked(loopback_udp().await);
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
        let current = tracked(loopback_udp().await);
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
        assert!(addr_of(connect_udp_target(&targets, &selector, 5000, always_ok).await).is_none());
    }
}
