use serde::{Deserialize, Serialize};

fn default_load_balance_strategy() -> String {
    "first".to_string()
}

#[derive(Debug, Serialize, Deserialize, sqlx::FromRow)]
pub struct User {
    pub id: i64,
    pub username: String,
    pub password: String,
    pub balance: String,
    pub plan_id: Option<i64>,
    /// v1.0.7: replaces the old `group_id` permission-group link. When true the
    /// user may use ALL device groups (admins are always treated as true). When
    /// false the user is limited to the device groups in `user_device_groups`;
    /// none assigned = cannot forward.
    #[serde(default)]
    pub all_device_groups: bool,
    pub max_rules: i32,
    pub speed_limit: i32,
    pub ip_limit: i32,
    pub traffic_used: i64,
    pub traffic_limit: i64,
    pub admin: bool,
    pub banned: bool,
    pub created_at: String,
    /// v0.4.10 PR4: force a password change on next login (admin reset).
    #[serde(default)]
    pub must_change_password: bool,
    /// v0.4.10 PR4: JWT session-version counter. Bumped on password change /
    /// admin reset / ban to instantly revoke previously-issued tokens.
    #[serde(default)]
    pub token_version: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct ForwardRuleTarget {
    pub id: i64,
    pub rule_id: i64,
    pub host: String,
    pub port: i32,
    pub position: i32,
    pub enabled: bool,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct ForwardRule {
    pub id: i64,
    pub name: String,
    pub uid: i64,
    pub paused: bool,
    pub listen_port: i32,
    pub protocol: String,
    /// v0.4.0: user-facing ingress transport (what the user picks).
    /// "raw" | "ws" | "wss" | "tls_simple". Legacy "tls" is mapped to
    /// "tls_simple" on read. Replaces the v0.3.x `entry_transport` column.
    pub public_transport: String,
    /// v0.4.0: the transport the NODE actually listens on, derived from
    /// public_transport at write time. "raw" | "ws" | "tls_simple". The node
    /// receives this verbatim (never "wss" — that's proxy-terminated).
    pub node_transport: String,
    /// v0.4.0: forwarding topology. "direct" | "group" | "chain".
    pub route_mode: String,
    pub device_group_in: i64,
    pub device_group_out: Option<i64>,
    pub forward_mode: String,
    /// v0.3.0: chain-mode tunnel profile. NULL → fall back to builtin 'direct'
    /// at config-build time.
    pub tunnel_profile_id: Option<i64>,
    /// v0.3.0: optional per-rule WS/TLS metadata. NULL = use profile default /
    /// not applicable for raw/tcp.
    pub domain: Option<String>,
    pub ws_path: Option<String>,
    pub ws_host: Option<String>,
    pub sni: Option<String>,
    pub target_addr: String,
    pub target_port: i32,
    #[serde(default)]
    #[sqlx(skip)]
    pub targets: Vec<ForwardRuleTarget>,
    /// v0.4.6: multi-target load-balancing strategy.
    /// "first" | "round_robin" | "failover". Defaults to "first".
    #[serde(default = "default_load_balance_strategy")]
    pub load_balance_strategy: String,
    /// v0.4.6: per-rule upload cap in decimal Mbps (1 Mbps = 1,000,000 bit/s).
    /// 0 = unlimited. Shared across all connections of the rule.
    #[serde(default)]
    pub upload_limit_mbps: i32,
    /// v0.4.6: per-rule download cap in decimal Mbps. 0 = unlimited.
    #[serde(default)]
    pub download_limit_mbps: i32,
    pub config: String,
    pub traffic_used: i64,
    pub status: String,
    pub created_at: String,
}

#[derive(Debug, Serialize, Deserialize, sqlx::FromRow)]
pub struct DeviceGroup {
    pub id: i64,
    pub name: String,
    pub group_type: String,
    pub token: String,
    pub uid: i64,
    pub connect_host: String,
    pub port_range: String,
    pub fallback_group: Option<i64>,
    pub config: String,
    /// v0.3.0: declared protocol capabilities (JSON array string). Used for
    /// pre-create validation only; e.g. `["tcp","udp"]`.
    pub capabilities: String,
    /// v0.3.0: descriptive metadata (nullable; "- " when absent).
    pub region: Option<String>,
    pub line_type: Option<String>,
    pub remark: Option<String>,
    /// v1.0.8: traffic billing multiplier for this line. Real bytes are stored
    /// on forward_rules / users; users are CHARGED `real * rate` (rounded) in
    /// apply_traffic_batch. 1.0 = bill what you use. Range 0.1..=100.
    pub rate: f64,
    pub created_at: String,
}

/// v0.4.11 PR3: summary of a device group visible to all authenticated users.
/// Does NOT include sensitive fields (token, uid, config, fallback_group).
/// Used by the shared-groups endpoint so regular users can select admin-provided
/// inbound/outbound groups when creating rules.
#[derive(Debug, Serialize, Deserialize, sqlx::FromRow)]
pub struct SharedGroupSummary {
    pub id: i64,
    pub name: String,
    pub group_type: String,
    pub connect_host: String,
    pub capabilities: String,
    pub region: Option<String>,
    pub line_type: Option<String>,
}

/// v0.4.13 PR2 / v0.4.14 PR1: per-NODE availability + load metrics for a shared
/// (admin-owned) inbound group, visible to regular users. Built in the handler
/// layer by scanning the `node_status:*` kvs keys — it is NOT a DB row mapping.
///
/// One row PER NODE. A shared group with no reporting node still yields one
/// placeholder row (node_id empty, online=false, metrics None) so the line
/// never disappears. Group metadata (group_id/name/connect_host/region/
/// line_type) repeats on each of the group's node rows.
///
/// v0.4.14: `node_id` and `public_ip` ARE exposed to regular users (confirmed
/// product requirement — users need to see which server they're using). Still
/// NEVER exposed: NODE_TOKEN, config, listener_errors, internal DB fields,
/// install commands, certificate / private-key material.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SharedNodeSummary {
    pub group_id: i64,
    pub group_name: String,
    pub connect_host: String,
    pub capabilities: String,
    pub region: Option<String>,
    pub line_type: Option<String>,
    /// Per-node identity (row key). Empty for a group's no-node placeholder row.
    pub node_id: String,
    /// This node's last_seen is within the online window (backend SoT).
    pub online: bool,
    /// v0.4.14: node public IP (exposed to regular users). v0.4.15: this is the
    /// legacy field (carries IPv4); prefer `public_ipv4` / `public_ipv6`.
    pub public_ip: Option<String>,
    /// v0.4.15: dual-stack public IPs. `public_ipv4` falls back to `public_ip`
    /// for older nodes. `public_ipv6` is None when the node has no IPv6.
    pub public_ipv4: Option<String>,
    pub public_ipv6: Option<String>,
    /// v0.4.15: node-level GeoIP (resolved by the PANEL from the IP, not
    /// reported by the node). None = lookup disabled / pending / unknown.
    pub ipv4_country_code: Option<String>,
    pub ipv4_country_name: Option<String>,
    pub ipv6_country_code: Option<String>,
    pub ipv6_country_name: Option<String>,
    /// v0.4.14: relay-node binary version (e.g. "0.4.13").
    pub node_version: Option<String>,
    /// v0.4.14: config-protocol version the node speaks.
    pub config_protocol_version: Option<i64>,
    /// v0.4.14: active connection count.
    pub connections: i64,
    /// v0.4.14: SYSTEM uptime (since OS boot), seconds.
    pub uptime: Option<i64>,
    /// v0.4.14: relay-node process uptime (since binary start), seconds.
    pub process_uptime: Option<i64>,
    /// v0.4.14: interface machine traffic is counted on (e.g. "eth0").
    pub network_interface: Option<String>,
    /// CPU usage percent (0-100). None on a placeholder / old node.
    pub cpu: Option<f64>,
    /// Memory usage percent (0-100).
    pub mem: Option<f64>,
    /// v0.4.14: primary-disk mount point (e.g. "/").
    pub disk_mount: Option<String>,
    /// Primary-disk usage percent (0-100).
    pub disk_usage_percent: Option<f64>,
    pub disk_used: Option<i64>,
    pub disk_total: Option<i64>,
    /// Realtime upload / download rate (bytes/sec).
    pub upload_bps: Option<i64>,
    pub download_bps: Option<i64>,
    /// Cumulative (since node boot) upload / download bytes.
    pub boot_upload_bytes: Option<i64>,
    pub boot_download_bytes: Option<i64>,
    /// This node's last_seen (RFC3339), if it has reported.
    pub last_seen: Option<String>,
}

/// v0.3.0: reusable tunnel profile describing the transport between an inbound
/// node and an outbound node (NOT the user-facing entry protocol). The six
/// builtin rows are seeded by Migration 6 and owned by the admin (uid=1).
#[derive(Debug, Serialize, Deserialize, sqlx::FromRow)]
pub struct TunnelProfile {
    pub id: i64,
    pub name: String,
    /// ws | tls_simple
    pub transport: String,
    /// none | terminate | passthrough
    pub tls_mode: String,
    pub ws_path: String,
    pub host_header: String,
    pub sni: String,
    /// Reserved for a future certificates table; NULL until then.
    pub cert_id: Option<i64>,
    /// 1 = seeded builtin (not deletable).
    pub is_builtin: bool,
    pub uid: i64,
    pub created_at: String,
}

#[derive(Debug, Serialize, Deserialize, sqlx::FromRow)]
pub struct Plan {
    pub id: i64,
    pub name: String,
    pub max_rules: i32,
    pub traffic: i64,
    pub speed_limit: i32,
    pub ip_limit: i32,
    pub price: String,
    pub created_at: String,
}

#[derive(Debug, Serialize, Deserialize, sqlx::FromRow)]
pub struct Statistic {
    pub id: i64,
    pub stat_type: String,
    pub stat_key: String,
    pub time: String,
    pub number: i64,
}
