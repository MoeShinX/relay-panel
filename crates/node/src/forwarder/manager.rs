use super::gate::RuleRuntime;
use super::limiter::RateLimit;
use super::selector::TargetSelector;
use super::tcp;
use super::tls;
use super::udp;
use super::ws;
use crate::reporter::{ConnectionTracker, TrafficCounter};
use relay_shared::protocol::{
    ListenerConfig, ListenerError, LoadBalanceStrategy, NodeConfigResponse, NodeTransport, Protocol,
};
use std::collections::{HashMap, HashSet};
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::sync::Mutex;
use tokio::task::JoinHandle;

/// Key: (port, protocol, node_transport). This lets two listeners coexist on
/// the same port + L4 protocol when their transport differs — e.g. a raw TCP
/// rule and a WS rule both on port 12345 are two distinct listeners. (The
/// panel already guarantees no two rules share the same (port, protocol) when
/// transport matches; this key is the precise identity of a listener.)
type ListenerKey = (u16, Protocol, NodeTransport);

/// A snapshot of the fields that change a running listener's behaviour but are
/// NOT part of the [`ListenerKey`]. v0.3.6: this is the "config fingerprint"
/// used to decide whether an existing listener must be restarted (hot update)
/// or left alone.
///
/// Why each field is here:
/// - `rule_id`: traffic attribution. If the rule id changed (e.g. a rule was
///   deleted and a new one reuses the same port), the listener must restart so
///   traffic is attributed to the new rule.
/// - `targets`: where the listener forwards. Changing target_addr / target_port
///   / outbound connect_host changes this; without a restart the old task keeps
///   using the captured-old targets forever. Targets compare in ORDER — the
///   primary/secondary target priority must be preserved, so we do NOT sort.
/// - `ws_path`: only meaningful for Ws listeners, but harmless to include for
///   all (Raw/Udp always have None). A ws_path change must restart the WS
///   listener so it validates the new path.
///
/// `speed_limit` / `ip_limit` are deliberately NOT here: they are placeholder
/// fields that are always None in v0.3.x (the node has no limiter), so they
/// never change behaviour and must not trigger spurious restarts.
///
/// `upload_limit_bps` / `download_limit_bps` ARE here (v1.0.9): the rate limiter
/// is captured by the listener task when it spawns, so a limit change with no
/// other change would otherwise leave the running task on the OLD cap until the
/// node restarts. Including them forces a restart that re-reads the new cap.
#[derive(Clone, Debug, PartialEq, Eq)]
struct ListenerFingerprint {
    rule_id: i64,
    targets: Vec<String>,
    ws_path: Option<String>,
    /// v0.4.6: a strategy change must restart the listener so the new selector
    /// (and its cursor) takes effect.
    load_balance_strategy: LoadBalanceStrategy,
    /// v0.4.7: a transport change (raw↔ws↔tls_simple) must restart the listener
    /// so the right forwarder (tcp/ws/tls) is spawned. Derived from a tunnel
    /// profile, so it can change without the rule's listen port moving.
    node_transport: NodeTransport,
    /// v1.0.9: per-rule rate caps (BYTES/sec, None = unlimited). A change here
    /// (including clearing a limit) must restart the listener so the new cap
    /// takes effect without a node restart.
    upload_limit_bps: Option<u64>,
    download_limit_bps: Option<u64>,
    /// v1.2.0: concurrent-connection cap (None = unlimited). The accept loop
    /// captures this when it spawns, so a change must restart the listener for
    /// the new cap to apply — same reasoning as the rate caps above.
    ///
    /// Note this restart does NOT drop live connections (only an explicit
    /// `restart_rule` does that), so editing the cap on a busy rule is safe:
    /// existing connections keep running and the new cap governs admissions
    /// from that point on. A lowered cap takes effect by attrition.
    max_connections: Option<u32>,
}

impl ListenerFingerprint {
    fn from_listener(l: &ListenerConfig) -> Self {
        Self {
            rule_id: l.rule_id,
            targets: l.targets.clone(),
            ws_path: l.ws_path.clone(),
            load_balance_strategy: l.load_balance_strategy,
            node_transport: l.node_transport,
            upload_limit_bps: l.upload_limit_bps,
            download_limit_bps: l.download_limit_bps,
            max_connections: l.max_connections,
        }
    }
}

struct ManagedListener {
    handle: JoinHandle<()>,
    fingerprint: ListenerFingerprint,
}

/// v0.4.8: snapshot of one rule's listener state, for diagnosis. `running`
/// reflects whether the listener task is alive right now (a task can exit
/// without the manager knowing until the next apply).
#[derive(Debug, Clone)]
pub struct ListenerInfo {
    pub port: u16,
    pub protocol: String,
    pub transport: String,
    pub targets: Vec<String>,
    pub running: bool,
}

pub struct ForwarderManager {
    listeners: HashMap<ListenerKey, ManagedListener>,
    /// v1.2.0: per-rule runtime state (live-connection counter + restart
    /// cancellation), keyed by rule_id. Lives HERE rather than in the listener
    /// because a rule's IPv4 and IPv6 listeners must share one connection
    /// budget, and because it has to survive the listener being torn down and
    /// rebuilt by `apply_config` (a config edit must not reset the count while
    /// the connections it counted are still alive). Entries are dropped when the
    /// rule leaves the config, which cancels its connections — see `gate.rs`.
    rule_runtime: HashMap<i64, RuleRuntime>,
    /// v1.2.0: the most recent config, kept so `restart_rule` can rebuild a
    /// rule's listeners without asking the panel for the config again. A restart
    /// must be able to run while the control channel is busy, and re-fetching
    /// would also let a config change ride in on what the operator asked to be a
    /// pure restart.
    last_config: Option<NodeConfigResponse>,
    counter: Arc<TrafficCounter>,
    connections: Arc<ConnectionTracker>,
    /// Bind/runtime errors captured from spawned listener tasks since the last
    /// `take_listener_errors()`. Shared so a task can push its failure after the
    /// manager has already moved on. Drained by the status reporter.
    listener_errors: Arc<Mutex<Vec<ListenerError>>>,
    /// v0.4.1: shared TLS acceptor for tls_simple listeners (supports hot-reload
    /// via cert_reloader). None = no cert configured (tls_simple rules skipped).
    tls_acceptor: Option<super::cert_reloader::SharedTlsAcceptor>,
    /// v1.0.4: dual-stack listen addresses from env.
    listen_ipv4: String,
    listen_ipv6: String,
    /// v1.0.4: resolved outbound source IPv4 (None = auto-route).
    source_ipv4: Option<std::net::Ipv4Addr>,
}

impl ForwarderManager {
    pub fn new(counter: Arc<TrafficCounter>, connections: Arc<ConnectionTracker>) -> Self {
        Self {
            listeners: HashMap::new(),
            rule_runtime: HashMap::new(),
            last_config: None,
            counter,
            connections,
            listener_errors: Arc::new(Mutex::new(Vec::new())),
            tls_acceptor: None,
            listen_ipv4: "0.0.0.0".into(),
            listen_ipv6: "::".into(),
            source_ipv4: None,
        }
    }

    /// v1.0.4: configure dual-stack listen and outbound source.
    /// Returns Err on misconfigured outbound (invalid IP, missing interface,
    /// non-local IP) so the caller can abort instead of silently auto-routing
    /// out the wrong NIC.
    pub fn set_network_config(
        &mut self,
        cfg: &crate::config::NodeConfig,
    ) -> Result<(), crate::forwarder::outbound::OutboundError> {
        self.listen_ipv4 = cfg.listen_ipv4.clone();
        self.listen_ipv6 = cfg.listen_ipv6.clone();
        self.source_ipv4 = crate::forwarder::outbound::init_outbound(
            &crate::forwarder::outbound::OutboundConfig {
                bind_ipv4: cfg.outbound_bind_ipv4.clone(),
                interface: cfg.outbound_interface.clone(),
            },
        )?;
        Ok(())
    }

    /// Drain the accumulated listener errors (called by the status reporter so
    /// each error is reported exactly once, then cleared). An empty Vec means
    /// all listeners bound successfully since the last call.
    pub async fn take_listener_errors(&self) -> Vec<ListenerError> {
        self.listener_errors.lock().await.drain(..).collect()
    }

    /// v0.4.9: return the rule's TCP listener, for diagnosis. Diagnosis is
    /// TCP-only, and a tcp_udp rule runs TWO listeners (Tcp + Udp) keyed in a
    /// HashMap — iterating that map and taking the first match would be
    /// nondeterministic and could return the Udp listener. This filters on
    /// `Protocol::Tcp` so the TCP listener is selected deterministically.
    ///
    /// For a pure-tcp rule there is exactly one (Tcp) listener, so this returns
    /// it. A pure-udp rule has no Tcp listener and returns None — but the panel
    /// rejects pure-UDP rules before dispatching a probe, so that branch is
    /// unreachable in practice (kept defensive). `running` is the JoinHandle's
    /// `is_finished()` inverse — a task that has exited (without the manager
    /// re-applying config) is reported as not running.
    ///
    /// (v0.4.8 had a generic `listener_info_for_rule` that returned the first
    /// match regardless of L4; it was removed in v0.4.9 since diagnosis is now
    /// TCP-only and the nondeterministic selection was a latent bug for
    /// tcp_udp rules.)
    pub fn listener_info_for_rule_tcp(&self, rule_id: i64) -> Option<ListenerInfo> {
        for ((port, proto, transport), ml) in &self.listeners {
            if ml.fingerprint.rule_id == rule_id && *proto == Protocol::Tcp {
                return Some(ListenerInfo {
                    port: *port,
                    protocol: "tcp".to_string(),
                    transport: format!("{:?}", transport).to_lowercase(),
                    targets: ml.fingerprint.targets.clone(),
                    running: !ml.handle.is_finished(),
                });
            }
        }
        None
    }

    /// v0.4.1: set the shared TLS acceptor for tls_simple listeners. Called at
    /// startup after loading the cert+key (or starting the CertReloader).
    /// None = no cert (tls_simple rules skipped).
    pub fn set_tls_acceptor(&mut self, acceptor: Option<super::cert_reloader::SharedTlsAcceptor>) {
        self.tls_acceptor = acceptor;
    }

    /// v0.4.1: expose the listener_errors Arc so the CertReloader (spawned
    /// before the manager is wrapped in Arc<Mutex>) can push reload errors.
    pub fn listener_errors_arc(&self) -> Arc<Mutex<Vec<ListenerError>>> {
        Arc::clone(&self.listener_errors)
    }

    pub async fn apply_config(&mut self, config: &NodeConfigResponse) {
        // ── Step 1: recover dead listeners ──
        // v0.3.6: a listener task that exited (bind failure, unrecoverable
        // error, or the v0.3.5 "instant accept error killed the task" bug) left
        // its JoinHandle registered, so apply_config thought it was still
        // running and the port stayed dead until the node restarted. Now we
        // detect finished handles up front and drop them, so the restart logic
        // below can bring them back if they're still desired.
        let dead: Vec<ListenerKey> = self
            .listeners
            .iter()
            .filter(|(_, m)| m.handle.is_finished())
            .map(|(k, _)| *k)
            .collect();
        let mut dead_rule_ids: Vec<i64> = Vec::new();
        for key in &dead {
            let (port, proto, transport) = *key;
            tracing::warn!(
                "listener {:?}/{:?} on port {} has exited; will restart if still desired",
                proto,
                transport,
                port
            );
            if let Some(m) = self.listeners.remove(key) {
                dead_rule_ids.push(m.fingerprint.rule_id);
            }
        }

        // ── Step 2: compute the desired set ──
        // Protocol::TcpUdp should never appear here (the panel expands it), but
        // we skip it defensively.
        let active_keys: HashSet<ListenerKey> = config
            .listeners
            .iter()
            .filter(|l| l.protocol != Protocol::TcpUdp)
            .map(|l| (l.port, l.protocol, l.node_transport))
            .collect();

        // v0.5.1: collect the rule_ids present in the NEW config so we can
        // decide which stopped listeners truly belong to deleted rules (and
        // therefore need their traffic counters pruned) vs. listeners that
        // are merely being restarted with a different fingerprint.
        let desired_rule_ids: HashSet<i64> = config.listeners.iter().map(|l| l.rule_id).collect();

        // v0.5.1: prune counters for dead listeners whose rule is no longer in
        // the new config AND has no other live listener referencing it.
        for rule_id in &dead_rule_ids {
            if !desired_rule_ids.contains(rule_id)
                && !self
                    .listeners
                    .values()
                    .any(|live| live.fingerprint.rule_id == *rule_id)
            {
                self.counter.prune_rule(*rule_id).await;
            }
        }

        // ── Step 3: stop listeners no longer desired, AND restart listeners
        // whose fingerprint changed (target / ws_path / rule_id). Both are
        // "tear down the current task" — the restart case just immediately
        // re-adds it in step 4.
        let mut to_stop: Vec<ListenerKey> = self
            .listeners
            .keys()
            .filter(|k| !active_keys.contains(k))
            .copied()
            .collect();
        // Fingerprint-changed listeners that ARE still desired: stop them now so
        // step 4 starts them fresh with the new config.
        for listener in &config.listeners {
            let key = (listener.port, listener.protocol, listener.node_transport);
            if let Some(m) = self.listeners.get(&key) {
                let new_fp = ListenerFingerprint::from_listener(listener);
                if m.fingerprint != new_fp {
                    to_stop.push(key);
                }
            }
        }
        for key in to_stop {
            if let Some(m) = self.listeners.remove(&key) {
                let handle = m.handle;
                let (port, proto, transport) = key;
                handle.abort();
                // v0.3.6: await the aborted task so the OS releases the listen
                // socket BEFORE we try to re-bind on the same port in step 4.
                // Without this, the new bind can race the old task's teardown
                // and fail with "address already in use". A wait on an aborted
                // task returns promptly (it's just the cleanup signal).
                let _ = (&mut { handle }).await;
                // v0.5.1: prune traffic-counter entries for this rule_id when
                // the rule is genuinely gone (not just being restarted with a
                // new fingerprint) AND no other live listener still references
                // this rule_id (e.g. the UDP listener of a tcp_udp rule). This
                // prevents orphaned bytes from poisoning future traffic batches.
                let rule_id = m.fingerprint.rule_id;
                if !desired_rule_ids.contains(&rule_id)
                    && !self
                        .listeners
                        .values()
                        .any(|live| live.fingerprint.rule_id == rule_id)
                {
                    self.counter.prune_rule(rule_id).await;
                }
                tracing::info!(
                    "stopped {:?}/{:?} listener on port {} for reconfiguration",
                    proto,
                    transport,
                    port
                );
            }
        }

        // ── Step 4: start new / changed listeners ──
        // v0.4.6: per-rule rate limiters are shared across ALL listeners of the same
        // rule (so a tcp_udp rule's TCP + UDP listeners draw from one bucket, not
        // two). We index them by rule_id within this apply; identical caps on the
        // two expanded listeners of one rule produce one Arc<RuleLimiter>.
        let mut rule_limiters: HashMap<i64, RateLimit> = HashMap::new();
        for listener in &config.listeners {
            let key = (listener.port, listener.protocol, listener.node_transport);
            // Skip if already running with the SAME fingerprint (no change).
            if let Some(m) = self.listeners.get(&key) {
                if m.fingerprint == ListenerFingerprint::from_listener(listener) {
                    continue;
                }
            }

            // v1.0.4: dual-stack listen — parse IPs via IpAddr (NEVER string
            // concatenation, which produced ":::port" for IPv6). Empty string
            // = that family disabled.
            let ip_v4 = crate::forwarder::outbound::parse_listen_ip(&self.listen_ipv4);
            let ip_v6 = crate::forwarder::outbound::parse_listen_ip(&self.listen_ipv6);
            let targets = listener.targets.clone();
            // v0.4.6: one selector per listener, shared across all of its
            // connections/sessions so a round-robin cursor advances globally.
            let selector = Arc::new(TargetSelector::new(
                listener.load_balance_strategy,
                targets.len(),
            ));
            // v0.4.6: shared per-rule limiter. Both expanded listeners of a
            // tcp_udp rule reuse the same Arc so the budget isn't doubled.
            let rate_limit = rule_limiters
                .entry(listener.rule_id)
                .or_insert_with(|| {
                    RateLimit::new(listener.upload_limit_bps, listener.download_limit_bps)
                })
                .clone();
            let counter = self.counter.clone();
            let connections = self.connections.clone();
            let port = listener.port;
            let rule_id = listener.rule_id;
            let ws_path = listener.ws_path.clone();
            let errors = self.listener_errors.clone();
            let src_ipv4 = self.source_ipv4;
            let proto_str = match listener.protocol {
                Protocol::Tcp => "tcp",
                Protocol::Udp => "udp",
                Protocol::TcpUdp => "tcpudp",
            }
            .to_string();

            // Defensive guards before spawning.
            // UDP only supports Raw transport (WS/TLS are TCP-only).
            if listener.protocol == Protocol::Udp && listener.node_transport != NodeTransport::Raw {
                tracing::warn!(
                    "rule {}: UDP does not support node_transport {:?} — skipping listener on {}",
                    rule_id,
                    listener.node_transport,
                    port
                );
                continue;
            }
            // v1.0.8: WS / TLS entry transports are DISABLED at runtime. The
            // panel hid them in v0.4.20 (every rule is `raw`), and having the
            // NODE terminate WS/TLS is fundamentally incompatible with
            // transparently relaying an end-to-end tunnel — VLESS+WS+TLS,
            // Trojan, VMess, etc. MUST be raw-forwarded, because the client's
            // WS/TLS handshake is meant for the FINAL server, not this relay.
            // The implementations (ws.rs / tls.rs / cert_reloader.rs) are kept
            // for possible future revival but are never served. A stray ws/tls
            // config is skipped + reported so the operator can see why the port
            // isn't forwarding (the `(Tcp, Ws)` / `(Tcp, TlsSimple)` match arms
            // below are consequently unreachable at runtime — kept on purpose).
            if matches!(
                listener.node_transport,
                NodeTransport::Ws | NodeTransport::TlsSimple
            ) {
                tracing::warn!(
                    "rule {}: node_transport {:?} is disabled and will not be served — \
                     skipping listener on port {} (use raw; WS/TLS entry transport is retired)",
                    rule_id,
                    listener.node_transport,
                    port
                );
                errors.lock().await.push(ListenerError {
                    port,
                    protocol: proto_str.clone(),
                    error: format!("{:?} entry transport is disabled", listener.node_transport),
                });
                continue;
            }

            let handle: tokio::task::JoinHandle<()> = match (
                listener.protocol,
                listener.node_transport,
            ) {
                // v1.0.4: TCP — bind BOTH families synchronously (errors surface
                // now, per-family success known), then supervise both serve loops
                // with select! so if either dies the task ends and the manager's
                // dead-listener detection restarts it.
                (Protocol::Tcp, NodeTransport::Raw) => {
                    use crate::forwarder::outbound::bind_tcp_listener;
                    let mut v4_listener = None;
                    let mut v6_listener = None;
                    if let Some(ip4) = ip_v4 {
                        match bind_tcp_listener(ip4, port) {
                            Ok(l) => {
                                tracing::info!(
                                    "TCP bound {} (rule {})",
                                    SocketAddr::new(ip4, port),
                                    rule_id
                                );
                                v4_listener = Some(l);
                            }
                            Err(e) => {
                                tracing::error!("TCP IPv4 bind {}:{} failed: {}", ip4, port, e);
                                errors.lock().await.push(ListenerError {
                                    port,
                                    protocol: proto_str.clone(),
                                    error: format!("IPv4: {}", e),
                                });
                            }
                        }
                    }
                    if let Some(ip6) = ip_v6 {
                        match bind_tcp_listener(ip6, port) {
                            Ok(l) => {
                                tracing::info!(
                                    "TCP bound {} (rule {})",
                                    SocketAddr::new(ip6, port),
                                    rule_id
                                );
                                v6_listener = Some(l);
                            }
                            Err(e) => {
                                tracing::warn!(
                                    "TCP IPv6 bind [{}]:{} failed: {} — IPv4 continues",
                                    ip6,
                                    port,
                                    e
                                );
                                errors.lock().await.push(ListenerError {
                                    port,
                                    protocol: proto_str.clone(),
                                    error: format!("IPv6: {}", e),
                                });
                            }
                        }
                    }
                    // Only fail the rule when NEITHER family bound.
                    if v4_listener.is_none() && v6_listener.is_none() {
                        tracing::error!(
                            "TCP rule {}: no listener bound on port {} (all families failed)",
                            rule_id,
                            port
                        );
                        continue;
                    }
                    let tgt = targets.clone();
                    let sel = selector.clone();
                    let rl = rate_limit.clone();
                    let ctr = counter.clone();
                    let cn = connections.clone();
                    let rid = rule_id;
                    let ipv4_src = src_ipv4;
                    // v1.2.0: both families get a gate cloned from the SAME
                    // RuleRuntime, so `max_connections` is a per-rule total
                    // rather than a per-family allowance.
                    let gate4 = self
                        .rule_runtime
                        .entry(rule_id)
                        .or_default()
                        .gate(listener.max_connections);
                    let gate6 = gate4.clone();
                    tokio::spawn(async move {
                        type SrvResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;
                        let (tgt4, sel4, rl4, ctr4, cn4) = (
                            tgt.clone(),
                            sel.clone(),
                            rl.clone(),
                            ctr.clone(),
                            cn.clone(),
                        );
                        let v4_fut = async move {
                            if let Some(l) = v4_listener {
                                tcp::serve_tcp_listener(
                                    l, tgt4, sel4, rl4, ctr4, cn4, rid, ipv4_src, gate4,
                                )
                                .await
                            } else {
                                std::future::pending::<SrvResult>().await
                            }
                        };
                        let v6_fut = async move {
                            if let Some(l) = v6_listener {
                                tcp::serve_tcp_listener(
                                    l, tgt, sel, rl, ctr, cn, rid, ipv4_src, gate6,
                                )
                                .await
                            } else {
                                std::future::pending::<SrvResult>().await
                            }
                        };
                        tokio::select! {
                            r = v4_fut => { if let Err(e) = r { tracing::error!("TCP v4 serve ended (rule {}): {}", rid, e); } }
                            r = v6_fut => { if let Err(e) = r { tracing::error!("TCP v6 serve ended (rule {}): {}", rid, e); } }
                        }
                    })
                }
                // v1.0.4: UDP — bind BOTH families synchronously, supervise both
                // receive loops with select! (mirrors the TCP arm above).
                (Protocol::Udp, NodeTransport::Raw) => {
                    use crate::forwarder::outbound::bind_udp_socket;
                    let mut v4_sock = None;
                    let mut v6_sock = None;
                    if let Some(ip4) = ip_v4 {
                        match bind_udp_socket(ip4, port) {
                            Ok(s) => {
                                tracing::info!(
                                    "UDP bound {} (rule {})",
                                    SocketAddr::new(ip4, port),
                                    rule_id
                                );
                                v4_sock = Some(Arc::new(s));
                            }
                            Err(e) => {
                                tracing::error!("UDP IPv4 bind {}:{} failed: {}", ip4, port, e);
                                errors.lock().await.push(ListenerError {
                                    port,
                                    protocol: proto_str.clone(),
                                    error: format!("IPv4: {}", e),
                                });
                            }
                        }
                    }
                    if let Some(ip6) = ip_v6 {
                        match bind_udp_socket(ip6, port) {
                            Ok(s) => {
                                tracing::info!(
                                    "UDP bound {} (rule {})",
                                    SocketAddr::new(ip6, port),
                                    rule_id
                                );
                                v6_sock = Some(Arc::new(s));
                            }
                            Err(e) => {
                                tracing::warn!(
                                    "UDP IPv6 bind [{}]:{} failed: {} — IPv4 continues",
                                    ip6,
                                    port,
                                    e
                                );
                                errors.lock().await.push(ListenerError {
                                    port,
                                    protocol: proto_str.clone(),
                                    error: format!("IPv6: {}", e),
                                });
                            }
                        }
                    }
                    if v4_sock.is_none() && v6_sock.is_none() {
                        tracing::error!(
                            "UDP rule {}: no listener bound on port {} (all families failed)",
                            rule_id,
                            port
                        );
                        continue;
                    }
                    let tgt = targets.clone();
                    let sel = selector.clone();
                    let rl = rate_limit.clone();
                    let ctr = counter.clone();
                    let cn = connections.clone();
                    let rid = rule_id;
                    let ipv4_src = src_ipv4;
                    tokio::spawn(async move {
                        type SrvResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;
                        let (tgt4, sel4, rl4, ctr4, cn4) = (
                            tgt.clone(),
                            sel.clone(),
                            rl.clone(),
                            ctr.clone(),
                            cn.clone(),
                        );
                        let v4_fut = async move {
                            if let Some(s) = v4_sock {
                                udp::serve_udp_listener(
                                    s, tgt4, sel4, rl4, ctr4, cn4, rid, ipv4_src,
                                )
                                .await
                            } else {
                                std::future::pending::<SrvResult>().await
                            }
                        };
                        let v6_fut = async move {
                            if let Some(s) = v6_sock {
                                udp::serve_udp_listener(s, tgt, sel, rl, ctr, cn, rid, ipv4_src)
                                    .await
                            } else {
                                std::future::pending::<SrvResult>().await
                            }
                        };
                        tokio::select! {
                            r = v4_fut => { if let Err(e) = r { tracing::error!("UDP v4 serve ended (rule {}): {}", rid, e); } }
                            r = v6_fut => { if let Err(e) = r { tracing::error!("UDP v6 serve ended (rule {}): {}", rid, e); } }
                        }
                    })
                }
                // WS and TLS use IPv4 only (unchanged — this PR does not extend
                // their IPv6/outbound capability).
                (Protocol::Tcp, NodeTransport::Ws) => {
                    let ws_addr = SocketAddr::new(
                        ip_v4.unwrap_or(std::net::IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED)),
                        port,
                    );
                    tokio::spawn(async move {
                        if let Err(e) = ws::start_ws_listener(
                            ws_addr,
                            targets,
                            selector,
                            rate_limit,
                            counter,
                            connections,
                            rule_id,
                            ws_path,
                        )
                        .await
                        {
                            tracing::error!("WS listener on {} failed: {}", port, e);
                            errors.lock().await.push(ListenerError {
                                port,
                                protocol: proto_str.clone(),
                                error: e.to_string(),
                            });
                        }
                    })
                }
                // v0.4.1: TLS Simple — node terminates TLS, then forwards TCP.
                // The tls_acceptor is cloned from the manager's shared Arc.
                // If None, the guard above already skipped this listener.
                (Protocol::Tcp, NodeTransport::TlsSimple) => {
                    let Some(tls_acceptor) = self.tls_acceptor.clone() else {
                        // Unreachable (guard above checks this), but defensive.
                        continue;
                    };
                    let tls_addr = SocketAddr::new(
                        ip_v4.unwrap_or(std::net::IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED)),
                        port,
                    );
                    tokio::spawn(async move {
                        if let Err(e) = tls::start_tls_listener(
                            tls_addr,
                            targets,
                            selector,
                            rate_limit,
                            counter,
                            connections,
                            rule_id,
                            tls_acceptor,
                        )
                        .await
                        {
                            tracing::error!("TLS listener on {} failed: {}", port, e);
                            errors.lock().await.push(ListenerError {
                                port,
                                protocol: proto_str.clone(),
                                error: e.to_string(),
                            });
                        }
                    })
                }
                (Protocol::TcpUdp, _) => {
                    tracing::warn!(
                        "Received Protocol::TcpUdp in node — panel should have expanded it. Skipping."
                    );
                    continue;
                }
                (proto, transport) => {
                    tracing::warn!(
                        "rule {}: no listener implementation for {:?}/{:?} — skipping port {}",
                        rule_id,
                        proto,
                        transport,
                        port
                    );
                    continue;
                }
            };

            self.listeners.insert(
                key,
                ManagedListener {
                    handle,
                    fingerprint: ListenerFingerprint::from_listener(listener),
                },
            );
        }

        // ── Step 5: forget rules that are gone ──
        // v1.2.0: dropping a rule's RuleRuntime drops its watch::Sender, which
        // cancels any connection still forwarding for it. That is deliberate: a
        // rule removed from the config can no longer have its traffic attributed
        // or billed (step 2/3 already pruned its counters), so letting its
        // connections outlive it would forward bytes nobody accounts for.
        self.rule_runtime
            .retain(|rule_id, _| desired_rule_ids.contains(rule_id));

        // v1.2.0: remember the applied config so restart_rule can rebuild a
        // rule's listeners from it without a round-trip to the panel.
        self.last_config = Some(config.clone());
    }

    /// v1.2.0: restart ONE rule — drop every connection it is currently
    /// forwarding, then rebuild its listeners from the last applied config.
    ///
    /// Returns `(connections_dropped, listeners_restarted)`. A rule with no
    /// listeners on this node returns `(0, 0)`; the caller reports that as
    /// "nothing to do here" rather than an error, because a rule legitimately
    /// spans only some of a group's nodes.
    ///
    /// Order matters. Connections are cancelled BEFORE the listeners are torn
    /// down and rebuilt: the connection tasks are detached, so tearing the
    /// listener down first would rebind the port while the old connections kept
    /// forwarding — the exact no-op this command exists to avoid.
    ///
    /// The rule's `paused` state is never consulted or written here. A restart
    /// is not a state transition: it re-creates whatever the current config
    /// says should be running. If the panel has paused the rule, the config
    /// carries no listener for it and this is a no-op.
    pub async fn restart_rule(&mut self, rule_id: i64) -> (u64, usize) {
        // Cancel first — see the ordering note above.
        let dropped = match self.rule_runtime.get(&rule_id) {
            Some(rt) => rt.cancel_all(),
            // No runtime → the rule has never had a listener here. Nothing to
            // restart; don't fabricate an empty runtime for it.
            None => return (0, 0),
        };

        let keys: Vec<ListenerKey> = self
            .listeners
            .iter()
            .filter(|(_, m)| m.fingerprint.rule_id == rule_id)
            .map(|(k, _)| *k)
            .collect();

        for key in &keys {
            if let Some(m) = self.listeners.remove(key) {
                let handle = m.handle;
                handle.abort();
                // Await the aborted task so the OS releases the listen socket
                // before the rebuild re-binds the same port — without this the
                // bind races teardown and fails with "address already in use".
                // (Same reason as the equivalent await in apply_config.)
                let _ = (&mut { handle }).await;
            }
        }

        let restarted = keys.len();
        if restarted > 0 {
            // Rebuild from the cached config. apply_config re-creates exactly
            // the listeners we just removed (every other listener still matches
            // its fingerprint and is skipped), and it reuses this rule's
            // existing RuleRuntime, so the live counter stays consistent with
            // the connections that survive — there are none, we just cancelled
            // them all.
            if let Some(cfg) = self.last_config.clone() {
                self.apply_config(&cfg).await;
            } else {
                tracing::warn!(
                    "rule {}: restart tore down {} listener(s) but no cached config exists \
                     to rebuild from; the next config push will restore them",
                    rule_id,
                    restarted
                );
            }
        }

        tracing::info!(
            "rule {}: restarted — dropped {} connection(s), rebuilt {} listener(s)",
            rule_id,
            dropped,
            restarted
        );
        (dropped, restarted)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::reporter::{ConnectionTracker, TrafficCounter};
    use relay_shared::protocol::{ListenerConfig, NodeConfigResponse, NodeTransport, Protocol};
    use std::sync::Arc;
    use std::time::Duration;

    impl ForwarderManager {
        /// Test-only accessor: the set of listener keys currently registered.
        fn listener_keys(&self) -> Vec<ListenerKey> {
            self.listeners.keys().copied().collect()
        }

        /// Test-only accessor for a listener's fingerprint, if present.
        fn fingerprint(&self, key: &ListenerKey) -> Option<ListenerFingerprint> {
            self.listeners.get(key).map(|m| m.fingerprint.clone())
        }
    }

    /// Build a single-rule config. `targets` defaults to a dummy; tests that
    /// exercise hot-update pass explicit targets.
    fn one_rule(port: u16, proto: Protocol, transport: NodeTransport) -> NodeConfigResponse {
        NodeConfigResponse {
            listeners: vec![ListenerConfig {
                rule_id: 1,
                port,
                protocol: proto,
                node_transport: transport,
                ws_path: None,
                targets: vec!["127.0.0.1:1".into()],
                load_balance_strategy: relay_shared::protocol::LoadBalanceStrategy::First,
                upload_limit_bps: None,
                download_limit_bps: None,
                max_connections: None,
            }],
        }
    }

    fn cfg(
        port: u16,
        proto: Protocol,
        transport: NodeTransport,
        targets: Vec<&str>,
        ws_path: Option<&str>,
    ) -> NodeConfigResponse {
        NodeConfigResponse {
            listeners: vec![ListenerConfig {
                rule_id: 1,
                port,
                protocol: proto,
                node_transport: transport,
                ws_path: ws_path.map(str::to_string),
                targets: targets.into_iter().map(String::from).collect(),
                load_balance_strategy: relay_shared::protocol::LoadBalanceStrategy::First,
                upload_limit_bps: None,
                download_limit_bps: None,
                max_connections: None,
            }],
        }
    }

    fn fresh_mgr() -> ForwarderManager {
        ForwarderManager::new(
            Arc::new(TrafficCounter::new()),
            Arc::new(ConnectionTracker::new()),
        )
    }

    /// v1.0.9: a rate-limit change (set OR cleared) must change the listener
    /// fingerprint so apply() restarts the listener and the new cap takes
    /// effect — without this, a running task keeps its captured old cap until
    /// the node restarts.
    #[test]
    fn rate_limit_change_alters_fingerprint() {
        let mut l = one_rule(40050, Protocol::Tcp, NodeTransport::Raw)
            .listeners
            .pop()
            .unwrap();
        let unlimited = ListenerFingerprint::from_listener(&l);

        l.upload_limit_bps = Some(1_000_000);
        let up_limited = ListenerFingerprint::from_listener(&l);
        assert_ne!(
            unlimited, up_limited,
            "setting an upload cap must change the fingerprint"
        );

        // Clearing the upload cap and setting a download cap: still distinct.
        l.upload_limit_bps = None;
        l.download_limit_bps = Some(2_000_000);
        let down_limited = ListenerFingerprint::from_listener(&l);
        assert_ne!(up_limited, down_limited);
        assert_ne!(unlimited, down_limited);
    }

    /// v1.2.0: a connection-cap change must restart the listener, for the same
    /// reason a rate-limit change must — the accept loop captures the cap when
    /// it spawns.
    #[test]
    fn max_connections_change_alters_fingerprint() {
        let mut l = one_rule(40051, Protocol::Tcp, NodeTransport::Raw)
            .listeners
            .pop()
            .unwrap();
        let uncapped = ListenerFingerprint::from_listener(&l);

        l.max_connections = Some(100);
        let capped = ListenerFingerprint::from_listener(&l);
        assert_ne!(
            uncapped, capped,
            "setting a connection cap must change the fingerprint"
        );

        l.max_connections = Some(200);
        assert_ne!(
            capped,
            ListenerFingerprint::from_listener(&l),
            "raising the cap must also change the fingerprint"
        );
    }

    /// Spawn an echo server and return its address. Used by the restart tests to
    /// prove a forwarded connection is really carrying data.
    async fn echo_target() -> SocketAddr {
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap();
        tokio::spawn(async move {
            loop {
                let Ok((mut s, _)) = l.accept().await else {
                    return;
                };
                tokio::spawn(async move {
                    use tokio::io::{AsyncReadExt, AsyncWriteExt};
                    let mut b = [0u8; 256];
                    loop {
                        match s.read(&mut b).await {
                            Ok(0) | Err(_) => return,
                            Ok(n) => {
                                if s.write_all(&b[..n]).await.is_err() {
                                    return;
                                }
                            }
                        }
                    }
                });
            }
        });
        addr
    }

    /// THE test for the restart feature: a live connection must actually be
    /// dropped, and the listener must come back up.
    ///
    /// This is not a formality. Connection tasks are detached `tokio::spawn`s,
    /// so the intuitive implementation (abort the listener, re-bind) leaves
    /// every established connection forwarding — the port gets rebound and not
    /// one connection is shed, which is the whole point of the feature. If this
    /// test regresses to "connection still alive", the restart is a placebo.
    #[tokio::test]
    async fn restart_rule_drops_live_connections_and_rebuilds_listener() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let target = echo_target().await;
        let mut mgr = fresh_mgr();
        mgr.listen_ipv6 = String::new(); // IPv4-only keeps the assertions simple.
        let port = 40561;
        let c = cfg(
            port,
            Protocol::Tcp,
            NodeTransport::Raw,
            vec![&target.to_string()],
            None,
        );
        mgr.apply_config(&c).await;

        // Establish a connection and prove it forwards.
        let mut client = tokio::net::TcpStream::connect(("127.0.0.1", port))
            .await
            .expect("listener must be up");
        client.write_all(b"before").await.unwrap();
        let mut buf = [0u8; 64];
        let n = client.read(&mut buf).await.unwrap();
        assert_eq!(&buf[..n], b"before", "the rule must forward before restart");

        let (dropped, restarted) = mgr.restart_rule(1).await;
        assert_eq!(dropped, 1, "the one live connection must be counted");
        assert_eq!(restarted, 1, "the rule's single TCP listener must rebuild");

        // The OLD connection must now be dead. Read returns EOF (or errors) —
        // anything that echoes here means the restart shed nothing.
        let r = tokio::time::timeout(Duration::from_secs(2), client.read(&mut buf)).await;
        match r {
            Ok(Ok(0)) | Ok(Err(_)) => {}
            Ok(Ok(n)) => panic!(
                "restart did NOT drop the connection — it echoed {:?}",
                String::from_utf8_lossy(&buf[..n])
            ),
            Err(_) => panic!("restart did NOT drop the connection — the read is still hanging"),
        }

        // ...and the listener must be serving again, on the same port.
        let mut fresh = tokio::net::TcpStream::connect(("127.0.0.1", port))
            .await
            .expect("listener must be re-bound after restart");
        fresh.write_all(b"after").await.unwrap();
        let n = fresh.read(&mut buf).await.unwrap();
        assert_eq!(&buf[..n], b"after", "the rebuilt listener must forward");
    }

    /// Restarting a rule this node doesn't serve is a no-op, not a panic or a
    /// fabricated runtime entry — a rule legitimately spans only some nodes.
    #[tokio::test]
    async fn restart_unknown_rule_is_a_noop() {
        let mut mgr = fresh_mgr();
        assert_eq!(mgr.restart_rule(999).await, (0, 0));
        assert!(
            mgr.rule_runtime.is_empty(),
            "must not create runtime state for a rule that isn't here"
        );
    }

    /// The cap must survive an unrelated config push. apply_config rebuilds its
    /// local limiter map each run; if the connection counter were rebuilt the
    /// same way, every config edit would reset the count to 0 while the counted
    /// connections were still alive, and the cap would over-admit.
    #[tokio::test]
    async fn rule_runtime_survives_apply_and_is_dropped_with_the_rule() {
        let mut mgr = fresh_mgr();
        mgr.listen_ipv6 = String::new();
        let c = cfg(
            40562,
            Protocol::Tcp,
            NodeTransport::Raw,
            vec!["127.0.0.1:1"],
            None,
        );
        mgr.apply_config(&c).await;
        assert!(
            mgr.rule_runtime.contains_key(&1),
            "runtime created on apply"
        );

        // Re-applying the identical config must not disturb the runtime.
        mgr.apply_config(&c).await;
        assert!(
            mgr.rule_runtime.contains_key(&1),
            "an unchanged apply must keep the rule's runtime"
        );

        // Removing the rule drops its runtime (which cancels its connections).
        mgr.apply_config(&NodeConfigResponse { listeners: vec![] })
            .await;
        assert!(
            !mgr.rule_runtime.contains_key(&1),
            "a removed rule must not leak its runtime"
        );
    }

    #[tokio::test]
    async fn raw_tcp_and_udp_are_scheduled() {
        let mut mgr = fresh_mgr();
        let c = NodeConfigResponse {
            listeners: vec![
                ListenerConfig {
                    rule_id: 1,
                    port: 40001,
                    protocol: Protocol::Tcp,
                    node_transport: NodeTransport::Raw,
                    ws_path: None,
                    targets: vec!["127.0.0.1:1".into()],
                    load_balance_strategy: relay_shared::protocol::LoadBalanceStrategy::First,
                    upload_limit_bps: None,
                    download_limit_bps: None,
                    max_connections: None,
                },
                ListenerConfig {
                    rule_id: 2,
                    port: 40002,
                    protocol: Protocol::Udp,
                    node_transport: NodeTransport::Raw,
                    ws_path: None,
                    targets: vec!["127.0.0.1:1".into()],
                    load_balance_strategy: relay_shared::protocol::LoadBalanceStrategy::First,
                    upload_limit_bps: None,
                    download_limit_bps: None,
                    max_connections: None,
                },
            ],
        };
        mgr.apply_config(&c).await;
        let keys = mgr.listener_keys();
        assert!(keys.contains(&(40001, Protocol::Tcp, NodeTransport::Raw)));
        assert!(keys.contains(&(40002, Protocol::Udp, NodeTransport::Raw)));
    }

    /// v1.0.8: WS entry transport is disabled — a ws rule must NOT start a
    /// listener, and a listener_error must be reported so the panel shows why.
    #[tokio::test]
    async fn ws_ingress_is_disabled() {
        let mut mgr = fresh_mgr();
        mgr.apply_config(&one_rule(40010, Protocol::Tcp, NodeTransport::Ws))
            .await;
        assert!(
            mgr.listener_keys().is_empty(),
            "ws entry transport is disabled — no listener must start"
        );
        let errs = mgr.take_listener_errors().await;
        assert_eq!(errs.len(), 1, "a listener_error must be pushed");
        assert!(errs[0].error.contains("disabled"), "got: {}", errs[0].error);
    }

    /// v1.0.8: TLS entry transport is disabled — a tls_simple rule is skipped
    /// (regardless of whether a cert is configured) with a reported error.
    #[tokio::test]
    async fn tls_simple_is_disabled() {
        let mut mgr = fresh_mgr();
        mgr.apply_config(&one_rule(40030, Protocol::Tcp, NodeTransport::TlsSimple))
            .await;
        assert!(
            mgr.listener_keys().is_empty(),
            "tls_simple is disabled — no listener must start"
        );
        let errs = mgr.take_listener_errors().await;
        assert_eq!(errs.len(), 1, "a listener_error must be pushed");
        assert!(errs[0].error.contains("disabled"), "got: {}", errs[0].error);
    }

    #[tokio::test]
    async fn udp_with_ws_is_skipped() {
        let mut mgr = fresh_mgr();
        mgr.apply_config(&one_rule(40040, Protocol::Udp, NodeTransport::Ws))
            .await;
        assert!(mgr.listener_keys().is_empty());
    }

    /// The ListenerKey includes the protocol, so a TCP and a UDP raw listener
    /// on the SAME port are two distinct listeners. (v1.0.8: this used to pair
    /// raw+ws, but ws is disabled now; tcp+udp is the remaining same-port case.)
    #[tokio::test]
    async fn same_port_tcp_and_udp_are_distinct_listeners() {
        let mut mgr = fresh_mgr();
        let c = NodeConfigResponse {
            listeners: vec![
                ListenerConfig {
                    rule_id: 1,
                    port: 40050,
                    protocol: Protocol::Tcp,
                    node_transport: NodeTransport::Raw,
                    ws_path: None,
                    targets: vec!["127.0.0.1:1".into()],
                    load_balance_strategy: relay_shared::protocol::LoadBalanceStrategy::First,
                    upload_limit_bps: None,
                    download_limit_bps: None,
                    max_connections: None,
                },
                ListenerConfig {
                    rule_id: 2,
                    port: 40050,
                    protocol: Protocol::Udp,
                    node_transport: NodeTransport::Raw,
                    ws_path: None,
                    targets: vec!["127.0.0.1:1".into()],
                    load_balance_strategy: relay_shared::protocol::LoadBalanceStrategy::First,
                    upload_limit_bps: None,
                    download_limit_bps: None,
                    max_connections: None,
                },
            ],
        };
        mgr.apply_config(&c).await;
        assert_eq!(mgr.listener_keys().len(), 2);
    }

    // ── v0.3.6: hot update + finished recovery ──

    /// Identical config applied twice must NOT restart the listener — the
    /// fingerprint comparison is an equality check, so the second apply is a
    /// no-op. We assert by checking the fingerprint object identity is unchanged
    /// and the key stays registered exactly once.
    #[tokio::test]
    async fn identical_config_does_not_restart() {
        let mut mgr = fresh_mgr();
        let c = cfg(
            40060,
            Protocol::Tcp,
            NodeTransport::Raw,
            vec!["127.0.0.1:9"],
            None,
        );
        mgr.apply_config(&c).await;
        let fp_before = mgr
            .fingerprint(&(40060, Protocol::Tcp, NodeTransport::Raw))
            .unwrap();
        // Re-apply the exact same config.
        mgr.apply_config(&c).await;
        let fp_after = mgr
            .fingerprint(&(40060, Protocol::Tcp, NodeTransport::Raw))
            .unwrap();
        assert_eq!(fp_before, fp_after, "fingerprint must be unchanged");
        assert_eq!(mgr.listener_keys().len(), 1);
    }

    /// Changing targets must restart the listener so the new target is used.
    /// We observe the restart via the fingerprint change (the new targets are
    /// captured on the re-registered listener).
    #[tokio::test]
    async fn target_change_restarts_listener() {
        let mut mgr = fresh_mgr();
        let c1 = cfg(
            40061,
            Protocol::Tcp,
            NodeTransport::Raw,
            vec!["127.0.0.1:9"],
            None,
        );
        mgr.apply_config(&c1).await;
        assert_eq!(
            mgr.fingerprint(&(40061, Protocol::Tcp, NodeTransport::Raw))
                .unwrap()
                .targets,
            vec!["127.0.0.1:9".to_string()]
        );

        let c2 = cfg(
            40061,
            Protocol::Tcp,
            NodeTransport::Raw,
            vec!["127.0.0.1:10"],
            None,
        );
        mgr.apply_config(&c2).await;
        assert_eq!(
            mgr.fingerprint(&(40061, Protocol::Tcp, NodeTransport::Raw))
                .unwrap()
                .targets,
            vec!["127.0.0.1:10".to_string()],
            "target change must update the running fingerprint"
        );
    }

    /// Target ORDER matters (primary vs secondary). Reordering without changing
    /// the set must still count as a change — we must not sort before comparing.
    #[tokio::test]
    async fn target_order_is_significant() {
        let mut mgr = fresh_mgr();
        let c1 = cfg(
            40062,
            Protocol::Tcp,
            NodeTransport::Raw,
            vec!["127.0.0.1:9", "127.0.0.1:10"],
            None,
        );
        mgr.apply_config(&c1).await;
        let fp1 = mgr
            .fingerprint(&(40062, Protocol::Tcp, NodeTransport::Raw))
            .unwrap();
        let c2 = cfg(
            40062,
            Protocol::Tcp,
            NodeTransport::Raw,
            vec!["127.0.0.1:10", "127.0.0.1:9"],
            None,
        );
        mgr.apply_config(&c2).await;
        let fp2 = mgr
            .fingerprint(&(40062, Protocol::Tcp, NodeTransport::Raw))
            .unwrap();
        assert_ne!(fp1, fp2, "reordered targets must be a different config");
    }

    /// A load_balance_strategy change must restart the listener so the new
    /// selector takes effect, even when targets and ws_path are unchanged.
    #[tokio::test]
    async fn strategy_change_restarts_listener() {
        let mut mgr = fresh_mgr();
        let mk = |strategy: LoadBalanceStrategy| NodeConfigResponse {
            listeners: vec![ListenerConfig {
                rule_id: 1,
                port: 40065,
                protocol: Protocol::Tcp,
                node_transport: NodeTransport::Raw,
                ws_path: None,
                targets: vec!["127.0.0.1:9".into(), "127.0.0.1:10".into()],
                load_balance_strategy: strategy,
                upload_limit_bps: None,
                download_limit_bps: None,
                max_connections: None,
            }],
        };
        mgr.apply_config(&mk(LoadBalanceStrategy::First)).await;
        let fp1 = mgr
            .fingerprint(&(40065, Protocol::Tcp, NodeTransport::Raw))
            .unwrap();
        mgr.apply_config(&mk(LoadBalanceStrategy::RoundRobin)).await;
        let fp2 = mgr
            .fingerprint(&(40065, Protocol::Tcp, NodeTransport::Raw))
            .unwrap();
        assert_ne!(fp1, fp2, "strategy change must be a different fingerprint");
        assert_eq!(fp2.load_balance_strategy, LoadBalanceStrategy::RoundRobin);
    }

    /// v1.0.8: flipping a raw listener to a now-DISABLED transport (ws) must
    /// tear the raw listener down and serve nothing — the disabled transport is
    /// skipped, so no new key appears. (Before ws was disabled this tested the
    /// raw→ws restart; ws.rs is kept but no longer served.)
    #[tokio::test]
    async fn transport_change_to_disabled_stops_listener() {
        let mut mgr = fresh_mgr();
        let mk = |transport: NodeTransport| NodeConfigResponse {
            listeners: vec![ListenerConfig {
                rule_id: 1,
                port: 40066,
                protocol: Protocol::Tcp,
                node_transport: transport,
                ws_path: None,
                targets: vec!["127.0.0.1:9".into()],
                load_balance_strategy: LoadBalanceStrategy::First,
                upload_limit_bps: None,
                download_limit_bps: None,
                max_connections: None,
            }],
        };
        mgr.apply_config(&mk(NodeTransport::Raw)).await;
        assert!(mgr
            .fingerprint(&(40066, Protocol::Tcp, NodeTransport::Raw))
            .is_some());
        // Flip to ws (disabled): the old raw listener is stopped, and ws is
        // skipped, so NO listener remains for this port.
        mgr.apply_config(&mk(NodeTransport::Ws)).await;
        assert!(
            mgr.fingerprint(&(40066, Protocol::Tcp, NodeTransport::Raw))
                .is_none(),
            "old raw listener must be stopped after transport flip"
        );
        assert!(
            mgr.fingerprint(&(40066, Protocol::Tcp, NodeTransport::Ws))
                .is_none(),
            "ws is disabled — no ws listener may start"
        );
        assert!(
            mgr.listener_keys().is_empty(),
            "no listener should remain for the port"
        );
    }

    /// Removing a rule from the config stops its listener.
    #[tokio::test]
    async fn removed_rule_stops_listener() {
        let mut mgr = fresh_mgr();
        let c1 = cfg(
            40064,
            Protocol::Tcp,
            NodeTransport::Raw,
            vec!["127.0.0.1:9"],
            None,
        );
        mgr.apply_config(&c1).await;
        assert_eq!(mgr.listener_keys().len(), 1);
        // Empty config = rule removed.
        mgr.apply_config(&NodeConfigResponse { listeners: vec![] })
            .await;
        assert!(mgr.listener_keys().is_empty(), "removed rule must stop");
    }

    /// Changing a field that does NOT affect runtime (here: rule_id on a port
    /// that isn't running yet — simulating an unrelated rule) must not restart
    /// an existing, unchanged listener on a different port.
    #[tokio::test]
    async fn unrelated_change_does_not_restart_other_listeners() {
        let mut mgr = fresh_mgr();
        let c1 = NodeConfigResponse {
            listeners: vec![
                ListenerConfig {
                    rule_id: 1,
                    port: 40070,
                    protocol: Protocol::Tcp,
                    node_transport: NodeTransport::Raw,
                    ws_path: None,
                    targets: vec!["127.0.0.1:9".into()],
                    load_balance_strategy: relay_shared::protocol::LoadBalanceStrategy::First,
                    upload_limit_bps: None,
                    download_limit_bps: None,
                    max_connections: None,
                },
                ListenerConfig {
                    rule_id: 2,
                    port: 40071,
                    protocol: Protocol::Tcp,
                    node_transport: NodeTransport::Raw,
                    ws_path: None,
                    targets: vec!["127.0.0.1:9".into()],
                    load_balance_strategy: relay_shared::protocol::LoadBalanceStrategy::First,
                    upload_limit_bps: None,
                    download_limit_bps: None,
                    max_connections: None,
                },
            ],
        };
        mgr.apply_config(&c1).await;
        let fp70 = mgr
            .fingerprint(&(40070, Protocol::Tcp, NodeTransport::Raw))
            .unwrap();
        // Change rule 2's target only; rule 1 (port 40070) must be untouched.
        let c2 = NodeConfigResponse {
            listeners: vec![
                ListenerConfig {
                    rule_id: 1,
                    port: 40070,
                    protocol: Protocol::Tcp,
                    node_transport: NodeTransport::Raw,
                    ws_path: None,
                    targets: vec!["127.0.0.1:9".into()],
                    load_balance_strategy: relay_shared::protocol::LoadBalanceStrategy::First,
                    upload_limit_bps: None,
                    download_limit_bps: None,
                    max_connections: None,
                },
                ListenerConfig {
                    rule_id: 2,
                    port: 40071,
                    protocol: Protocol::Tcp,
                    node_transport: NodeTransport::Raw,
                    ws_path: None,
                    targets: vec!["127.0.0.1:10".into()], // changed
                    load_balance_strategy: relay_shared::protocol::LoadBalanceStrategy::First,
                    upload_limit_bps: None,
                    download_limit_bps: None,
                    max_connections: None,
                },
            ],
        };
        mgr.apply_config(&c2).await;
        assert_eq!(
            mgr.fingerprint(&(40070, Protocol::Tcp, NodeTransport::Raw))
                .unwrap(),
            fp70,
            "unchanged listener on 40070 must not restart"
        );
    }

    /// A finished JoinHandle is detected and cleared, so a dead listener can be
    /// restarted on the next apply if still desired.
    ///
    /// We simulate a listener task that has already exited: spawn a task that
    /// returns immediately, let the runtime poll it to completion, then inject
    /// its handle into the manager under a known key. The next apply_config
    /// must (a) drop the dead handle and (b) re-start the listener because the
    /// config still wants it.
    #[tokio::test]
    async fn finished_handle_is_recovered() {
        let mut mgr = fresh_mgr();

        // A handle for a task that has finished. Spawn + yield so the runtime
        // completes it; the JoinHandle is NOT awaited (awaiting would consume
        // it), so we can still query is_finished() and insert it.
        let finished_handle: JoinHandle<()> = tokio::spawn(async {});
        // Give the runtime a chance to run the task to completion.
        for _ in 0..10 {
            tokio::task::yield_now().await;
            if finished_handle.is_finished() {
                break;
            }
        }
        assert!(
            finished_handle.is_finished(),
            "test setup: handle must be finished before injection"
        );

        // Inject it as if a listener had been running and then exited.
        let key = (40072, Protocol::Tcp, NodeTransport::Raw);
        mgr.listeners.insert(
            key,
            ManagedListener {
                handle: finished_handle,
                fingerprint: ListenerFingerprint {
                    rule_id: 1,
                    targets: vec!["stale".into()],
                    ws_path: None,
                    load_balance_strategy: LoadBalanceStrategy::First,
                    node_transport: NodeTransport::Raw,
                    upload_limit_bps: None,
                    download_limit_bps: None,
                    max_connections: None,
                },
            },
        );
        assert_eq!(mgr.listener_keys().len(), 1);

        // Apply a config that still wants this port. apply_config must detect
        // the dead handle, remove it, and start a fresh listener.
        let c = cfg(
            40072,
            Protocol::Tcp,
            NodeTransport::Raw,
            vec!["127.0.0.1:9"],
            None,
        );
        mgr.apply_config(&c).await;

        // The key is still registered (restarted), but with the NEW fingerprint
        // — proving the stale entry was cleared and replaced, not reused.
        assert!(
            mgr.listener_keys().contains(&key),
            "dead listener must be restarted"
        );
        assert_eq!(
            mgr.fingerprint(&key).unwrap().targets,
            vec!["127.0.0.1:9".to_string()],
            "restarted listener must carry the new config, not the stale one"
        );
    }

    /// v0.4.9: listener_info_for_rule_tcp must select the TCP listener for a
    /// tcp_udp rule (which runs Tcp + Udp under the same rule_id). HashMap
    /// iteration order is nondeterministic, so the generic
    /// listener_info_for_rule could return either; this asserts the TCP one is
    /// picked deterministically. Uses direct injection (no port binding) so the
    /// test is fast and not order-dependent.
    #[tokio::test]
    async fn listener_info_for_rule_tcp_picks_tcp_for_tcp_udp_rule() {
        let mut mgr = fresh_mgr();
        // A tcp_udp rule → two listeners: Tcp + Udp, same rule_id, same port,
        // different protocol. Each gets its own live (pending) JoinHandle —
        // JoinHandle isn't Clone, so we spawn one per listener.
        let mk_live_handle = || {
            tokio::spawn(async {
                // never completes during the test → is_finished() stays false
                std::future::pending::<()>().await;
            })
        };
        mgr.listeners.insert(
            (40080, Protocol::Tcp, NodeTransport::Raw),
            ManagedListener {
                handle: mk_live_handle(),
                fingerprint: ListenerFingerprint {
                    rule_id: 7,
                    targets: vec!["tcp-target".into()],
                    ws_path: None,
                    load_balance_strategy: LoadBalanceStrategy::First,
                    node_transport: NodeTransport::Raw,
                    upload_limit_bps: None,
                    download_limit_bps: None,
                    max_connections: None,
                },
            },
        );
        mgr.listeners.insert(
            (40080, Protocol::Udp, NodeTransport::Raw),
            ManagedListener {
                handle: mk_live_handle(),
                fingerprint: ListenerFingerprint {
                    rule_id: 7,
                    targets: vec!["udp-target".into()],
                    ws_path: None,
                    load_balance_strategy: LoadBalanceStrategy::First,
                    node_transport: NodeTransport::Raw,
                    upload_limit_bps: None,
                    download_limit_bps: None,
                    max_connections: None,
                },
            },
        );
        // Both listeners are registered under rule 7.
        assert_eq!(mgr.listener_keys().len(), 2);

        // The TCP selector returns the TCP listener deterministically,
        // regardless of HashMap iteration order.
        let info = mgr
            .listener_info_for_rule_tcp(7)
            .expect("rule 7 has a TCP listener");
        assert_eq!(info.protocol, "tcp");
        assert_eq!(info.port, 40080);
        assert_eq!(info.targets, vec!["tcp-target".to_string()]);
        assert!(info.running, "a pending task is alive → running");
    }

    /// v0.4.9: a pure-udp rule has no TCP listener → listener_info_for_rule_tcp
    /// returns None. The panel rejects pure-UDP rules before dispatch, so this
    /// is defensive, but the contract must hold. An unknown rule_id is also None.
    #[tokio::test]
    async fn listener_info_for_rule_tcp_returns_none_for_udp_only_rule() {
        let mut mgr = fresh_mgr();
        let live_handle: JoinHandle<()> = tokio::spawn(async {
            std::future::pending::<()>().await;
        });
        mgr.listeners.insert(
            (40090, Protocol::Udp, NodeTransport::Raw),
            ManagedListener {
                handle: live_handle,
                fingerprint: ListenerFingerprint {
                    rule_id: 9,
                    targets: vec!["udp-target".into()],
                    ws_path: None,
                    load_balance_strategy: LoadBalanceStrategy::First,
                    node_transport: NodeTransport::Raw,
                    upload_limit_bps: None,
                    download_limit_bps: None,
                    max_connections: None,
                },
            },
        );
        assert!(mgr.listener_info_for_rule_tcp(9).is_none());
        // An unknown rule_id also returns None.
        assert!(mgr.listener_info_for_rule_tcp(999).is_none());
    }

    // ── v1.0.3 PR1: traffic counter poison-pill pruning ──

    /// When a rule is deleted from the config, the counter entry for its
    /// rule_id must be pruned so orphaned bytes don't poison future batches.
    #[tokio::test]
    async fn deleted_rule_prunes_traffic_counter() {
        let counter = Arc::new(TrafficCounter::new());
        let connections = Arc::new(ConnectionTracker::new());
        let mut mgr = ForwarderManager::new(counter.clone(), connections.clone());

        // v1.0.8: use a port not shared with any other test in this file.
        // `cargo test` runs #[tokio::test] fns in parallel by default, and this
        // test previously hardcoded port 40001 — the SAME port used by
        // `raw_tcp_and_udp_are_scheduled`, `tcp_udp_to_tcp_does_not_prune_
        // surviving_rule_counter`, and `dead_listener_prunes_counter_when_
        // rule_removed`. When two of those tests overlapped in time, one lost
        // the OS-level bind race on 40001 for BOTH the v4 and v6 listener, hit
        // the "no listener bound on port" branch, and never inserted a
        // `self.listeners` entry at all. The `mgr.listeners.get(&key)` below
        // then found nothing, silently skipping the abort/prune path entirely
        // — so the manually-primed counter entry for rule 1 was never touched
        // and the final assertion failed. This was the actual flake (not a
        // timing issue with is_finished(), despite what an earlier fix here
        // assumed) — confirmed by port collision, not the spin loop below.
        // Fix: give each of the 4 tests its own port (40001/40003/40004/40005).
        //
        // Apply a config with one rule.
        mgr.apply_config(&one_rule(40003, Protocol::Tcp, NodeTransport::Raw))
            .await;
        // Simulate traffic: accumulate bytes for rule 1.
        counter.add(1, 100, 50).await;
        assert!(counter.has_rule(1).await);

        // Abort the listener so it finishes, then apply empty config.
        // Without this, the listener is still running when apply_config
        // checks is_finished() and won't be detected as dead.
        //
        // abort() only REQUESTS cancellation; the task isn't actually finished
        // until the runtime polls it once more. Spin on is_finished(), yielding
        // so the runtime drives the cancelled task to completion, before
        // applying the empty config. (This spin is still correct defensive
        // practice even though it wasn't the source of the observed flake.)
        let key = (40003, Protocol::Tcp, NodeTransport::Raw);
        if let Some(m) = mgr.listeners.get(&key) {
            m.handle.abort();
            while !m.handle.is_finished() {
                tokio::task::yield_now().await;
            }
        }
        mgr.apply_config(&NodeConfigResponse {
            listeners: Vec::new(),
        })
        .await;

        // Counter must be pruned.
        assert!(
            !counter.has_rule(1).await,
            "orphan rule_id must be pruned after rule deletion"
        );
    }

    /// When a tcp_udp rule is changed to tcp-only (one listener removed), the
    /// remaining listener's counter must NOT be pruned — only the deleted
    /// listener is gone, but the rule itself still exists.
    ///
    /// v1.0.8: uses port 40004, dedicated to this test — see the port-collision
    /// note in `deleted_rule_prunes_traffic_counter` above for why every test
    /// in this file needs its own port.
    #[tokio::test]
    async fn tcp_udp_to_tcp_does_not_prune_surviving_rule_counter() {
        let counter = Arc::new(TrafficCounter::new());
        let connections = Arc::new(ConnectionTracker::new());
        let mut mgr = ForwarderManager::new(counter.clone(), connections.clone());

        // tcp_udp rule: two listeners share rule_id 1.
        let tcp_udp_cfg = NodeConfigResponse {
            listeners: vec![
                ListenerConfig {
                    rule_id: 1,
                    port: 40004,
                    protocol: Protocol::Tcp,
                    node_transport: NodeTransport::Raw,
                    ws_path: None,
                    targets: vec!["127.0.0.1:1".into()],
                    load_balance_strategy: LoadBalanceStrategy::First,
                    upload_limit_bps: None,
                    download_limit_bps: None,
                    max_connections: None,
                },
                ListenerConfig {
                    rule_id: 1,
                    port: 40004,
                    protocol: Protocol::Udp,
                    node_transport: NodeTransport::Raw,
                    ws_path: None,
                    targets: vec!["127.0.0.1:1".into()],
                    load_balance_strategy: LoadBalanceStrategy::First,
                    upload_limit_bps: None,
                    download_limit_bps: None,
                    max_connections: None,
                },
            ],
        };
        mgr.apply_config(&tcp_udp_cfg).await;
        counter.add(1, 200, 100).await;
        assert!(counter.has_rule(1).await);

        // Change to tcp-only: remove the UDP listener for rule 1.
        let tcp_cfg = NodeConfigResponse {
            listeners: vec![ListenerConfig {
                rule_id: 1,
                port: 40004,
                protocol: Protocol::Tcp,
                node_transport: NodeTransport::Raw,
                ws_path: None,
                targets: vec!["127.0.0.1:2".into()],
                load_balance_strategy: LoadBalanceStrategy::First,
                upload_limit_bps: None,
                download_limit_bps: None,
                max_connections: None,
            }],
        };
        mgr.apply_config(&tcp_cfg).await;

        // Rule 1 still exists (TCP listener survived) — counter must NOT be pruned.
        assert!(
            counter.has_rule(1).await,
            "surviving rule's counter must not be pruned when only the UDP listener is removed"
        );
    }

    /// A dead listener whose rule was also removed from the config must have
    /// its counter pruned, same as a normally-stopped listener.
    ///
    /// v1.0.8: uses port 40005, dedicated to this test — see the port-collision
    /// note in `deleted_rule_prunes_traffic_counter` above.
    #[tokio::test]
    async fn dead_listener_prunes_counter_when_rule_removed() {
        let counter = Arc::new(TrafficCounter::new());
        let connections = Arc::new(ConnectionTracker::new());
        let mut mgr = ForwarderManager::new(counter.clone(), connections.clone());

        // Apply config with rule 1.
        mgr.apply_config(&one_rule(40005, Protocol::Tcp, NodeTransport::Raw))
            .await;
        counter.add(1, 50, 25).await;

        // Simulate a dead listener: abort its JoinHandle so is_finished() is true.
        let key = (40005, Protocol::Tcp, NodeTransport::Raw);
        if let Some(m) = mgr.listeners.get(&key) {
            m.handle.abort();
            // Briefly wait for the abort to propagate.
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }

        // Apply empty config — step 1 finds the dead listener and removes it.
        mgr.apply_config(&NodeConfigResponse {
            listeners: Vec::new(),
        })
        .await;

        assert!(
            !counter.has_rule(1).await,
            "dead listener for a removed rule must prune its counter entry"
        );
    }
}
