use crate::api::system::ReleaseCache;
use crate::config::Config;
use crate::db::repo::Repository;
use axum::Router;
use std::sync::Arc;

pub mod admin;
pub mod auth;
pub mod diagnose;
pub mod geoip;
pub mod groups;
pub mod middleware;
pub mod node;
pub mod stats;
pub mod system;
pub mod ws;

#[derive(Clone)]
pub struct AppState {
    /// v0.4.3: data access goes through the Repository trait, not a raw
    /// `SqlitePool`. PR1 wires `Arc<SqliteRepository>` here; PR2 will switch
    /// this to `Arc<PgRepository>` for the Postgres backend without changing
    /// a single handler.
    pub db: Arc<dyn Repository>,
    pub config: Config,
    pub release_cache: ReleaseCache,
    pub node_connections: ws::NodeConnections,
    /// v0.4.8: in-memory rule-diagnosis task registry (request_id → run).
    pub diagnose: diagnose::DiagnoseRegistry,
    /// v0.4.15: GeoIP concurrent-lookup de-duplication (set of IPs being
    /// fetched right now). Shared across all report_status handlers.
    pub geoip_in_flight: Arc<tokio::sync::Mutex<std::collections::HashSet<String>>>,
}

pub fn routes() -> Router<AppState> {
    Router::new()
        // Auth
        .route("/auth/login", axum::routing::post(auth::login))
        .route("/auth/register", axum::routing::post(auth::register))
        // v0.4.10 PR3: public (unauthenticated) registration-status probe.
        .route(
            "/auth/registration-status",
            axum::routing::get(auth::registration_status),
        )
        // Any authenticated user can change their own password
        .route("/user/password", axum::routing::put(admin::change_password))
        // Any authenticated user can read their own account info (no password)
        .route("/user/me", axum::routing::get(admin::get_me))
        // v1.0.8: self-service plan purchase + order history.
        .route("/user/buy-plan", axum::routing::post(admin::buy_plan))
        .route("/user/orders", axum::routing::get(admin::list_my_orders))
        // v1.0.8: public plan list (hidden excluded) for the shop.
        .route("/plans", axum::routing::get(admin::list_public_plans))
        // Admin
        .route(
            "/admin/users",
            axum::routing::get(admin::list_users).post(admin::create_user),
        )
        .route(
            "/admin/users/{id}",
            axum::routing::put(admin::update_user).delete(admin::delete_user),
        )
        // v0.3.4: reset a user's traffic_used + all their rules' traffic_used
        // (atomic). Separate POST so a PUT to /{id} can't accidentally zero it.
        .route(
            "/admin/users/{id}/reset-traffic",
            axum::routing::post(admin::reset_user_traffic),
        )
        // v0.4.10 PR4: admin sets a (temporary) password for a user. Separate
        // route so a PUT to /{id} (quota/ban edits) can never touch passwords.
        .route(
            "/admin/users/{id}/password",
            axum::routing::put(admin::reset_user_password),
        )
        // v0.4.10: rules are now owner-scoped and usable by any authenticated
        // user (the handler folds the caller's ResourceScope into every query).
        // Moved out of /admin/* — the path no longer implies an admin guard.
        .route(
            "/rules",
            axum::routing::get(admin::list_rules).post(admin::create_rule),
        )
        .route(
            "/rules/{id}",
            axum::routing::put(admin::update_rule).delete(admin::delete_rule),
        )
        // v0.4.8: rule diagnosis — on-demand probe of a rule's targets from
        // the node's vantage point (side-channel, doesn't count as traffic).
        // v0.4.10: owner-scoped; a user may diagnose only their own rules.
        .route(
            "/rules/{id}/diagnose",
            axum::routing::post(diagnose::diagnose_rule),
        )
        // v0.4.12: device groups are admin-only shared infrastructure again.
        // Writes use the AdminOnly guard; the service layer operates with
        // ResourceScope::All rather than owner-scoped mutations.
        .route(
            "/groups",
            axum::routing::get(admin::list_groups).post(admin::create_group),
        )
        .route(
            "/groups/{id}",
            axum::routing::put(admin::update_group).delete(admin::delete_group),
        )
        // Rotate a group's node token (revokes the old one; broadcasts
        // config_changed so nodes re-authenticate). Owner-scoped.
        .route(
            "/groups/{id}/rotate-token",
            axum::routing::post(admin::rotate_group_token),
        )
        // v0.4.11 PR3: shared infrastructure — regular users can discover and
        // bind rules to inbound groups owned by an admin.
        .route(
            "/groups/shared",
            axum::routing::get(groups::list_shared_groups),
        )
        // v1.0.7: per-user device-group authorization (replaces user-groups).
        // GET preloads a user's current assignment for the editor; updates go
        // through PUT /users/{id} (update_user) carrying device_group_ids /
        // all_device_groups.
        .route(
            "/admin/users/{id}/device-groups",
            axum::routing::get(admin::get_user_device_groups),
        )
        .route(
            "/nodes/shared",
            axum::routing::get(groups::list_shared_node_summary),
        )
        // v0.4.0: tunnel profile catalog. v0.4.10: the GET list is readable by
        // any authenticated user (admins see all profiles; regular users see
        // only the builtin catalog they can bind to). WRITES stay admin-only on
        // /admin/tunnel-profiles — template authoring is an admin power.
        // v0.4.11 PR1: /tunnel-profiles returns available templates (ws/tls_simple,
        // builtin + admin custom) for rule selection. /admin/tunnel-profiles GET
        // returns only manageable custom templates (is_builtin=false) for the
        // admin management page.
        .route(
            "/tunnel-profiles",
            axum::routing::get(admin::list_tunnel_profiles),
        )
        .route(
            "/admin/tunnel-profiles",
            axum::routing::get(admin::list_admin_tunnel_profiles)
                .post(admin::create_tunnel_profile),
        )
        .route(
            "/admin/tunnel-profiles/{id}",
            axum::routing::put(admin::update_tunnel_profile).delete(admin::delete_tunnel_profile),
        )
        .route(
            "/admin/plans",
            axum::routing::get(admin::list_plans)
                .post(admin::create_plan),
        )
        .route(
            "/admin/plans/{id}",
            axum::routing::put(admin::update_plan).delete(admin::delete_plan),
        )
        // v0.4.10 PR3: admin-managed registration settings (read + update).
        .route(
            "/admin/settings/registration",
            axum::routing::get(admin::get_registration_settings)
                .put(admin::update_registration_settings),
        )
        // Stats & monitoring
        .route("/stats", axum::routing::get(stats::get_stats))
        // v0.4.10: node status is owner-scoped (a user sees only nodes for
        // groups they own). Renamed /node_status → /nodes.
        .route("/nodes", axum::routing::get(stats::get_node_status))
        // v0.4.10: manually delete a node status record (owner-scoped — the
        // caller must own the group). Renamed /node_status/{id} → /nodes/{id}.
        .route(
            "/nodes/{group_id}",
            axum::routing::delete(stats::delete_node_status),
        )
        // System
        .route("/system/version", axum::routing::get(system::get_version))
        // Public, unauthenticated health probe (status + version only). Used by
        // deploy.sh and external monitors; NOT behind AdminOnly.
        .route("/health", axum::routing::get(system::health))
        // WebSocket (node control channel)
        .route("/node/ws", axum::routing::get(ws::node_ws_handler))
        // Node (polled by relay-node binary)
        .route("/node/config", axum::routing::get(node::get_config))
        .route(
            "/node/report_traffic",
            axum::routing::post(node::report_traffic),
        )
        .route(
            "/node/report_status",
            axum::routing::post(node::report_status),
        )
        // v0.4.8: node reports diagnosis probe results back (authenticated by
        // NODE_TOKEN, same channel as report_status).
        .route(
            "/node/diagnose_result",
            axum::routing::post(diagnose::receive_diagnose_result),
        )
}
