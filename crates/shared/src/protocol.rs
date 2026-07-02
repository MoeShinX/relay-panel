use crate::models::Plan;
use serde::{Deserialize, Serialize};

/// The config-protocol version this panel/node build speaks.
///
/// v0.4.0 introduces a deliberate compatibility gate (see ROADMAP-v0.4.md
/// "Compatibility strategy"): the node sends this in an
/// `X-Config-Protocol-Version` header on `get_config` and WS upgrade; the panel
/// refuses to build/send config on mismatch (the node keeps its cached config).
///
/// Bump this ONLY when the wire format of `ListenerConfig` /
/// `NodeConfigResponse` / `StatusReport` breaks in a way old nodes can't
/// deserialize. Within the same value, panel and node releases are
/// interoperable even if the product version differs.
///
/// v1 = the v0.4.0 split: `ListenerConfig.entry_transport` renamed to
/// `node_transport` (type `NodeTransport`), new `protocol`/`route_mode` fields,
/// `PublicTransport`/`NodeTransport` enums replace `EntryTransport`.
/// v2 = the v0.4.1 TlsSimple semantics change: a v0.4.0 node receiving a
/// `TlsSimple` listener silently skips it (no rustls integration), while a
/// v0.4.1 node actually runs a TLS listener. The gate forces panel/node to
/// upgrade in lockstep so a v0.4.0 node can't silently fail to forward a
/// tls_simple rule. (WSS variant removal is NOT the reason — Wss lives in the
/// admin API enum, not in ListenerConfig.)
/// v3 = v0.4.6 multi-target load balancing: `ListenerConfig` gains
/// `load_balance_strategy`. Old nodes ignore the strategy and would silently
/// run their implicit ordered-failover behavior, so the gate forces panel/node
/// to upgrade together when a rule relies on round-robin / failover semantics.
/// v4 = v0.4.7: removed the dead `speed_limit` / `ip_limit` / `route_mode`
/// wire fields from ListenerConfig. A v0.4.6 node still expects those fields,
/// so deserialization would fail or misread — the gate forces a coordinated
/// upgrade. Also adds node_transport to the listener fingerprint.
pub const CONFIG_PROTOCOL_VERSION: u32 = 4;

// === Auth ===
#[derive(Debug, Serialize, Deserialize)]
pub struct LoginRequest {
    pub username: String,
    pub password: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct LoginResponse {
    pub token: String,
    pub admin: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RegisterRequest {
    pub username: String,
    pub password: String,
    /// v0.4.21 PR2: optional plan selection during registration.
    /// When omitted, the server uses the default registration plan.
    #[serde(default)]
    pub plan_id: Option<i64>,
}

/// v0.4.10 PR3: public registration-status response (GET /auth/registration-status).
/// v0.4.21 PR2: now includes default_plan_id and the list of allowed plans so the
/// registration page can render a plan selector.
#[derive(Debug, Serialize, Deserialize)]
pub struct RegistrationStatus {
    pub enabled: bool,
    pub default_plan_id: i64,
    pub plans: Vec<Plan>,
    /// v0.4.22: whether the default admin account still has must_change_password
    /// set. The login page uses this to decide whether to show the security
    /// reminder banner. Only meaningful when the DB has been seeded.
    pub default_password_change_required: bool,
}

/// v0.4.10 PR3: admin update body for PUT /admin/settings/registration.
/// v0.4.21 PR2: added allowed_plan_ids for multi-plan registration support.
#[derive(Debug, Serialize, Deserialize)]
pub struct RegistrationSettingsRequest {
    pub enabled: bool,
    pub default_plan_id: i64,
    pub allowed_plan_ids: Vec<i64>,
}

// === Admin API — Users ===
/// Update an existing user's admin-editable fields. All fields optional — only
/// provided fields are updated. Deliberately does NOT allow changing:
///   - password (separate endpoint with current-password verification)
///   - admin role (no privilege escalation via this endpoint)
///   - user id / username (immutable identity)
///
/// v0.3.4: single-admin MVP — no owner isolation, no self-service for non-admins.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct UpdateUserRequest {
    /// Account balance (stored as TEXT in DB).
    /// v0.3.5: validated strictly — non-negative decimal, ≤ 2 fraction digits,
    /// ≤ 9999999999.99. The handler canonicalises before storing so every row
    /// looks the same regardless of what the caller typed.
    #[serde(default)]
    pub balance: Option<String>,
    /// Max forwarding rules the user can create (advisory in single-admin mode).
    /// Clamped to 0..=100000 to prevent overflow / absurd values.
    #[serde(default)]
    pub max_rules: Option<i32>,
    /// Traffic cap in bytes; 0 = unlimited.
    #[serde(default)]
    pub traffic_limit: Option<i64>,
    /// Ban / unban the user. true = banned (all their rules stop forwarding).
    /// Cannot ban admin users (the handler rejects it).
    #[serde(default)]
    pub banned: Option<bool>,
    /// v1.0.8: suspend / unsuspend the user. true = forwarding gated off via
    /// list_active_for_config (login still allowed; no token_version bump).
    /// Cannot suspend admin users (the handler rejects it). Buying a plan does
    /// NOT auto-clear suspension.
    #[serde(default)]
    pub suspended: Option<bool>,
    /// v1.0.7: set the user's device-group authorization directly. `Some(list)`
    /// replaces the user's explicit device-group assignments. Ignored when
    /// `all_device_groups` is Some(true).
    #[serde(default)]
    pub device_group_ids: Option<Vec<i64>>,
    /// v1.0.7: when Some, sets the per-user "all device groups" flag.
    #[serde(default)]
    pub all_device_groups: Option<bool>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct NodeConfigRequest {
    pub token: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct NodeConfigResponse {
    pub listeners: Vec<ListenerConfig>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ListenerConfig {
    /// The forward_rules.id this listener corresponds to. Traffic is
    /// attributed to this rule, NOT to the listen port (which may collide
    /// across inbound groups).
    pub rule_id: i64,
    pub port: u16,
    pub protocol: Protocol,
    /// v0.4.7: the route_mode wire field was removed (the node never read it —
    /// direct/group are resolved identically by the panel). CONFIG_PROTOCOL_VERSION
    /// bumped to 4 so a v0.4.6 node (which expects the field) is gated.
    /// v0.4.0: the transport the NODE actually listens on. This is DERIVED by
    /// the panel from the user-facing `public_transport` (see `PublicTransport`)
    /// and sent explicitly — the node never guesses and never receives `Wss`
    /// (reverse-proxy-terminated) or `TlsSimple` until v0.4.1 implements it.
    /// Replaces the v0.3.x `entry_transport` field (breaking wire change, hence
    /// the `CONFIG_PROTOCOL_VERSION` gate).
    #[serde(default)]
    pub node_transport: NodeTransport,
    /// WS path the listener should accept on (e.g. "/relay").
    /// Only meaningful when node_transport=ws; the node ignores it otherwise.
    /// None → node uses its built-in default ("/relay").
    #[serde(default)]
    pub ws_path: Option<String>,
    pub targets: Vec<String>,
    /// v0.4.6: how the node picks among `targets` for each new connection /
    /// UDP session. Defaults to `First` so old configs and v0.4.5 rows behave
    /// exactly like the legacy ordered single-target path.
    #[serde(default)]
    pub load_balance_strategy: LoadBalanceStrategy,
    /// v0.4.6: per-rule upload cap in BYTES/sec (0 / None = unlimited). Shared
    /// across ALL connections and both TCP/UDP listeners of the rule (a
    /// `tcp_udp` rule does NOT get double the budget). The panel converts the
    /// user-facing Mbps value to bytes/sec so the node doesn't reinterpret it.
    #[serde(default)]
    pub upload_limit_bps: Option<u64>,
    /// v0.4.6: per-rule download cap in BYTES/sec (0 / None = unlimited).
    #[serde(default)]
    pub download_limit_bps: Option<u64>,
    // v0.4.7: the placeholder `speed_limit` / `ip_limit` wire fields were
    // removed. They were always None and no node ever read them. The DB columns
    // on users/plans are kept (deprecated) to avoid a pointless migration, but
    // the ListenerConfig wire struct no longer carries them. CONFIG_PROTOCOL_VERSION
    // bumps 3→4 so a v0.4.6 node (which still expects these fields) is gated.
}

/// v0.4.6: convert a rule-level Mbps cap to bytes/sec for the node.
/// 1 Mbps (decimal) = 1_000_000 bit/s = 125_000 byte/s.
/// Returns None (unlimited) for 0 or negative values.
pub fn mbps_to_bps(mbps: i32) -> Option<u64> {
    if mbps <= 0 {
        return None;
    }
    // 125_000 bytes/sec per Mbps. Cap at a sane u64 ceiling; i32 max Mbps is
    // ~2.1e9 Mbps = ~2.7e14 byte/s, well within u64.
    Some(mbps as u64 * 125_000)
}

/// Expand a rule's `protocol` string into the concrete L4 protocols its node
/// listeners must run. "tcp_udp" expands to BOTH Tcp and Udp (two listeners);
/// everything else is a single entry. Pure + shared so the HTTP poll path
/// (node.rs::get_config) and the WS push path (ws.rs::build_config_snapshot)
/// can never disagree on expansion — the v0.2.x drift was exactly here.
pub fn expand_protocols(protocol: &str) -> Vec<Protocol> {
    match protocol {
        "udp" => vec![Protocol::Udp],
        "tcp_udp" => vec![Protocol::Tcp, Protocol::Udp],
        _ => vec![Protocol::Tcp], // default: tcp
    }
}

/// Build the ListenerConfig entries for ONE rule, given its already-resolved
/// target address list. This is the SINGLE place that turns a ForwardRule into
/// listener configs — both get_config (HTTP poll) and build_config_snapshot
/// (WS push) MUST call it, so transport derivation / ws_path passthrough /
/// protocol expansion stay identical. (Regression: v0.2.x had this logic
/// duplicated and ws.rs hardcoded Raw, which broke WS rules on first push.)
///
/// `targets` is resolved by the caller because it needs a DB lookup (outbound
/// group's connect_host) — that async step can't live in this pure function.
pub fn build_listeners_for_rule(
    rule: &crate::models::ForwardRule,
    targets: Vec<String>,
) -> Vec<ListenerConfig> {
    // v0.4.0: the node transport is read DIRECTLY from the rule's stored
    // `node_transport` column. The panel derives this from `public_transport`
    // at rule create/update time (identity for raw/ws, tls_simple for tls_simple), so
    // here we just pass it through. The old v0.3.x `derive_node_transport`
    // derivation is gone — the derivation happens once, at write time, not at
    // every config build.
    let transport = NodeTransport::from_db_str(&rule.node_transport);
    expand_protocols(&rule.protocol)
        .into_iter()
        .map(|proto| ListenerConfig {
            rule_id: rule.id,
            port: rule.listen_port as u16,
            protocol: proto,
            node_transport: transport,
            // Per-rule WS path override; None → node uses its built-in "/relay".
            ws_path: rule.ws_path.clone(),
            targets: targets.clone(),
            load_balance_strategy: LoadBalanceStrategy::from_db_str(&rule.load_balance_strategy),
            // v0.4.6: convert the user-facing Mbps caps to bytes/sec here so the
            // node never reinterprets the unit. 1 Mbps (decimal) = 1e6 bit/s =
            // 125_000 byte/s. 0 / negative → unlimited (None). The same pair is
            // applied to BOTH expanded listeners of a tcp_udp rule, and the node
            // shares one token bucket per (rule_id, direction) so the budget is
            // NOT doubled.
            upload_limit_bps: mbps_to_bps(rule.upload_limit_mbps),
            download_limit_bps: mbps_to_bps(rule.download_limit_mbps),
        })
        .collect()
}

/// Note: in NodeConfigResponse, a TcpUdp rule is expanded into TWO separate
/// ListenerConfig entries (one Tcp, one Udp) by the panel's get_config.
/// The node manager never receives Protocol::TcpUdp directly.
#[derive(Debug, Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Hash)]
#[serde(rename_all = "lowercase")]
pub enum Protocol {
    Tcp,
    Udp,
    #[serde(rename = "tcp_udp")]
    TcpUdp,
}

/// Forwarding topology (v0.4.0). Orthogonal to protocol and transport.
/// - `Direct` = inbound listener connects to target_addr:target_port directly.
/// - `Group` = forward via the outbound device group's connect_host.
///
/// v0.4.7: `Chain` was removed (it was reserved/never implemented; the API
/// rejected it and the node never read the field). Historical DB rows with
/// `route_mode='chain'` are paused by the v0.4.7 migration rather than
/// silently reinterpreted.
#[derive(Debug, Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[serde(rename_all = "lowercase")]
pub enum RouteMode {
    #[default]
    Direct,
    Group,
}

impl RouteMode {
    /// Parse the stored DB string. Unknown/empty → Direct (safe default).
    /// Note: a stored `"chain"` value (left over from pre-v0.4.7) also maps to
    /// Direct here — the migration pauses such rules, so this only matters for
    /// rows the migration didn't touch.
    pub fn from_db_str(s: &str) -> Self {
        match s {
            "group" => RouteMode::Group,
            _ => RouteMode::Direct,
        }
    }
    /// Stable machine string for DB storage.
    pub fn to_db_str(self) -> &'static str {
        match self {
            RouteMode::Direct => "direct",
            RouteMode::Group => "group",
        }
    }
}

/// Multi-target load-balancing strategy (v0.4.6). Decides how the node picks
/// among a rule's enabled targets for each new connection / UDP session.
/// - `First` = always use the first target; if it fails the connection fails
///   (no automatic fallback). Later targets are standby config only.
/// - `RoundRobin` = each new connection/session advances to the next target
///   (A→B→C→A); a failed pick may try the others in ring order.
/// - `Failover` = strict priority order A→B→C; new connections always start
///   from A and fall through on failure. UDP only detects local errors.
#[derive(Debug, Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[serde(rename_all = "snake_case")]
pub enum LoadBalanceStrategy {
    #[default]
    First,
    RoundRobin,
    Failover,
}

impl LoadBalanceStrategy {
    /// Parse the stored DB string. Unknown/empty → First (safe default that
    /// matches the legacy single-target behavior).
    pub fn from_db_str(s: &str) -> Self {
        match s {
            "round_robin" => LoadBalanceStrategy::RoundRobin,
            "failover" => LoadBalanceStrategy::Failover,
            _ => LoadBalanceStrategy::First,
        }
    }
    /// Stable machine string for DB storage.
    pub fn to_db_str(self) -> &'static str {
        match self {
            LoadBalanceStrategy::First => "first",
            LoadBalanceStrategy::RoundRobin => "round_robin",
            LoadBalanceStrategy::Failover => "failover",
        }
    }
}

/// The user-facing ingress protocol (v0.4.0). What the user picks in the UI —
/// how clients reach the listener from the outside. DISTINCT from
/// `NodeTransport` (what the node actually listens on) and from `Protocol`
/// (tcp/udp/tcp_udp = the forwarded payload).
///
/// - `Raw` = plain TCP/UDP
/// - `Ws` = plaintext WebSocket
/// - `TlsSimple` = raw TCP over TLS, terminated at relay-node (v0.4.1).
///
/// v0.4.1: `Wss` (WebSocket Secure via reverse proxy) is REMOVED. Any old DB
/// row with `public_transport='wss'` is converted to `'ws'` by Migration 18
/// before this code runs; `from_db_str("wss")` falls back to `Raw` as a
/// safety net (should never be reached post-migration).
///
/// Stored in `forward_rules.public_transport`. The panel derives
/// `node_transport` from this at write time.
#[derive(Debug, Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[serde(rename_all = "snake_case")]
pub enum PublicTransport {
    #[default]
    Raw,
    Ws,
    TlsSimple,
}

impl PublicTransport {
    /// Parse the stored DB string into the enum. Accepts legacy v0.3.x "tls"
    /// (maps to tls_simple). Unknown/empty/"wss" → Raw (wss rows are migrated
    /// by Migration 18; this fallback is a safety net only).
    pub fn from_db_str(s: &str) -> Self {
        match s {
            "ws" => PublicTransport::Ws,
            // Legacy v0.3.x "tls" → tls_simple.
            "tls" | "tls_simple" => PublicTransport::TlsSimple,
            _ => PublicTransport::Raw,
        }
    }
    /// Stable machine string for DB storage.
    pub fn to_db_str(self) -> &'static str {
        match self {
            PublicTransport::Raw => "raw",
            PublicTransport::Ws => "ws",
            PublicTransport::TlsSimple => "tls_simple",
        }
    }
    /// Derive the transport the NODE actually listens on.
    /// - TlsSimple → TlsSimple (node terminates TLS itself — v0.4.1).
    /// - Raw/Ws → identity.
    pub fn derive_node_transport(self) -> NodeTransport {
        match self {
            PublicTransport::Raw => NodeTransport::Raw,
            PublicTransport::Ws => NodeTransport::Ws,
            PublicTransport::TlsSimple => NodeTransport::TlsSimple,
        }
    }
}

/// The transport the NODE actually listens on (v0.4.0). Sent explicitly in
/// `ListenerConfig.node_transport` — the node never guesses. Has NO `Wss`
/// variant (WSS is reverse-proxy-terminated; the node runs plain Ws).
#[derive(Debug, Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[serde(rename_all = "snake_case")]
pub enum NodeTransport {
    #[default]
    Raw,
    Ws,
    /// v0.4.1: node terminates TLS directly (tokio-rustls). In v0.4.0 the node
    /// logs and skips a TlsSimple listener (no rustls integration yet).
    TlsSimple,
}

impl NodeTransport {
    /// Parse the stored DB string. Accepts legacy "tls" → TlsSimple.
    /// Unknown/empty → Raw.
    pub fn from_db_str(s: &str) -> Self {
        match s {
            "ws" => NodeTransport::Ws,
            "tls" | "tls_simple" => NodeTransport::TlsSimple,
            _ => NodeTransport::Raw,
        }
    }
    /// Stable machine string for DB storage.
    pub fn to_db_str(self) -> &'static str {
        match self {
            NodeTransport::Raw => "raw",
            NodeTransport::Ws => "ws",
            NodeTransport::TlsSimple => "tls_simple",
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct TrafficReport {
    pub reports: Vec<TrafficEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrafficEntry {
    pub rule_id: i64,
    pub upload: u64,
    pub download: u64,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct StatusReport {
    pub cpu_usage: f32,
    pub mem_usage: f32,
    pub active_connections: u32,
    pub uptime_secs: u64,
    // --- Extended metrics (all optional; older nodes that don't report them
    //     still deserialize fine, and the panel renders "-" for missing). ---
    /// Node's public egress IP (for the node-status page). Detected by the
    /// node via a lightweight external check; null if unknown.
    ///
    /// v0.4.15: this field is kept for backward compat (it carries the IPv4).
    /// New nodes ALSO report `public_ipv4` / `public_ipv6` separately. The panel
    /// prefers the new fields and falls back to this one when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub public_ip: Option<String>,
    /// v0.4.15: public egress IPv4 (detected independently from IPv6). Additive
    /// optional field — does NOT bump CONFIG_PROTOCOL_VERSION.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub public_ipv4: Option<String>,
    /// v0.4.15: public egress IPv6 (detected independently from IPv4). None
    /// when the node has no IPv6 connectivity; the panel shows only IPv4 then.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub public_ipv6: Option<String>,
    /// Primary disk (root partition `/`): total / used bytes + usage %.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disk_total: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disk_used: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disk_usage_percent: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disk_mount: Option<String>,
    /// Real-time network rate (bytes/sec), computed from the delta between
    /// the last two samples — NOT cumulative counters.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub upload_bps: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub download_bps: Option<u64>,
    /// Cumulative bytes transferred over all non-loopback NICs since boot
    /// (system-wide, not just RelayPanel's forwarding).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub boot_upload_bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub boot_download_bytes: Option<u64>,
    /// v0.4.6: the interface machine traffic is counted on (e.g. "eth0"), so
    /// the panel can show "统计网卡: eth0". None for older nodes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub network_interface: Option<String>,
    /// v0.3.0: stable per-node identity. Generated once by the node on first
    /// start and persisted to a `node-id` file, so it survives restarts. The
    /// panel uses it to key node status (node_status:{group_id}:{node_id}) so
    /// multiple nodes sharing one group token no longer overwrite each other's
    /// status. Older nodes that don't send this deserialize as None and the
    /// panel falls back to the legacy per-group key (no regression).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub node_id: Option<String>,
    /// v0.3.2: relay-node PROCESS uptime (since this binary started). Reset to
    /// 0 on every restart/upgrade. Older nodes don't send this; the panel
    /// falls back to uptime_secs (which on old nodes IS the process uptime).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub process_uptime_secs: Option<u64>,
    /// v0.3.4: the relay-node binary version (env!("CARGO_PKG_VERSION")).
    /// The panel shows it + flags stale nodes for upgrade. Older nodes don't
    /// send this; the panel renders "-" for them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub node_version: Option<String>,
    /// v0.4.0: the config-protocol version the node speaks. Mirrors the value
    /// sent in the `X-Config-Protocol-Version` header on get_config / WS
    /// upgrade. Stored here purely for the frontend status display (the actual
    /// gate is request-scoped via the header). Older nodes don't send this; the
    /// panel treats a missing value as "incompatible — upgrade".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config_protocol_version: Option<u32>,
    /// Listeners that failed to bind on the node during the last config apply
    /// (e.g. port already in use, permission denied). Surfaced on the panel so
    /// an operator can see WHY a rule isn't forwarding, not just that it isn't.
    /// Older nodes don't send this; the panel renders "ok" for them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub listener_errors: Option<Vec<ListenerError>>,
    /// v1.0.10: how this node is run — "systemd" | "docker" | "manual". Drives
    /// the panel's one-click upgrade affordance: only systemd nodes can safely
    /// self-replace their binary and be restarted; docker nodes must update the
    /// image; manual runs have no supervisor to restart them. Older nodes don't
    /// send this; the panel treats a missing value as "unknown" (no self-upgrade).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub install_method: Option<String>,
}

/// One listener bind failure reported by a node. Carries enough context for the
/// panel to point at the offending rule/port without a round-trip.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListenerError {
    pub port: u16,
    /// "tcp" / "udp" / "ws" — matches the ListenerConfig.protocol vocabulary.
    pub protocol: String,
    /// Short human-readable reason (e.g. "Address already in use (os error 98)").
    pub error: String,
}

// === v0.4.8: Rule diagnosis ===  === v0.4.9: secure-diagnose challenge + TCP-only ===
//
// Flow: panel sends `DiagnoseRuleMessage` to a node over WS (group-scoped);
// the node runs side-channel TCP reachability probes (NOT through the
// forwarder, so they don't count against rule traffic or rate limits) and
// POSTs a `DiagnoseResult` back over the existing HTTP node→panel channel.
// The panel correlates by request_id + node_id.
//
// v0.4.9 — diagnosis is now TCP-ONLY:
//   - Only TCP reachability is probed. The old UDP "route-only" check
//     (`TargetProbeOutcome::RouteOnly`) is gone — UDP can't be verified
//     cheaply, and a "resolved but not probed" result misled operators.
//   - A pure-UDP rule is rejected by the panel (`POST .../diagnose` → 400
//     "UDP 暂不支持诊断") before any probe is sent. The node never receives
//     such a rule.
//   - A tcp_udp rule is probed on its TCP listener ONLY. The node explicitly
//     selects the TCP listener for the rule (it does NOT rely on HashMap
//     iteration order, which would be nondeterministic for a tcp_udp rule
//     that runs two listeners).
//
// Versioning (v0.4.9 hardened the protocol):
//   - The diagnose FEATURE first shipped in v0.4.8, but v0.4.8 nodes do NOT
//     speak the secure challenge protocol: they ignore the `challenge` field
//     on the way in and omit it on the way back. To keep them from silently
//     bypassing the challenge check, the panel only dispatches to nodes that
//     support the SECURE protocol, i.e. >= 0.4.9 (see node_supports_secure_diagnose).
//     A v0.4.8 node is surfaced as "诊断协议过旧，请升级" — it is NOT treated
//     as a "no diagnose at all" node, because the feature does exist on it.
//   - pre-0.4.8 nodes never understood diagnose_rule at all and just ignore
//     the WS message; they also fall under the same unsupported branch.
//   - CONFIG_PROTOCOL_VERSION is intentionally NOT bumped: diagnose is an
//     on-demand probe carried on the WS control channel, not part of the
//     ListenerConfig wire format. Normal forwarding is unaffected for any
//     version. The `challenge` field uses #[serde(default)] both ways so old
//     builds still deserialize each other's messages.
//
// Challenge: the panel generates a random per-run challenge; the node MUST
// echo it back verbatim in DiagnoseResult.challenge. The panel rejects any
// result whose challenge is empty or doesn't byte-for-byte match — this
// defeats a forged result that guesses request_id + node_id without having
// received the probe.

/// Panel → node, over the WS control channel. Asks the node to probe a rule's
/// targets from the node's own vantage point.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiagnoseRuleMessage {
    #[serde(rename = "type")]
    pub msg_type: String,
    pub request_id: String,
    pub rule_id: i64,
    /// v0.4.9: opaque per-run challenge the node MUST echo back in its
    /// DiagnoseResult. `#[serde(default)]` so a v0.4.8 node still deserializes
    /// the message (it just ignores the field); the panel never sends a probe
    /// to a <0.4.9 node anyway, so this is belt-and-suspenders.
    #[serde(default)]
    pub challenge: String,
}

/// v1.0.10: Panel → node, over the WS control channel (delivered via
/// `send_node`, so only the targeted node receives it). Asks the node to
/// self-upgrade to a SPECIFIC version. The node downloads that exact release
/// tag's binary for its arch, verifies its sha256, swaps its own binary, and
/// restarts. The message carries the version string but NO URL/binary — the
/// node only ever pulls from the hardcoded official GitHub release for that
/// version (and validates the version is a plain semver), so a compromised
/// panel can at most force an upgrade to an official build, never run arbitrary
/// code. Pinning the version (rather than "latest") keeps the node at the same
/// version as the panel, avoiding a node jumping ahead of an older panel.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpgradeNodeMessage {
    #[serde(rename = "type")]
    pub msg_type: String, // "upgrade_node"
    /// The intended target's node_id (informational — send_node already routed
    /// this only to the matching connection; the node may re-check as defence).
    pub node_id: String,
    /// Target version WITHOUT a leading "v" (e.g. "1.0.10"). The node validates
    /// this is a plain semver before building the download URL.
    pub version: String,
}

impl DiagnoseRuleMessage {
    pub fn new(request_id: String, rule_id: i64, challenge: String) -> Self {
        Self {
            msg_type: "diagnose_rule".into(),
            request_id,
            rule_id,
            challenge,
        }
    }
}

/// Outcome of probing ONE target from the node (TCP-only since v0.4.9).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TargetProbeOutcome {
    /// TCP connect succeeded within the deadline. `elapsed_ms` is the connect time.
    Reachable { elapsed_ms: u64 },
    /// TCP connect failed (refused/reset/etc). `error` is a short reason.
    Failed { error: String },
    /// Connect did not complete within the deadline.
    Timeout,
}

/// One target's diagnosis entry in the result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiagnoseTargetResult {
    /// The target address the node actually probed (host:port).
    pub address: String,
    pub outcome: TargetProbeOutcome,
}

/// Node → panel, POSTed to /api/v1/node/diagnose_result. Authenticated by the
/// node's NODE_TOKEN (same as report_status); the panel additionally verifies
/// the rule belongs to the token's inbound group AND that the echoed challenge
/// matches the one it sent for request_id.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiagnoseResult {
    #[serde(rename = "type")]
    pub msg_type: String,
    pub request_id: String,
    pub rule_id: i64,
    /// The node_id the node reports in its StatusReport (may be empty on old
    /// nodes; the panel falls back to a group-scoped id).
    #[serde(default)]
    pub node_id: String,
    /// v0.4.9: the challenge the panel sent in DiagnoseRuleMessage, echoed back
    /// verbatim. `#[serde(default)]` so a pre-0.4.9 result still deserializes
    /// (its challenge will be empty, which the panel rejects). The panel only
    /// dispatches to >=0.4.9 nodes, so a legitimately-accepted result MUST
    /// carry a non-empty, exact-matching challenge.
    #[serde(default)]
    pub challenge: String,
    /// Whether the node has an active listener task for this rule.
    pub listener_running: bool,
    /// The listen port the node is actually serving (0 if not running).
    #[serde(default)]
    pub listen_port: u16,
    /// "tcp" / "udp" / "tcp_udp" — the ingress protocol the listener serves.
    #[serde(default)]
    pub protocol: String,
    /// "raw" / "ws" / "tls_simple" — the transport the listener uses.
    #[serde(default)]
    pub transport: String,
    /// Per-target probe results (max 32, matching the rule target cap).
    #[serde(default)]
    pub results: Vec<DiagnoseTargetResult>,
}

/// Compare a reported node_version against "0.4.9". Returns true if the node
/// supports the SECURE diagnose protocol (the one that echoes back the
/// challenge). Tolerates missing/malformed versions (treats them as
/// unsupported) so a stale/garbled status never silently bypasses the upgrade
/// prompt.
///
/// NOTE on naming: this is specifically about the *secure* diagnose protocol
/// (the v0.4.9 challenge handshake). The diagnose *feature* itself existed
/// since v0.4.8, but a v0.4.8 node can't satisfy the challenge check, so the
/// panel never dispatches to it. Future diagnose-protocol evolutions should
/// introduce a dedicated `diagnose_protocol_version` field rather than keep
/// piggy-backing on the product version number.
pub fn node_supports_secure_diagnose(version: Option<&str>) -> bool {
    let Some(v) = version else {
        return false;
    };
    // Parse major.minor.patch; any parse failure → unsupported. A pre-release
    // suffix like "-rc1" is stripped so an exact 0.4.9-rc1 is still accepted
    // (rc builds of the same release are protocol-compatible).
    let base = v.split('-').next().unwrap_or("");
    let mut parts = base.split('.');
    let major: u32 = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0);
    let minor: u32 = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0);
    let patch: u32 = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0);
    (major, minor, patch) >= (0, 4, 9)
}

/// v0.4.14: whether a node can be targeted by DIRECTED (per-node) diagnosis.
/// Directed diagnosis relies on the node advertising its `X-Node-ID` on the WS
/// handshake, which only landed in v0.4.14. An older node (even a healthy
/// v0.4.13 that supports secure diagnose) cannot be targeted — it won't appear
/// in `online_node_ids` — so the panel must surface "please upgrade" rather
/// than a misleading "control channel offline". Returns true for >= 0.4.14.
pub fn node_supports_directed_diagnose(version: Option<&str>) -> bool {
    let Some(v) = version else {
        return false;
    };
    let base = v.split('-').next().unwrap_or("");
    let mut parts = base.split('.');
    let major: u32 = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0);
    let minor: u32 = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0);
    let patch: u32 = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0);
    (major, minor, patch) >= (0, 4, 14)
}

// === Admin API — Rules ===
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuleTargetRequest {
    pub host: String,
    pub port: u16,
    #[serde(default = "default_target_enabled")]
    pub enabled: bool,
}

fn default_target_enabled() -> bool {
    true
}

#[derive(Debug, Serialize, Deserialize)]
pub struct CreateRuleRequest {
    pub name: String,
    /// If None, the server auto-assigns a free port from 10000-65535.
    pub listen_port: Option<u16>,
    pub protocol: Protocol,
    /// v0.4.10: optional owner. Only an admin may set this (to create a rule
    /// on behalf of another user); a non-admin's value is IGNORED and the rule
    /// is attributed to the caller. Omitted → the caller owns the rule.
    #[serde(default)]
    pub owner_uid: Option<i64>,
    pub device_group_in: i64,
    /// None or omitted when forward_mode is "direct" (inbound connects to
    /// target directly, no outbound group needed).
    #[serde(default)]
    pub device_group_out: Option<i64>,
    /// "group" (default) = forward via outbound group; "direct" = inbound
    /// connects to target_addr:target_port directly.
    #[serde(default = "default_forward_mode")]
    pub forward_mode: String,
    /// v0.4.0: forwarding topology. Defaults to Direct. The panel accepts
    /// direct/group; chain is rejected (node engine not implemented).
    #[serde(default)]
    pub route_mode: RouteMode,
    /// v0.4.0: user-facing ingress transport. Defaults to Raw. The panel
    /// derives `node_transport` from this (identity for raw/ws) and
    /// stores both. Replaces the v0.3.x `entry_transport` field.
    #[serde(default)]
    pub public_transport: PublicTransport,
    /// WS path for ws rules (e.g. "/relay"). Ignored for Raw rules.
    /// If None/empty for a Ws rule, the node uses its built-in default
    /// ("/relay") — so this is purely an override, not a required field.
    #[serde(default)]
    pub ws_path: Option<String>,
    pub target_addr: String,
    pub target_port: u16,
    /// v0.4.6: optional multi-target list. Omitted means use the legacy
    /// target_addr/target_port pair as a single enabled target.
    #[serde(default)]
    pub targets: Option<Vec<RuleTargetRequest>>,
    /// v0.4.6: multi-target load-balancing strategy. Defaults to First.
    #[serde(default)]
    pub load_balance_strategy: LoadBalanceStrategy,
    /// v0.4.6: per-rule upload cap in Mbps (0 / omitted = unlimited).
    #[serde(default)]
    pub upload_limit_mbps: Option<i32>,
    /// v0.4.6: per-rule download cap in Mbps (0 / omitted = unlimited).
    #[serde(default)]
    pub download_limit_mbps: Option<i32>,
    /// v0.4.7: bind this rule to a tunnel profile (the source of transport
    /// config). None/omitted = legacy behavior (use public_transport/ws_path).
    #[serde(default)]
    pub tunnel_profile_id: Option<i64>,
}

fn default_forward_mode() -> String {
    "group".to_string()
}

/// Update an existing rule. All fields optional — only provided fields are
/// updated. listen_port=None means "keep current port" (NOT auto-assign —
/// auto-assign only happens on create).
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct UpdateRuleRequest {
    pub name: Option<String>,
    pub listen_port: Option<u16>,
    pub protocol: Option<Protocol>,
    pub device_group_in: Option<i64>,
    pub device_group_out: Option<i64>,
    pub forward_mode: Option<String>,
    /// v0.4.0: forwarding topology. Some(value) updates; omitted keeps current.
    #[serde(default)]
    pub route_mode: Option<RouteMode>,
    /// v0.4.0: user-facing ingress transport. Some(value) updates (and
    /// re-derives node_transport); omitted keeps current. Replaces the v0.3.x
    /// `entry_transport` field.
    #[serde(default)]
    pub public_transport: Option<PublicTransport>,
    /// WS path override for ws rules. Update with Some(value) to set,
    /// Some(None)/omitted keeps current. Not present on the request = leave the
    /// stored value untouched.
    #[serde(default)]
    pub ws_path: Option<Option<String>>,
    pub target_addr: Option<String>,
    pub target_port: Option<u16>,
    /// v0.4.6: replace the rule's target list. Omitted keeps current targets.
    #[serde(default)]
    pub targets: Option<Vec<RuleTargetRequest>>,
    /// v0.4.6: update the multi-target load-balancing strategy. Omitted keeps current.
    #[serde(default)]
    pub load_balance_strategy: Option<LoadBalanceStrategy>,
    /// v0.4.6: per-rule upload cap in Mbps (0 = unlimited). Omitted keeps current.
    #[serde(default)]
    pub upload_limit_mbps: Option<i32>,
    /// v0.4.6: per-rule download cap in Mbps (0 = unlimited). Omitted keeps current.
    #[serde(default)]
    pub download_limit_mbps: Option<i32>,
    /// v0.4.7: bind (Some) or unbind (None) the rule's tunnel profile. Omitted
    /// = leave current binding.
    #[serde(default)]
    pub tunnel_profile_id: Option<Option<i64>>,
    /// v0.3.0: pause/resume a rule without deleting it. true = paused (the node
    /// stops forwarding — get_config filters `WHERE paused = 0`), false = active.
    /// Omitted = leave current. Added because there was previously NO way to
    /// toggle paused after creation, even though the node already honored it.
    #[serde(default)]
    pub paused: Option<bool>,
}

// === Admin API — Groups ===
#[derive(Debug, Serialize, Deserialize)]
pub struct CreateGroupRequest {
    pub name: String,
    pub group_type: GroupType,
    pub connect_host: String,
    pub port_range: String,
    /// v0.4.10: optional owner. Only an admin may set this; a non-admin's
    /// value is IGNORED and the group is attributed to the caller. Omitted →
    /// the caller owns the group.
    #[serde(default)]
    pub owner_uid: Option<i64>,
    /// v1.0.8: traffic billing multiplier for this line. Users are charged
    /// real bytes × rate (rounded) in apply_traffic_batch; rule/user byte
    /// counters keep real bytes. Range 0.1..=100, default 1.0. The handler
    /// clamps `None` to 1.0 and rejects out-of-range values with 400.
    #[serde(default)]
    pub rate: Option<f64>,
    /// v1.0.7: hide this group from regular users' shared views. Default false.
    #[serde(default)]
    pub hidden: Option<bool>,
}

/// Update an existing group. All fields optional. Token is NOT updatable
/// here (regenerating tokens is a separate future endpoint).
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct UpdateGroupRequest {
    pub name: Option<String>,
    pub group_type: Option<GroupType>,
    pub connect_host: Option<String>,
    pub port_range: Option<String>,
    /// v1.0.8: billing rate. Range 0.1..=100 (validated at the handler).
    #[serde(default)]
    pub rate: Option<f64>,
    /// v1.0.7: hide from regular users' shared views. None = leave unchanged.
    #[serde(default)]
    pub hidden: Option<bool>,
}

// === Admin API — Plans (v1.0.8) ===
/// Create a plan. price is a decimal string (canonicalized via parse_balance).
/// plan_type 'data' = traffic-quota plan, 'time' = time-limited (duration_days).
#[derive(Debug, Serialize, Deserialize)]
pub struct CreatePlanRequest {
    pub name: String,
    pub max_rules: i32,
    pub traffic: i64,
    pub price: String,
    /// 'data' or 'time'. Defaults to 'data' when omitted.
    #[serde(default = "default_plan_type")]
    pub plan_type: String,
    /// Validity in days (0 = unlimited). Required > 0 for time plans.
    #[serde(default)]
    pub duration_days: i32,
    #[serde(default)]
    pub hidden: bool,
    #[serde(default)]
    pub reset_traffic: bool,
    #[serde(default)]
    pub description: String,
    /// v1.0.9: buying grants ALL inbound groups (sets all_device_groups).
    #[serde(default)]
    pub grant_all_groups: bool,
    /// v1.0.9: device groups granted on purchase (when grant_all_groups=false).
    #[serde(default)]
    pub device_group_ids: Vec<i64>,
}

fn default_plan_type() -> String {
    "data".to_string()
}

/// Update a plan. All fields optional.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct UpdatePlanRequest {
    pub name: Option<String>,
    pub max_rules: Option<i32>,
    pub traffic: Option<i64>,
    pub price: Option<String>,
    #[serde(default)]
    pub plan_type: Option<String>,
    #[serde(default)]
    pub duration_days: Option<i32>,
    #[serde(default)]
    pub hidden: Option<bool>,
    #[serde(default)]
    pub reset_traffic: Option<bool>,
    #[serde(default)]
    pub description: Option<String>,
    /// v1.0.9: grant ALL inbound groups on purchase.
    #[serde(default)]
    pub grant_all_groups: Option<bool>,
    /// v1.0.9: REPLACE the plan's device-group grant set. None = leave as-is.
    #[serde(default)]
    pub device_group_ids: Option<Vec<i64>>,
}

/// v1.0.8: self-purchase body. plan_id must reference a visible (hidden=0) plan.
#[derive(Debug, Serialize, Deserialize)]
pub struct BuyPlanRequest {
    pub plan_id: i64,
}

/// v1.0.7: admin assigns a plan to a user, charging the user's balance (same
/// rules as a self-purchase). Hidden plans are allowed here.
#[derive(Debug, Serialize, Deserialize)]
pub struct AdminBuyPlanRequest {
    pub plan_id: i64,
}

/// v1.0.7: admin edits a user's plan association + expiry WITHOUT charging.
/// `clear = true` removes the plan entirely (plan_id + plan_expire_at → NULL).
/// `clear = false` keeps the user's current plan_id and sets the expiry, where
/// `plan_expire_at = None` means "never expires".
#[derive(Debug, Serialize, Deserialize)]
pub struct AdminSetUserPlanRequest {
    #[serde(default)]
    pub clear: bool,
    #[serde(default)]
    pub plan_expire_at: Option<String>,
}

// === Admin API — Tunnel Profiles (v0.4.0) ===
/// Create a user-defined tunnel profile. Builtin profiles (is_builtin=1) are
/// seeded by migration and cannot be created/edited/deleted through this API.
#[derive(Debug, Serialize, Deserialize)]
pub struct CreateTunnelProfileRequest {
    pub name: String,
    /// direct / ws / tls_simple / chain — matches tunnel_profiles.transport.
    pub transport: String,
    /// none / terminate / passthrough (TLS termination mode; relevant for tls).
    #[serde(default = "default_tls_mode")]
    pub tls_mode: String,
    /// WS path (e.g. "/relay"); empty for non-WS transports.
    #[serde(default)]
    pub ws_path: String,
    /// Host header value for WS routing; empty if not used.
    #[serde(default)]
    pub host_header: String,
    /// SNI for TLS; empty if not used.
    #[serde(default)]
    pub sni: String,
}

/// Update an existing tunnel profile. All fields optional. Builtin profiles
/// reject this (handler returns 400).
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct UpdateTunnelProfileRequest {
    pub name: Option<String>,
    pub transport: Option<String>,
    pub tls_mode: Option<String>,
    pub ws_path: Option<String>,
    pub host_header: Option<String>,
    pub sni: Option<String>,
}

fn default_tls_mode() -> String {
    "none".to_string()
}

/// Device group types. Values map to stable machine strings in the DB:
/// - In → "in" (listener node, receives forwarding rules)
/// - Out → "out" (egress node, target for forwarding)
/// - Monitor → "monitor" (observability only, no forwarding yet)
///
/// v0.4.7: `ChainedOutbound` was removed (chain mode is gone). The migration
/// rewrites historical `group_type='chained_outbound'` rows to `'out'`.
///
/// Note: "in"/"out" are kept for backward compat with v0.1.0/v0.1.1 DBs.
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub enum GroupType {
    #[serde(rename = "in")]
    In,
    #[serde(rename = "out")]
    Out,
    #[serde(rename = "monitor")]
    Monitor,
}

// === Common ===
#[derive(Debug, Serialize, Deserialize)]
pub struct ApiResponse<T: Serialize> {
    pub code: i32,
    pub message: String,
    pub data: Option<T>,
}

impl<T: Serialize> ApiResponse<T> {
    pub fn success(data: T) -> Self {
        Self {
            code: 0,
            message: "ok".into(),
            data: Some(data),
        }
    }
    pub fn error(code: i32, message: &str) -> ApiResponse<()> {
        ApiResponse {
            code,
            message: message.into(),
            data: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── PublicTransport / NodeTransport / RouteMode parsing ──

    #[test]
    fn public_transport_from_db_str_known_values() {
        assert_eq!(PublicTransport::from_db_str("raw"), PublicTransport::Raw);
        assert_eq!(PublicTransport::from_db_str("ws"), PublicTransport::Ws);
        // v0.4.1: "wss" is no longer a valid transport — falls back to Raw.
        // (Migration 18 converts existing wss rows to ws before this runs.)
        assert_eq!(PublicTransport::from_db_str("wss"), PublicTransport::Raw);
        assert_eq!(
            PublicTransport::from_db_str("tls_simple"),
            PublicTransport::TlsSimple
        );
        // Legacy v0.3.x "tls" maps to tls_simple.
        assert_eq!(
            PublicTransport::from_db_str("tls"),
            PublicTransport::TlsSimple
        );
    }

    #[test]
    fn public_transport_from_db_str_unknown_defaults_to_raw() {
        assert_eq!(PublicTransport::from_db_str(""), PublicTransport::Raw);
        assert_eq!(
            PublicTransport::from_db_str("unknown"),
            PublicTransport::Raw
        );
        assert_eq!(PublicTransport::from_db_str("quic"), PublicTransport::Raw);
    }

    #[test]
    fn node_transport_from_db_str_known_values() {
        assert_eq!(NodeTransport::from_db_str("raw"), NodeTransport::Raw);
        assert_eq!(NodeTransport::from_db_str("ws"), NodeTransport::Ws);
        assert_eq!(
            NodeTransport::from_db_str("tls_simple"),
            NodeTransport::TlsSimple
        );
        // Legacy "tls" → tls_simple.
        assert_eq!(NodeTransport::from_db_str("tls"), NodeTransport::TlsSimple);
    }

    /// derive_node_transport: the v0.4.1 public→node mapping.
    /// Raw→Raw, Ws→Ws, TlsSimple→TlsSimple. (Wss is removed in v0.4.1.)
    #[test]
    fn public_transport_derives_node_transport() {
        assert_eq!(
            PublicTransport::Raw.derive_node_transport(),
            NodeTransport::Raw
        );
        assert_eq!(
            PublicTransport::Ws.derive_node_transport(),
            NodeTransport::Ws
        );
        assert_eq!(
            PublicTransport::TlsSimple.derive_node_transport(),
            NodeTransport::TlsSimple
        );
    }

    #[test]
    fn route_mode_from_db_str_known_values() {
        assert_eq!(RouteMode::from_db_str("direct"), RouteMode::Direct);
        assert_eq!(RouteMode::from_db_str("group"), RouteMode::Group);
        // v0.4.7: chain was removed; a stale "chain" row maps to Direct (the
        // migration pauses such rules, so this only governs unmigrated rows).
        assert_eq!(RouteMode::from_db_str("chain"), RouteMode::Direct);
        assert_eq!(RouteMode::from_db_str(""), RouteMode::Direct);
        assert_eq!(RouteMode::from_db_str("unknown"), RouteMode::Direct);
    }

    // ── build_listeners_for_rule / expand_protocols ──
    // These are the shared listener-construction entry points; both
    // get_config (HTTP poll) and build_config_snapshot (WS push) call them, so
    // a regression here is a regression in BOTH config paths at once.

    /// Minimal helper to build a ForwardRule with only the fields that
    /// build_listeners_for_rule reads, defaulting the rest. Keeps the tests
    /// below readable.
    /// `node_transport` is the DB-stored value (already derived from public).
    fn rule(id: i64, protocol: &str, node_transport: &str) -> crate::models::ForwardRule {
        crate::models::ForwardRule {
            id,
            name: format!("rule-{id}"),
            uid: 1,
            paused: false,
            listen_port: 10000 + id as i32,
            protocol: protocol.into(),
            public_transport: node_transport.into(),
            node_transport: node_transport.into(),
            route_mode: "direct".into(),
            device_group_in: 1,
            device_group_out: None,
            forward_mode: "direct".into(),
            tunnel_profile_id: None,
            domain: None,
            ws_path: None,
            ws_host: None,
            sni: None,
            target_addr: "127.0.0.1".into(),
            target_port: 53,
            targets: Vec::new(),
            load_balance_strategy: "first".into(),
            upload_limit_mbps: 0,
            download_limit_mbps: 0,
            config: "{}".into(),
            traffic_used: 0,
            status: "active".into(),
            created_at: String::new(),
        }
    }

    #[test]
    fn expand_protocols_splits_tcp_udp() {
        assert_eq!(expand_protocols("tcp"), vec![Protocol::Tcp]);
        assert_eq!(expand_protocols("udp"), vec![Protocol::Udp]);
        // tcp_udp → TWO entries (Tcp then Udp), so the node runs both listeners.
        assert_eq!(
            expand_protocols("tcp_udp"),
            vec![Protocol::Tcp, Protocol::Udp]
        );
        // Unknown / empty defaults to Tcp (defensive — DB should never hold these).
        assert_eq!(expand_protocols(""), vec![Protocol::Tcp]);
        assert_eq!(expand_protocols("quic"), vec![Protocol::Tcp]);
    }

    #[test]
    fn build_listeners_tcp_udp_rule_yields_two_entries() {
        let r = rule(5, "tcp_udp", "raw");
        let ls = build_listeners_for_rule(&r, vec!["10.0.0.1:53".into()]);
        assert_eq!(ls.len(), 2, "tcp_udp must expand to Tcp + Udp listeners");
        assert_eq!(ls[0].protocol, Protocol::Tcp);
        assert_eq!(ls[1].protocol, Protocol::Udp);
        // Both share the rule's id, port, targets — only protocol differs.
        for l in &ls {
            assert_eq!(l.rule_id, 5);
            assert_eq!(l.port, 10005);
            assert_eq!(l.targets, vec!["10.0.0.1:53".to_string()]);
        }
    }

    /// v0.4.0: the node_transport column is passed through verbatim — the panel
    /// no longer derives at config-build time (derivation happens at write
    /// time, see admin.rs). A rule whose node_transport="ws" produces a Ws
    /// listener; this is what a wss public rule resolves to after write-time
    /// derivation.
    #[test]
    fn build_listeners_passes_node_transport_through() {
        let r = rule(1, "tcp", "ws");
        let ls = build_listeners_for_rule(&r, vec!["t:1".into()]);
        assert_eq!(ls.len(), 1);
        assert_eq!(
            ls[0].node_transport,
            NodeTransport::Ws,
            "node_transport column passes through unchanged"
        );
    }

    #[test]
    fn build_listeners_passes_through_ws_path() {
        // The per-rule ws_path override must reach the node's ListenerConfig.
        let mut r = rule(2, "tcp", "ws");
        r.ws_path = Some("/custom".into());
        let ls = build_listeners_for_rule(&r, vec!["t:1".into()]);
        assert_eq!(ls.len(), 1);
        assert_eq!(ls[0].ws_path.as_deref(), Some("/custom"));
    }

    #[test]
    fn mbps_to_bps_converts_and_treats_zero_as_unlimited() {
        assert_eq!(mbps_to_bps(0), None);
        assert_eq!(mbps_to_bps(-1), None);
        // 1 Mbps (decimal) = 1_000_000 bit/s = 125_000 byte/s.
        assert_eq!(mbps_to_bps(1), Some(125_000));
        assert_eq!(mbps_to_bps(8), Some(1_000_000));
    }

    #[test]
    fn build_listeners_passes_rate_limits_per_listener() {
        // A rule with caps: each expanded listener carries the same converted
        // bytes/sec cap (a tcp_udp rule does NOT get double — sharing happens
        // node-side, keyed by rule_id).
        let mut r = rule(4, "tcp_udp", "raw");
        r.upload_limit_mbps = 8; // 1_000_000 byte/s
        r.download_limit_mbps = 16; // 2_000_000 byte/s
        let ls = build_listeners_for_rule(&r, vec!["t:1".into()]);
        assert_eq!(ls.len(), 2);
        for l in &ls {
            assert_eq!(l.upload_limit_bps, Some(1_000_000));
            assert_eq!(l.download_limit_bps, Some(2_000_000));
        }
    }

    #[test]
    fn node_supports_secure_diagnose_version_gate() {
        // Unsupported: missing, malformed, older than 0.4.9, AND exactly 0.4.8
        // (v0.4.8 has the diagnose feature but not the secure challenge echo,
        // so it is gated out to prevent silently bypassing the challenge check).
        assert!(!node_supports_secure_diagnose(None));
        assert!(!node_supports_secure_diagnose(Some("")));
        assert!(!node_supports_secure_diagnose(Some("0.4.7")));
        assert!(!node_supports_secure_diagnose(Some("0.4.7-rc1")));
        assert!(!node_supports_secure_diagnose(Some("0.4.8")));
        assert!(!node_supports_secure_diagnose(Some("garbage")));
        assert!(!node_supports_secure_diagnose(Some("0.3.99")));
        // Supported: exactly 0.4.9 and above (rc of the same release accepted).
        assert!(node_supports_secure_diagnose(Some("0.4.9")));
        assert!(node_supports_secure_diagnose(Some("0.4.9-rc1")));
        assert!(node_supports_secure_diagnose(Some("0.5.0")));
        assert!(node_supports_secure_diagnose(Some("1.0.0")));
    }

    #[test]
    fn node_supports_directed_diagnose_version_gate() {
        // v0.4.14: directed diagnosis needs X-Node-ID, which only exists from
        // 0.4.14. A healthy 0.4.13 is NOT targetable → false (the caller turns
        // this into "please upgrade", not "control channel offline").
        assert!(!node_supports_directed_diagnose(None));
        assert!(!node_supports_directed_diagnose(Some("")));
        assert!(!node_supports_directed_diagnose(Some("0.4.9")));
        assert!(!node_supports_directed_diagnose(Some("0.4.13")));
        assert!(!node_supports_directed_diagnose(Some("garbage")));
        // Supported: exactly 0.4.14 and above (rc of the same release accepted).
        assert!(node_supports_directed_diagnose(Some("0.4.14")));
        assert!(node_supports_directed_diagnose(Some("0.4.14-rc1")));
        assert!(node_supports_directed_diagnose(Some("0.5.0")));
        assert!(node_supports_directed_diagnose(Some("1.0.0")));
    }
}
