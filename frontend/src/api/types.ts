// Shared types matching the backend (crates/shared/src/models.rs + protocol.rs).
// Keep these in sync when changing Rust structs.

export interface ApiEnvelope<T> {
  code: number;
  message: string;
  data: T | null;
}

export interface User {
  id: number;
  username: string;
  balance: string;
  plan_id: number | null;
  /** v1.0.7: replaces group_id. true = user may use all device groups. */
  all_device_groups: boolean;
  max_rules: number;
  /** @deprecated PLACEHOLDER — stored but never enforced. Do not surface in UI. */
  speed_limit: number;
  /** @deprecated PLACEHOLDER — stored but never enforced. Do not surface in UI. */
  ip_limit: number;
  traffic_used: number;
  traffic_limit: number;
  admin: boolean;
  banned: boolean;
  created_at: string;
  /** v1.0.8: plan expiry ('YYYY-MM-DD HH:MM:SS' UTC, null = no expiry). */
  plan_expire_at?: string | null;
  /** v1.0.8: admin suspension (login allowed; forwarding gated). */
  suspended?: boolean;
}

export interface ForwardRuleTarget {
  id: number;
  rule_id: number;
  host: string;
  port: number;
  position: number;
  enabled: boolean;
  created_at: string;
}

export interface RuleTargetInput {
  host: string;
  port: number;
  enabled: boolean;
}

export interface ForwardRule {
  id: number;
  name: string;
  uid: number;
  paused: boolean;
  listen_port: number;
  protocol: string;
  /** v0.4.0: user-facing ingress transport. "raw" | "ws" | "wss" | "tls_simple".
   *  Legacy "tls" is mapped to "tls_simple" on read. Replaces entry_transport. */
  public_transport?: string;
  /** v0.4.0: the transport the node listens on (derived from public_transport).
   *  "raw" | "ws" | "tls_simple". The node never receives "wss". */
  node_transport?: string;
  /** v0.4.0: forwarding topology. "direct" | "group". (v0.4.7: chain removed.) */
  route_mode?: string;
  /** v0.4.0: WS path override for ws/wss rules. Null/undefined → the node uses
   *  its built-in default ("/relay"). Only meaningful for ws/wss. */
  ws_path?: string | null;
  device_group_in: number;
  device_group_out: number | null;
  forward_mode: string;
  target_addr: string;
  target_port: number;
  targets?: ForwardRuleTarget[];
  /** v0.4.6: multi-target load-balancing strategy.
   *  "first" | "round_robin" | "failover". Defaults to "first". */
  load_balance_strategy?: string;
  /** v0.4.6: per-rule upload cap in Mbps (0 = unlimited). */
  upload_limit_mbps?: number;
  /** v0.4.6: per-rule download cap in Mbps (0 = unlimited). */
  download_limit_mbps?: number;
  /** v0.4.7: bound tunnel profile (source of transport config).
   *  null/undefined = legacy (use public_transport/ws_path). */
  tunnel_profile_id?: number | null;
  config: string;
  traffic_used: number;
  status: string;
  created_at: string;
}

/** v0.4.0: a tunnel profile (matches backend TunnelProfile struct). Builtin
 *  profiles (is_builtin) are read-only in the UI. */
export interface TunnelProfile {
  id: number;
  name: string;
  transport: string;
  tls_mode: string;
  ws_path: string;
  host_header: string;
  sni: string;
  cert_id: number | null;
  is_builtin: boolean;
  uid: number;
  created_at: string;
}

export interface DeviceGroup {
  id: number;
  name: string;
  group_type: string;
  token: string;
  uid: number;
  connect_host: string;
  port_range: string;
  fallback_group: number | null;
  config: string;
  /** v1.0.8: traffic billing multiplier (0.1..=100, default 1.0). Users are
   *  charged real bytes * rate; rule/user byte counters stay real. */
  rate: number;
  /** v1.0.7: hidden from regular users' shared views (node status / available
   *  lines). Admins are unaffected. */
  hidden?: boolean;
  created_at: string;
}

export interface Plan {
  id: number;
  name: string;
  max_rules: number;
  traffic: number;
  /** @deprecated PLACEHOLDER — stored but never enforced. Do not surface in UI. */
  speed_limit: number;
  /** @deprecated PLACEHOLDER — stored but never enforced. Do not surface in UI. */
  ip_limit: number;
  price: string;
  /** v1.0.8: 'data' = traffic-quota plan, 'time' = time-limited plan. */
  plan_type?: string;
  /** v1.0.8: validity in days (0 = unlimited). */
  duration_days?: number;
  /** v1.0.8: hidden from the public list + not self-purchasable. */
  hidden?: boolean;
  /** v1.0.8: buying resets traffic_used to 0. */
  reset_traffic?: boolean;
  /** v1.0.8: free-form line shown under the plan name. */
  description?: string;
  /** v1.0.9: buying grants ALL inbound groups (sets all_device_groups). */
  grant_all_groups?: boolean;
  /** v1.0.9: device groups this plan grants on purchase (when not grant_all).
   *  Populated by GET /admin/plans and the shop list; sent on create/update. */
  device_group_ids?: number[];
  created_at: string;
}

/** v1.0.8: a purchase order. plan_name + price are snapshots at buy time. */
export interface Order {
  id: number;
  user_id: number;
  plan_id: number | null;
  plan_name: string;
  price: string;
  created_at: string;
}

/** One listener bind/runtime failure reported by a node. Matches the backend
 *  ListenerError struct in crates/shared/src/protocol.rs. */
export interface ListenerError {
  port: number;
  /** "tcp" | "udp" | "ws" */
  protocol: string;
  /** Human-readable reason, e.g. "Address already in use (os error 98)". */
  error: string;
}

export interface NodeStatus {
  group_id: number;
  /** Per-node identity. Null/undefined for legacy single-node status rows. */
  node_id?: string | null;
  /** Present in the API response but not the legacy type; both Nodes and
   *  Dashboard pages render it. Optional for safety on older payloads. */
  group_name?: string;
  /** relay-node binary version; missing on older nodes. */
  node_version?: string | null;
  /** v0.4.0: config-protocol version the node speaks. Missing/old →
   *  "配置协议不兼容，请升级节点". Compared against the panel's current version. */
  config_protocol_version?: number | null;
  /** v0.4.15 PR3: server-computed online flag (status_is_online, 30s window).
   *  The admin /nodes endpoint now stamps this so the frontend never recomputes
   *  an online threshold of its own. Optional for older payloads. */
  online?: boolean;
  cpu: number;
  mem: number;
  connections: number;
  /** v0.3.2: SYSTEM uptime (since OS boot). Was process uptime before v0.3.2. */
  uptime: number;
  /** v0.3.2: relay-node process uptime (since this binary started). Optional —
   *  older nodes don't report it; renders as "-". */
  process_uptime?: number | null;
  last_seen: string;
  // --- Extended metrics (all optional; "-" is shown when missing/old node) ---
  public_ip?: string | null;
  /** v0.4.15: dual-stack public IPs. public_ipv4 falls back to public_ip for
   *  older nodes. public_ipv6 is null when the node has no IPv6. */
  public_ipv4?: string | null;
  public_ipv6?: string | null;
  /** v0.4.15: node-level GeoIP (resolved by the panel, not the node). */
  ipv4_country_code?: string | null;
  ipv4_country_name?: string | null;
  ipv6_country_code?: string | null;
  ipv6_country_name?: string | null;
  disk_total?: number | null;
  disk_used?: number | null;
  disk_usage_percent?: number | null;
  disk_mount?: string | null;
  upload_bps?: number | null;
  download_bps?: number | null;
  boot_upload_bytes?: number | null;
  boot_download_bytes?: number | null;
  /** v0.4.6: interface machine traffic is counted on (e.g. "eth0"). Missing on
   *  older nodes; render "-". */
  network_interface?: string | null;
  /** v0.3.6: listeners that failed to bind on the node (port in use, permission
   *  denied, etc.). Missing/empty = all listeners healthy. Older nodes don't
   *  report it; render "ok" for them. */
  listener_errors?: ListenerError[] | null;
}

export interface LoginResponse {
  token: string;
  admin: boolean;
}

/** v0.4.9: a user's view of their own account (GET /user/me). Mirrors the
 *  backend UserSelf struct — no password hash, only the fields the account
 *  page renders.
 *  v0.4.10: expanded to the full account projection — plan_id/plan_name,
 *  current_rules (owned rule count), and registered_at (renamed from
 *  created_at). must_change_password is added in PR4. */
export interface UserSelf {
  id: number;
  username: string;
  admin: boolean;
  balance: string;
  plan_id: number | null;
  plan_name: string | null;
  max_rules: number;
  current_rules: number;
  traffic_used: number;
  traffic_limit: number;
  registered_at: string;
  /** v0.4.10 PR4: when true the app redirects to the force-password-change
   *  page (only /user/me + /user/password are reachable until changed). */
  must_change_password: boolean;
  /** v1.0.8: plan expiry (null = no expiry). */
  plan_expire_at?: string | null;
  /** v1.0.8: admin suspension (login allowed; forwarding gated). */
  suspended?: boolean;
  /** v1.0.8: true when unrestricted (admin, or all_device_groups) — every
   *  inbound line is usable. False = `available_groups` lists the specific
   *  lines this user is authorized for. */
  all_groups?: boolean;
  /** v1.0.8: names of the device groups (lines) this user can currently use.
   *  Empty when `all_groups` is true or the user has no authorization. */
  available_groups?: string[];
}

/** v0.4.10 PR4: admin password reset body (PUT /admin/users/{id}/password). */
export interface ResetPasswordRequest {
  new_password: string;
  must_change_password: boolean;
}

/** v0.4.10 PR3 / v0.4.21 PR2: public registration-status response (GET /auth/registration-status). */
export interface RegistrationStatus {
  enabled: boolean;
  default_plan_id: number;
  plans: Plan[];
  default_password_change_required: boolean;
}

/** v0.4.10 PR3 / v0.4.21 PR2: admin-managed registration settings (GET/PUT /admin/settings/registration). */
export interface RegistrationSettings {
  registration_enabled: boolean;
  default_registration_plan_id: number;
  allowed_plan_ids: number[];
}

// === v0.4.8: rule diagnosis ===  === v0.4.9: TCP-only + panel→ingress probe ===

/** Outcome of probing ONE target from the node (TCP-only since v0.4.9).
 *  The old `route_only` variant (UDP route check) is gone — UDP isn't probed. */
export type TargetProbeOutcome =
  | { reachable: { elapsed_ms: number } }
  | { failed: { error: string } }
  | 'timeout';

export interface DiagnoseTargetResult {
  address: string;
  outcome: TargetProbeOutcome;
}

export interface DiagnoseResult {
  type: string;
  request_id: string;
  rule_id: number;
  node_id: string;
  /** v0.4.9: per-run challenge echoed back from the node. The backend verifies
   *  it's non-empty and matches what it sent; the UI doesn't render it. */
  challenge?: string;
  listener_running: boolean;
  listen_port: number;
  protocol: string;
  transport: string;
  results: DiagnoseTargetResult[];
}

/** One node's diagnosis view. Mirrors the backend NodeDiagnoseStatus enum
 *  (serde tag="status", rename_all snake_case; the Result variant flattens its
 *  DiagnoseResult fields onto the same object as `status`).
 *
 *  v0.4.9: `unsupported` covers nodes < 0.4.9 — they either have the diagnose
 *  feature but NOT the secure-challenge protocol (0.4.8), or have no diagnose
 *  at all (<0.4.8). The panel never sent them a probe; the UI shows
 *  "诊断协议过旧，请升级". */
/** v0.4.15: group_name + public_ip are PANEL-supplied display fields on every
 *  variant (the node's diagnose wire message is unchanged). The frontend shows
 *  "分组名 · 公网IP" as the node label; node_id stays in a tooltip only and is
 *  never rendered as a visible label. */
export type NodeDiagnoseStatus =
  | ({ status: 'result'; group_name: string; public_ip?: string | null } & DiagnoseResult)
  | { status: 'unsupported'; node_id: string; node_version: string; group_name: string; public_ip?: string | null }
  | { status: 'control_channel_offline'; node_id: string; group_name: string; public_ip?: string | null }
  | { status: 'timeout'; node_id: string; group_name: string; public_ip?: string | null };

export interface DiagnoseResponse {
  request_id: string;
  rule_id: number;
  nodes: NodeDiagnoseStatus[];
}

/** v0.4.11 PR3: inbound groups owned by an admin that are available for
 *  regular users to attach their rules to. Mirrors DeviceGroup shape so the
 *  same picker component can render both. */
export interface SharedGroupSummary {
  id: number;
  name: string;
  group_type: string;
  connect_host: string;
  capabilities: string;
  region?: string | null;
  line_type?: string | null;
}

/** v0.4.13 PR2 / v0.4.14 PR1: per-NODE availability + load metrics for a shared
 *  (admin-owned) inbound group, visible to regular users. One row per node; a
 *  group with no reporting node still yields one placeholder row (node_id empty,
 *  online false, metrics null). Group metadata repeats per node row.
 *  v0.4.14: node_id and public_ip ARE exposed to regular users (product
 *  requirement). Still NEVER exposed: token, config, listener_errors, internal
 *  DB fields, install commands, cert/key material. */
export interface SharedNodeSummary {
  group_id: number;
  group_name: string;
  connect_host: string;
  capabilities: string;
  region?: string | null;
  line_type?: string | null;
  node_id: string;
  online: boolean;
  public_ip?: string | null;
  /** v0.4.15: dual-stack public IPs + node-level GeoIP. */
  public_ipv4?: string | null;
  public_ipv6?: string | null;
  ipv4_country_code?: string | null;
  ipv4_country_name?: string | null;
  ipv6_country_code?: string | null;
  ipv6_country_name?: string | null;
  node_version?: string | null;
  config_protocol_version?: number | null;
  connections: number;
  uptime?: number | null;
  process_uptime?: number | null;
  network_interface?: string | null;
  cpu?: number | null;
  mem?: number | null;
  disk_mount?: string | null;
  disk_usage_percent?: number | null;
  disk_used?: number | null;
  disk_total?: number | null;
  upload_bps?: number | null;
  download_bps?: number | null;
  boot_upload_bytes?: number | null;
  boot_download_bytes?: number | null;
  last_seen?: string | null;
}

/** v0.4.15 PR3: the unified row the node-status board components render. It is
 *  the loose superset of NodeStatus (admin /nodes) and SharedNodeSummary
 *  (user /nodes/shared) — every metric field is optional + nullable so BOTH
 *  source rows assign to it directly, and the components stay `any`-free.
 *
 *  Field availability differs by source (admin sees node_id, config protocol,
 *  network_interface, disk_mount, listener_errors; users get region/line_type),
 *  hence the broad optionality. `online` is always server-supplied now. */
export interface NodeDisplayRow {
  group_id: number;
  group_name?: string | null;
  node_id?: string | null;
  online?: boolean;
  node_version?: string | null;
  config_protocol_version?: number | null;
  connections?: number | null;
  cpu?: number | null;
  mem?: number | null;
  uptime?: number | null;
  process_uptime?: number | null;
  last_seen?: string | null;
  public_ip?: string | null;
  public_ipv4?: string | null;
  public_ipv6?: string | null;
  ipv4_country_code?: string | null;
  ipv4_country_name?: string | null;
  ipv6_country_code?: string | null;
  ipv6_country_name?: string | null;
  disk_total?: number | null;
  disk_used?: number | null;
  disk_usage_percent?: number | null;
  disk_mount?: string | null;
  upload_bps?: number | null;
  download_bps?: number | null;
  boot_upload_bytes?: number | null;
  boot_download_bytes?: number | null;
  network_interface?: string | null;
  listener_errors?: ListenerError[] | null;
  /** Shared-group-only metadata (user view). */
  region?: string | null;
  line_type?: string | null;
}
