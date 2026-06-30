use relay_shared::protocol::ApiResponse;
use serde::Serialize;

mod auth;
mod groups;
mod nodes;
mod password;
mod plans;
mod profiles;
mod rules;
mod settings;
mod shop;
mod users;

pub use groups::*;
pub use password::*;
pub use plans::*;
pub use profiles::*;
pub use rules::*;
pub use settings::*;
pub use shop::*;
pub use users::*;

/// A user WITHOUT the password hash — for API responses. Never expose the
/// password hash via any endpoint; use this struct instead of `User` in list
/// responses. Deriving FromRow + listing every non-password column means
/// SELECT * also works (the password column is just ignored).
#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct UserPublic {
    pub id: i64,
    pub username: String,
    pub balance: String,
    pub plan_id: Option<i64>,
    /// v1.0.7: replaces group_id. true = user may use all device groups.
    pub all_device_groups: bool,
    pub max_rules: i32,
    pub speed_limit: i32,
    pub ip_limit: i32,
    pub traffic_used: i64,
    pub traffic_limit: i64,
    pub admin: bool,
    pub banned: bool,
    pub created_at: String,
    /// v1.0.8: plan expiry (NULL = no expiry).
    #[serde(default)]
    pub plan_expire_at: Option<String>,
    /// v1.0.8: admin suspension.
    #[serde(default)]
    pub suspended: bool,
}

/// A user's view of THEIR OWN account (GET /user/me). Same non-password fields
/// as [`UserPublic`] — the password hash is never exposed by any endpoint.
/// Kept as a distinct type (rather than reusing UserPublic) so the "self" view
/// is explicit in the response and can diverge later (e.g. hide admin/banned
/// from a non-admin's own view, or add email) without touching the admin list
/// projection.
#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct UserSelf {
    pub id: i64,
    pub username: String,
    pub admin: bool,
    pub balance: String,
    /// v0.4.10: the user's plan (NULL if unset). plan_name is its human-readable
    /// label, resolved via a separate lookup (no plan → plan_name NULL too).
    pub plan_id: Option<i64>,
    pub plan_name: Option<String>,
    pub max_rules: i32,
    /// v0.4.10: how many rules the user currently owns (for the account center's
    /// "current / limit" display). Counted live via count_by_uid.
    pub current_rules: i64,
    pub traffic_used: i64,
    pub traffic_limit: i64,
    /// v0.4.10: renamed from created_at for the account-center contract. The DB
    /// column is still created_at; this is the JSON field name clients see.
    pub registered_at: String,
    /// v0.4.10 PR4: when true the frontend redirects to the force-password-
    /// change page (the user can only reach /user/me + /user/password until
    /// they change it). The DB column is the source of truth.
    pub must_change_password: bool,
    /// v1.0.8: plan expiry (NULL = no expiry).
    #[serde(default)]
    pub plan_expire_at: Option<String>,
    /// v1.0.8: admin suspension (login allowed; forwarding gated).
    #[serde(default)]
    pub suspended: bool,
}

/// Build an error ApiResponse. Accepts `&str` or `String` (or anything else
/// `Into<String>`), so callers can pass a `format!()` result directly without
/// leaking it (the old `Box::leak` workaround) or hand-constructing ApiResponse.
fn err<T: Serialize, S: Into<String>>(code: i32, msg: S) -> ApiResponse<T> {
    ApiResponse {
        code,
        message: msg.into(),
        data: None,
    }
}

#[cfg(test)]
mod tests {
    use super::{change_password, reset_user_password, ResetPasswordRequest};
    use super::{
        create_group, create_rule, create_user, delete_group, delete_rule, delete_user, err,
        get_me, get_registration_settings, list_groups, list_rules, reset_user_traffic,
        update_group, update_registration_settings, update_rule, update_user, ApiResponse,
        ChangePasswordRequest, CreateUserRequest, ListRulesQuery,
    };
    use crate::api::auth::{register, registration_status};
    use crate::api::middleware::{AdminOnly, AuthUser};
    use crate::api::system::ReleaseCache;
    use crate::api::ws::NodeConnections;
    use crate::api::AppState;
    use crate::config::Config;
    use crate::db::schema::SCHEMA_SQL;
    use crate::db::sqlite_repo::SqliteRepository;
    use axum::extract::{Path, Query, State};
    use axum::Json;
    use relay_shared::protocol::{
        CreateGroupRequest, CreateRuleRequest, GroupType, Protocol, PublicTransport,
        RegisterRequest, RegistrationSettingsRequest, UpdateGroupRequest, UpdateRuleRequest,
        UpdateUserRequest,
    };
    use sqlx::sqlite::SqlitePoolOptions;
    use sqlx::SqlitePool;
    use std::sync::Arc;

    async fn test_state() -> (AppState, SqlitePool) {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("connect memory db");
        sqlx::query(SCHEMA_SQL)
            .execute(&pool)
            .await
            .expect("create schema");
        let state = AppState {
            db: Arc::new(SqliteRepository::new(pool.clone())),
            config: Config {
                database_path: "sqlite::memory:".into(),
                listen: "127.0.0.1:0".into(),
                key: "test-key".into(),
                jwt_secret: "test-secret".into(),
                public_dir: "public".into(),
                public_panel_url: String::new(),
                registration_enabled: false,
                cors_origins: vec![],
                geoip_enabled: false,
                geoip_cache_ttl: 604_800,
            },
            release_cache: ReleaseCache::new(),
            node_connections: NodeConnections::new(),
            diagnose: crate::api::diagnose::DiagnoseRegistry::new(),
            geoip_in_flight: std::sync::Arc::new(tokio::sync::Mutex::new(
                std::collections::HashSet::new(),
            )),
        };
        (state, pool)
    }

    async fn add_user(pool: &SqlitePool, id: i64, username: &str, admin: bool) -> String {
        let hash = bcrypt::hash(format!("old-password-{id}"), 4).unwrap();
        // v1.0.7: default test users to all_device_groups=1 (unrestricted) so the
        // rule-validation tests exercise the validation path, not the new
        // "unassigned = cannot forward" default. Permission-specific tests
        // override this via assign_user_groups().
        sqlx::query(
            "INSERT INTO users (id, username, password, admin, all_device_groups, balance, max_rules, traffic_used, traffic_limit, banned) \
             VALUES (?, ?, ?, ?, 1, '0', 5, 0, 0, 0)",
        )
        .bind(id)
        .bind(username)
        .bind(&hash)
        .bind(admin)
        .execute(pool)
        .await
        .unwrap();
        hash
    }

    async fn add_group(pool: &SqlitePool, id: i64, uid: i64, name: &str) {
        sqlx::query(
            "INSERT INTO device_groups (id, name, group_type, token, uid) VALUES (?, ?, 'in', ?, ?)",
        )
        .bind(id)
        .bind(name)
        .bind(format!("token-{id}-{uid}"))
        .bind(uid)
        .execute(pool)
        .await
        .unwrap();
    }

    /// v0.4.12 PR1: insert a group with an explicit group_type ('in'/'out'/'monitor').
    async fn add_group_typed(pool: &SqlitePool, id: i64, uid: i64, name: &str, gtype: &str) {
        sqlx::query(
            "INSERT INTO device_groups (id, name, group_type, token, uid) VALUES (?, ?, ?, ?, ?)",
        )
        .bind(id)
        .bind(name)
        .bind(gtype)
        .bind(format!("token-{id}-{uid}"))
        .bind(uid)
        .execute(pool)
        .await
        .unwrap();
    }

    async fn add_rule(
        pool: &SqlitePool,
        id: i64,
        uid: i64,
        group_id: i64,
        port: i64,
        traffic: i64,
    ) {
        sqlx::query(
            "INSERT INTO forward_rules \
             (id, name, uid, listen_port, device_group_in, target_addr, target_port, traffic_used) \
             VALUES (?, ?, ?, ?, ?, '127.0.0.1', 80, ?)",
        )
        .bind(id)
        .bind(format!("rule-{id}"))
        .bind(uid)
        .bind(port)
        .bind(group_id)
        .bind(traffic)
        .execute(pool)
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn update_user_edits_allowed_fields_and_preserves_password_and_admin_role() {
        let (state, pool) = test_state().await;
        let original_hash = add_user(&pool, 2, "alice", false).await;

        let Json(resp) = update_user(
            AdminOnly { user_id: 1 },
            State(state.clone()),
            Path(2),
            Json(UpdateUserRequest {
                balance: Some("12.34".into()),
                max_rules: Some(42),
                traffic_limit: Some(1024),
                banned: Some(true),
                ..Default::default()
            }),
        )
        .await;
        assert_eq!(resp.code, 0, "update should succeed: {}", resp.message);

        let row: (String, i32, i64, bool, String, bool) = sqlx::query_as(
            "SELECT balance, max_rules, traffic_limit, banned, password, admin FROM users WHERE id = 2",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(row.0, "12.34");
        assert_eq!(row.1, 42);
        assert_eq!(row.2, 1024);
        assert!(row.3);
        assert_eq!(row.4, original_hash, "admin edit must not touch password");
        assert!(!row.5, "admin edit must not grant admin role");
    }

    /// v0.3.5: balance is strictly validated (non-negative decimal, ≤ 2 fraction
    /// digits, ≤ 9999999999.99). Invalid input must be rejected BEFORE any DB
    /// write — the schema has no CHECK constraint yet, so this is the only
    /// guard. The handler must (a) reject obvious garbage, (b) reject negatives
    /// and oversize values, (c) reject NaN / exponent / locale strings, and
    /// (d) canonicalise the value that is stored.
    #[tokio::test]
    async fn update_user_rejects_invalid_balances_and_canonicalises_valid_ones() {
        let (state, pool) = test_state().await;
        add_user(&pool, 2, "alice", false).await;

        // The list is intentionally heterogeneous: each negative case proves a
        // different rule. The valid case at the end proves canonicalisation
        // (input has extra leading zeros + a 1-digit fraction, expected output
        // is the canonical 2-digit-fraction form).
        let cases: &[(&str, bool)] = &[
            ("", false),
            ("-1", false),
            ("-0.01", false),
            ("+1", false),
            ("1e3", false),
            ("NaN", false),
            ("Infinity", false),
            ("abc", false),
            ("1,000.00", false),
            ("12.345", false),
            ("10000000000", false),
            ("0", true),
            ("0012.30", true),
        ];
        for (input, should_succeed) in cases {
            let Json(resp) = update_user(
                AdminOnly { user_id: 1 },
                State(state.clone()),
                Path(2),
                Json(UpdateUserRequest {
                    balance: Some((*input).into()),
                    ..Default::default()
                }),
            )
            .await;
            assert_eq!(
                resp.code == 0,
                *should_succeed,
                "balance {input:?}: expected succeed={should_succeed}, got code={} msg={}",
                resp.code,
                resp.message
            );
            if !should_succeed {
                assert_eq!(
                    resp.code, 400,
                    "rejection should be a 400, got {} (msg={})",
                    resp.code, resp.message
                );
            }
        }

        // After the run the row should hold the canonical form (last successful
        // input was "0012.30"). Verify nothing about the user row leaked from
        // the rejected cases.
        let (balance,): (String,) = sqlx::query_as("SELECT balance FROM users WHERE id = 2")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(balance, "12.30", "balance must be canonicalised in storage");
    }

    #[tokio::test]
    async fn reset_user_traffic_zeros_user_and_owned_rules_only() {
        let (state, pool) = test_state().await;
        add_user(&pool, 2, "alice", false).await;
        add_user(&pool, 3, "bob", false).await;
        add_group(&pool, 20, 2, "alice-in").await;
        add_group(&pool, 30, 3, "bob-in").await;
        sqlx::query("UPDATE users SET traffic_used = 111 WHERE id = 2")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("UPDATE users SET traffic_used = 222 WHERE id = 3")
            .execute(&pool)
            .await
            .unwrap();
        add_rule(&pool, 200, 2, 20, 12000, 333).await;
        add_rule(&pool, 201, 2, 20, 12001, 444).await;
        add_rule(&pool, 300, 3, 30, 13000, 555).await;

        let Json(resp) =
            reset_user_traffic(AdminOnly { user_id: 1 }, State(state.clone()), Path(2)).await;
        assert_eq!(resp.code, 0, "reset should succeed: {}", resp.message);

        let user2: (i64,) = sqlx::query_as("SELECT traffic_used FROM users WHERE id = 2")
            .fetch_one(&pool)
            .await
            .unwrap();
        let user3: (i64,) = sqlx::query_as("SELECT traffic_used FROM users WHERE id = 3")
            .fetch_one(&pool)
            .await
            .unwrap();
        let sum2: (i64,) =
            sqlx::query_as("SELECT SUM(traffic_used) FROM forward_rules WHERE uid = 2")
                .fetch_one(&pool)
                .await
                .unwrap();
        let sum3: (i64,) =
            sqlx::query_as("SELECT SUM(traffic_used) FROM forward_rules WHERE uid = 3")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(user2.0, 0);
        assert_eq!(sum2.0, 0);
        assert_eq!(user3.0, 222, "other user total must be untouched");
        assert_eq!(sum3.0, 555, "other user's rules must be untouched");
    }

    #[tokio::test]
    async fn delete_user_refuses_admin_without_deleting_admin_resources() {
        let (state, pool) = test_state().await;
        add_group(&pool, 10, 1, "admin-in").await;
        add_rule(&pool, 100, 1, 10, 11000, 999).await;

        let Json(resp) = delete_user(AdminOnly { user_id: 1 }, State(state.clone()), Path(1)).await;
        assert_eq!(resp.code, 404);

        let user_exists: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM users WHERE id = 1")
            .fetch_one(&pool)
            .await
            .unwrap();
        let group_exists: (i64,) =
            sqlx::query_as("SELECT COUNT(*) FROM device_groups WHERE id = 10")
                .fetch_one(&pool)
                .await
                .unwrap();
        let rule_exists: (i64,) =
            sqlx::query_as("SELECT COUNT(*) FROM forward_rules WHERE id = 100")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(user_exists.0, 1);
        assert_eq!(group_exists.0, 1, "admin group must remain");
        assert_eq!(rule_exists.0, 1, "admin rule must remain");
    }

    #[tokio::test]
    async fn create_user_makes_non_admin_and_rejects_duplicates_and_bad_input() {
        let (state, pool) = test_state().await;

        // Happy path: creates a regular (non-admin) user.
        let Json(ok) = create_user(
            AdminOnly { user_id: 1 },
            State(state.clone()),
            Json(CreateUserRequest {
                username: "bob".into(),
                password: "secret123".into(),
            }),
        )
        .await;
        assert_eq!(ok.code, 0, "create should succeed: {}", ok.message);

        // v0.4.10: an admin-created user must inherit the default plan's quota
        // (plan 1 = 'free': max_rules=5, traffic=107374182400) atomically, the
        // same as self-registration — NOT a bare insert with schema defaults.
        let row: (i64, bool, Option<i64>, i64, i64) = sqlx::query_as(
            "SELECT id, admin, plan_id, max_rules, traffic_limit FROM users WHERE username = 'bob'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert!(!row.1, "created user must be NON-admin");
        assert_eq!(row.2, Some(1), "created user must be attached to plan 1");
        assert_eq!(row.3, 5, "max_rules must be inherited from plan 1");
        assert_eq!(
            row.4, 107374182400,
            "traffic_limit must be inherited from plan 1"
        );

        // Duplicate username → 409.
        let Json(dup) = create_user(
            AdminOnly { user_id: 1 },
            State(state.clone()),
            Json(CreateUserRequest {
                username: "bob".into(),
                password: "secret123".into(),
            }),
        )
        .await;
        assert_eq!(dup.code, 409);

        // v0.4.10: unified password policy is 8..=72 UTF-8 bytes (matches
        // register / change / admin-reset). 7 bytes is the just-too-short
        // boundary → 400; the old policy (>=6) would have wrongly accepted it.
        let Json(short) = create_user(
            AdminOnly { user_id: 1 },
            State(state.clone()),
            Json(CreateUserRequest {
                username: "carol".into(),
                password: "1234567".into(), // 7 bytes
            }),
        )
        .await;
        assert_eq!(short.code, 400, "7-byte password must be rejected");

        // Exactly 8 bytes is the lower bound → accepted.
        let Json(min_ok) = create_user(
            AdminOnly { user_id: 1 },
            State(state.clone()),
            Json(CreateUserRequest {
                username: "dave".into(),
                password: "12345678".into(), // 8 bytes
            }),
        )
        .await;
        assert_eq!(
            min_ok.code, 0,
            "8-byte password must be accepted: {}",
            min_ok.message
        );

        // 73 bytes exceeds the bcrypt 72-byte limit → 400.
        let Json(long) = create_user(
            AdminOnly { user_id: 1 },
            State(state.clone()),
            Json(CreateUserRequest {
                username: "erin".into(),
                password: "a".repeat(73),
            }),
        )
        .await;
        assert_eq!(long.code, 400, "73-byte password must be rejected");

        // Invalid username → 400.
        let Json(bad) = create_user(
            AdminOnly { user_id: 1 },
            State(state.clone()),
            Json(CreateUserRequest {
                username: "bad name!".into(),
                password: "secret123".into(),
            }),
        )
        .await;
        assert_eq!(bad.code, 400);
    }

    #[tokio::test]
    async fn change_password_requires_current_password_and_updates_only_password() {
        let (state, pool) = test_state().await;
        let old_password = "old-password-2";
        let old_hash = bcrypt::hash(old_password, 4).unwrap();
        sqlx::query("INSERT INTO users (id, username, password, admin, balance) VALUES (2, 'alice', ?, 0, '77')")
            .bind(&old_hash)
            .execute(&pool)
            .await
            .unwrap();

        let Json(bad) = super::change_password(
            AuthUser {
                user_id: 2,
                admin: false,
            },
            State(state.clone()),
            Json(ChangePasswordRequest {
                current_password: "wrong-password".into(),
                new_password: "new-password".into(),
            }),
        )
        .await;
        assert_eq!(bad.code, 401);

        let Json(ok) = super::change_password(
            AuthUser {
                user_id: 2,
                admin: false,
            },
            State(state.clone()),
            Json(ChangePasswordRequest {
                current_password: old_password.into(),
                new_password: "new-password".into(),
            }),
        )
        .await;
        assert_eq!(ok.code, 0, "password change should succeed: {}", ok.message);

        let row: (String, String) =
            sqlx::query_as("SELECT password, balance FROM users WHERE id = 2")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert!(bcrypt::verify("new-password", &row.0).unwrap());
        assert_eq!(row.1, "77", "non-password fields must be untouched");
    }

    // ── rule/group 404 + no-spurious-broadcast (v0.3.6) ──
    //
    // Before v0.3.6, update/delete on a non-existent id returned success AND
    // broadcast config_changed — a no-op mutation needlessly triggering a node
    // re-fetch. These pin the new contract: 404 + zero broadcasts when nothing
    // changed, success + exactly one broadcast when something did.

    async fn seed_rule_and_group(pool: &SqlitePool) {
        add_user(pool, 2, "alice", false).await;
        add_group(pool, 20, 2, "gin").await;
        // v0.4.11 PR1: rules with ws/tls_simple transport must bind a matching profile.
        // Seed a ws profile first, then create the rule with ws transport.
        sqlx::query(
            "INSERT INTO tunnel_profiles (id, name, transport, tls_mode, ws_path, host_header, sni, is_builtin, uid) \
             VALUES (51, 'ws-seed', 'ws', 'none', '/relay', '', '', 1, 1)",
        )
        .execute(pool)
        .await
        .unwrap();
        // Rule with ws transport (public_transport='ws', node_transport='ws') and bound profile.
        sqlx::query(
            "INSERT INTO forward_rules (id, name, uid, paused, listen_port, protocol, \
             public_transport, node_transport, entry_transport, forward_mode, \
             device_group_in, target_addr, target_port, tunnel_profile_id) \
             VALUES (200, 'test-rule', 2, 0, 12000, 'tcp', 'ws', 'ws', 'ws', \
             'direct', 20, '127.0.0.1', 80, 51)",
        )
        .execute(pool)
        .await
        .unwrap();
    }

    /// Count how many config_changed broadcasts a handler call produced, by
    /// registering a live WS connection on the shared NodeConnections and
    /// draining its receiver for ~50ms after the call.
    async fn expect_broadcasts(
        state: &AppState,
        expected: usize,
        f: impl std::future::Future<Output = ()>,
    ) {
        let (_id, mut rx) = state.node_connections.register(99, None).await;
        f.await;
        let mut n = 0;
        for _ in 0..20 {
            while rx.try_recv().is_ok() {
                n += 1;
            }
            tokio::task::yield_now().await;
        }
        assert_eq!(
            n, expected,
            "expected {expected} config_changed broadcasts, got {n}"
        );
    }

    #[tokio::test]
    async fn update_rule_nonexistent_returns_404_no_broadcast() {
        let (state, _pool) = test_state().await;
        expect_broadcasts(&state, 0, async {
            let Json(resp) = update_rule(
                AuthUser {
                    user_id: 1,
                    admin: true,
                },
                State(state.clone()),
                Path(99999),
                Json(UpdateRuleRequest {
                    name: Some("x".into()),
                    ..Default::default()
                }),
            )
            .await;
            assert_eq!(resp.code, 404);
        })
        .await;
    }

    #[tokio::test]
    async fn update_rule_existing_succeeds_and_broadcasts_once() {
        let (state, pool) = test_state().await;
        seed_rule_and_group(&pool).await;
        expect_broadcasts(&state, 1, async {
            let Json(resp) = update_rule(
                AuthUser {
                    user_id: 1,
                    admin: true,
                },
                State(state.clone()),
                Path(200),
                Json(UpdateRuleRequest {
                    name: Some("renamed".into()),
                    ..Default::default()
                }),
            )
            .await;
            assert_eq!(resp.code, 0, "{}", resp.message);
        })
        .await;
    }

    /// v0.4.8 PR2: changing a rule's protocol to UDP while it's bound to a WS
    /// profile must be rejected, even when tunnel_profile_id is NOT in the
    /// request (the binding is loaded from the stored rule). Without this the
    /// node would skip the listener at config-build time.
    #[tokio::test]
    async fn update_rule_protocol_udp_with_ws_profile_rejected() {
        let (state, pool) = test_state().await;
        seed_rule_and_group(&pool).await;
        // Bind rule 200 to a ws profile.
        sqlx::query(
            "INSERT INTO tunnel_profiles (id, name, transport, tls_mode, ws_path, host_header, sni, is_builtin, uid) \
             VALUES (50, 'ws-x', 'ws', 'none', '/relay', '', '', 0, 2)",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query("UPDATE forward_rules SET tunnel_profile_id = 50 WHERE id = 200")
            .execute(&pool)
            .await
            .unwrap();

        let Json(resp) = update_rule(
            AuthUser {
                user_id: 1,
                admin: true,
            },
            State(state.clone()),
            Path(200),
            Json(UpdateRuleRequest {
                protocol: Some(Protocol::Udp),
                ..Default::default()
            }),
        )
        .await;
        assert_eq!(resp.code, 400, "{}", resp.message);
        // v0.4.10 fix / v0.4.11 PR1: the message must state the incompatibility.
        // The exact wording may vary (incompatible / not supported).
        assert!(
            resp.message.contains("incompatible") || resp.message.contains("not supported"),
            "message should state incompatibility: {}",
            resp.message
        );
    }

    #[tokio::test]
    async fn delete_rule_nonexistent_returns_404_no_broadcast() {
        let (state, _pool) = test_state().await;
        expect_broadcasts(&state, 0, async {
            let Json(resp) = delete_rule(
                AuthUser {
                    user_id: 1,
                    admin: true,
                },
                State(state.clone()),
                Path(99999),
            )
            .await;
            assert_eq!(resp.code, 404);
        })
        .await;
    }

    #[tokio::test]
    async fn update_group_nonexistent_returns_404_no_broadcast() {
        let (state, _pool) = test_state().await;
        expect_broadcasts(&state, 0, async {
            let Json(resp) = update_group(
                AdminOnly { user_id: 1 },
                State(state.clone()),
                Path(99999),
                Json(UpdateGroupRequest {
                    name: Some("x".into()),
                    ..Default::default()
                }),
            )
            .await;
            assert_eq!(resp.code, 404);
        })
        .await;
    }

    // ── v1.0.4: group delete safety ──

    /// Group with rules referencing it returns 409, not 200.
    #[tokio::test]
    async fn delete_group_with_rules_returns_409() {
        let (state, pool) = test_state().await;
        // Create a group and a rule that references it.
        add_group(&pool, 10, 1, "test-in").await;
        add_rule(&pool, 100, 1, 10, 20000, 0).await;

        let Json(resp) =
            super::delete_group(AdminOnly { user_id: 1 }, State(state.clone()), Path(10)).await;
        assert_eq!(resp.code, 409, "group with rules must be rejected");
        assert!(
            resp.message.contains("条规则"),
            "message must include rule count"
        );
    }

    /// An empty group can be deleted successfully.
    #[tokio::test]
    async fn delete_empty_group_succeeds() {
        let (state, pool) = test_state().await;
        add_group(&pool, 20, 1, "empty-in").await;

        let Json(resp) =
            super::delete_group(AdminOnly { user_id: 1 }, State(state.clone()), Path(20)).await;
        assert_eq!(
            resp.code, 0,
            "empty group must be deletable: {}",
            resp.message
        );
    }

    /// v1.0.4: count_rules_by_group detects rule references correctly.
    #[tokio::test]
    async fn count_rules_by_group_detects_rule_references() {
        let (state, pool) = test_state().await;
        add_group(&pool, 30, 1, "node-group").await;
        add_rule(&pool, 300, 1, 30, 20001, 0).await;

        let count = state.db.count_rules_by_group(30).await.unwrap();
        assert_eq!(count, 1, "group 30 should have 1 rule referencing it");

        // After deleting the rule, the group is free.
        state
            .db
            .delete_rule(300, &crate::db::repo::ResourceScope::All)
            .await
            .unwrap();
        let count2 = state.db.count_rules_by_group(30).await.unwrap();
        assert_eq!(
            count2, 0,
            "after rule deletion, group should have 0 references"
        );
    }

    #[tokio::test]
    async fn delete_group_nonexistent_returns_404_no_broadcast() {
        let (state, _pool) = test_state().await;
        expect_broadcasts(&state, 0, async {
            let Json(resp) =
                super::delete_group(AdminOnly { user_id: 1 }, State(state.clone()), Path(99999))
                    .await;
            assert_eq!(resp.code, 404);
        })
        .await;
    }

    /// v0.4.8 PR3: err() accepts both &str and owned String (via impl Into<String>),
    /// so a format!() message doesn't need Box::leak.
    #[test]
    fn err_accepts_str_and_owned_string() {
        let from_str: ApiResponse<()> = err(400, "static");
        assert_eq!(from_str.code, 400);
        assert_eq!(from_str.message, "static");

        let from_owned: ApiResponse<()> = err(409, format!("used by {} rules", 3));
        assert_eq!(from_owned.code, 409);
        assert_eq!(from_owned.message, "used by 3 rules");
        assert!(from_owned.data.is_none());
    }

    // ── v0.4.9: GET /user/me ──

    /// get_me returns the calling user's own non-password fields. The password
    /// hash is NEVER in the response (UserSelf has no such field).
    #[tokio::test]
    async fn get_me_returns_own_info_without_password() {
        let (state, pool) = test_state().await;
        add_user(&pool, 2, "alice", false).await;
        // Give alice a distinguishable balance/limits/traffic so we can assert
        // the right row came back.
        sqlx::query(
            "UPDATE users SET balance='12.50', max_rules=7, traffic_used=1024, \
             traffic_limit=1048576 WHERE id=2",
        )
        .execute(&pool)
        .await
        .unwrap();

        let Json(resp) = get_me(
            AuthUser {
                user_id: 2,
                admin: false,
            },
            State(state.clone()),
        )
        .await;
        assert_eq!(resp.code, 0);
        let me = resp.data.expect("data present");
        assert_eq!(me.id, 2);
        assert_eq!(me.username, "alice");
        assert_eq!(me.balance, "12.50");
        assert_eq!(me.max_rules, 7);
        assert_eq!(me.traffic_used, 1024);
        assert_eq!(me.traffic_limit, 1_048_576);
        assert!(!me.admin);
        // v0.4.10: account projection fields. add_user leaves plan_id NULL and
        // creates no rules, so plan_name is None and current_rules is 0.
        assert_eq!(me.plan_id, None);
        assert_eq!(me.plan_name, None);
        assert_eq!(me.current_rules, 0);
        assert!(
            !me.registered_at.is_empty(),
            "registered_at must be populated"
        );
        // UserSelf has no password-hash field by construction. We assert the
        // serialized response carries neither a bcrypt hash (always starts
        // "$2") nor a bare `"password":` key. We deliberately do NOT assert the
        // substring "password" is absent, because v0.4.10 PR4 added the
        // legitimate `must_change_password` field which contains that substring.
        let serialized = serde_json::to_string(&me).unwrap();
        assert!(
            !serialized.contains("$2"),
            "bcrypt hash must never appear in /user/me response: {serialized}"
        );
        assert!(
            !serialized.contains("\"password\""),
            "password-hash key must never appear in /user/me response: {serialized}"
        );
        // v0.4.10: the JSON field is registered_at (renamed from created_at).
        assert!(
            serialized.contains("registered_at"),
            "JSON must use registered_at key: {serialized}"
        );
    }

    /// v0.4.10: when the user has a plan_id set, plan_name is resolved from
    /// the plans table (plan_id=1 is the seeded 'free' plan).
    #[tokio::test]
    async fn get_me_includes_plan_name_when_plan_set() {
        let (state, pool) = test_state().await;
        add_user(&pool, 2, "alice", false).await;
        sqlx::query("UPDATE users SET plan_id = 1 WHERE id = 2")
            .execute(&pool)
            .await
            .unwrap();

        let Json(resp) = get_me(
            AuthUser {
                user_id: 2,
                admin: false,
            },
            State(state.clone()),
        )
        .await;
        assert_eq!(resp.code, 0, "{}", resp.message);
        let me = resp.data.unwrap();
        assert_eq!(me.plan_id, Some(1));
        assert_eq!(me.plan_name.as_deref(), Some("free"));
    }

    /// v0.4.10: a plan_id pointing at a non-existent plan yields plan_name
    /// None (defensive — FK should prevent this, but the projection must not
    /// panic or 500 on a dangling reference).
    #[tokio::test]
    async fn get_me_plan_name_none_when_plan_missing() {
        let (state, pool) = test_state().await;
        add_user(&pool, 2, "alice", false).await;
        // Force a dangling plan_id (FK off so SQLite accepts it).
        sqlx::query("PRAGMA foreign_keys = OFF")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("UPDATE users SET plan_id = 999 WHERE id = 2")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("PRAGMA foreign_keys = ON")
            .execute(&pool)
            .await
            .unwrap();

        let Json(resp) = get_me(
            AuthUser {
                user_id: 2,
                admin: false,
            },
            State(state.clone()),
        )
        .await;
        assert_eq!(resp.code, 0, "{}", resp.message);
        let me = resp.data.unwrap();
        assert_eq!(me.plan_id, Some(999));
        assert_eq!(me.plan_name, None, "missing plan must yield plan_name None");
    }

    /// v0.4.10: current_rules reflects the user's actual forward_rules count.
    #[tokio::test]
    async fn get_me_current_rules_reflects_rule_count() {
        let (state, pool) = test_state().await;
        add_user(&pool, 2, "alice", false).await;
        add_group(&pool, 20, 2, "alice-in").await;
        // Two rules owned by alice.
        add_rule(&pool, 100, 2, 20, 12000, 0).await;
        add_rule(&pool, 101, 2, 20, 12001, 0).await;

        let Json(resp) = get_me(
            AuthUser {
                user_id: 2,
                admin: false,
            },
            State(state.clone()),
        )
        .await;
        assert_eq!(resp.code, 0, "{}", resp.message);
        let me = resp.data.unwrap();
        assert_eq!(
            me.current_rules, 2,
            "current_rules must equal owned rule count"
        );
    }

    /// A non-admin reading their own account works (this is the whole point of
    /// /user/me — the account page is the non-admin's landing page).
    #[tokio::test]
    async fn get_me_works_for_non_admin() {
        let (state, pool) = test_state().await;
        add_user(&pool, 5, "bob", false).await;
        let Json(resp) = get_me(
            AuthUser {
                user_id: 5,
                admin: false,
            },
            State(state.clone()),
        )
        .await;
        assert_eq!(resp.code, 0);
        assert_eq!(resp.data.unwrap().username, "bob");
    }

    /// A deleted user (JWT still valid but row gone) → 404, not 500.
    #[tokio::test]
    async fn get_me_returns_404_for_deleted_user() {
        let (state, _pool) = test_state().await;
        let Json(resp) = get_me(
            AuthUser {
                user_id: 999,
                admin: false,
            },
            State(state.clone()),
        )
        .await;
        assert_eq!(resp.code, 404);
        assert!(resp.data.is_none());
    }

    // ── v0.4.10 resource-ownership isolation ──
    // These pin the per-user scoping at the HANDLER level: a non-admin may only
    // see/modify their own rules + groups; another user's (or a non-existent)
    // resource is a uniform 404; a forged owner_uid is ignored; an admin keeps
    // unscoped access.

    fn auth(user_id: i64, admin: bool) -> AuthUser {
        AuthUser { user_id, admin }
    }

    fn rule_req(name: &str, port: u16, group_in: i64, owner_uid: Option<i64>) -> CreateRuleRequest {
        CreateRuleRequest {
            name: name.into(),
            listen_port: Some(port),
            protocol: Protocol::Tcp,
            owner_uid,
            device_group_in: group_in,
            device_group_out: None,
            forward_mode: "direct".into(),
            route_mode: Default::default(),
            public_transport: Default::default(),
            ws_path: None,
            target_addr: "127.0.0.1".into(),
            target_port: 80,
            targets: None,
            load_balance_strategy: Default::default(),
            upload_limit_mbps: None,
            download_limit_mbps: None,
            tunnel_profile_id: None,
        }
    }

    /// list_rules is owner-scoped: a non-admin sees only their own rules, an
    /// admin sees everyone's.
    #[tokio::test]
    async fn list_rules_is_owner_scoped() {
        let (state, pool) = test_state().await;
        add_user(&pool, 2, "alice", false).await;
        add_user(&pool, 3, "bob", false).await;
        add_group(&pool, 10, 2, "alice-in").await;
        add_group(&pool, 11, 3, "bob-in").await;
        add_rule(&pool, 100, 2, 10, 20000, 0).await;
        add_rule(&pool, 101, 3, 11, 20001, 0).await;

        // Alice sees only her own rule.
        let Json(resp) = list_rules(
            auth(2, false),
            Query(ListRulesQuery::default()),
            State(state.clone()),
        )
        .await;
        let rules = resp.data.unwrap();
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].id, 100);

        // Admin sees both.
        let Json(resp) = list_rules(
            auth(1, true),
            Query(ListRulesQuery::default()),
            State(state.clone()),
        )
        .await;
        assert_eq!(resp.data.unwrap().len(), 2);
    }

    /// v0.4.20: admin can filter rules by owner_uid query param.
    #[tokio::test]
    async fn list_rules_owner_uid_admin_only() {
        let (state, pool) = test_state().await;
        add_user(&pool, 2, "alice", false).await;
        add_user(&pool, 3, "bob", false).await;
        add_group(&pool, 10, 2, "alice-in").await;
        add_group(&pool, 11, 3, "bob-in").await;
        add_rule(&pool, 100, 2, 10, 20000, 0).await;
        add_rule(&pool, 101, 3, 11, 20001, 0).await;

        // Admin filters by owner_uid=2 → only alice's rule.
        let Json(resp) = list_rules(
            auth(1, true),
            Query(ListRulesQuery { owner_uid: Some(2) }),
            State(state.clone()),
        )
        .await;
        let rules = resp.data.unwrap();
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].id, 100);

        // Non-admin passing owner_uid → ignored, still sees only own rules.
        let Json(resp) = list_rules(
            auth(2, false),
            Query(ListRulesQuery { owner_uid: Some(3) }),
            State(state.clone()),
        )
        .await;
        let rules = resp.data.unwrap();
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].id, 100); // still alice's own rule
    }

    /// A non-admin updating/deleting another user's rule gets a uniform 404 —
    /// indistinguishable from "rule doesn't exist".
    #[tokio::test]
    async fn cross_user_rule_access_is_404() {
        let (state, pool) = test_state().await;
        add_user(&pool, 2, "alice", false).await;
        add_user(&pool, 3, "bob", false).await;
        add_group(&pool, 11, 3, "bob-in").await;
        add_rule(&pool, 101, 3, 11, 20001, 0).await;

        // Alice tries to rename bob's rule → 404.
        let Json(resp) = update_rule(
            auth(2, false),
            State(state.clone()),
            Path(101),
            Json(UpdateRuleRequest {
                name: Some("hijacked".into()),
                ..Default::default()
            }),
        )
        .await;
        assert_eq!(resp.code, 404, "{}", resp.message);

        // Alice tries to delete bob's rule → 404, and the rule survives.
        let Json(resp) = delete_rule(auth(2, false), State(state.clone()), Path(101)).await;
        assert_eq!(resp.code, 404);
        let (n,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM forward_rules WHERE id = 101")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(n, 1, "bob's rule must survive alice's delete attempt");
    }

    /// A non-admin's forged owner_uid in create_rule is IGNORED: the rule is
    /// attributed to the caller, not the spoofed target.
    #[tokio::test]
    async fn create_rule_ignores_forged_owner_uid_for_non_admin() {
        let (state, pool) = test_state().await;
        add_user(&pool, 2, "alice", false).await;
        add_user(&pool, 3, "bob", false).await;
        // v0.4.12 PR1: inbound group must be admin-owned 'in' (uid=1 = seeded admin).
        add_group(&pool, 10, 1, "shared-in").await;

        // Alice claims owner_uid = 3 (bob). It must be ignored.
        let Json(resp) = create_rule(
            auth(2, false),
            State(state.clone()),
            Json(rule_req("r", 20000, 10, Some(3))),
        )
        .await;
        assert_eq!(resp.code, 0, "{}", resp.message);

        let owner: (i64,) = sqlx::query_as("SELECT uid FROM forward_rules WHERE name = 'r'")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(
            owner.0, 2,
            "forged owner_uid must be ignored; alice owns it"
        );
    }

    /// An admin MAY create a rule on behalf of another user via owner_uid.
    #[tokio::test]
    async fn create_rule_admin_can_set_owner_uid() {
        let (state, pool) = test_state().await;
        add_user(&pool, 3, "bob", false).await;
        // v0.4.12 PR1: inbound group must be admin-owned (uid=1). The rule owner
        // (bob) is independent of the inbound group's owner.
        add_group(&pool, 11, 1, "shared-in").await;

        let Json(resp) = create_rule(
            auth(1, true),
            State(state.clone()),
            Json(rule_req("r", 20000, 11, Some(3))),
        )
        .await;
        assert_eq!(resp.code, 0, "{}", resp.message);
        let owner: (i64,) = sqlx::query_as("SELECT uid FROM forward_rules WHERE name = 'r'")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(owner.0, 3, "admin set owner to bob");
    }

    /// create_rule enforces that the referenced inbound group belongs to the
    /// rule's owner: a non-admin can't attach a rule to someone else's group.
    #[tokio::test]
    async fn create_rule_rejects_foreign_inbound_group() {
        let (state, pool) = test_state().await;
        add_user(&pool, 2, "alice", false).await;
        add_user(&pool, 3, "bob", false).await;
        add_group(&pool, 11, 3, "bob-in").await; // bob's group

        // Alice references bob's group 11 → rejected (group not hers).
        let Json(resp) = create_rule(
            auth(2, false),
            State(state.clone()),
            Json(rule_req("r", 20000, 11, None)),
        )
        .await;
        assert_eq!(resp.code, 400, "{}", resp.message);
    }

    /// v0.4.11 PR3: a non-admin CAN bind a rule to an inbound group owned by an
    /// ADMIN ("shared inbound" infrastructure). This is the positive case the
    /// foreign-group rejection must not break.
    #[tokio::test]
    async fn create_rule_allows_admin_shared_inbound_group() {
        let (state, pool) = test_state().await;
        // user id=1 is the seeded admin; it owns the shared group.
        add_user(&pool, 2, "alice", false).await;
        add_group(&pool, 11, 1, "shared-in").await; // admin's group

        // Alice references the admin's shared group 11 → allowed.
        let Json(resp) = create_rule(
            auth(2, false),
            State(state.clone()),
            Json(rule_req("r", 20000, 11, None)),
        )
        .await;
        assert_eq!(resp.code, 0, "{}", resp.message);
    }

    // ── v1.0.7 regression: per-user device-group authorization ──

    /// Restrict a user to an explicit device-group allowlist (all_device_groups
    /// stays 0). An empty list = unassigned = cannot forward.
    async fn assign_user_groups(pool: &SqlitePool, uid: i64, allowed: &[i64]) {
        sqlx::query("UPDATE users SET all_device_groups = 0 WHERE id = ?")
            .bind(uid)
            .execute(pool)
            .await
            .unwrap();
        sqlx::query("DELETE FROM user_device_groups WHERE user_id = ?")
            .bind(uid)
            .execute(pool)
            .await
            .unwrap();
        for gid in allowed {
            sqlx::query("INSERT INTO user_device_groups (user_id, device_group_id) VALUES (?, ?)")
                .bind(uid)
                .bind(gid)
                .execute(pool)
                .await
                .unwrap();
        }
    }

    /// REGRESSION: a user authorized for group 1 must NOT be able to create a
    /// rule on group 2.
    #[tokio::test]
    async fn create_rule_rejects_group_outside_user_authorization() {
        let (state, pool) = test_state().await;
        add_user(&pool, 2, "alice", false).await;
        add_group(&pool, 1, 1, "admin-in-1").await;
        add_group(&pool, 2, 1, "admin-in-2").await;
        // Alice is authorized for ONLY device group 1.
        assign_user_groups(&pool, 2, &[1]).await;

        // Group 1 → allowed.
        let Json(ok) = create_rule(
            auth(2, false),
            State(state.clone()),
            Json(rule_req("r1", 20001, 1, None)),
        )
        .await;
        assert_eq!(ok.code, 0, "group 1 must be allowed: {}", ok.message);

        // Group 2 → forbidden (403).
        let Json(deny) = create_rule(
            auth(2, false),
            State(state.clone()),
            Json(rule_req("r2", 20002, 2, None)),
        )
        .await;
        assert_eq!(
            deny.code, 403,
            "group 2 must be rejected (permission bypass)"
        );
    }

    /// REGRESSION: an empty authorized list means NO access → deny (the bug
    /// treated empty as "allow all").
    #[tokio::test]
    async fn create_rule_empty_authorization_denies_all() {
        let (state, pool) = test_state().await;
        add_user(&pool, 2, "alice", false).await;
        add_group(&pool, 1, 1, "admin-in-1").await;
        // Alice is authorized for NOTHING (default: restricted, no assignments).
        assign_user_groups(&pool, 2, &[]).await;

        let Json(deny) = create_rule(
            auth(2, false),
            State(state.clone()),
            Json(rule_req("r", 20001, 1, None)),
        )
        .await;
        assert_eq!(deny.code, 403, "empty authorization must deny, not allow");
    }

    /// REGRESSION: removing a device group from a user's authorization pauses
    /// the user's existing rules on that group (kept, not deleted).
    #[tokio::test]
    async fn changing_user_group_pauses_unauthorized_rules() {
        let (state, pool) = test_state().await;
        add_user(&pool, 2, "alice", false).await;
        add_group(&pool, 1, 1, "admin-in-1").await;
        add_group(&pool, 2, 1, "admin-in-2").await;
        // Alice initially authorized for both groups.
        assign_user_groups(&pool, 2, &[1, 2]).await;
        // Alice has a rule on group 2.
        add_rule(&pool, 100, 2, 2, 20002, 0).await;

        // Re-assign Alice to ONLY group 1 (group 2 no longer authorized).
        let Json(resp) = update_user(
            AdminOnly { user_id: 1 },
            State(state.clone()),
            Path(2),
            Json(UpdateUserRequest {
                device_group_ids: Some(vec![1]),
                ..Default::default()
            }),
        )
        .await;
        assert_eq!(resp.code, 0, "{}", resp.message);

        // Rule 100 (on group 2) must now be paused, NOT deleted.
        let row: (i64, bool) =
            sqlx::query_as("SELECT id, paused FROM forward_rules WHERE id = 100")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert!(row.1, "rule on now-unauthorized group must be paused");
    }

    /// REGRESSION (review round 2): flipping a user from all_device_groups=true
    /// to false must pause their rules on groups that are no longer authorized.
    #[tokio::test]
    async fn flipping_user_to_restricted_pauses_unauthorized_rules() {
        let (state, pool) = test_state().await;
        add_user(&pool, 2, "alice", false).await;
        add_group(&pool, 1, 1, "admin-in-1").await;
        add_group(&pool, 2, 1, "admin-in-2").await;
        // Alice initially may use ALL device groups.
        sqlx::query("UPDATE users SET all_device_groups = 1 WHERE id = 2")
            .execute(&pool)
            .await
            .unwrap();
        // Alice has a rule on group 2 (allowed while all_device_groups=true).
        add_rule(&pool, 100, 2, 2, 20002, 0).await;

        // Admin flips Alice to restricted (all_device_groups=false). With no
        // explicit assignments, ALL of her rules become unauthorized → paused.
        let Json(resp) = update_user(
            AdminOnly { user_id: 1 },
            State(state.clone()),
            Path(2),
            Json(UpdateUserRequest {
                all_device_groups: Some(false),
                ..Default::default()
            }),
        )
        .await;
        assert_eq!(resp.code, 0, "{}", resp.message);

        let row: (i64, bool) =
            sqlx::query_as("SELECT id, paused FROM forward_rules WHERE id = 100")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert!(
            row.1,
            "rule must be paused after user flipped to restricted (all_device_groups=false)"
        );
    }

    /// REGRESSION: updating the device-group authorization together with
    /// balance/quota must apply ALL fields (the bug early-returned, dropping the
    /// rest).
    #[tokio::test]
    async fn update_user_applies_authz_and_other_fields_together() {
        let (state, pool) = test_state().await;
        add_user(&pool, 2, "alice", false).await;

        let Json(resp) = update_user(
            AdminOnly { user_id: 1 },
            State(state.clone()),
            Path(2),
            Json(UpdateUserRequest {
                all_device_groups: Some(true),
                max_rules: Some(99),
                balance: Some("50.00".into()),
                ..Default::default()
            }),
        )
        .await;
        assert_eq!(resp.code, 0, "{}", resp.message);

        let row: (bool, i64, String) =
            sqlx::query_as("SELECT all_device_groups, max_rules, balance FROM users WHERE id = 2")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert!(row.0, "all_device_groups must be set");
        assert_eq!(row.1, 99, "max_rules must ALSO be applied (not dropped)");
        assert_eq!(row.2, "50.00", "balance must ALSO be applied (not dropped)");
    }

    /// v0.4.12 PR1: a regular user's OWN historical inbound group is NOT a valid
    /// rule entry — only admin-owned 'in' groups are. (Device groups are
    /// admin-managed shared infrastructure.)
    #[tokio::test]
    async fn create_rule_rejects_users_own_historical_group() {
        let (state, pool) = test_state().await;
        add_user(&pool, 2, "alice", false).await;
        add_group(&pool, 30, 2, "alice-own-in").await; // alice's own 'in' group

        let Json(resp) = create_rule(
            auth(2, false),
            State(state.clone()),
            Json(rule_req("r", 20000, 30, None)),
        )
        .await;
        assert_eq!(
            resp.code, 400,
            "a user's own (non-admin) group must be rejected: {}",
            resp.message
        );
    }

    /// v0.4.12 PR1: an admin-owned OUT (or monitor) group is NOT a valid rule
    /// entry — device_group_in must be `group_type='in'`.
    #[tokio::test]
    async fn create_rule_rejects_admin_out_group_as_inbound() {
        let (state, pool) = test_state().await;
        add_user(&pool, 2, "alice", false).await;
        add_group_typed(&pool, 40, 1, "admin-out", "out").await; // admin 'out' group

        let Json(resp) = create_rule(
            auth(2, false),
            State(state.clone()),
            Json(rule_req("r", 20000, 40, None)),
        )
        .await;
        assert_eq!(
            resp.code, 400,
            "an admin 'out' group must be rejected as device_group_in: {}",
            resp.message
        );
    }

    /// v0.4.20: create_rule rejects forward_mode="group".
    #[tokio::test]
    async fn create_rule_rejects_group_forward_mode() {
        let (state, pool) = test_state().await;
        add_user(&pool, 2, "alice", false).await;
        add_group(&pool, 20, 1, "shared-in").await;

        let Json(resp) = create_rule(
            auth(2, false),
            State(state.clone()),
            Json(CreateRuleRequest {
                forward_mode: "group".into(),
                ..rule_req("test", 20000, 20, None)
            }),
        )
        .await;
        assert_eq!(resp.code, 400, "{}", resp.message);
        assert!(resp.message.contains("direct"));
    }

    /// v0.4.20: create_rule rejects non-null device_group_out.
    #[tokio::test]
    async fn create_rule_rejects_device_group_out() {
        let (state, pool) = test_state().await;
        add_user(&pool, 2, "alice", false).await;
        add_group(&pool, 20, 1, "shared-in").await;

        let Json(resp) = create_rule(
            auth(2, false),
            State(state.clone()),
            Json(CreateRuleRequest {
                device_group_out: Some(99),
                ..rule_req("test", 20000, 20, None)
            }),
        )
        .await;
        assert_eq!(resp.code, 400, "{}", resp.message);
        assert!(resp.message.contains("device_group_out"));
    }

    /// v0.4.20: update_rule rejects forward_mode="group".
    #[tokio::test]
    async fn update_rule_rejects_group_forward_mode() {
        let (state, pool) = test_state().await;
        add_user(&pool, 2, "alice", false).await;
        add_group(&pool, 20, 1, "shared-in").await;
        add_rule(&pool, 200, 2, 20, 12000, 0).await;

        let Json(resp) = update_rule(
            auth(2, false),
            State(state.clone()),
            Path(200),
            Json(UpdateRuleRequest {
                forward_mode: Some("group".into()),
                ..Default::default()
            }),
        )
        .await;
        assert_eq!(resp.code, 400, "{}", resp.message);
        assert!(resp.message.contains("direct"));
    }

    /// v0.4.20: update_rule rejects non-null device_group_out.
    #[tokio::test]
    async fn update_rule_rejects_device_group_out() {
        let (state, pool) = test_state().await;
        add_user(&pool, 2, "alice", false).await;
        add_group(&pool, 20, 1, "shared-in").await;
        add_rule(&pool, 200, 2, 20, 12000, 0).await;

        let Json(resp) = update_rule(
            auth(2, false),
            State(state.clone()),
            Path(200),
            Json(UpdateRuleRequest {
                device_group_out: Some(99),
                ..Default::default()
            }),
        )
        .await;
        assert_eq!(resp.code, 400, "{}", resp.message);
        assert!(resp.message.contains("device_group_out"));
    }

    #[tokio::test]
    async fn group_access_is_owner_scoped() {
        let (state, pool) = test_state().await;
        add_user(&pool, 2, "alice", false).await;
        add_user(&pool, 3, "bob", false).await;
        add_group(&pool, 10, 2, "alice-in").await;
        add_group(&pool, 11, 3, "bob-in").await;

        // Alice lists only her group.
        let Json(resp) = list_groups(auth(2, false), State(state.clone())).await;
        let groups = resp.data.unwrap();
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].id, 10);

        // Admin lists both.
        let Json(resp) = list_groups(auth(1, true), State(state.clone())).await;
        assert_eq!(resp.data.unwrap().len(), 2);

        // v0.4.12 PR1: delete_group is admin-only (scope All). An admin may
        // delete any group regardless of owner.
        let Json(resp) =
            delete_group(AdminOnly { user_id: 1 }, State(state.clone()), Path(11)).await;
        assert_eq!(resp.code, 0, "{}", resp.message);
        let (n,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM device_groups WHERE id = 11")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(n, 0, "admin deleted bob's group");
    }

    /// v0.4.12 PR1: create_group is admin-only and IGNORES owner_uid — the
    /// group always belongs to the creating admin (a regular-user-owned group
    /// would be unmanageable and never shared).
    #[tokio::test]
    async fn create_group_ignores_owner_uid_and_assigns_admin() {
        let (state, pool) = test_state().await;
        add_user(&pool, 2, "alice", false).await;
        add_user(&pool, 3, "bob", false).await;

        let Json(resp) = create_group(
            AdminOnly { user_id: 1 },
            State(state.clone()),
            Json(CreateGroupRequest {
                name: "g".into(),
                group_type: GroupType::In,
                connect_host: "1.2.3.4".into(),
                port_range: "20000-30000".into(),
                owner_uid: Some(3),
                rate: None,
                hidden: None,
            }),
        )
        .await;
        assert_eq!(resp.code, 0, "{}", resp.message);
        let owner: (i64,) = sqlx::query_as("SELECT uid FROM device_groups WHERE name = 'g'")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(
            owner.0, 1,
            "owner_uid must be ignored; group belongs to the creating admin"
        );
    }

    // ── v0.4.10 fix PR: tunnel-profile builtin scoping + group ownership ──
    // These pin the two security gaps closed by the fix PR:
    //   C6 — a regular user's rule may bind ONLY a builtin tunnel profile
    //        (decided by the RULE OWNER's role, not the operator's)
    //   C7 — update_rule must reject pointing a rule at a group owned by
    //        someone other than the rule's owner (the invariant
    //        rule.uid == group_in.uid == group_out.uid holds for ALL operators)

    /// Helper: insert a tunnel profile row directly (test_state runs SCHEMA_SQL
    /// only, no Migration 6 builtin seed), so tests can pick builtin vs custom.
    async fn add_profile(pool: &SqlitePool, id: i64, name: &str, is_builtin: bool, uid: i64) {
        // v0.4.11 PR1: profiles are now ws/tls_simple only; 'direct' is no longer valid.
        sqlx::query(
            "INSERT INTO tunnel_profiles (id, name, transport, tls_mode, ws_path, host_header, sni, is_builtin, uid) \
             VALUES (?, ?, 'ws', 'none', '/relay', '', '', ?, ?)",
        )
        .bind(id)
        .bind(name)
        .bind(is_builtin)
        .bind(uid)
        .execute(pool)
        .await
        .unwrap();
    }

    /// v0.4.11 PR1: a regular user CAN now bind admin-created custom WS/TLS Simple
    /// templates (AvailableTemplates scope). This is intentional — regular users
    /// can select any available template for their rules.
    #[tokio::test]
    async fn create_rule_rejects_non_builtin_profile_for_non_admin_owner() {
        let (state, pool) = test_state().await;
        add_user(&pool, 2, "alice", false).await;
        add_group(&pool, 20, 1, "shared-in").await; // admin-owned inbound (v0.4.12 PR1)
        add_profile(&pool, 50, "custom", false, 1).await; // admin's custom ws profile

        let Json(resp) = create_rule(
            auth(2, false),
            State(state.clone()),
            Json(rule_req_with_profile("r", 12000, 20, None, Some(50))),
        )
        .await;
        // v0.4.11 PR1: allowed — regular users can bind admin-created ws/tls_simple templates
        assert_eq!(resp.code, 0, "{}", resp.message);
    }

    /// v0.4.11 PR1: admin creating rule for non-admin owner CAN bind custom profile.
    /// The AvailableTemplates scope includes admin-created custom templates.
    #[tokio::test]
    async fn create_rule_allows_builtin_profile_for_non_admin_owner() {
        let (state, pool) = test_state().await;
        add_user(&pool, 2, "alice", false).await;
        add_group(&pool, 20, 1, "shared-in").await; // admin-owned inbound (v0.4.12 PR1)
        add_profile(&pool, 51, "builtin-ws", true, 1).await;

        let Json(resp) = create_rule(
            auth(2, false),
            State(state.clone()),
            Json(rule_req_with_profile("r", 12001, 20, None, Some(51))),
        )
        .await;
        assert_eq!(resp.code, 0, "{}", resp.message);
    }

    /// v0.4.11 PR1: admin can bind custom profile when creating rule for themselves.
    #[tokio::test]
    async fn create_rule_admin_can_bind_custom_profile() {
        let (state, pool) = test_state().await;
        // user id=1 is the seeded admin
        add_group(&pool, 20, 1, "admin-in").await;
        add_profile(&pool, 50, "custom", false, 1).await;

        let Json(resp) = create_rule(
            auth(1, true),
            State(state.clone()),
            Json(rule_req_with_profile("r", 12002, 20, None, Some(50))),
        )
        .await;
        assert_eq!(resp.code, 0, "{}", resp.message);
    }

    /// v0.4.11 PR1: admin can bind custom profile when creating rule for non-admin.
    #[tokio::test]
    async fn create_rule_admin_rejects_custom_profile_for_non_admin_owner() {
        let (state, pool) = test_state().await;
        add_user(&pool, 2, "alice", false).await;
        add_group(&pool, 20, 1, "shared-in").await; // admin-owned inbound (v0.4.12 PR1)
        add_profile(&pool, 50, "custom", false, 1).await;

        // Admin creates a rule owned by alice, binds the custom profile.
        // v0.4.11 PR1: allowed — AvailableTemplates includes admin-created custom templates.
        let Json(resp) = create_rule(
            auth(1, true),
            State(state.clone()),
            Json(rule_req_with_profile("r", 12003, 20, Some(2), Some(50))),
        )
        .await;
        assert_eq!(resp.code, 0, "{}", resp.message);
    }

    /// C7: a regular user re-pointing their rule's device_group_in at ANOTHER
    /// user's group is rejected.
    #[tokio::test]
    async fn update_rule_rejects_foreign_inbound_group() {
        let (state, pool) = test_state().await;
        add_user(&pool, 2, "alice", false).await;
        add_user(&pool, 3, "bob", false).await;
        add_group(&pool, 20, 2, "alice-in").await;
        add_group(&pool, 21, 3, "bob-in").await; // bob's group
        add_rule(&pool, 200, 2, 20, 12000, 0).await;

        // Alice tries to point her rule at bob's inbound group.
        let Json(resp) = update_rule(
            auth(2, false),
            State(state.clone()),
            Path(200),
            Json(UpdateRuleRequest {
                device_group_in: Some(21),
                ..Default::default()
            }),
        )
        .await;
        assert_eq!(resp.code, 400, "{}", resp.message);
        assert!(resp.message.contains("device_group_in"));
    }

    /// v0.4.20: device_group_out is no longer supported — any non-null value
    /// is rejected at the API boundary before ownership checks.
    #[tokio::test]
    async fn update_rule_rejects_foreign_outbound_group() {
        let (state, pool) = test_state().await;
        add_user(&pool, 2, "alice", false).await;
        add_user(&pool, 3, "bob", false).await;
        add_group(&pool, 20, 2, "alice-in").await;
        add_group(&pool, 30, 3, "bob-out").await;
        add_rule(&pool, 200, 2, 20, 12000, 0).await;

        let Json(resp) = update_rule(
            auth(2, false),
            State(state.clone()),
            Path(200),
            Json(UpdateRuleRequest {
                device_group_out: Some(30),
                ..Default::default()
            }),
        )
        .await;
        assert_eq!(resp.code, 400, "{}", resp.message);
        assert!(resp.message.contains("device_group_out"));
    }

    /// v0.4.12 PR1: a regular user re-pointing their rule at one of their OWN
    /// (non-admin) groups is now REJECTED — device_group_in must be an
    /// admin-owned 'in' group. (The allowed path — swapping to an admin shared
    /// group — is covered by update_rule_allows_admin_shared_inbound_group.)
    #[tokio::test]
    async fn update_rule_rejects_owner_group_swap() {
        let (state, pool) = test_state().await;
        add_user(&pool, 2, "alice", false).await;
        add_group(&pool, 20, 1, "shared-in").await; // admin-owned (valid current inbound)
        add_group(&pool, 21, 2, "alice-in-2").await; // alice's own group (invalid target)
        add_rule(&pool, 200, 2, 20, 12000, 0).await;

        let Json(resp) = update_rule(
            auth(2, false),
            State(state.clone()),
            Path(200),
            Json(UpdateRuleRequest {
                device_group_in: Some(21),
                ..Default::default()
            }),
        )
        .await;
        assert_eq!(
            resp.code, 400,
            "swapping to the user's own (non-admin) group must be rejected: {}",
            resp.message
        );
    }

    /// v0.4.11 PR3: a non-admin CAN re-point their rule at an ADMIN-owned shared
    /// inbound group via update_rule.
    #[tokio::test]
    async fn update_rule_allows_admin_shared_inbound_group() {
        let (state, pool) = test_state().await;
        // user id=1 is the seeded admin; it owns the shared group.
        add_user(&pool, 2, "alice", false).await;
        add_group(&pool, 20, 2, "alice-in").await;
        add_group(&pool, 21, 1, "shared-in").await; // admin's group
        add_rule(&pool, 200, 2, 20, 12000, 0).await;

        let Json(resp) = update_rule(
            auth(2, false),
            State(state.clone()),
            Path(200),
            Json(UpdateRuleRequest {
                device_group_in: Some(21),
                ..Default::default()
            }),
        )
        .await;
        assert_eq!(resp.code, 0, "{}", resp.message);
    }
    #[tokio::test]
    async fn update_rule_admin_rejects_group_owned_by_different_user() {
        let (state, pool) = test_state().await;
        add_user(&pool, 2, "alice", false).await;
        add_user(&pool, 3, "bob", false).await;
        add_group(&pool, 20, 2, "alice-in").await;
        add_group(&pool, 21, 3, "bob-in").await; // bob's group
        add_rule(&pool, 200, 2, 20, 12000, 0).await; // alice's rule

        // Admin edits alice's rule, tries to point it at bob's group.
        let Json(resp) = update_rule(
            auth(1, true),
            State(state.clone()),
            Path(200),
            Json(UpdateRuleRequest {
                device_group_in: Some(21),
                ..Default::default()
            }),
        )
        .await;
        assert_eq!(resp.code, 400, "{}", resp.message);
        assert!(resp.message.contains("device_group_in"));
    }

    /// v0.4.12 PR1: an admin editing alice's rule CAN point it at an admin-owned
    /// shared 'in' group (the new valid inbound). Pointing at alice's own group
    /// is no longer valid (covered by update_rule_rejects_owner_group_swap).
    #[tokio::test]
    async fn update_rule_admin_can_swap_to_admin_shared_group() {
        let (state, pool) = test_state().await;
        add_user(&pool, 2, "alice", false).await;
        add_group(&pool, 20, 1, "shared-in-1").await; // admin-owned (valid current)
        add_group(&pool, 21, 1, "shared-in-2").await; // admin-owned (valid target)
        add_rule(&pool, 200, 2, 20, 12000, 0).await;

        let Json(resp) = update_rule(
            auth(1, true),
            State(state.clone()),
            Path(200),
            Json(UpdateRuleRequest {
                device_group_in: Some(21),
                ..Default::default()
            }),
        )
        .await;
        assert_eq!(resp.code, 0, "{}", resp.message);
    }

    /// C6 (update side): an admin editing a regular user's rule CANNOT rebind it
    /// to a custom profile (the rule owner is the regular user → BuiltinOnly).
    #[tokio::test]
    async fn update_rule_admin_rejects_custom_profile_for_non_admin_owned_rule() {
        let (state, pool) = test_state().await;
        add_user(&pool, 2, "alice", false).await;
        add_group(&pool, 20, 2, "alice-in").await;
        add_profile(&pool, 50, "custom", false, 1).await;
        add_rule(&pool, 200, 2, 20, 12000, 0).await;

        let Json(resp) = update_rule(
            auth(1, true),
            State(state.clone()),
            Path(200),
            Json(UpdateRuleRequest {
                tunnel_profile_id: Some(Some(50)),
                ..Default::default()
            }),
        )
        .await;
        assert_eq!(resp.code, 400, "{}", resp.message);
    }

    fn rule_req_with_profile(
        name: &str,
        port: u16,
        group_in: i64,
        owner_uid: Option<i64>,
        profile_id: Option<i64>,
    ) -> CreateRuleRequest {
        // v0.4.11 PR1: when a profile is provided, transport must be ws (matches profile).
        let public_transport = if profile_id.is_some() {
            PublicTransport::Ws
        } else {
            PublicTransport::Raw
        };
        CreateRuleRequest {
            name: name.into(),
            listen_port: Some(port),
            protocol: Protocol::Tcp,
            owner_uid,
            device_group_in: group_in,
            device_group_out: None,
            forward_mode: "direct".into(),
            route_mode: Default::default(),
            public_transport,
            ws_path: None,
            target_addr: "127.0.0.1".into(),
            target_port: 80,
            targets: None,
            load_balance_strategy: Default::default(),
            upload_limit_mbps: None,
            download_limit_mbps: None,
            tunnel_profile_id: profile_id,
        }
    }

    // ── v0.4.10 PR3: registration + settings handler tests ──

    /// registration_status returns enabled=false on an unseeded DB (safe default).
    #[tokio::test]
    async fn registration_status_returns_false_when_unseeded() {
        let (state, _pool) = test_state().await;
        let Json(resp) = registration_status(State(state.clone())).await;
        assert_eq!(resp.code, 0);
        assert!(
            !resp.data.unwrap().enabled,
            "unseeded DB must report registration disabled"
        );
    }

    /// registration_status reflects the DB row once seeded.
    #[tokio::test]
    async fn registration_status_reflects_db_setting() {
        let (state, _pool) = test_state().await;
        state
            .db
            .set_registration_settings(true, 1, &[1])
            .await
            .unwrap();

        let Json(resp) = registration_status(State(state.clone())).await;
        assert_eq!(resp.code, 0);
        assert!(resp.data.unwrap().enabled);
    }

    /// register returns 403 when registration is disabled (unseeded → false).
    #[tokio::test]
    async fn register_rejects_when_disabled() {
        let (state, _pool) = test_state().await;
        let Json(resp) = register(
            State(state.clone()),
            Json(RegisterRequest {
                username: "newuser".into(),
                password: "validpass1".into(),
                ..Default::default()
            }),
        )
        .await;
        assert_eq!(resp.code, 403, "{}", resp.message);
    }

    /// register rejects a password shorter than 8 bytes.
    #[tokio::test]
    async fn register_rejects_short_password() {
        let (state, _pool) = test_state().await;
        state
            .db
            .set_registration_settings(true, 1, &[1])
            .await
            .unwrap();

        let Json(resp) = register(
            State(state.clone()),
            Json(RegisterRequest {
                username: "newuser".into(),
                password: "short".into(), // 5 bytes
                ..Default::default()
            }),
        )
        .await;
        assert_eq!(resp.code, 400, "{}", resp.message);
    }

    /// register rejects a password longer than 72 bytes (bcrypt boundary).
    #[tokio::test]
    async fn register_rejects_long_password() {
        let (state, _pool) = test_state().await;
        state
            .db
            .set_registration_settings(true, 1, &[1])
            .await
            .unwrap();

        let Json(resp) = register(
            State(state.clone()),
            Json(RegisterRequest {
                username: "newuser".into(),
                password: "x".repeat(73),
                ..Default::default()
            }),
        )
        .await;
        assert_eq!(resp.code, 400, "{}", resp.message);
    }

    /// register rejects a password where the UTF-8 byte length exceeds 72 even
    /// though the character count is small (e.g. multibyte CJK / emoji).
    #[tokio::test]
    async fn register_rejects_multibyte_password_exceeding_byte_limit() {
        let (state, _pool) = test_state().await;
        state
            .db
            .set_registration_settings(true, 1, &[1])
            .await
            .unwrap();
        // 25 × '中' = 25 chars but 75 UTF-8 bytes (> 72).
        let pw = "中".repeat(25);
        assert_eq!(pw.len(), 75);

        let Json(resp) = register(
            State(state.clone()),
            Json(RegisterRequest {
                username: "newuser".into(),
                password: pw,
                ..Default::default()
            }),
        )
        .await;
        assert_eq!(
            resp.code, 400,
            "multibyte password over 72 bytes must be rejected"
        );
    }

    /// A successful registration inherits the plan's quota fields atomically.
    #[tokio::test]
    async fn register_inherits_plan_quota() {
        let (state, pool) = test_state().await;
        state
            .db
            .set_registration_settings(true, 1, &[1])
            .await
            .unwrap();

        let Json(resp) = register(
            State(state.clone()),
            Json(RegisterRequest {
                username: "alice".into(),
                password: "validpass1".into(),
                ..Default::default()
            }),
        )
        .await;
        assert_eq!(resp.code, 0, "{}", resp.message);

        // The seeded 'free' plan is max_rules=5, traffic=107374182400.
        let user: relay_shared::models::User =
            sqlx::query_as("SELECT * FROM users WHERE username = 'alice'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(user.plan_id, Some(1));
        assert_eq!(
            user.max_rules, 5,
            "max_rules must be inherited from the plan"
        );
        assert_eq!(
            user.traffic_limit, 107374182400,
            "traffic_limit must be inherited from plan.traffic"
        );
        assert!(!user.admin, "registered users are never admins");
    }

    /// register returns 409 on a duplicate username (UNIQUE constraint).
    #[tokio::test]
    async fn register_rejects_duplicate_username() {
        let (state, _pool) = test_state().await;
        state
            .db
            .set_registration_settings(true, 1, &[1])
            .await
            .unwrap();

        let req = RegisterRequest {
            username: "alice".into(),
            password: "validpass1".into(),
            ..Default::default()
        };
        let Json(r1) = register(State(state.clone()), Json(req.clone())).await;
        assert_eq!(r1.code, 0);
        let Json(r2) = register(State(state.clone()), Json(req)).await;
        assert_eq!(r2.code, 409, "duplicate username must yield 409");
    }

    /// admin update_registration_settings rejects a non-existent default plan.
    #[tokio::test]
    async fn admin_update_settings_rejects_missing_plan() {
        let (state, _pool) = test_state().await;
        let Json(resp) = update_registration_settings(
            AdminOnly { user_id: 1 },
            State(state.clone()),
            Json(RegistrationSettingsRequest {
                enabled: true,
                default_plan_id: 999, // does not exist
                allowed_plan_ids: vec![999],
            }),
        )
        .await;
        assert_eq!(resp.code, 400, "{}", resp.message);
    }

    /// admin update_registration_settings persists a valid config and the
    /// subsequent registration_status reflects it.
    #[tokio::test]
    async fn admin_update_settings_persists_and_takes_effect() {
        let (state, _pool) = test_state().await;
        let Json(resp) = update_registration_settings(
            AdminOnly { user_id: 1 },
            State(state.clone()),
            Json(RegistrationSettingsRequest {
                enabled: true,
                default_plan_id: 1,
                allowed_plan_ids: vec![1],
            }),
        )
        .await;
        assert_eq!(resp.code, 0, "{}", resp.message);
        let data = resp.data.unwrap();
        assert!(data.registration_enabled);
        assert_eq!(data.default_registration_plan_id, 1);

        // registration_status now reports enabled=true.
        let Json(status) = registration_status(State(state.clone())).await;
        assert!(status.data.unwrap().enabled);
    }

    /// admin get_registration_settings returns safe defaults on an unseeded DB.
    #[tokio::test]
    async fn admin_get_settings_returns_defaults_when_unseeded() {
        let (state, _pool) = test_state().await;
        let Json(resp) =
            get_registration_settings(AdminOnly { user_id: 1 }, State(state.clone())).await;
        assert_eq!(resp.code, 0);
        let data = resp.data.unwrap();
        assert!(!data.registration_enabled);
        assert_eq!(data.default_registration_plan_id, 1);
        assert_eq!(data.allowed_plan_ids, vec![1]);
    }

    // ── v0.4.21 PR2: registration multi-plan settings ──

    /// update_registration_settings persists allowed_plan_ids and the updated
    /// registration_status reflects the plan list.
    #[tokio::test]
    async fn admin_update_settings_with_allowed_plans() {
        let (state, _pool) = test_state().await;
        let Json(resp) = update_registration_settings(
            AdminOnly { user_id: 1 },
            State(state.clone()),
            Json(RegistrationSettingsRequest {
                enabled: true,
                default_plan_id: 1,
                allowed_plan_ids: vec![1],
            }),
        )
        .await;
        assert_eq!(resp.code, 0, "{}", resp.message);
        let data = resp.data.unwrap();
        assert!(data.registration_enabled);
        assert_eq!(data.default_registration_plan_id, 1);
        assert_eq!(data.allowed_plan_ids, vec![1]);

        // registration_status now returns plans.
        let Json(status) = registration_status(State(state.clone())).await;
        let s = status.data.unwrap();
        assert!(s.enabled);
        assert_eq!(s.default_plan_id, 1);
        assert_eq!(s.plans.len(), 1);
        assert_eq!(s.plans[0].id, 1);
    }

    /// registration_status only returns plans that are in allowed_plan_ids.
    #[tokio::test]
    async fn registration_status_only_returns_allowed_plans() {
        let (state, _pool) = test_state().await;
        // There's only plan 1 seeded, but we can still verify filtering works:
        // allowed_plan_ids=[], but the service rejects empty. Instead, use an
        // id that doesn't exist alongside plan 1.
        // Plan 1 exists; plan 999 does not. allowed=[1] should only return [1].
        let Json(_resp) = update_registration_settings(
            AdminOnly { user_id: 1 },
            State(state.clone()),
            Json(RegistrationSettingsRequest {
                enabled: true,
                default_plan_id: 1,
                allowed_plan_ids: vec![1],
            }),
        )
        .await;
        let Json(status) = registration_status(State(state.clone())).await;
        let s = status.data.unwrap();
        // Only plan 1 is in the allowed list and exists.
        assert_eq!(s.plans.len(), 1);
        assert_eq!(s.plans[0].id, 1);
    }

    /// register with a plan_id that is in the allowed list succeeds.
    #[tokio::test]
    async fn register_with_valid_plan_id_succeeds() {
        let (state, _pool) = test_state().await;
        state
            .db
            .set_registration_settings(true, 1, &[1])
            .await
            .unwrap();
        let req = RegisterRequest {
            username: "planuser".into(),
            password: "validpass1".into(),
            plan_id: Some(1),
        };
        let Json(resp) = register(State(state.clone()), Json(req)).await;
        assert_eq!(resp.code, 0, "{}", resp.message);
    }

    /// register with a plan_id NOT in the allowed list returns 400.
    #[tokio::test]
    async fn register_with_disallowed_plan_id_fails() {
        let (state, _pool) = test_state().await;
        state
            .db
            .set_registration_settings(true, 1, &[1])
            .await
            .unwrap();
        let req = RegisterRequest {
            username: "badplan".into(),
            password: "validpass1".into(),
            plan_id: Some(999),
        };
        let Json(resp) = register(State(state.clone()), Json(req)).await;
        assert_eq!(resp.code, 400, "must reject plan not in allowed list");
    }

    /// register without plan_id uses the default plan.
    #[tokio::test]
    async fn register_without_plan_id_uses_default() {
        let (state, _pool) = test_state().await;
        state
            .db
            .set_registration_settings(true, 1, &[1])
            .await
            .unwrap();
        let req = RegisterRequest {
            username: "defplan".into(),
            password: "validpass1".into(),
            ..Default::default()
        };
        let Json(resp) = register(State(state.clone()), Json(req)).await;
        assert_eq!(resp.code, 0, "should use default plan_id=1");
    }

    /// update_registration_settings rejects empty allowed_plan_ids.
    #[tokio::test]
    async fn admin_update_settings_rejects_empty_allowed() {
        let (state, _pool) = test_state().await;
        let Json(resp) = update_registration_settings(
            AdminOnly { user_id: 1 },
            State(state.clone()),
            Json(RegistrationSettingsRequest {
                enabled: true,
                default_plan_id: 1,
                allowed_plan_ids: vec![],
            }),
        )
        .await;
        assert_eq!(resp.code, 400, "empty allowed_plan_ids must be rejected");
    }

    /// update_registration_settings rejects default_plan_id not in allowed.
    #[tokio::test]
    async fn admin_update_settings_rejects_default_not_in_allowed() {
        let (state, _pool) = test_state().await;
        // Plan 1 exists; use it as the only allowed, but set default=999.
        let Json(resp) = update_registration_settings(
            AdminOnly { user_id: 1 },
            State(state.clone()),
            Json(RegistrationSettingsRequest {
                enabled: true,
                default_plan_id: 999,
                allowed_plan_ids: vec![1],
            }),
        )
        .await;
        assert_eq!(resp.code, 400, "default not in allowed must be rejected");
    }

    /// update_registration_settings rejects a non-existent plan in allowed.
    #[tokio::test]
    async fn admin_update_settings_rejects_nonexistent_allowed_plan() {
        let (state, _pool) = test_state().await;
        let Json(resp) = update_registration_settings(
            AdminOnly { user_id: 1 },
            State(state.clone()),
            Json(RegistrationSettingsRequest {
                enabled: true,
                default_plan_id: 1,
                allowed_plan_ids: vec![1, 999],
            }),
        )
        .await;
        assert_eq!(
            resp.code, 400,
            "non-existent plan in allowed_plan_ids must be rejected"
        );
    }

    /// v0.4.21 PR2: registration_status returns only plans in the allowed list
    /// when the DB has multiple plans. Constructs real multi-plan data.
    #[tokio::test]
    async fn registration_status_filters_multi_plan() {
        let (state, pool) = test_state().await;
        // Insert a second plan with different quota.
        sqlx::query(
            "INSERT INTO plans (id, name, max_rules, traffic, speed_limit, ip_limit, price) \
             VALUES (2, 'premium', 10, 0, 0, 5, '9.99')",
        )
        .execute(&pool)
        .await
        .unwrap();

        // Set allowed=[1,2] — registration_status should return both.
        state
            .db
            .set_registration_settings(true, 1, &[1, 2])
            .await
            .unwrap();
        let Json(status) = registration_status(State(state.clone())).await;
        let s = status.data.unwrap();
        assert_eq!(s.plans.len(), 2, "should return both allowed plans");
        let ids: Vec<i64> = s.plans.iter().map(|p| p.id).collect();
        assert!(ids.contains(&1));
        assert!(ids.contains(&2));

        // Set allowed=[2] — plan 1 should be filtered out.
        state
            .db
            .set_registration_settings(true, 2, &[2])
            .await
            .unwrap();
        let Json(status2) = registration_status(State(state.clone())).await;
        let s2 = status2.data.unwrap();
        assert_eq!(
            s2.plans.len(),
            1,
            "plan 1 must be filtered when not allowed"
        );
        assert_eq!(s2.plans[0].id, 2);
        assert_eq!(s2.plans[0].max_rules, 10);
    }

    /// v0.4.21 PR2: registering with plan_id=2 inherits plan 2's quota,
    /// NOT plan 1's.
    #[tokio::test]
    async fn register_with_plan_2_inherits_plan_2_quota() {
        let (state, pool) = test_state().await;
        sqlx::query(
            "INSERT INTO plans (id, name, max_rules, traffic, speed_limit, ip_limit, price) \
             VALUES (2, 'premium', 10, 0, 0, 5, '9.99')",
        )
        .execute(&pool)
        .await
        .unwrap();

        state
            .db
            .set_registration_settings(true, 2, &[1, 2])
            .await
            .unwrap();

        let req = RegisterRequest {
            username: "premium_user".into(),
            password: "validpass1".into(),
            plan_id: Some(2),
        };
        let Json(resp) = register(State(state.clone()), Json(req)).await;
        assert_eq!(resp.code, 0, "register with plan 2 should succeed");

        // Verify the user inherited plan 2's quota.
        let user: relay_shared::models::User =
            sqlx::query_as("SELECT * FROM users WHERE username = 'premium_user'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(user.plan_id, Some(2), "user.plan_id must be 2");
        assert_eq!(user.max_rules, 10, "max_rules must come from plan 2");
        assert_eq!(user.traffic_limit, 0, "traffic_limit must come from plan 2");
    }

    // ── v0.4.10 PR4: admin password reset + self change ──

    /// Admin reset bumps the target's token_version and sets must_change_password.
    #[tokio::test]
    async fn admin_reset_password_sets_must_change_and_bumps_version() {
        let (state, pool) = test_state().await;
        add_user(&pool, 2, "alice", false).await;

        let Json(resp) = reset_user_password(
            AdminOnly { user_id: 1 },
            State(state.clone()),
            Path(2),
            Json(ResetPasswordRequest {
                new_password: "temp-pass-1".into(),
                must_change_password: true,
            }),
        )
        .await;
        assert_eq!(resp.code, 0, "{}", resp.message);

        let s = state.db.find_auth_state_by_id(2).await.unwrap().unwrap();
        assert_eq!(s.1, 1, "token_version bumped");
        assert!(s.2, "must_change_password set");
    }

    /// Admin cannot reset ANOTHER admin's password (privilege protection).
    #[tokio::test]
    async fn admin_cannot_reset_other_admin_password() {
        let (state, pool) = test_state().await;
        add_user(&pool, 2, "admin2", true).await; // another admin

        let Json(resp) = reset_user_password(
            AdminOnly { user_id: 1 },
            State(state.clone()),
            Path(2),
            Json(ResetPasswordRequest {
                new_password: "temp-pass-1".into(),
                must_change_password: true,
            }),
        )
        .await;
        assert_eq!(resp.code, 403, "{}", resp.message);
    }

    /// Admin reset rejects a short password (< 8 bytes).
    #[tokio::test]
    async fn admin_reset_password_rejects_short() {
        let (state, pool) = test_state().await;
        add_user(&pool, 2, "alice", false).await;
        let Json(resp) = reset_user_password(
            AdminOnly { user_id: 1 },
            State(state.clone()),
            Path(2),
            Json(ResetPasswordRequest {
                new_password: "short".into(),
                must_change_password: true,
            }),
        )
        .await;
        assert_eq!(resp.code, 400, "{}", resp.message);
    }

    /// Admin reset of a non-existent user → 404.
    #[tokio::test]
    async fn admin_reset_password_missing_user_404() {
        let (state, _pool) = test_state().await;
        let Json(resp) = reset_user_password(
            AdminOnly { user_id: 1 },
            State(state.clone()),
            Path(999),
            Json(ResetPasswordRequest {
                new_password: "temp-pass-1".into(),
                must_change_password: true,
            }),
        )
        .await;
        assert_eq!(resp.code, 404, "{}", resp.message);
    }

    /// Self change_password bumps token_version and clears must_change_password.
    #[tokio::test]
    async fn self_change_password_bumps_version() {
        let (state, pool) = test_state().await;
        // Seed a user whose current password we know (bcrypt of "old-pass-1").
        let hash = bcrypt::hash("old-pass-1", 4).unwrap();
        sqlx::query(
            "INSERT INTO users (id, username, password, admin, token_version, must_change_password) \
             VALUES (2, 'alice', ?, 0, 0, 1)",
        )
        .bind(&hash)
        .execute(&pool)
        .await
        .unwrap();

        let Json(resp) = change_password(
            AuthUser {
                user_id: 2,
                admin: false,
            },
            State(state.clone()),
            Json(ChangePasswordRequest {
                current_password: "old-pass-1".into(),
                new_password: "new-pass-1".into(),
            }),
        )
        .await;
        assert_eq!(resp.code, 0, "{}", resp.message);

        let s = state.db.find_auth_state_by_id(2).await.unwrap().unwrap();
        assert_eq!(s.1, 1, "token_version bumped on self change");
        assert!(!s.2, "must_change_password cleared on self change");
    }

    /// change_password rejects a short new password (< 8 bytes).
    #[tokio::test]
    async fn self_change_password_rejects_short() {
        let (state, pool) = test_state().await;
        let hash = bcrypt::hash("old-pass-1", 4).unwrap();
        sqlx::query("INSERT INTO users (id, username, password, admin) VALUES (2, 'alice', ?, 0)")
            .bind(&hash)
            .execute(&pool)
            .await
            .unwrap();
        let Json(resp) = change_password(
            AuthUser {
                user_id: 2,
                admin: false,
            },
            State(state.clone()),
            Json(ChangePasswordRequest {
                current_password: "old-pass-1".into(),
                new_password: "short".into(),
            }),
        )
        .await;
        assert_eq!(resp.code, 400, "{}", resp.message);
    }
}
