// v0.4.3: Repository trait layer.
//
// Domain-specific traits (UserRepository, RuleRepository, ...) define the data
// access contract. The aggregate `Repository` trait combines them so handlers
// take a single `Arc<dyn Repository>` dependency.
//
// PR1: only SqliteRepository implements these traits. PR2 will add
// PgRepository (PostgreSQL) implementing the same traits.
//
// Design principles:
//   - Methods return domain models (User, ForwardRule, ...) — no DB types leak.
//   - Transactions are encapsulated inside methods (e.g. apply_traffic_batch,
//     reset_traffic) — no `begin()` / `Tx` leaks to handlers.
//   - Errors are DbError (unified codes), not raw sqlx::Error.
//   - Service-layer logic (config assembly, protocol derivation, stale sweep)
//     stays in the caller, NOT in the repository.
//
// dead_code: some trait methods in this module are part of the contract but
// not yet wired to a handler in PR1 (e.g. increment_user_traffic,
// delete_rules_by_uid). They're reachable through the trait object and will be
// used by future callers / PgRepository parity tests; silence the lints rather
// than delete the contract.
#![allow(dead_code)]

use async_trait::async_trait;
use relay_shared::models::{
    DeviceGroup, ForwardRule, ForwardRuleTarget, Order, Plan, SharedGroupSummary, Statistic,
    TunnelProfile, User,
};
use relay_shared::protocol::{RuleTargetRequest, TrafficEntry};
use serde::Serialize;

use super::error::DbError;

// ── Resource scoping (v0.4.10 multi-user isolation) ──

/// The ownership scope a resource query is restricted to.
///
/// `All` = the caller may see/modify every row (administrators). `Owner(uid)`
/// = only rows whose `uid` column equals `uid`. This is the single type the
/// Repository layer uses to enforce per-user isolation; the API layer builds it
/// from the authenticated user (see `AuthUser::resource_scope` in middleware.rs)
/// and the db layer never imports from the api layer.
///
/// `Owner(uid)` is folded into the SQL WHERE clause (e.g.
/// `WHERE id = ? AND uid = ?`), so a miss — "row doesn't exist" vs "row belongs
/// to someone else" — is indistinguishable to the caller (both return None →
/// 404). That closes a resource-id existence oracle.
#[derive(Debug, Clone, Copy)]
pub enum ResourceScope {
    All,
    Owner(i64),
}

impl ResourceScope {
    /// `Some(uid)` when scoped to one owner, `None` when unscoped (admin).
    /// Repository impls use this to pick the scoped vs unscoped SQL branch.
    pub fn owner_id(&self) -> Option<i64> {
        match self {
            ResourceScope::All => None,
            ResourceScope::Owner(uid) => Some(*uid),
        }
    }
}

/// Scope for tunnel-profile reads. Distinct from [`ResourceScope`] because
/// profile isolation is by usage-context, not ownership:
/// - `AvailableTemplates`: templates available for rule selection (ws/tls_simple,
///   builtin + admin-created custom). Used by `GET /tunnel-profiles` so any
///   logged-in user can select a template for their rules.
/// - `ManageableCustomTemplates`: custom templates the admin can manage
///   (is_builtin = false, ws/tls_simple only). Used by `GET /admin/tunnel-profiles`.
/// - `All`: internal use (config generation, audit, migration).
///
/// v0.4.11 PR1: replaced `BuiltinOnly` with context-based scopes. A regular
/// user may now select any available WS/TLS Simple template (builtin or admin-
/// created custom), not just builtin ones.
#[derive(Debug, Clone, Copy)]
pub enum ProfileScope {
    /// Internal: all profiles (config generation, audit, migration).
    All,
    /// Available for rule selection: ws/tls_simple, builtin + admin custom.
    AvailableTemplates,
    /// Manageable custom templates: is_builtin=false, ws/tls_simple only.
    ManageableCustomTemplates,
}

// ── User ──

#[async_trait]
pub trait UserRepository: Send + Sync {
    /// Login lookup: username must exist AND not be banned.
    async fn find_by_username_not_banned(&self, username: &str) -> Result<Option<User>, DbError>;
    /// Register existing check: username exists (regardless of banned).
    async fn find_by_username(&self, username: &str) -> Result<Option<User>, DbError>;
    /// Load full user by id.
    async fn find_by_id(&self, id: i64) -> Result<Option<User>, DbError>;
    /// Password hash for change_password.
    async fn find_password_by_id(&self, id: i64) -> Result<Option<String>, DbError>;
    /// banned flag for auth extractor. Returns None if user doesn't exist.
    async fn find_banned_by_id(&self, id: i64) -> Result<Option<bool>, DbError>;
    /// v0.4.10 PR4: the auth state the middleware needs in ONE query —
    /// (banned, token_version, must_change_password). None = user deleted.
    /// Replaces three separate lookups per request.
    async fn find_auth_state_by_id(&self, id: i64) -> Result<Option<(bool, i64, bool)>, DbError>;
    /// Check if user is admin (returns None if not found or not admin).
    async fn is_admin(&self, id: i64) -> Result<bool, DbError>;
    /// Check if user exists by id.
    async fn exists_by_id(&self, id: i64) -> Result<bool, DbError>;
    /// Insert a new user (register).
    async fn insert_user(
        &self,
        username: &str,
        password_hash: &str,
        plan_id: i64,
    ) -> Result<(), DbError>;
    /// v0.4.10 PR3: register a user whose quota fields (max_rules,
    /// traffic_limit, speed_limit, ip_limit) are inherited ATOMICALLY from the
    /// plan via `INSERT ... SELECT`. This closes the race where a separate
    /// "validate plan then insert" sequence could see the plan change (or be
    /// deleted) between the two steps. Returns rows_affected: 0 means the plan
    /// does not exist (the SELECT matched no row) and no user was created —
    /// the caller surfaces this as a registration failure. A UNIQUE violation
    /// on username still surfaces as `DbError::UniqueViolation` (→ 409).
    async fn insert_user_from_plan(
        &self,
        username: &str,
        password_hash: &str,
        plan_id: i64,
    ) -> Result<u64, DbError>;
    /// Update password.
    async fn update_password(&self, id: i64, new_hash: &str) -> Result<u64, DbError>;
    /// v0.4.10 PR4: self-service password change. Atomically sets the new hash,
    /// bumps token_version (revoking all the user's existing sessions including
    /// the one making this call), and clears must_change_password. Returns rows
    /// affected (0 = user not found).
    async fn change_own_password(&self, id: i64, new_hash: &str) -> Result<u64, DbError>;
    /// v0.4.10 PR4: admin password reset. Atomically sets the new hash, bumps
    /// token_version (revoking the target's sessions), and sets
    /// must_change_password to the given value (so a temporary password forces
    /// a change on first use). Returns rows affected (0 = user not found).
    async fn admin_reset_password(
        &self,
        id: i64,
        new_hash: &str,
        must_change_password: bool,
    ) -> Result<u64, DbError>;
    /// Dynamic field update (balance/max_rules/traffic_limit/banned).
    /// v0.4.10 PR4: when `banned` is set to `Some(true)`, the same UPDATE also
    /// bumps token_version so the ban instantly revokes the user's JWTs (the
    /// per-request banned check already blocks them, but bumping keeps the
    /// revocation signal uniform with admin-reset / self-change).
    async fn update_user_fields(
        &self,
        id: i64,
        balance: Option<&str>,
        max_rules: Option<i32>,
        traffic_limit: Option<i64>,
        banned: Option<bool>,
        suspended: Option<bool>,
    ) -> Result<u64, DbError>;
    /// v1.0.7: admin directly sets a user's plan association + expiry WITHOUT
    /// charging (the "edit user plan" panel uses this for removing a plan — both
    /// NULL — and for adjusting the expiry). Unconditionally writes both columns;
    /// the caller composes the pair (e.g. keep plan_id, change expiry). Skips
    /// admin users (WHERE admin = false). Returns rows affected (0 = not found
    /// or target is an admin).
    async fn admin_set_user_plan(
        &self,
        id: i64,
        plan_id: Option<i64>,
        plan_expire_at: Option<String>,
    ) -> Result<u64, DbError>;
    /// Increment user traffic_used (called inside traffic batch tx).
    async fn increment_user_traffic(&self, id: i64, delta: i64) -> Result<(), DbError>;
    /// Reset traffic_used to 0 for user AND their rules (atomic).
    async fn reset_traffic(&self, id: i64) -> Result<(), DbError>;
    /// Delete user (only if not admin). Returns rows affected (0 = not found or admin).
    async fn delete_non_admin(&self, id: i64) -> Result<u64, DbError>;
    /// Delete a non-admin user AND all their owned resources (rules,
    /// tunnel_profiles, device_groups) in ONE transaction. Returns rows affected
    /// on the users table (0 = not found or admin → nothing deleted, fully rolled
    /// back). Replaces the old non-transactional cascade that missed
    /// tunnel_profiles and could leave a half-deleted account.
    async fn delete_user_cascade(&self, uid: i64) -> Result<u64, DbError>;
    /// List all users (public projection, no password).
    async fn list_users_public(&self) -> Result<Vec<crate::api::admin::UserPublic>, DbError>;
    /// Count users with placeholder admin password (system boot check).
    async fn count_placeholder_admin_password(&self) -> Result<i64, DbError>;
    /// Replace placeholder admin password with a real hash (system boot).
    async fn replace_placeholder_admin_password(&self, hash: &str) -> Result<(), DbError>;
}

// ── Rule (forward_rules) ──

#[async_trait]
pub trait RuleRepository: Send + Sync {
    async fn list_rules(&self, scope: &ResourceScope) -> Result<Vec<ForwardRule>, DbError>;
    /// Look up a single rule by id within the scope. None = no such rule OR a
    /// rule that exists but is outside the caller's scope (indistinguishable,
    /// by design — closes a resource-id existence oracle).
    async fn find_rule_by_id(
        &self,
        rule_id: i64,
        scope: &ResourceScope,
    ) -> Result<Option<ForwardRule>, DbError>;
    /// List all target rows for a rule (within scope), ordered by position.
    async fn list_rule_targets(
        &self,
        rule_id: i64,
        scope: &ResourceScope,
    ) -> Result<Vec<ForwardRuleTarget>, DbError>;
    /// List enabled target rows for a rule (within scope), ordered by position.
    async fn list_enabled_rule_targets(
        &self,
        rule_id: i64,
        scope: &ResourceScope,
    ) -> Result<Vec<ForwardRuleTarget>, DbError>;
    /// Replace all targets for an existing rule (within scope). Positions are
    /// assigned by input order.
    async fn replace_rule_targets(
        &self,
        rule_id: i64,
        scope: &ResourceScope,
        targets: &[RuleTargetRequest],
    ) -> Result<(), DbError>;
    /// Update a rule's load-balancing strategy (within scope). Returns rows affected.
    async fn set_rule_load_balance_strategy(
        &self,
        rule_id: i64,
        scope: &ResourceScope,
        strategy: &str,
    ) -> Result<u64, DbError>;
    /// Update a rule's per-rule upload/download Mbps caps (0 = unlimited),
    /// within scope.
    async fn set_rule_rate_limits(
        &self,
        rule_id: i64,
        scope: &ResourceScope,
        upload_limit_mbps: i32,
        download_limit_mbps: i32,
    ) -> Result<u64, DbError>;
    /// Bind (or unbind, when profile_id is None) a rule to a tunnel profile,
    /// within scope.
    async fn set_rule_tunnel_profile(
        &self,
        rule_id: i64,
        scope: &ResourceScope,
        profile_id: Option<i64>,
    ) -> Result<u64, DbError>;
    /// v0.4.11 PR4: the (listen_port, protocol) pairs already in use on a
    /// specific inbound group. Used by auto_assign_port to pick a free port
    /// scoped to the rule's device_group_in — different groups (and different
    /// users sharing the same group's pool) are evaluated independently.
    async fn list_group_port_protocols(
        &self,
        device_group_in: i64,
    ) -> Result<Vec<(i32, String)>, DbError>;
    /// Count rules for a user (quota reporting).
    async fn count_by_uid(&self, uid: i64) -> Result<i64, DbError>;
    /// Get max_rules for a user (quota ceiling; 0 = unlimited).
    async fn max_rules_for_uid(&self, uid: i64) -> Result<i32, DbError>;
    /// Insert a rule with quota guard AND port-conflict guard, in ONE
    /// transaction. The port check is socket-type aware (TCP-bearing rules
    /// conflict with TCP-bearing, UDP-bearing with UDP-bearing) and scoped to
    /// device_group_in.
    ///
    /// Returns `Ok(1)` on success, `Ok(0)` if the user's max_rules quota is
    /// exhausted, `Err(DbError::PortConflict)` if the port is already occupied
    /// on the group by a conflicting socket type, or `Err(DbError::UniqueViolation)`
    /// as the DB-layer backstop (partial unique index) when a concurrent insert
    /// won the race.
    ///
    /// Concurrency: SQLite uses BEGIN IMMEDIATE (acquire the write lock up
    /// front); PostgreSQL takes a per-group advisory xact lock plus the
    /// existing user-row FOR UPDATE quota lock.
    #[allow(clippy::too_many_arguments)]
    async fn insert_quota_guarded(
        &self,
        name: &str,
        uid: i64,
        listen_port: i32,
        protocol: &str,
        public_transport: &str,
        node_transport: &str,
        route_mode: &str,
        entry_transport: &str,
        ws_path: Option<&str>,
        device_group_in: i64,
        device_group_out: Option<i64>,
        forward_mode: &str,
        target_addr: &str,
        target_port: i32,
    ) -> Result<u64, DbError>;
    /// Find (protocol, public_transport) for effective-combo validation, scoped.
    async fn find_transport_by_id(
        &self,
        id: i64,
        scope: &ResourceScope,
    ) -> Result<Option<(String, String)>, DbError>;
    /// Find device_group_out for update_rule, scoped.
    async fn find_device_group_out_by_id(
        &self,
        id: i64,
        scope: &ResourceScope,
    ) -> Result<Option<Option<i64>>, DbError>;
    /// Dynamic update of rule fields, scoped. Returns rows affected.
    #[allow(clippy::too_many_arguments)]
    async fn update_rule_fields(
        &self,
        id: i64,
        scope: &ResourceScope,
        name: Option<&str>,
        listen_port: Option<i32>,
        protocol: Option<&str>,
        public_transport: Option<&str>,
        node_transport: Option<&str>,
        entry_transport: Option<&str>,
        route_mode: Option<&str>,
        ws_path: Option<Option<&str>>,
        device_group_in: Option<i64>,
        device_group_out: Option<Option<i64>>,
        forward_mode: Option<&str>,
        target_addr: Option<&str>,
        target_port: Option<i32>,
        paused: Option<bool>,
    ) -> Result<u64, DbError>;
    /// Increment rule traffic (upload, download).
    async fn increment_rule_traffic(
        &self,
        id: i64,
        upload: u64,
        download: u64,
    ) -> Result<(), DbError>;
    /// Find rule owner (rule_id, uid) for traffic report ownership check.
    async fn find_rule_owner(
        &self,
        rule_id: i64,
        device_group_in: i64,
    ) -> Result<Option<(i64, i64)>, DbError>;
    /// Delete rule by id, scoped. Returns rows affected.
    async fn delete_rule(&self, id: i64, scope: &ResourceScope) -> Result<u64, DbError>;
    /// Delete all rules for a user (cascade cleanup).
    async fn delete_rules_by_uid(&self, uid: i64) -> Result<u64, DbError>;
    /// List active rules for config build (the JOIN+filter query).
    /// This returns raw ForwardRule rows; config assembly is service-layer.
    async fn list_active_for_config(&self, group_id: i64) -> Result<Vec<ForwardRule>, DbError>;
}

// ── Group (device_groups) ──

#[async_trait]
pub trait GroupRepository: Send + Sync {
    /// Returns all groups the caller has access to, scoped by ownership.
    /// Non-admins see only their own groups.
    async fn list_groups(&self, scope: &ResourceScope) -> Result<Vec<DeviceGroup>, DbError>;
    /// v0.4.12 PR1: returns a summary of ADMIN-owned `group_type = 'in'` groups,
    /// available for ANY regular user to attach rules to — independent of
    /// whether the user already has rules. Admins get an empty list (they manage
    /// groups directly, not via shared infrastructure). Never includes sensitive
    /// fields (token, uid, config, fallback_group). The companion node-status
    /// aggregation is done in the handler layer over the `node_status:*` kvs
    /// keys (there is NO node_status table), so it is NOT a Repository method.
    async fn list_shared_groups(
        &self,
        uid: i64,
        is_admin: bool,
    ) -> Result<Vec<SharedGroupSummary>, DbError>;
    async fn find_by_token(&self, token: &str) -> Result<Option<DeviceGroup>, DbError>;
    /// Look up a group by id within the scope. None = no such group OR a group
    /// outside the caller's scope (indistinguishable → 404).
    async fn find_by_id(
        &self,
        id: i64,
        scope: &ResourceScope,
    ) -> Result<Option<DeviceGroup>, DbError>;
    async fn find_name_by_id(
        &self,
        id: i64,
        scope: &ResourceScope,
    ) -> Result<Option<String>, DbError>;
    #[allow(clippy::too_many_arguments)]
    async fn insert_group(
        &self,
        name: &str,
        group_type: &str,
        token: &str,
        uid: i64,
        connect_host: &str,
        port_range: &str,
        rate: f64,
        hidden: bool,
    ) -> Result<(), DbError>;
    async fn find_by_token_after_insert(&self, token: &str)
        -> Result<Option<DeviceGroup>, DbError>;
    #[allow(clippy::too_many_arguments)]
    async fn update_group_fields(
        &self,
        id: i64,
        scope: &ResourceScope,
        name: Option<&str>,
        group_type: Option<&str>,
        connect_host: Option<&str>,
        port_range: Option<&str>,
        rate: Option<f64>,
        hidden: Option<bool>,
    ) -> Result<u64, DbError>;
    async fn update_group_token(
        &self,
        id: i64,
        scope: &ResourceScope,
        new_token: &str,
    ) -> Result<u64, DbError>;
    /// v1.0.4: count how many forward_rules reference this group via
    /// device_group_in, device_group_out, or fallback_group. Used as a
    /// pre-delete safety check so the admin sees a clear 409 instead of
    /// a cryptic FK violation or orphaned references.
    async fn count_rules_by_group(&self, id: i64) -> Result<i64, DbError>;
    async fn delete_group(&self, id: i64, scope: &ResourceScope) -> Result<u64, DbError>;
    async fn delete_groups_by_uid(&self, uid: i64) -> Result<u64, DbError>;
    /// v1.0.8: list all inbound device groups (group_type = 'in'). Used by the
    /// purchase flow to compute the authorized set when grant_all_groups=true
    /// — in that mode the user gains access to every inbound group, so rules
    /// bound to inbound groups are NOT paused.
    async fn list_all_inbound_group_ids(&self) -> Result<Vec<i64>, DbError>;
}

// ── v1.0.7: per-user device-group authorization ──
// Replaces the v1.0.4 user-permission-group layer (user → named group →
// device-group allowlist) with a direct user ↔ device_group link plus a
// per-user `all_device_groups` flag. Admins are always treated as all-allowed.

#[async_trait]
pub trait DeviceGroupAuthRepository: Send + Sync {
    /// List the device-group IDs explicitly assigned to this user (the raw
    /// `user_device_groups` rows). Does NOT expand `all_device_groups`; use
    /// `authorized_device_group_ids` for the effective set. For the admin UI.
    async fn list_user_device_groups(&self, user_id: i64) -> Result<Vec<i64>, DbError>;
    /// Replace a user's explicit device-group assignments (clear + re-insert).
    async fn set_user_device_groups(
        &self,
        user_id: i64,
        device_group_ids: &[i64],
    ) -> Result<(), DbError>;
    /// Set the per-user `all_device_groups` flag. Returns rows affected
    /// (0 = user not found).
    async fn set_user_all_device_groups(&self, user_id: i64, all: bool) -> Result<u64, DbError>;
    /// Effective set of inbound ('in') device-group IDs the user may use:
    /// admins and `all_device_groups` users get ALL 'in' groups; everyone else
    /// gets only their explicit assignments. Empty = cannot forward.
    async fn authorized_device_group_ids(&self, user_id: i64) -> Result<Vec<i64>, DbError>;
    /// v1.0.4: pause all of `user_id`'s rules whose device_group_in is NOT in
    /// `allowed_group_ids` (the user lost authorization for that group). Rules
    /// are paused, never deleted, so an admin can re-authorize and resume them.
    /// An empty `allowed_group_ids` pauses ALL the user's rules. Returns the
    /// number of rules newly paused (0 = nothing to do, skip node broadcast).
    async fn pause_rules_outside_groups(
        &self,
        user_id: i64,
        allowed_group_ids: &[i64],
    ) -> Result<u64, DbError>;
    /// Whether the user is subject to device-group restriction — i.e. a
    /// non-admin without `all_device_groups`. The rule API uses this to decide
    /// whether to enforce the allowlist. Admins / all-device-groups users → false.
    async fn is_user_restricted(&self, user_id: i64) -> Result<bool, DbError>;
}

// ── Tunnel Profile ──

#[async_trait]
pub trait TunnelProfileRepository: Send + Sync {
    async fn list_profiles(&self, scope: &ProfileScope) -> Result<Vec<TunnelProfile>, DbError>;
    async fn find_builtin_flag_by_id(
        &self,
        id: i64,
        scope: &ResourceScope,
    ) -> Result<Option<bool>, DbError>;
    async fn find_by_name(&self, name: &str) -> Result<Option<TunnelProfile>, DbError>;
    /// Look up a full profile row by id, scoped by builtin-ness (NOT ownership —
    /// see [`ProfileScope`]). `None` = no such profile OR outside scope
    /// (indistinguishable, so a caller can't tell "exists but foreign" from
    /// "doesn't exist").
    ///
    /// v0.4.10 fix: the scope was previously `ResourceScope` (owner-based), but
    /// tunnel-profile isolation is by builtin-ness per the v0.4.10 roadmap §5:
    /// a regular user may bind ONLY builtin profiles, an admin may bind any.
    /// The scoping decision is made by the CALLER based on the RULE OWNER's
    /// role (not the operator's), so an admin creating a rule on behalf of a
    /// regular user is still restricted to builtin profiles for that rule.
    /// Internal consistency checks (e.g. protocol-vs-bound-profile validation)
    /// and node-config builds use `ProfileScope::All` since they must resolve
    /// the real binding without leaking existence to the user.
    async fn find_profile_by_id(
        &self,
        id: i64,
        scope: &ProfileScope,
    ) -> Result<Option<TunnelProfile>, DbError>;
    /// Count rules currently bound to this profile (for delete-protection),
    /// scoped.
    async fn count_rules_by_profile(
        &self,
        profile_id: i64,
        scope: &ResourceScope,
    ) -> Result<i64, DbError>;
    /// List the stored protocols of rules bound to this profile (for
    /// transport-change validation: a new transport must be compatible with
    /// every referencing rule's protocol), scoped.
    async fn list_rule_protocols_by_profile(
        &self,
        profile_id: i64,
        scope: &ResourceScope,
    ) -> Result<Vec<String>, DbError>;
    #[allow(clippy::too_many_arguments)]
    async fn insert_profile(
        &self,
        name: &str,
        transport: &str,
        tls_mode: &str,
        ws_path: &str,
        host_header: &str,
        sni: &str,
        uid: i64,
    ) -> Result<(), DbError>;
    #[allow(clippy::too_many_arguments)]
    async fn update_profile_fields(
        &self,
        id: i64,
        scope: &ResourceScope,
        name: Option<&str>,
        transport: Option<&str>,
        tls_mode: Option<&str>,
        ws_path: Option<&str>,
        host_header: Option<&str>,
        sni: Option<&str>,
    ) -> Result<u64, DbError>;
    async fn delete_profile(&self, id: i64, scope: &ResourceScope) -> Result<u64, DbError>;
}

// ── Traffic (atomic batch) ──

/// Outcome of a traffic batch.
///
/// SECURITY (v0.4.9): the per-entry result deliberately does NOT distinguish
/// "rule doesn't exist" from "rule belongs to another group". A node holding a
/// valid group token could otherwise enumerate rule_ids and tell, from the
/// response, whether a given id exists in another group (a rule-id existence
/// oracle). Both cases now produce `Unavailable`, which the caller maps to a
/// single uniform 403 with a generic message. The specific reason (missing vs
/// foreign) is logged server-side only.
#[derive(Debug)]
pub enum TrafficEntryResult {
    /// The batch was applied successfully.
    Ok,
    /// At least one entry referenced a rule that is NOT available to this node
    /// — either it does not exist, or it belongs to a different group. The
    /// whole batch is rolled back; the caller returns a uniform 403. The
    /// real rule_id / reason are logged, never returned to the node.
    Unavailable,
    /// At least one entry would overflow an i64 traffic counter (per-rule
    /// cumulative, per-user cumulative, or existing value + delta). The whole
    /// batch is rolled back; the caller returns a uniform 400.
    Overflow,
}

#[async_trait]
pub trait TrafficRepository: Send + Sync {
    /// Apply a batch of traffic entries atomically in ONE transaction.
    ///
    /// Contract (v0.4.9 hardened):
    ///   - Ownership is checked with a SINGLE query
    ///     `SELECT id, uid FROM forward_rules WHERE id = ? AND device_group_in = ?`.
    ///     A miss (rule missing OR foreign) → `Unavailable`; the whole batch is
    ///     rolled back. There is NO second "does this id exist elsewhere?"
    ///     query — that was the rule-id existence oracle.
    ///   - Duplicate rule_ids in one batch are AGGREGATED first (summed), so
    ///     the per-rule overflow check sees the batch's true cumulative delta.
    ///   - Overflow is checked with checked arithmetic for: each rule's
    ///     (existing traffic_used + batch delta) and each user's
    ///     (existing traffic_used + sum of their rules' deltas). Any overflow →
    ///     `Overflow`, whole batch rolled back.
    ///   - upload/download arrive as u64 but are converted to i64 with an
    ///     overflow guard (values > i64::MAX are rejected before any write).
    ///   - On any rejection the transaction is rolled back — NO partial update
    ///     of rules or users.
    ///
    /// Returns `Ok(vec![result])` even on the rejected paths (the single
    /// result element tells the caller which rejection happened); `Err` only
    /// for a genuine DB failure.
    async fn apply_traffic_batch(
        &self,
        group_id: i64,
        entries: &[TrafficEntry],
    ) -> Result<Vec<TrafficEntryResult>, DbError>;
}

// ── KVS (generic key-value) ──

#[async_trait]
pub trait KvsRepository: Send + Sync {
    async fn get(&self, key: &str) -> Result<Option<String>, DbError>;
    async fn set(&self, key: &str, value: &str) -> Result<(), DbError>;
    async fn delete(&self, key: &str) -> Result<u64, DbError>;
    async fn scan_prefix(&self, prefix: &str) -> Result<Vec<(String, String)>, DbError>;
}

// ── Statistics ──

#[async_trait]
pub trait StatisticsRepository: Send + Sync {
    async fn query_stats(
        &self,
        stat_type: Option<&str>,
        stat_key: Option<&str>,
        from: Option<&str>,
        to: Option<&str>,
    ) -> Result<Vec<Statistic>, DbError>;
}

// ── Plan ──

#[async_trait]
pub trait PlanRepository: Send + Sync {
    async fn list_plans(&self) -> Result<Vec<Plan>, DbError>;
    /// v1.0.8: plans visible to regular users for self-purchase (hidden = 0).
    async fn list_visible_plans(&self) -> Result<Vec<Plan>, DbError>;
    /// Look up a plan's name by id. None = no such plan. Used by /user/me to
    /// project the user's plan_id into a human-readable plan_name without
    /// exposing other plan columns.
    async fn find_plan_name_by_id(&self, id: i64) -> Result<Option<String>, DbError>;
    /// v1.0.8: fetch a full plan row by id (for purchase validation). None =
    /// no such plan (or hidden, when buying — gated by the caller).
    async fn find_plan_by_id(&self, id: i64) -> Result<Option<Plan>, DbError>;
    /// v1.0.8: create a plan. Returns the new row's id.
    #[allow(clippy::too_many_arguments)]
    async fn insert_plan(
        &self,
        name: &str,
        max_rules: i32,
        traffic: i64,
        price: &str,
        plan_type: &str,
        duration_days: i32,
        hidden: bool,
        reset_traffic: bool,
        description: &str,
        grant_all_groups: bool,
    ) -> Result<i64, DbError>;
    /// v1.0.8: update a plan's mutable fields. Returns rows affected (0 = not
    /// found). speed_limit/ip_limit are intentionally NOT updatable here
    /// (placeholders, never enforced) to keep the API surface minimal.
    #[allow(clippy::too_many_arguments)]
    async fn update_plan_fields(
        &self,
        id: i64,
        name: Option<&str>,
        max_rules: Option<i32>,
        traffic: Option<i64>,
        price: Option<&str>,
        plan_type: Option<&str>,
        duration_days: Option<i32>,
        hidden: Option<bool>,
        reset_traffic: Option<bool>,
        description: Option<&str>,
        grant_all_groups: Option<bool>,
    ) -> Result<u64, DbError>;
    /// v1.0.8: delete a plan. Returns rows affected (0 = not found).
    async fn delete_plan(&self, id: i64) -> Result<u64, DbError>;
    /// v1.0.8: count users whose plan_id points at this plan. Used as a
    /// pre-delete safety check (count > 0 → 409).
    async fn count_users_on_plan(&self, plan_id: i64) -> Result<i64, DbError>;

    /// v1.0.9: list the device-group ids this plan grants on purchase.
    async fn list_plan_device_groups(&self, plan_id: i64) -> Result<Vec<i64>, DbError>;
    /// v1.0.9: REPLACE the plan's grant set (delete-then-insert, deduped). Used
    /// by the admin create/update plan handlers.
    async fn set_plan_device_groups(
        &self,
        plan_id: i64,
        device_group_ids: &[i64],
    ) -> Result<(), DbError>;

    /// v1.0.8: atomically purchase a plan in ONE transaction (防双花):
    ///   - lock + read the user's balance
    ///   - refuse if balance < price_cents (returns `BuyPlanError::InsufficientBalance`)
    ///   - balance -= price_cents, traffic_limit += traffic_to_add
    ///   - max_rules = plan_max_rules, plan_id = plan_id
    ///   - reset traffic_used to 0 when `reset_traffic`
    ///   - plan_expire_at = max(now, current expiry) + duration_days (NULL when duration_days=0)
    ///   - insert an orders row (snapshots plan_name + price)
    ///   - v1.0.9: grant device groups in the SAME tx. v1.0.8: purchase REPLACES
    ///     authorization — when `grant_all_groups` set all_device_groups=1 (and
    ///     clear explicit rows); else reset all_device_groups=0 and replace
    ///     user_device_groups with the plan's `device_group_ids`. Rules bound to
    ///     groups outside `new_authorized_group_ids` are paused in the same tx.
    /// All on the same tx handle so a concurrent purchase can't double-spend.
    /// `price_cents` / `traffic_to_add` / `plan_max_rules` / `duration_days` are
    /// resolved by the caller from the plan row (and re-checked hidden=0 there),
    /// so this method trusts them and only owns the atomic money + bookkeeping.
    #[allow(clippy::too_many_arguments)]
    async fn buy_plan(
        &self,
        user_id: i64,
        plan_id: i64,
        plan_name: &str,
        price_cents: i64,
        traffic_to_add: i64,
        plan_max_rules: i32,
        duration_days: i32,
        reset_traffic: bool,
        grant_all_groups: bool,
        device_group_ids: &[i64],
        // v1.0.8: the NEW authorized group set AFTER purchase. Used inside the
        // transaction to pause rules outside this set (replacement semantics).
        // Computed by the caller: all inbound groups if grant_all_groups, else
        // device_group_ids (the plan's grants).
        new_authorized_group_ids: &[i64],
    ) -> Result<(), BuyPlanError>;
}

/// v1.0.8: errors from the atomic purchase transaction.
#[derive(Debug)]
pub enum BuyPlanError {
    /// User balance < plan price. Caller → 400.
    InsufficientBalance,
    /// DB error. Caller → 500.
    Database(DbError),
}

impl From<DbError> for BuyPlanError {
    fn from(e: DbError) -> Self {
        BuyPlanError::Database(e)
    }
}

impl From<sqlx::Error> for BuyPlanError {
    fn from(e: sqlx::Error) -> Self {
        BuyPlanError::Database(DbError::from(e))
    }
}

// ── App settings (registration config) ──

/// The registration settings row (always id=1 in app_settings).
/// v0.4.21 PR2: added allowed_plan_ids for multi-plan registration support.
#[derive(Debug, Clone, Serialize)]
pub struct RegistrationSettings {
    pub registration_enabled: bool,
    pub default_registration_plan_id: i64,
    pub allowed_plan_ids: Vec<i64>,
}

/// v0.4.10 PR3: registration settings stored in the `app_settings` single-row
/// table (NOT env vars, NOT kvs). The env var REGISTRATION_ENABLED only seeds
/// the row once on first boot via [`insert_settings_if_absent`]; after that
/// the admin owns the value via PUT /admin/settings/registration and the env
/// var is never consulted again.
#[async_trait]
pub trait SettingsRepository: Send + Sync {
    /// Read the registration settings row. `None` = the row hasn't been seeded
    /// yet (fresh DB before main.rs's first insert_settings_if_absent pass).
    async fn get_registration_settings(&self) -> Result<Option<RegistrationSettings>, DbError>;
    /// Atomically insert the settings row ONLY if it does not already exist.
    /// If a row is present this is a no-op (the env-var seed value is NOT
    /// applied over an admin-configured row). This is the sole path by which
    /// REGISTRATION_ENABLED enters the database.
    async fn insert_settings_if_absent(
        &self,
        enabled: bool,
        default_plan_id: i64,
        allowed_plan_ids: &[i64],
    ) -> Result<(), DbError>;
    /// Atomic upsert (INSERT ... ON CONFLICT DO UPDATE). Used by the admin
    /// PUT endpoint: creates the row if missing, overwrites if present, with
    /// no observable intermediate state under concurrent admin requests.
    async fn set_registration_settings(
        &self,
        enabled: bool,
        default_plan_id: i64,
        allowed_plan_ids: &[i64],
    ) -> Result<(), DbError>;
}

// ── Aggregate ──

/// v1.0.8: purchase-order history.
#[async_trait]
pub trait OrderRepository: Send + Sync {
    /// List a user's orders, newest first.
    async fn list_orders_by_user(&self, user_id: i64) -> Result<Vec<Order>, DbError>;
    /// Insert an order row (snapshots plan_name + price). Used inside the
    /// purchase transaction.
    async fn insert_order(
        &self,
        user_id: i64,
        plan_id: Option<i64>,
        plan_name: &str,
        price: &str,
    ) -> Result<(), DbError>;
}

/// The aggregate repository trait. Handlers depend on `Arc<dyn Repository>`
/// and get access to all domain-specific methods.
#[async_trait]
pub trait Repository:
    UserRepository
    + RuleRepository
    + GroupRepository
    + DeviceGroupAuthRepository
    + TunnelProfileRepository
    + TrafficRepository
    + KvsRepository
    + StatisticsRepository
    + PlanRepository
    + SettingsRepository
    + OrderRepository
    + Send
    + Sync
{
}
