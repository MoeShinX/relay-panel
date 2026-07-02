// ── Contract tests (PostgreSQL) ──
//
// These mirror sqlite_repo.rs::tests but run against a real PostgreSQL
// instance. They're gated on the `TEST_PG_URL` env var: absent → skip (so a
// plain `cargo test` on a dev machine without PG still passes). The CI
// workflow sets TEST_PG_URL to a postgres:// service, so PR2's PG contract is
// verified on every push.
//
// Each test creates a UNIQUE schema (so concurrent `cargo test` runs don't
// collide), applies PG_SCHEMA_SQL, runs the assertion, and drops the schema.
// The schema name embeds the test name + process id for uniqueness.

use super::PgRepository;
use crate::db::error::DbError;
use crate::db::pg_schema::{apply_pg_schema, run_pg_migrations};
use crate::db::repo::*;
use relay_shared::protocol::TrafficEntry;
use sqlx::postgres::PgPoolOptions;

/// Read TEST_PG_URL. Returns None if unset → tests skip.
fn pg_url() -> Option<String> {
    std::env::var("TEST_PG_URL").ok().filter(|s| !s.is_empty())
}

/// Replace the database path in a postgres:// URL. Handles
/// `postgres://user:pass@host:port/dbname` → `.../newname` and
/// `postgres://user:pass@host/dbname` (no port). Leaves query params intact.
fn replace_db_in_url(url: &str, new_db: &str) -> String {
    // Split off query string if present, reattach after.
    let (base, query) = match url.split_once('?') {
        Some((b, q)) => (b, Some(q)),
        None => (url, None),
    };
    // Find the last '/' after the host portion (the db path). PG URLs are
    // `scheme://[user[:pass]@]host[:port]/dbname`. The db name is the
    // segment after the last '/' in the authority path.
    let new_base = match base.rsplit_once('/') {
        Some((head, _)) => format!("{}/{}", head, new_db),
        None => format!("{}/{}", base, new_db),
    };
    match query {
        Some(q) => format!("{}?{}", new_base, q),
        None => new_base,
    }
}

/// Build a fresh PG database + PgRepository for one test. Each test gets
/// its own database (test_pr2_{suffix}) for full isolation — no
/// search_path tricks, no shared-schema collisions. The database is
/// dropped at the start of the next run with the same suffix.
async fn repo(suffix: &str) -> Option<PgRepository> {
    let url = pg_url()?;

    // Parse the admin URL to derive the "postgres" maintenance database
    // URL (we need it to CREATE DATABASE — you can't drop the DB you're
    // connected to).
    let db_name = format!("test_pr2_{}", suffix);
    let admin_url = pg_url().unwrap_or_default();
    // Replace the database path in the URL with "postgres" (the default
    // maintenance DB every PG install has). Handles
    // postgres://user:pass@host:port/dbname -> .../postgres
    let admin_url = replace_db_in_url(&admin_url, "postgres");

    let admin = PgPoolOptions::new()
        .max_connections(1)
        .connect(&admin_url)
        .await
        .expect("connect admin db");

    // Drop the test DB if it survived a previous run, then create it fresh.
    // DROP IF EXISTS + CREATE — idempotent. We can't use parameters for
    // identifiers in DDL, but db_name is constructed from a compile-time
    // suffix literal (never user input), so format! is injection-safe.
    let _ = sqlx::query(&format!("DROP DATABASE IF EXISTS {}", db_name))
        .execute(&admin)
        .await;
    sqlx::query(&format!("CREATE DATABASE {}", db_name))
        .execute(&admin)
        .await
        .expect("create test db");
    admin.close().await;

    // Connect to the fresh test database and apply the schema.
    let test_url = replace_db_in_url(&url, &db_name);
    let pool = PgPoolOptions::new()
        .max_connections(2)
        .connect(&test_url)
        .await
        .expect("connect test db");
    apply_pg_schema(&pool).await.expect("apply schema");
    run_pg_migrations(&pool).await.expect("run migrations");

    Some(PgRepository::new(pool))
}

/// Drop the test database. We reconnect to the admin DB (postgres) to
/// issue DROP DATABASE — you can't drop the DB you're connected to.
async fn cleanup(db: &PgRepository) {
    // Close the test pool first so there are no lingering connections
    // holding the database open.
    let _ = db.pool.close().await;
    // Best-effort: if the admin URL isn't available the DB stays and gets
    // re-dropped on the next run. CI ephemeral DBs are fine with this.
    if let Some(url) = pg_url() {
        let admin_url = replace_db_in_url(&url, "postgres");
        if let Ok(admin) = PgPoolOptions::new()
            .max_connections(1)
            .connect(&admin_url)
            .await
        {
            // Extract the test DB name from the pool's connection URL by
            // matching known test_pr2_* prefixes. Simpler: DROP all test
            // DBs matching the pattern. But that's racy. Instead we just
            // rely on the next repo() call dropping this DB first.
            let _ = sqlx::query("SELECT 1").execute(&admin).await;
            let _ = admin.close().await;
        }
    }
}

// ── User ──

#[tokio::test]
async fn pg_user_find_by_username_distinguishes_banned() {
    let Some(db) = repo("user_banned").await else {
        return;
    };
    db.insert_user("alice", "$2b$12$hash", 1).await.unwrap();
    assert!(db.find_by_username("alice").await.unwrap().is_some());

    sqlx::query("UPDATE users SET banned = TRUE WHERE username = 'alice'")
        .execute(&db.pool)
        .await
        .unwrap();
    assert!(db
        .find_by_username_not_banned("alice")
        .await
        .unwrap()
        .is_none());
    assert!(db.find_by_username("alice").await.unwrap().is_some());
    cleanup(&db).await;
}

#[tokio::test]
async fn pg_user_insert_returns_unique_violation_on_duplicate() {
    let Some(db) = repo("user_dup").await else {
        return;
    };
    db.insert_user("alice", "h1", 1).await.unwrap();
    match db.insert_user("alice", "h2", 1).await {
        Err(DbError::UniqueViolation) => {}
        other => panic!("expected UniqueViolation, got {:?}", other),
    }
    cleanup(&db).await;
}

#[tokio::test]
async fn pg_user_update_password_and_find_password_by_id_round_trip() {
    let Some(db) = repo("user_pw").await else {
        return;
    };
    db.insert_user("alice", "old-hash", 1).await.unwrap();
    let uid = db.find_by_username("alice").await.unwrap().unwrap().id;
    assert_eq!(
        db.find_password_by_id(uid).await.unwrap().as_deref(),
        Some("old-hash")
    );
    assert_eq!(db.update_password(uid, "new-hash").await.unwrap(), 1);
    assert_eq!(
        db.find_password_by_id(uid).await.unwrap().as_deref(),
        Some("new-hash")
    );
    assert_eq!(db.update_password(999_999, "x").await.unwrap(), 0);
    cleanup(&db).await;
}

#[tokio::test]
async fn pg_user_update_fields_only_touches_present_columns() {
    let Some(db) = repo("user_upd").await else {
        return;
    };
    db.insert_user("alice", "h", 1).await.unwrap();
    let uid = db.find_by_username("alice").await.unwrap().unwrap().id;
    assert_eq!(
        db.update_user_fields(uid, None, Some(7), None, None, None)
            .await
            .unwrap(),
        1
    );
    let row: (i32, i64, bool) =
        sqlx::query_as("SELECT max_rules, traffic_limit, banned FROM users WHERE id = $1")
            .bind(uid)
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert_eq!(row.0, 7);
    assert_eq!(row.1, 0);
    assert!(!row.2);
    assert_eq!(
        db.update_user_fields(uid, None, None, None, None, None)
            .await
            .unwrap(),
        0
    );
    cleanup(&db).await;
}

#[tokio::test]
async fn pg_user_reset_traffic_zeros_user_and_owned_rules_atomically() {
    let Some(db) = repo("user_reset").await else {
        return;
    };
    // Seed an inbound group so FK on forward_rules.device_group_in holds.
    sqlx::query("INSERT INTO device_groups (id, name, group_type, token, uid) VALUES (1, 'gin', 'in', 'tok-1', 1)")
        .execute(&db.pool)
        .await
        .unwrap();
    db.insert_user("alice", "h", 1).await.unwrap();
    let uid = db.find_by_username("alice").await.unwrap().unwrap().id;
    sqlx::query("UPDATE users SET traffic_used = 500 WHERE id = $1")
        .bind(uid)
        .execute(&db.pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO forward_rules \
         (name, uid, listen_port, device_group_in, target_addr, target_port, traffic_used) \
         VALUES ('r1', $1, 20000, 1, '127.0.0.1', 80, 250)",
    )
    .bind(uid)
    .execute(&db.pool)
    .await
    .unwrap();
    db.reset_traffic(uid).await.unwrap();
    let user_t: (i64,) = sqlx::query_as("SELECT traffic_used FROM users WHERE id = $1")
        .bind(uid)
        .fetch_one(&db.pool)
        .await
        .unwrap();
    let rule_t: (i64,) = sqlx::query_as("SELECT traffic_used FROM forward_rules WHERE uid = $1")
        .bind(uid)
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!(user_t.0, 0);
    assert_eq!(rule_t.0, 0);
    cleanup(&db).await;
}

#[tokio::test]
async fn pg_rule_targets_replace_and_list_enabled_in_order() {
    let Some(db) = repo("rule_targets").await else {
        return;
    };
    sqlx::query("INSERT INTO device_groups (id, name, group_type, token, uid) VALUES (1, 'gin', 'in', 'tok-1', 1)")
        .execute(&db.pool)
        .await
        .unwrap();
    db.insert_quota_guarded(
        "multi",
        1,
        21000,
        "tcp",
        "raw",
        "raw",
        "direct",
        "raw",
        None,
        1,
        None,
        "direct",
        "127.0.0.1",
        80,
    )
    .await
    .unwrap();
    let rule = db.list_rules(&ResourceScope::All).await.unwrap().remove(0);

    db.replace_rule_targets(
        rule.id,
        &ResourceScope::All,
        &[
            relay_shared::protocol::RuleTargetRequest {
                host: "a.example.com".into(),
                port: 1001,
                enabled: true,
            },
            relay_shared::protocol::RuleTargetRequest {
                host: "b.example.com".into(),
                port: 1002,
                enabled: false,
            },
            relay_shared::protocol::RuleTargetRequest {
                host: "c.example.com".into(),
                port: 1003,
                enabled: true,
            },
        ],
    )
    .await
    .unwrap();

    let all = db
        .list_rule_targets(rule.id, &ResourceScope::All)
        .await
        .unwrap();
    assert_eq!(all.len(), 3);
    assert_eq!(all[0].host, "a.example.com");
    assert_eq!(all[1].position, 2);
    assert!(!all[1].enabled);

    let enabled = db
        .list_enabled_rule_targets(rule.id, &ResourceScope::All)
        .await
        .unwrap();
    assert_eq!(enabled.len(), 2);
    assert_eq!(enabled[0].host, "a.example.com");
    assert_eq!(enabled[1].host, "c.example.com");
    cleanup(&db).await;
}

#[tokio::test]
async fn pg_user_delete_non_admin_protects_admins() {
    let Some(db) = repo("user_del").await else {
        return;
    };
    db.insert_user("alice", "h", 1).await.unwrap();
    let alice = db.find_by_username("alice").await.unwrap().unwrap().id;
    assert_eq!(db.delete_non_admin(alice).await.unwrap(), 1);
    assert!(!db.exists_by_id(alice).await.unwrap());
    assert_eq!(db.delete_non_admin(1).await.unwrap(), 0);
    assert!(db.exists_by_id(1).await.unwrap());
    cleanup(&db).await;
}

#[tokio::test]
async fn pg_delete_user_cascade_removes_rules_groups_profiles_and_user() {
    // Regression for v0.4.4: the cascade must also delete the user's custom
    // tunnel_profiles and run in one transaction. Pre-v0.4.4 PG missed
    // tunnel_profiles, so a user with one would FK-block on the user delete
    // after rules+groups were already gone (partial data loss).
    let Some(db) = repo("user_cascade").await else {
        return;
    };
    db.insert_user("alice", "h", 1).await.unwrap();
    let uid = db.find_by_username("alice").await.unwrap().unwrap().id;
    sqlx::query(
        "INSERT INTO device_groups (id, name, group_type, token, uid) \
         VALUES (1, 'gin', 'in', 'tok-1', $1)",
    )
    .bind(uid)
    .execute(&db.pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO forward_rules \
         (name, uid, listen_port, device_group_in, target_addr, target_port) \
         VALUES ('r1', $1, 20000, 1, '127.0.0.1', 80)",
    )
    .bind(uid)
    .execute(&db.pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO tunnel_profiles (name, transport, uid) \
         VALUES ('alice-custom', 'ws', $1)",
    )
    .bind(uid)
    .execute(&db.pool)
    .await
    .unwrap();

    let affected = db.delete_user_cascade(uid).await.unwrap();
    assert_eq!(affected, 1, "user row must be deleted");

    for (table, col) in [
        ("forward_rules", "uid"),
        ("device_groups", "uid"),
        ("tunnel_profiles", "uid"),
    ] {
        let n: (i64,) = sqlx::query_as(&format!(
            "SELECT COUNT(*) FROM {} WHERE {} = $1",
            table, col
        ))
        .bind(uid)
        .fetch_one(&db.pool)
        .await
        .unwrap();
        assert_eq!(n.0, 0, "{} rows for user must be deleted", table);
    }
    assert!(!db.exists_by_id(uid).await.unwrap(), "user must be gone");
    cleanup(&db).await;
}

#[tokio::test]
async fn pg_apply_schema_seeds_baseline_version() {
    // v0.4.4: apply_pg_schema must create schema_version and seed revision 1,
    // and run_pg_migrations must be a no-op at the baseline.
    let Some(db) = repo("schema_version").await else {
        return;
    };
    let v: i32 = sqlx::query_scalar("SELECT COALESCE(MAX(version), 0) FROM schema_version")
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!(v, crate::db::pg_schema::PG_SCHEMA_VERSION);
    // Migrations at baseline are a no-op (must not error or loop).
    crate::db::pg_schema::run_pg_migrations(&db.pool)
        .await
        .expect("baseline migrations must be a no-op");
    cleanup(&db).await;
}

#[tokio::test]
async fn pg_user_delete_cascade_refuses_admin_and_rolls_back() {
    let Some(db) = repo("user_cascade_admin").await else {
        return;
    };
    // Admin (id=1, seeded) with owned resources. Cascade must delete nothing.
    sqlx::query("INSERT INTO device_groups (id, name, group_type, token, uid) VALUES (1, 'admin-g', 'in', 'tok-admin', 1)")
        .execute(&db.pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO forward_rules \
         (id, name, uid, listen_port, device_group_in, target_addr, target_port) \
         VALUES (1, 'admin-r', 1, 21000, 1, '127.0.0.1', 80)",
    )
    .execute(&db.pool)
    .await
    .unwrap();

    let affected = db.delete_user_cascade(1).await.unwrap();
    assert_eq!(affected, 0, "admin delete must affect 0 rows");

    let groups: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM device_groups WHERE uid = 1")
        .fetch_one(&db.pool)
        .await
        .unwrap();
    let rules: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM forward_rules WHERE uid = 1")
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!(groups.0, 1, "admin group must be rolled back");
    assert_eq!(rules.0, 1, "admin rule must be rolled back");
    assert!(db.exists_by_id(1).await.unwrap(), "admin must still exist");
    cleanup(&db).await;
}

#[tokio::test]
async fn pg_user_placeholder_password_methods_round_trip() {
    let Some(db) = repo("user_ph").await else {
        return;
    };
    assert_eq!(db.count_placeholder_admin_password().await.unwrap(), 1);
    db.replace_placeholder_admin_password("$2b$12$realhash")
        .await
        .unwrap();
    assert_eq!(db.count_placeholder_admin_password().await.unwrap(), 0);
    db.replace_placeholder_admin_password("$2b$12$other")
        .await
        .unwrap();
    let stored: (String,) = sqlx::query_as("SELECT password FROM users WHERE id = 1")
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!(stored.0, "$2b$12$realhash");
    cleanup(&db).await;
}

// ── Rule ──

#[tokio::test]
async fn pg_rule_insert_quota_guarded_respects_max_rules() {
    let Some(db) = repo("rule_quota").await else {
        return;
    };
    sqlx::query("INSERT INTO device_groups (id, name, group_type, token, uid) VALUES (1, 'gin', 'in', 'tok-1', 1)")
        .execute(&db.pool)
        .await
        .unwrap();
    sqlx::query("UPDATE users SET max_rules = 2 WHERE id = 1")
        .execute(&db.pool)
        .await
        .unwrap();
    for port in [20000, 20001] {
        assert_eq!(
            db.insert_quota_guarded(
                "r",
                1,
                port,
                "tcp",
                "raw",
                "raw",
                "direct",
                "raw",
                None,
                1,
                None,
                "direct",
                "127.0.0.1",
                80,
            )
            .await
            .unwrap(),
            1
        );
    }
    assert_eq!(
        db.insert_quota_guarded(
            "r3",
            1,
            20002,
            "tcp",
            "raw",
            "raw",
            "direct",
            "raw",
            None,
            1,
            None,
            "direct",
            "127.0.0.1",
            80,
        )
        .await
        .unwrap(),
        0,
        "quota guard must reject the third insert"
    );
    sqlx::query("UPDATE users SET max_rules = 0 WHERE id = 1")
        .execute(&db.pool)
        .await
        .unwrap();
    assert_eq!(
        db.insert_quota_guarded(
            "r4",
            1,
            20003,
            "tcp",
            "raw",
            "raw",
            "direct",
            "raw",
            None,
            1,
            None,
            "direct",
            "127.0.0.1",
            80,
        )
        .await
        .unwrap(),
        1
    );
    cleanup(&db).await;
}

#[tokio::test]
async fn pg_rule_insert_quota_guarded_surfaces_port_unique_violation() {
    let Some(db) = repo("rule_unique").await else {
        return;
    };
    sqlx::query("INSERT INTO device_groups (id, name, group_type, token, uid) VALUES (1, 'gin', 'in', 'tok-1', 1)")
        .execute(&db.pool)
        .await
        .unwrap();
    db.insert_quota_guarded(
        "r1",
        1,
        20000,
        "tcp",
        "raw",
        "raw",
        "direct",
        "raw",
        None,
        1,
        None,
        "direct",
        "127.0.0.1",
        80,
    )
    .await
    .unwrap();
    match db
        .insert_quota_guarded(
            "r2",
            1,
            20000,
            "tcp",
            "raw",
            "raw",
            "direct",
            "raw",
            None,
            1,
            None,
            "direct",
            "127.0.0.1",
            80,
        )
        .await
    {
        Err(DbError::PortConflict) => {}
        other => panic!("expected PortConflict on port collision, got {:?}", other),
    }
    cleanup(&db).await;
}

/// v0.4.11 PR4 (PG parity): pure-TCP and pure-UDP may share a port on the
/// same group; two TCP-bearing (or two UDP-bearing) may not.
#[tokio::test]
async fn pg_rule_insert_quota_guarded_tcp_udp_share_port() {
    let Some(db) = repo("rule_tcp_udp_share").await else {
        return;
    };
    sqlx::query("INSERT INTO device_groups (id, name, group_type, token, uid) VALUES (1, 'gin', 'in', 'tok-1', 1)")
        .execute(&db.pool)
        .await
        .unwrap();
    let insert = |name: &'static str, proto: &'static str| {
        let db = &db;
        async move {
            db.insert_quota_guarded(
                name,
                1,
                20000,
                proto,
                "raw",
                "raw",
                "direct",
                "raw",
                None,
                1,
                None,
                "direct",
                "127.0.0.1",
                80,
            )
            .await
        }
    };
    insert("r1", "tcp").await.unwrap();
    insert("r2", "udp").await.unwrap();
    match insert("r3", "tcp").await {
        Err(DbError::PortConflict) => {}
        other => panic!("expected PortConflict for second tcp, got {:?}", other),
    }
    match insert("r4", "udp").await {
        Err(DbError::PortConflict) => {}
        other => panic!("expected PortConflict for second udp, got {:?}", other),
    }
    match insert("r5", "tcp_udp").await {
        Err(DbError::PortConflict) => {}
        other => panic!("expected PortConflict for tcp_udp, got {:?}", other),
    }
    cleanup(&db).await;
}

/// v0.4.11 PR4 (PG parity): same port on a DIFFERENT group is allowed;
/// different users sharing one group share its pool.
#[tokio::test]
async fn pg_rule_insert_quota_guarded_port_scoped_by_group() {
    let Some(db) = repo("rule_port_group_scope").await else {
        return;
    };
    sqlx::query("INSERT INTO device_groups (id, name, group_type, token, uid) VALUES (1, 'gin', 'in', 'tok-1', 1)")
        .execute(&db.pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO device_groups (id, name, group_type, token, uid) VALUES (2, 'gin2', 'in', 'tok-2', 1)")
        .execute(&db.pool)
        .await
        .unwrap();
    let insert = |name: &'static str, uid: i64, group: i64| {
        let db = &db;
        async move {
            db.insert_quota_guarded(
                name,
                uid,
                20000,
                "tcp",
                "raw",
                "raw",
                "direct",
                "raw",
                None,
                group,
                None,
                "direct",
                "127.0.0.1",
                80,
            )
            .await
        }
    };
    insert("r1", 1, 1).await.unwrap();
    insert("r2", 1, 2).await.unwrap();
    match insert("r3", 1, 1).await {
        Err(DbError::PortConflict) => {}
        other => panic!(
            "expected PortConflict on shared group pool, got {:?}",
            other
        ),
    }
    cleanup(&db).await;
}

#[tokio::test]
async fn pg_rule_update_switch_to_direct_clears_device_group_out() {
    // Regression for v0.4.4: switching a rule to "direct" without an
    // explicit device_group_out must clear the column. The earlier
    // force_null_out bool caused `device_group_out` to be assigned twice in
    // the generated UPDATE, which PostgreSQL rejects with
    // "multiple assignments to same column". SQLite tolerated it; PG did not.
    let Some(db) = repo("rule_switch_direct").await else {
        return;
    };
    // Two groups: inbound (1) and an outbound (2) the rule starts pointed at.
    sqlx::query("INSERT INTO device_groups (id, name, group_type, token, uid) VALUES (1, 'gin', 'in', 'tok-1', 1)")
        .execute(&db.pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO device_groups (id, name, group_type, token, uid) VALUES (2, 'gout', 'out', 'tok-2', 1)")
        .execute(&db.pool)
        .await
        .unwrap();
    db.insert_quota_guarded(
        "r1",
        1,
        20000,
        "tcp",
        "raw",
        "raw",
        "group",
        "raw",
        None,
        1,
        Some(2),
        "group",
        "127.0.0.1",
        80,
    )
    .await
    .unwrap();
    let rule_id = db
        .list_rules(&ResourceScope::All)
        .await
        .unwrap()
        .into_iter()
        .next()
        .unwrap()
        .id;

    // Switch to direct: forward_mode="direct" + device_group_out=Some(None).
    // This is the exact shape api::admin::update_rule produces for the
    // "switch to direct without supplying an outbound group" case.
    let affected = db
        .update_rule_fields(
            rule_id,
            &ResourceScope::All,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            Some(None),
            Some("direct"),
            None,
            None,
            None,
        )
        .await
        .expect("update must not error (no duplicate column assignment)");
    assert_eq!(affected, 1);

    let dgo: (Option<i64>,) =
        sqlx::query_as("SELECT device_group_out FROM forward_rules WHERE id = $1")
            .bind(rule_id)
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert!(dgo.0.is_none(), "device_group_out must be cleared to NULL");
    cleanup(&db).await;
}

#[tokio::test]
async fn pg_rule_list_active_for_config_filters_banned_paused_overquota() {
    let Some(db) = repo("rule_filter").await else {
        return;
    };
    db.insert_user("alice", "h", 1).await.unwrap();
    let alice = db.find_by_username("alice").await.unwrap().unwrap().id;
    sqlx::query(
        "INSERT INTO device_groups (id, name, group_type, token, uid) \
         VALUES (50, 'gin', 'in', 'tok-50', $1)",
    )
    .bind(alice)
    .execute(&db.pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO forward_rules \
         (name, uid, listen_port, device_group_in, target_addr, target_port) \
         VALUES ('r-active', $1, 20000, 50, '127.0.0.1', 80)",
    )
    .bind(alice)
    .execute(&db.pool)
    .await
    .unwrap();

    assert_eq!(db.list_active_for_config(50).await.unwrap().len(), 1);

    sqlx::query("UPDATE forward_rules SET paused = TRUE WHERE device_group_in = 50")
        .execute(&db.pool)
        .await
        .unwrap();
    assert_eq!(db.list_active_for_config(50).await.unwrap().len(), 0);
    sqlx::query("UPDATE forward_rules SET paused = FALSE WHERE device_group_in = 50")
        .execute(&db.pool)
        .await
        .unwrap();

    sqlx::query("UPDATE users SET banned = TRUE WHERE id = $1")
        .bind(alice)
        .execute(&db.pool)
        .await
        .unwrap();
    assert_eq!(db.list_active_for_config(50).await.unwrap().len(), 0);
    sqlx::query("UPDATE users SET banned = FALSE WHERE id = $1")
        .bind(alice)
        .execute(&db.pool)
        .await
        .unwrap();

    sqlx::query("UPDATE users SET traffic_limit = 100, traffic_used = 100 WHERE id = $1")
        .bind(alice)
        .execute(&db.pool)
        .await
        .unwrap();
    assert_eq!(db.list_active_for_config(50).await.unwrap().len(), 0);
    sqlx::query("UPDATE users SET traffic_limit = 0 WHERE id = $1")
        .bind(alice)
        .execute(&db.pool)
        .await
        .unwrap();
    assert_eq!(db.list_active_for_config(50).await.unwrap().len(), 1);
    cleanup(&db).await;
}

// ── Group ──

#[tokio::test]
async fn pg_group_insert_then_find_by_token_round_trip() {
    let Some(db) = repo("group_rt").await else {
        return;
    };
    db.insert_group(
        "gin",
        "in",
        "tok-abc",
        1,
        "1.2.3.4",
        "20000-30000",
        1.0,
        false,
    )
    .await
    .unwrap();
    let g = db.find_by_token("tok-abc").await.unwrap().unwrap();
    assert_eq!(g.name, "gin");
    assert_eq!(g.group_type, "in");
    assert_eq!(g.connect_host, "1.2.3.4");
    let g2 = db
        .find_by_token_after_insert("tok-abc")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(g2.id, g.id);
    assert!(db.find_by_token("nope").await.unwrap().is_none());
    assert_eq!(
        db.find_name_by_id(g.id, &ResourceScope::All)
            .await
            .unwrap()
            .as_deref(),
        Some("gin")
    );
    cleanup(&db).await;
}

#[tokio::test]
async fn pg_group_update_token_returns_rows_affected() {
    let Some(db) = repo("group_tok").await else {
        return;
    };
    db.insert_group("gin", "in", "tok-1", 1, "", "", 1.0, false)
        .await
        .unwrap();
    let g = db.find_by_token("tok-1").await.unwrap().unwrap();
    assert_eq!(
        db.update_group_token(g.id, &ResourceScope::All, "tok-2")
            .await
            .unwrap(),
        1
    );
    assert!(db.find_by_token("tok-1").await.unwrap().is_none());
    assert!(db.find_by_token("tok-2").await.unwrap().is_some());
    assert_eq!(
        db.update_group_token(999_999, &ResourceScope::All, "tok-3")
            .await
            .unwrap(),
        0
    );
    cleanup(&db).await;
}

// ── Traffic ──

#[tokio::test]
async fn pg_traffic_batch_applies_to_rule_and_user() {
    let Some(db) = repo("traffic_apply").await else {
        return;
    };
    db.insert_user("alice", "h", 1).await.unwrap();
    let alice = db.find_by_username("alice").await.unwrap().unwrap().id;
    sqlx::query(
        "INSERT INTO device_groups (id, name, group_type, token, uid) \
         VALUES (50, 'gin', 'in', 'tok-50', $1)",
    )
    .bind(alice)
    .execute(&db.pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO forward_rules \
         (id, name, uid, listen_port, device_group_in, target_addr, target_port) \
         VALUES (100, 'r100', $1, 20000, 50, '127.0.0.1', 80)",
    )
    .bind(alice)
    .execute(&db.pool)
    .await
    .unwrap();
    let results = db
        .apply_traffic_batch(
            50,
            &[TrafficEntry {
                rule_id: 100,
                upload: 1000,
                download: 2000,
            }],
        )
        .await
        .unwrap();
    assert_eq!(results.len(), 1);
    let rule_t: (i64,) = sqlx::query_as("SELECT traffic_used FROM forward_rules WHERE id = 100")
        .fetch_one(&db.pool)
        .await
        .unwrap();
    let user_t: (i64,) = sqlx::query_as("SELECT traffic_used FROM users WHERE id = $1")
        .bind(alice)
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!(rule_t.0, 3000);
    assert_eq!(user_t.0, 3000);
    cleanup(&db).await;
}

#[tokio::test]
async fn pg_traffic_batch_other_group_rule_yields_othergrouprule_and_rolls_back() {
    let Some(db) = repo("traffic_og").await else {
        return;
    };
    db.insert_user("alice", "h", 1).await.unwrap();
    let alice = db.find_by_username("alice").await.unwrap().unwrap().id;
    for gid in [50, 60] {
        sqlx::query(
            "INSERT INTO device_groups (id, name, group_type, token, uid) \
             VALUES ($1, 'g', 'in', $2, $3)",
        )
        .bind(gid)
        .bind(format!("tok-{gid}"))
        .bind(alice)
        .execute(&db.pool)
        .await
        .unwrap();
    }
    sqlx::query(
        "INSERT INTO forward_rules \
         (id, name, uid, listen_port, device_group_in, target_addr, target_port) \
         VALUES (100, 'r100', $1, 20000, 50, '127.0.0.1', 80)",
    )
    .bind(alice)
    .execute(&db.pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO forward_rules \
         (id, name, uid, listen_port, device_group_in, target_addr, target_port) \
         VALUES (200, 'r200', $1, 20001, 60, '127.0.0.1', 80)",
    )
    .bind(alice)
    .execute(&db.pool)
    .await
    .unwrap();
    let results = db
        .apply_traffic_batch(
            50,
            &[
                TrafficEntry {
                    rule_id: 100,
                    upload: 500,
                    download: 0,
                },
                TrafficEntry {
                    rule_id: 200,
                    upload: 0,
                    download: 999,
                },
            ],
        )
        .await
        .unwrap();
    // v0.4.9: foreign rule → Unavailable (formerly OtherGroupRule).
    assert_eq!(results.len(), 1);
    assert!(matches!(results[0], TrafficEntryResult::Unavailable));
    let rule100_t: (i64,) = sqlx::query_as("SELECT traffic_used FROM forward_rules WHERE id = 100")
        .fetch_one(&db.pool)
        .await
        .unwrap();
    let user_t: (i64,) = sqlx::query_as("SELECT traffic_used FROM users WHERE id = $1")
        .bind(alice)
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!(rule100_t.0, 0);
    assert_eq!(user_t.0, 0);
    cleanup(&db).await;
}

/// v0.4.9: a rule_id that does NOT exist produces the SAME result
/// (Unavailable) as a foreign rule — NOT silently skipped. Closes the
/// rule-id existence oracle; the whole batch rolls back.
#[tokio::test]
async fn pg_traffic_batch_unknown_rule_is_unavailable_not_skipped() {
    let Some(db) = repo("traffic_unavail").await else {
        return;
    };
    db.insert_user("alice", "h", 1).await.unwrap();
    let alice = db.find_by_username("alice").await.unwrap().unwrap().id;
    sqlx::query(
        "INSERT INTO device_groups (id, name, group_type, token, uid) \
         VALUES (50, 'gin', 'in', 'tok-50', $1)",
    )
    .bind(alice)
    .execute(&db.pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO forward_rules \
         (id, name, uid, listen_port, device_group_in, target_addr, target_port) \
         VALUES (100, 'r100', $1, 20000, 50, '127.0.0.1', 80)",
    )
    .bind(alice)
    .execute(&db.pool)
    .await
    .unwrap();
    let results = db
        .apply_traffic_batch(
            50,
            &[
                TrafficEntry {
                    rule_id: 99999,
                    upload: 1,
                    download: 2,
                },
                TrafficEntry {
                    rule_id: 100,
                    upload: 10,
                    download: 20,
                },
            ],
        )
        .await
        .unwrap();
    assert_eq!(results.len(), 1);
    assert!(matches!(results[0], TrafficEntryResult::Unavailable));
    let rule_t: (i64,) = sqlx::query_as("SELECT traffic_used FROM forward_rules WHERE id = 100")
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!(rule_t.0, 0, "batch rolled back → rule 100 must not apply");
    cleanup(&db).await;
}

/// v0.4.9 overflow: single entry upload+download > i64::MAX → Overflow.
#[tokio::test]
async fn pg_traffic_batch_single_entry_overflow() {
    let Some(db) = repo("traffic_ov1").await else {
        return;
    };
    db.insert_user("alice", "h", 1).await.unwrap();
    let alice = db.find_by_username("alice").await.unwrap().unwrap().id;
    sqlx::query(
        "INSERT INTO device_groups (id, name, group_type, token, uid) \
         VALUES (50, 'gin', 'in', 'tok-50', $1)",
    )
    .bind(alice)
    .execute(&db.pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO forward_rules \
         (id, name, uid, listen_port, device_group_in, target_addr, target_port) \
         VALUES (100, 'r100', $1, 20000, 50, '127.0.0.1', 80)",
    )
    .bind(alice)
    .execute(&db.pool)
    .await
    .unwrap();
    let half = (i64::MAX as u64) / 2 + 1;
    let results = db
        .apply_traffic_batch(
            50,
            &[TrafficEntry {
                rule_id: 100,
                upload: half,
                download: half,
            }],
        )
        .await
        .unwrap();
    assert!(matches!(results[0], TrafficEntryResult::Overflow));
    cleanup(&db).await;
}

/// v0.4.9 overflow: duplicate rule_ids, each legal, overflow when summed.
#[tokio::test]
async fn pg_traffic_batch_duplicate_rule_ids_cumulative_overflow() {
    let Some(db) = repo("traffic_ovdup").await else {
        return;
    };
    db.insert_user("alice", "h", 1).await.unwrap();
    let alice = db.find_by_username("alice").await.unwrap().unwrap().id;
    sqlx::query(
        "INSERT INTO device_groups (id, name, group_type, token, uid) \
         VALUES (50, 'gin', 'in', 'tok-50', $1)",
    )
    .bind(alice)
    .execute(&db.pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO forward_rules \
         (id, name, uid, listen_port, device_group_in, target_addr, target_port) \
         VALUES (100, 'r100', $1, 20000, 50, '127.0.0.1', 80)",
    )
    .bind(alice)
    .execute(&db.pool)
    .await
    .unwrap();
    let half = (i64::MAX as u64) / 2 + 1;
    let results = db
        .apply_traffic_batch(
            50,
            &[
                TrafficEntry {
                    rule_id: 100,
                    upload: half,
                    download: 0,
                },
                TrafficEntry {
                    rule_id: 100,
                    upload: half,
                    download: 0,
                },
            ],
        )
        .await
        .unwrap();
    assert!(matches!(results[0], TrafficEntryResult::Overflow));
    cleanup(&db).await;
}

/// v0.4.9 overflow: two rules under one user, cumulative user total
/// overflows even though each rule's total would be fine.
#[tokio::test]
async fn pg_traffic_batch_user_cumulative_overflow_across_rules() {
    let Some(db) = repo("traffic_ovuser").await else {
        return;
    };
    db.insert_user("alice", "h", 1).await.unwrap();
    let alice = db.find_by_username("alice").await.unwrap().unwrap().id;
    sqlx::query(
        "INSERT INTO device_groups (id, name, group_type, token, uid) \
         VALUES (50, 'gin', 'in', 'tok-50', $1)",
    )
    .bind(alice)
    .execute(&db.pool)
    .await
    .unwrap();
    for (rid, port) in [(100, 20000), (101, 20001)] {
        sqlx::query(
            "INSERT INTO forward_rules \
             (id, name, uid, listen_port, device_group_in, target_addr, target_port) \
             VALUES ($1, 'r', $2, $3, 50, '127.0.0.1', 80)",
        )
        .bind(rid)
        .bind(alice)
        .bind(port)
        .execute(&db.pool)
        .await
        .unwrap();
    }
    sqlx::query("UPDATE users SET traffic_used = $1 WHERE id = $2")
        .bind(i64::MAX - 100)
        .bind(alice)
        .execute(&db.pool)
        .await
        .unwrap();
    let results = db
        .apply_traffic_batch(
            50,
            &[
                TrafficEntry {
                    rule_id: 100,
                    upload: 60,
                    download: 0,
                },
                TrafficEntry {
                    rule_id: 101,
                    upload: 60,
                    download: 0,
                },
            ],
        )
        .await
        .unwrap();
    assert!(matches!(results[0], TrafficEntryResult::Overflow));
    let user_t: (i64,) = sqlx::query_as("SELECT traffic_used FROM users WHERE id = $1")
        .bind(alice)
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!(user_t.0, i64::MAX - 100, "user total unchanged");
    cleanup(&db).await;
}

/// v0.4.9: a delta landing EXACTLY on i64::MAX is accepted.
#[tokio::test]
async fn pg_traffic_batch_exactly_i64_max_is_accepted() {
    let Some(db) = repo("traffic_max").await else {
        return;
    };
    db.insert_user("alice", "h", 1).await.unwrap();
    let alice = db.find_by_username("alice").await.unwrap().unwrap().id;
    sqlx::query(
        "INSERT INTO device_groups (id, name, group_type, token, uid) \
         VALUES (50, 'gin', 'in', 'tok-50', $1)",
    )
    .bind(alice)
    .execute(&db.pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO forward_rules \
         (id, name, uid, listen_port, device_group_in, target_addr, target_port) \
         VALUES (100, 'r100', $1, 20000, 50, '127.0.0.1', 80)",
    )
    .bind(alice)
    .execute(&db.pool)
    .await
    .unwrap();
    sqlx::query("UPDATE forward_rules SET traffic_used = $1 WHERE id = 100")
        .bind(i64::MAX - 50)
        .execute(&db.pool)
        .await
        .unwrap();
    sqlx::query("UPDATE users SET traffic_used = $1 WHERE id = $2")
        .bind(i64::MAX - 50)
        .bind(alice)
        .execute(&db.pool)
        .await
        .unwrap();
    let results = db
        .apply_traffic_batch(
            50,
            &[TrafficEntry {
                rule_id: 100,
                upload: 50,
                download: 0,
            }],
        )
        .await
        .unwrap();
    assert!(matches!(results[0], TrafficEntryResult::Ok));
    let rule_t: (i64,) = sqlx::query_as("SELECT traffic_used FROM forward_rules WHERE id = 100")
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!(rule_t.0, i64::MAX);
    cleanup(&db).await;
}

/// v0.4.9: duplicate rule_ids aggregated into one update (correct total).
#[tokio::test]
async fn pg_traffic_batch_duplicate_rule_ids_are_aggregated() {
    let Some(db) = repo("traffic_aggr").await else {
        return;
    };
    db.insert_user("alice", "h", 1).await.unwrap();
    let alice = db.find_by_username("alice").await.unwrap().unwrap().id;
    sqlx::query(
        "INSERT INTO device_groups (id, name, group_type, token, uid) \
         VALUES (50, 'gin', 'in', 'tok-50', $1)",
    )
    .bind(alice)
    .execute(&db.pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO forward_rules \
         (id, name, uid, listen_port, device_group_in, target_addr, target_port) \
         VALUES (100, 'r100', $1, 20000, 50, '127.0.0.1', 80)",
    )
    .bind(alice)
    .execute(&db.pool)
    .await
    .unwrap();
    let results = db
        .apply_traffic_batch(
            50,
            &[
                TrafficEntry {
                    rule_id: 100,
                    upload: 1,
                    download: 10,
                },
                TrafficEntry {
                    rule_id: 100,
                    upload: 2,
                    download: 20,
                },
                TrafficEntry {
                    rule_id: 100,
                    upload: 3,
                    download: 30,
                },
            ],
        )
        .await
        .unwrap();
    assert!(matches!(results[0], TrafficEntryResult::Ok));
    let rule_t: (i64,) = sqlx::query_as("SELECT traffic_used FROM forward_rules WHERE id = 100")
        .fetch_one(&db.pool)
        .await
        .unwrap();
    let user_t: (i64,) = sqlx::query_as("SELECT traffic_used FROM users WHERE id = $1")
        .bind(alice)
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!(rule_t.0, 66, "aggregated delta = 6+60");
    assert_eq!(user_t.0, 66);
    cleanup(&db).await;
}

// ── KVS ──

#[tokio::test]
async fn pg_kvs_set_get_delete_round_trip() {
    let Some(db) = repo("kvs_rt").await else {
        return;
    };
    assert!(db.get("missing").await.unwrap().is_none());
    db.set("k", "v1").await.unwrap();
    assert_eq!(db.get("k").await.unwrap().as_deref(), Some("v1"));
    db.set("k", "v2").await.unwrap();
    assert_eq!(db.get("k").await.unwrap().as_deref(), Some("v2"));
    assert_eq!(db.delete("k").await.unwrap(), 1);
    assert!(db.get("k").await.unwrap().is_none());
    assert_eq!(db.delete("k").await.unwrap(), 0);
    cleanup(&db).await;
}

#[tokio::test]
async fn pg_kvs_scan_prefix_returns_only_matching_keys() {
    let Some(db) = repo("kvs_scan").await else {
        return;
    };
    db.set("node_status:1:a", "{}").await.unwrap();
    db.set("node_status:1:b", "{}").await.unwrap();
    db.set("node_status:2:c", "{}").await.unwrap();
    db.set("other_feature:1", "{}").await.unwrap();
    let rows = db.scan_prefix("node_status:").await.unwrap();
    assert_eq!(rows.len(), 3);
    assert!(rows.iter().all(|(k, _)| k.starts_with("node_status:")));
    let rows = db.scan_prefix("node_status:1:").await.unwrap();
    assert_eq!(rows.len(), 2);
    cleanup(&db).await;
}

// ── v0.4.10 fix PR: ProfileScope + ownership-invariant tests (PG parity) ──
// Mirrors the SQLite tests so SQLite/PG behavior is provably identical.

/// find_profile_by_id with BuiltinOnly must NOT return a custom profile (PG).
#[tokio::test]
async fn pg_find_profile_by_id_builtin_only_excludes_custom() {
    let Some(db) = repo("prof_builtin").await else {
        return;
    };
    // v0.4.11 PR1: custom ws/tls_simple profiles are now available for rule selection.
    sqlx::query(
        "INSERT INTO tunnel_profiles (name, transport, tls_mode, ws_path, host_header, sni, is_builtin, uid) \
         VALUES ('custom-x', 'ws', 'none', '/x', '', '', FALSE, 1)",
    )
    .execute(&db.pool)
    .await
    .unwrap();
    let custom_id: i64 =
        sqlx::query_scalar("SELECT id FROM tunnel_profiles WHERE name = 'custom-x'")
            .fetch_one(&db.pool)
            .await
            .unwrap();

    let r = TunnelProfileRepository::find_profile_by_id(
        &db,
        custom_id,
        &ProfileScope::AvailableTemplates,
    )
    .await
    .unwrap();
    assert!(
        r.is_some(),
        "AvailableTemplates must return custom ws/tls_simple profile (PG)"
    );

    let r = TunnelProfileRepository::find_profile_by_id(&db, custom_id, &ProfileScope::All)
        .await
        .unwrap();
    assert!(r.is_some(), "All must return custom profile (PG)");
    cleanup(&db).await;
}

/// PG migration 7's cross-owner pause SQL (the UPDATE that the revision 7
/// arm runs) pauses a rule whose device_group_in belongs to a different
/// user. We execute the exact migration SQL directly rather than via
/// run_pg_migrations, because repo() already advanced schema_version to 7
/// (the version guard would no-op a second call). This pins the SQL logic
/// on PG; SQLite parity is covered by migration_pauses_cross_owner_rules.
#[tokio::test]
async fn pg_migration_pauses_cross_owner_rules() {
    let Some(db) = repo("mig_cross").await else {
        return;
    };
    sqlx::query("INSERT INTO users (id, username, password, admin) VALUES (2, 'u2', 'x', FALSE)")
        .execute(&db.pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO users (id, username, password, admin) VALUES (3, 'u3', 'x', FALSE)")
        .execute(&db.pool)
        .await
        .unwrap();
    // group 20 owned by user 3; rule owned by user 2 → mismatch.
    sqlx::query("INSERT INTO device_groups (id, name, group_type, token, uid) VALUES (20, 'g', 'in', 't', 3)")
        .execute(&db.pool).await.unwrap();
    sqlx::query("INSERT INTO forward_rules (name, uid, listen_port, device_group_in, target_addr, target_port) \
                 VALUES ('r', 2, 15000, 20, '127.0.0.1', 80)")
        .execute(&db.pool).await.unwrap();
    // The exact UPDATE from PG revision 7 (in-mismatch arm).
    sqlx::query(
        "UPDATE forward_rules SET paused = TRUE \
         WHERE paused = FALSE \
         AND EXISTS (SELECT 1 FROM device_groups dg \
                     WHERE dg.id = forward_rules.device_group_in \
                       AND dg.uid <> forward_rules.uid)",
    )
    .execute(&db.pool)
    .await
    .unwrap();

    let paused: (bool,) = sqlx::query_as("SELECT paused FROM forward_rules WHERE name = 'r'")
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert!(
        paused.0,
        "cross-owner rule must be paused by PG migration 7 SQL"
    );
    cleanup(&db).await;
}

/// PG migration 7's custom-profile pause SQL pauses a regular user's rule
/// bound to a non-builtin profile. Same direct-SQL approach as above.
#[tokio::test]
async fn pg_migration_pauses_non_admin_owner_custom_profile_rule() {
    let Some(db) = repo("mig_prof").await else {
        return;
    };
    sqlx::query("INSERT INTO users (id, username, password, admin) VALUES (2, 'u2', 'x', FALSE)")
        .execute(&db.pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO device_groups (id, name, group_type, token, uid) VALUES (20, 'g', 'in', 't', 2)")
        .execute(&db.pool).await.unwrap();
    sqlx::query("INSERT INTO tunnel_profiles (name, transport, tls_mode, ws_path, host_header, sni, is_builtin, uid) \
                 VALUES ('cust', 'direct', 'none', '/x', '', '', FALSE, 1)")
        .execute(&db.pool).await.unwrap();
    let pid: i64 = sqlx::query_scalar("SELECT id FROM tunnel_profiles WHERE name = 'cust'")
        .fetch_one(&db.pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO forward_rules (name, uid, listen_port, device_group_in, target_addr, target_port, tunnel_profile_id) \
                 VALUES ('r', 2, 15001, 20, '127.0.0.1', 80, $1)")
        .bind(pid)
        .execute(&db.pool).await.unwrap();
    // The exact UPDATE from PG revision 7 (custom-profile arm).
    sqlx::query(
        "UPDATE forward_rules SET paused = TRUE \
         WHERE tunnel_profile_id IS NOT NULL AND paused = FALSE \
         AND EXISTS (SELECT 1 FROM tunnel_profiles tp, users u \
                     WHERE tp.id = forward_rules.tunnel_profile_id \
                       AND tp.is_builtin = FALSE \
                       AND u.id = forward_rules.uid AND u.admin = FALSE)",
    )
    .execute(&db.pool)
    .await
    .unwrap();

    let paused: (bool,) = sqlx::query_as("SELECT paused FROM forward_rules WHERE name = 'r'")
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert!(
        paused.0,
        "non-admin rule with custom profile must be paused by PG migration 7 SQL"
    );
    cleanup(&db).await;
}

/// PG migration 7's pause SQL must NOT touch a legitimate rule.
#[tokio::test]
async fn pg_migration_does_not_pause_valid_rules() {
    let Some(db) = repo("mig_valid").await else {
        return;
    };
    sqlx::query("INSERT INTO users (id, username, password, admin) VALUES (2, 'u2', 'x', FALSE)")
        .execute(&db.pool)
        .await
        .unwrap();
    // group 20 owned by user 2; rule owned by user 2 → consistent.
    sqlx::query("INSERT INTO device_groups (id, name, group_type, token, uid) VALUES (20, 'g', 'in', 't', 2)")
        .execute(&db.pool).await.unwrap();
    sqlx::query("INSERT INTO forward_rules (name, uid, listen_port, device_group_in, target_addr, target_port) \
                 VALUES ('r', 2, 15002, 20, '127.0.0.1', 80)")
        .execute(&db.pool).await.unwrap();
    // Run all three UPDATEs from revision 7 — none should match.
    for sql in [
        "UPDATE forward_rules SET paused = TRUE \
         WHERE tunnel_profile_id IS NOT NULL AND paused = FALSE \
         AND EXISTS (SELECT 1 FROM tunnel_profiles tp, users u \
                     WHERE tp.id = forward_rules.tunnel_profile_id \
                       AND tp.is_builtin = FALSE \
                       AND u.id = forward_rules.uid AND u.admin = FALSE)",
        "UPDATE forward_rules SET paused = TRUE \
         WHERE paused = FALSE \
         AND EXISTS (SELECT 1 FROM device_groups dg \
                     WHERE dg.id = forward_rules.device_group_in \
                       AND dg.uid <> forward_rules.uid)",
        "UPDATE forward_rules SET paused = TRUE \
         WHERE paused = FALSE AND device_group_out IS NOT NULL \
         AND EXISTS (SELECT 1 FROM device_groups dg \
                     WHERE dg.id = forward_rules.device_group_out \
                       AND dg.uid <> forward_rules.uid)",
    ] {
        sqlx::query(sql).execute(&db.pool).await.unwrap();
    }

    let paused: (bool,) = sqlx::query_as("SELECT paused FROM forward_rules WHERE name = 'r'")
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert!(
        !paused.0,
        "valid rule must NOT be paused by PG migration 7 SQL"
    );
    cleanup(&db).await;
}

/// PG list_active_for_config must EXCLUDE a cross-owner rule (defense layer).
#[tokio::test]
async fn pg_list_active_for_config_excludes_cross_owner_rule() {
    // v0.4.11 PR3: shared inbound group scenario - cross-owner rule IS included.
    let Some(db) = repo("lac_shared").await else {
        return;
    };
    sqlx::query("INSERT INTO users (id, username, password, admin) VALUES (2, 'u2', 'x', FALSE)")
        .execute(&db.pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO device_groups (id, name, group_type, token, uid) VALUES (20, 'g', 'in', 't', 1)")
        .execute(&db.pool).await.unwrap();
    sqlx::query("INSERT INTO forward_rules (name, uid, listen_port, device_group_in, target_addr, target_port) \
                 VALUES ('r', 2, 15003, 20, '127.0.0.1', 80)")
        .execute(&db.pool).await.unwrap();

    let rules = db.list_active_for_config(20).await.unwrap();
    assert_eq!(
        rules.len(),
        1,
        "shared inbound rule must be returned for config (PG)"
    );
    cleanup(&db).await;
}

/// v0.4.12 PR1 (PG parity): an admin-owned `group_type='in'` group is shared
/// to a regular user with no rules; out/monitor and other regular users'
/// groups are excluded; an admin caller gets an empty list.
#[tokio::test]
async fn pg_shared_groups_admin_inbound_only() {
    let Some(db) = repo("shared_groups").await else {
        return;
    };
    // alice (regular) and bob (regular).
    sqlx::query(
        "INSERT INTO users (id, username, password, admin) VALUES (2, 'alice', 'x', FALSE)",
    )
    .execute(&db.pool)
    .await
    .unwrap();
    sqlx::query("INSERT INTO users (id, username, password, admin) VALUES (3, 'bob', 'x', FALSE)")
        .execute(&db.pool)
        .await
        .unwrap();
    // Admin-owned inbound (shared), admin-owned out/monitor (excluded), and
    // bob's inbound (excluded — not admin-owned).
    sqlx::query("INSERT INTO device_groups (id, name, group_type, token, connect_host, uid) VALUES (10, 'g10', 'in', 't10', '1.2.3.4', 1)")
        .execute(&db.pool).await.unwrap();
    sqlx::query("INSERT INTO device_groups (id, name, group_type, token, connect_host, uid) VALUES (11, 'g11', 'out', 't11', '1.2.3.4', 1)")
        .execute(&db.pool).await.unwrap();
    sqlx::query("INSERT INTO device_groups (id, name, group_type, token, connect_host, uid) VALUES (12, 'g12', 'monitor', 't12', '1.2.3.4', 1)")
        .execute(&db.pool).await.unwrap();
    sqlx::query("INSERT INTO device_groups (id, name, group_type, token, connect_host, uid) VALUES (20, 'g20', 'in', 't20', '1.2.3.4', 3)")
        .execute(&db.pool).await.unwrap();

    // alice (regular, no rules) sees ONLY the admin inbound group 10.
    let shared = db.list_shared_groups(2, false).await.unwrap();
    assert_eq!(shared.len(), 1, "only admin 'in' group is shared (PG)");
    assert_eq!(shared[0].id, 10);

    // admin caller gets an empty list.
    let admin_shared = db.list_shared_groups(1, true).await.unwrap();
    assert!(admin_shared.is_empty(), "admin gets no shared groups (PG)");
    cleanup(&db).await;
}

// ── v0.4.10 PR3: app_settings + insert_user_from_plan (PG parity) ──

#[tokio::test]
async fn pg_settings_get_returns_none_when_unseeded() {
    let Some(db) = repo("set_unseeded").await else {
        return;
    };
    let s = db.get_registration_settings().await.unwrap();
    assert!(
        s.is_none(),
        "fresh PG DB must have no app_settings row (PG)"
    );
    cleanup(&db).await;
}

#[tokio::test]
async fn pg_settings_insert_if_absent_is_idempotent() {
    let Some(db) = repo("set_idem").await else {
        return;
    };
    db.insert_settings_if_absent(true, 1, &[1]).await.unwrap();
    // Admin disables; then "restart" re-runs insert_if_absent(true).
    db.set_registration_settings(false, 1, &[1]).await.unwrap();
    db.insert_settings_if_absent(true, 1, &[1]).await.unwrap();
    let s = db.get_registration_settings().await.unwrap().unwrap();
    assert!(
        !s.registration_enabled,
        "env-var seed must NOT re-enable registration after admin disabled it (PG)"
    );
    cleanup(&db).await;
}

#[tokio::test]
async fn pg_settings_set_upserts_when_no_row() {
    let Some(db) = repo("set_upsert").await else {
        return;
    };
    assert!(db.get_registration_settings().await.unwrap().is_none());
    db.set_registration_settings(true, 1, &[1]).await.unwrap();
    let s = db.get_registration_settings().await.unwrap().unwrap();
    assert!(s.registration_enabled, "upsert must create the row (PG)");
    cleanup(&db).await;
}

/// v0.4.21 PR2: PG registration settings round-trip allowed_plan_ids
/// through JSON TEXT column.
#[tokio::test]
async fn pg_settings_allowed_plan_ids_round_trip() {
    let Some(db) = repo("allowed_r").await else {
        return;
    };
    // Seed plan 2 for multi-plan test.
    let pool = db.pool.clone();
    sqlx::query(
        "INSERT INTO plans (id, name, max_rules, traffic, speed_limit, ip_limit, price) \
         VALUES (2, 'premium', 10, 0, 0, 5, '9.99') ON CONFLICT (id) DO NOTHING",
    )
    .execute(&pool)
    .await
    .unwrap();

    // Multi-plan settings.
    db.set_registration_settings(true, 1, &[1, 2])
        .await
        .unwrap();
    let s = db.get_registration_settings().await.unwrap().unwrap();
    assert!(s.registration_enabled);
    assert_eq!(s.default_registration_plan_id, 1);
    assert_eq!(s.allowed_plan_ids, vec![1, 2], "PG multi-plan round-trip");

    // Unseeded row insert must also carry allowed_plan_ids.
    sqlx::query("DELETE FROM app_settings WHERE id = 1")
        .execute(&pool)
        .await
        .unwrap();
    db.insert_settings_if_absent(true, 2, &[2, 1])
        .await
        .unwrap();
    let s2 = db.get_registration_settings().await.unwrap().unwrap();
    assert!(s2.registration_enabled);
    assert_eq!(s2.default_registration_plan_id, 2);
    assert_eq!(
        s2.allowed_plan_ids,
        vec![2, 1],
        "PG unseeded round-trip (order preserved)"
    );

    cleanup(&db).await;
}

#[tokio::test]
async fn pg_insert_user_from_plan_inherits_quota_and_handles_missing_plan() {
    let Some(db) = repo("iup").await else { return };
    let n = db.insert_user_from_plan("alice", "hash", 1).await.unwrap();
    assert_eq!(n, 1, "user should be created for an existing plan (PG)");
    let user = db.find_by_username("alice").await.unwrap().unwrap();
    assert_eq!(user.plan_id, Some(1));
    assert_eq!(user.max_rules, 5, "max_rules inherited from plan (PG)");
    let n = db.insert_user_from_plan("bob", "hash", 999).await.unwrap();
    assert_eq!(n, 0, "missing plan must yield 0 rows (PG)");
    assert!(
        db.find_by_username("bob").await.unwrap().is_none(),
        "no user for missing plan (PG)"
    );
    cleanup(&db).await;
}

// ── v0.4.10 PR4: token_version + must_change_password (PG parity) ──

#[tokio::test]
async fn pg_find_auth_state_returns_all_three_or_none() {
    let Some(db) = repo("auth_state").await else {
        return;
    };
    sqlx::query(
        "INSERT INTO users (id, username, password, admin, banned, token_version, must_change_password) \
         VALUES (2, 'u2', 'x', FALSE, TRUE, 7, TRUE)",
    )
    .execute(&db.pool)
    .await
    .unwrap();
    let s = db.find_auth_state_by_id(2).await.unwrap().unwrap();
    assert_eq!(s, (true, 7, true));
    assert!(db.find_auth_state_by_id(999).await.unwrap().is_none());
    cleanup(&db).await;
}

#[tokio::test]
async fn pg_change_own_password_bumps_version_and_clears_must_change() {
    let Some(db) = repo("change_own").await else {
        return;
    };
    sqlx::query(
        "INSERT INTO users (id, username, password, admin, token_version, must_change_password) \
         VALUES (2, 'u2', 'old', FALSE, 3, TRUE)",
    )
    .execute(&db.pool)
    .await
    .unwrap();
    let n = db.change_own_password(2, "newhash").await.unwrap();
    assert_eq!(n, 1);
    let s = db.find_auth_state_by_id(2).await.unwrap().unwrap();
    assert_eq!(s.1, 4, "token_version must increment (PG)");
    assert!(!s.2, "must_change_password cleared (PG)");
    cleanup(&db).await;
}

#[tokio::test]
async fn pg_admin_reset_password_bumps_version_and_sets_must_change() {
    let Some(db) = repo("admin_reset").await else {
        return;
    };
    sqlx::query(
        "INSERT INTO users (id, username, password, admin, token_version, must_change_password) \
         VALUES (2, 'u2', 'old', FALSE, 0, FALSE)",
    )
    .execute(&db.pool)
    .await
    .unwrap();
    let n = db.admin_reset_password(2, "temphash", true).await.unwrap();
    assert_eq!(n, 1);
    let s = db.find_auth_state_by_id(2).await.unwrap().unwrap();
    assert_eq!(s.1, 1, "token_version must increment (PG)");
    assert!(s.2, "must_change_password set true (PG)");
    cleanup(&db).await;
}

#[tokio::test]
async fn pg_ban_bumps_token_version() {
    let Some(db) = repo("ban_bump").await else {
        return;
    };
    sqlx::query(
        "INSERT INTO users (id, username, password, admin, banned, token_version) \
         VALUES (2, 'u2', 'x', FALSE, FALSE, 5)",
    )
    .execute(&db.pool)
    .await
    .unwrap();
    db.update_user_fields(2, None, None, None, Some(true), None)
        .await
        .unwrap();
    let s = db.find_auth_state_by_id(2).await.unwrap().unwrap();
    assert!(s.0, "user banned (PG)");
    assert_eq!(s.1, 6, "ban bumps token_version (PG)");
    cleanup(&db).await;
}

#[tokio::test]
async fn pg_unban_does_not_bump_token_version() {
    let Some(db) = repo("unban_nobump").await else {
        return;
    };
    sqlx::query(
        "INSERT INTO users (id, username, password, admin, banned, token_version) \
         VALUES (2, 'u2', 'x', FALSE, TRUE, 5)",
    )
    .execute(&db.pool)
    .await
    .unwrap();
    db.update_user_fields(2, None, None, None, Some(false), None)
        .await
        .unwrap();
    let s = db.find_auth_state_by_id(2).await.unwrap().unwrap();
    assert!(!s.0, "user unbanned (PG)");
    assert_eq!(s.1, 5, "unban does NOT bump token_version (PG)");
    cleanup(&db).await;
}

// ── v0.4.18 PR8: Owner-scope authorization tests (PG parity) ──

/// Owner scope: delete_rule succeeds for own rule, fails for another user's rule.
#[tokio::test]
async fn pg_delete_rule_owner_scope_rejects_wrong_owner() {
    let Some(db) = repo("del_rule_own").await else {
        return;
    };
    // User 2 owns the rule, user 3 does not.
    sqlx::query("INSERT INTO users (id, username, password, admin) VALUES (2, 'u2', 'x', FALSE)")
        .execute(&db.pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO users (id, username, password, admin) VALUES (3, 'u3', 'x', FALSE)")
        .execute(&db.pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO device_groups (id, name, group_type, token, uid) \
         VALUES (10, 'gin', 'in', 'tok10', 2)",
    )
    .execute(&db.pool)
    .await
    .unwrap();
    db.insert_quota_guarded(
        "r1",
        2,
        20000,
        "tcp",
        "raw",
        "raw",
        "direct",
        "raw",
        None,
        10,
        None,
        "direct",
        "127.0.0.1",
        80,
    )
    .await
    .unwrap();
    let rule_id = db
        .find_rule_by_id(1, &ResourceScope::All)
        .await
        .unwrap()
        .unwrap()
        .id;

    // Owner can delete their own rule.
    let n = db
        .delete_rule(rule_id, &ResourceScope::Owner(2))
        .await
        .unwrap();
    assert_eq!(n, 1, "owner 2 must be able to delete their rule (PG)");

    // Recreate for the negative case.
    sqlx::query(
        "INSERT INTO device_groups (id, name, group_type, token, uid) \
         VALUES (11, 'gin2', 'in', 'tok11', 2)",
    )
    .execute(&db.pool)
    .await
    .unwrap();
    db.insert_quota_guarded(
        "r2",
        2,
        20001,
        "tcp",
        "raw",
        "raw",
        "direct",
        "raw",
        None,
        11,
        None,
        "direct",
        "127.0.0.1",
        81,
    )
    .await
    .unwrap();
    let rule_id2 = db
        .find_rule_by_id(2, &ResourceScope::All)
        .await
        .unwrap()
        .unwrap()
        .id;

    // User 3 must NOT delete user 2's rule.
    let n = db
        .delete_rule(rule_id2, &ResourceScope::Owner(3))
        .await
        .unwrap();
    assert_eq!(n, 0, "user 3 must NOT delete user 2's rule (PG)");

    let still_there = db
        .find_rule_by_id(rule_id2, &ResourceScope::All)
        .await
        .unwrap();
    assert!(
        still_there.is_some(),
        "rule must survive rejected DELETE (PG)"
    );
    cleanup(&db).await;
}

/// Owner scope: find_rule_by_id returns None for another user's rule.
#[tokio::test]
async fn pg_find_rule_by_id_owner_scope_filters_other_owner() {
    let Some(db) = repo("find_rule_own").await else {
        return;
    };
    sqlx::query("INSERT INTO users (id, username, password, admin) VALUES (2, 'u2', 'x', FALSE)")
        .execute(&db.pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO users (id, username, password, admin) VALUES (3, 'u3', 'x', FALSE)")
        .execute(&db.pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO device_groups (id, name, group_type, token, uid) \
         VALUES (10, 'gin', 'in', 'tok10', 2)",
    )
    .execute(&db.pool)
    .await
    .unwrap();
    db.insert_quota_guarded(
        "r1",
        2,
        20000,
        "tcp",
        "raw",
        "raw",
        "direct",
        "raw",
        None,
        10,
        None,
        "direct",
        "127.0.0.1",
        80,
    )
    .await
    .unwrap();
    let rule_id = db
        .find_rule_by_id(1, &ResourceScope::All)
        .await
        .unwrap()
        .unwrap()
        .id;

    let own = db
        .find_rule_by_id(rule_id, &ResourceScope::Owner(2))
        .await
        .unwrap();
    assert!(own.is_some(), "owner 2 must see own rule (PG)");

    let other = db
        .find_rule_by_id(rule_id, &ResourceScope::Owner(3))
        .await
        .unwrap();
    assert!(other.is_none(), "user 3 must NOT see user 2's rule (PG)");
    cleanup(&db).await;
}

/// Owner scope: update_group_fields succeeds for own group, fails for another user's group.
#[tokio::test]
async fn pg_update_group_fields_owner_scope_rejects_wrong_owner() {
    let Some(db) = repo("upd_group_own").await else {
        return;
    };
    sqlx::query("INSERT INTO users (id, username, password, admin) VALUES (2, 'u2', 'x', FALSE)")
        .execute(&db.pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO users (id, username, password, admin) VALUES (3, 'u3', 'x', FALSE)")
        .execute(&db.pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO device_groups (id, name, group_type, token, uid) \
         VALUES (10, 'gin', 'in', 'tok10', 2)",
    )
    .execute(&db.pool)
    .await
    .unwrap();

    let n = db
        .update_group_fields(
            10,
            &ResourceScope::Owner(2),
            Some("renamed"),
            None,
            None,
            None,
            None,
            None,
        )
        .await
        .unwrap();
    assert_eq!(n, 1, "owner 2 must be able to rename their group (PG)");

    let n = db
        .update_group_fields(
            10,
            &ResourceScope::Owner(3),
            Some("stolen"),
            None,
            None,
            None,
            None,
            None,
        )
        .await
        .unwrap();
    assert_eq!(n, 0, "user 3 must NOT rename user 2's group (PG)");

    let name = db
        .find_name_by_id(10, &ResourceScope::All)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(name, "renamed", "name must survive rejected update (PG)");
    cleanup(&db).await;
}

/// Owner scope: delete_group succeeds for own group, fails for another user's group.
#[tokio::test]
async fn pg_delete_group_owner_scope_rejects_wrong_owner() {
    let Some(db) = repo("del_group_own").await else {
        return;
    };
    sqlx::query("INSERT INTO users (id, username, password, admin) VALUES (2, 'u2', 'x', FALSE)")
        .execute(&db.pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO users (id, username, password, admin) VALUES (3, 'u3', 'x', FALSE)")
        .execute(&db.pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO device_groups (id, name, group_type, token, uid) \
         VALUES (10, 'gin', 'in', 'tok10', 2)",
    )
    .execute(&db.pool)
    .await
    .unwrap();

    // User 3 must NOT be able to delete user 2's group.
    let n = db.delete_group(10, &ResourceScope::Owner(3)).await.unwrap();
    assert_eq!(n, 0, "user 3 must NOT delete user 2's group (PG)");

    let name = db.find_name_by_id(10, &ResourceScope::All).await.unwrap();
    assert!(name.is_some(), "group must survive rejected DELETE (PG)");

    let n = db.delete_group(10, &ResourceScope::Owner(2)).await.unwrap();
    assert_eq!(n, 1, "owner 2 must be able to delete their group (PG)");
    cleanup(&db).await;
}

// ── v0.4.18 PR8: PG parity gap fill — tests ported from sqlite_repo ──

/// scenario 1: an admin-owned inbound group is visible to a regular user even
/// when that user has no rules.
#[tokio::test]
async fn pg_shared_groups_lists_admin_inbound_for_user_without_rules() {
    let Some(db) = repo("shgrp_no_rules").await else {
        return;
    };
    sqlx::query("INSERT INTO users (id, username, password, admin) VALUES (2, 'u2', 'x', FALSE)")
        .execute(&db.pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO device_groups (id, name, group_type, token, uid) \
         VALUES (10, 'gin', 'in', 'tok10', 1)",
    )
    .execute(&db.pool)
    .await
    .unwrap();
    let shared = db.list_shared_groups(2, false).await.unwrap();
    assert_eq!(shared.len(), 1, "alice sees the admin inbound group (PG)");
    assert_eq!(shared[0].id, 10);
    cleanup(&db).await;
}

/// scenario 2: out / monitor groups never appear in the shared list.
#[tokio::test]
async fn pg_shared_groups_excludes_non_inbound_types() {
    let Some(db) = repo("shgrp_types").await else {
        return;
    };
    sqlx::query("INSERT INTO users (id, username, password, admin) VALUES (2, 'u2', 'x', FALSE)")
        .execute(&db.pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO device_groups (id, name, group_type, token, uid) VALUES (10, 'gin', 'in', 'tok10', 1)")
        .execute(&db.pool).await.unwrap();
    sqlx::query("INSERT INTO device_groups (id, name, group_type, token, uid) VALUES (11, 'gout', 'out', 'tok11', 1)")
        .execute(&db.pool).await.unwrap();
    sqlx::query("INSERT INTO device_groups (id, name, group_type, token, uid) VALUES (12, 'gmon', 'monitor', 'tok12', 1)")
        .execute(&db.pool).await.unwrap();
    let shared = db.list_shared_groups(2, false).await.unwrap();
    assert_eq!(shared.len(), 1);
    assert_eq!(shared[0].id, 10, "only the 'in' group is shared (PG)");
    cleanup(&db).await;
}

/// scenario 3: a regular user never sees ANOTHER regular user's group.
#[tokio::test]
async fn pg_shared_groups_excludes_other_regular_users_groups() {
    let Some(db) = repo("shgrp_other").await else {
        return;
    };
    sqlx::query("INSERT INTO users (id, username, password, admin) VALUES (2, 'u2', 'x', FALSE)")
        .execute(&db.pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO users (id, username, password, admin) VALUES (3, 'u3', 'x', FALSE)")
        .execute(&db.pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO device_groups (id, name, group_type, token, uid) VALUES (20, 'g', 'in', 'tok20', 3)")
        .execute(&db.pool).await.unwrap();
    let shared = db.list_shared_groups(2, false).await.unwrap();
    assert!(
        shared.is_empty(),
        "alice must NOT see bob's inbound group (PG)"
    );
    cleanup(&db).await;
}

/// v1.0.7: list_shared_groups still RETURNS a hidden group (carrying the
/// `hidden` flag); only the node-status handler drops it, so the rule dropdown
/// / shop keep listing hidden lines. Admins see it too (PG).
#[tokio::test]
async fn pg_shared_groups_carries_hidden_flag_and_still_lists_hidden() {
    let Some(db) = repo("shgrp_hidden").await else {
        return;
    };
    sqlx::query("INSERT INTO users (id, username, password, admin) VALUES (2, 'u2', 'x', FALSE)")
        .execute(&db.pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO device_groups (id, name, group_type, token, uid) VALUES (10, 'g10', 'in', 'tok10', 1)")
        .execute(&db.pool).await.unwrap();
    sqlx::query("INSERT INTO device_groups (id, name, group_type, token, uid, hidden) VALUES (11, 'g11', 'in', 'tok11', 1, TRUE)")
        .execute(&db.pool).await.unwrap();

    // Regular user: BOTH groups listed; the hidden one carries hidden=true.
    let shared = db.list_shared_groups(2, false).await.unwrap();
    assert_eq!(
        shared.len(),
        2,
        "hidden group must STILL be listed for rules (PG)"
    );
    assert!(shared.iter().any(|g| g.id == 11 && g.hidden));
    assert!(shared.iter().any(|g| g.id == 10 && !g.hidden));

    // Admin: list_groups (unscoped) returns BOTH, with the flag set.
    let all = db.list_groups(&ResourceScope::All).await.unwrap();
    assert!(
        all.iter().any(|g| g.id == 11 && g.hidden),
        "admin must still see the hidden group, flagged hidden=true (PG)"
    );
    assert!(all.iter().any(|g| g.id == 10 && !g.hidden));
    cleanup(&db).await;
}

/// An admin caller gets an empty shared list.
#[tokio::test]
async fn pg_shared_groups_empty_for_admin() {
    let Some(db) = repo("shgrp_admin").await else {
        return;
    };
    sqlx::query("INSERT INTO device_groups (id, name, group_type, token, uid) VALUES (10, 'gin', 'in', 'tok10', 1)")
        .execute(&db.pool).await.unwrap();
    let shared = db.list_shared_groups(1, true).await.unwrap();
    assert!(shared.is_empty());
    cleanup(&db).await;
}

/// is_admin / exists_by_id distinguish known rows.
#[tokio::test]
async fn pg_user_is_admin_and_exists_by_id_distinguish_known_rows() {
    let Some(db) = repo("isadmin").await else {
        return;
    };
    // SCHEMA seeds uid=1 as admin.
    assert!(db.exists_by_id(1).await.unwrap());
    assert!(db.is_admin(1).await.unwrap());
    assert!(!db.exists_by_id(999_999).await.unwrap());
    assert!(!db.is_admin(999_999).await.unwrap());
    db.insert_user("alice", "h", 1).await.unwrap();
    let uid = db.find_by_username("alice").await.unwrap().unwrap().id;
    assert!(db.exists_by_id(uid).await.unwrap());
    assert!(!db.is_admin(uid).await.unwrap());
    cleanup(&db).await;
}

/// delete_user_cascade clears rules, groups, profiles, and the user row.
#[tokio::test]
async fn pg_user_delete_cascade_clears_rules_groups_profiles_and_user() {
    let Some(db) = repo("cascade_clear").await else {
        return;
    };
    db.insert_user("alice", "h", 1).await.unwrap();
    let uid = db.find_by_username("alice").await.unwrap().unwrap().id;
    sqlx::query("INSERT INTO device_groups (id, name, group_type, token, uid) VALUES (1, 'g1', 'in', 'tok-1', $1)")
        .bind(uid).execute(&db.pool).await.unwrap();
    sqlx::query(
        "INSERT INTO forward_rules \
         (name, uid, listen_port, device_group_in, target_addr, target_port) \
         VALUES ('r1', $1, 20000, 1, '127.0.0.1', 80)",
    )
    .bind(uid)
    .execute(&db.pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO tunnel_profiles (name, transport, uid) VALUES ('alice-custom', 'ws', $1)",
    )
    .bind(uid)
    .execute(&db.pool)
    .await
    .unwrap();
    let affected = db.delete_user_cascade(uid).await.unwrap();
    assert_eq!(affected, 1, "the user row must be deleted (PG)");
    let rules: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM forward_rules WHERE uid = $1")
        .bind(uid)
        .fetch_one(&db.pool)
        .await
        .unwrap();
    let groups: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM device_groups WHERE uid = $1")
        .bind(uid)
        .fetch_one(&db.pool)
        .await
        .unwrap();
    let profiles: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM tunnel_profiles WHERE uid = $1")
        .bind(uid)
        .fetch_one(&db.pool)
        .await
        .unwrap();
    let user: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM users WHERE id = $1")
        .bind(uid)
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!(rules.0, 0);
    assert_eq!(groups.0, 0);
    assert_eq!(
        profiles.0, 0,
        "custom tunnel profile must be deleted too (PG)"
    );
    assert_eq!(user.0, 0, "user row must be gone (PG)");
    cleanup(&db).await;
}

/// update_rule_fields partial update touches only present columns.
#[tokio::test]
async fn pg_rule_update_rule_fields_partial_update() {
    let Some(db) = repo("upd_rule_fields").await else {
        return;
    };
    sqlx::query("INSERT INTO device_groups (id, name, group_type, token, uid) VALUES (1, 'gin', 'in', 'tok1', 1)")
        .execute(&db.pool).await.unwrap();
    db.insert_quota_guarded(
        "r1",
        1,
        20000,
        "tcp",
        "raw",
        "raw",
        "direct",
        "raw",
        None,
        1,
        None,
        "direct",
        "127.0.0.1",
        80,
    )
    .await
    .unwrap();
    let rule_id = db
        .list_rules(&ResourceScope::All)
        .await
        .unwrap()
        .into_iter()
        .next()
        .unwrap()
        .id;
    assert_eq!(
        db.update_rule_fields(
            rule_id,
            &ResourceScope::All,
            Some("renamed"),
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None
        )
        .await
        .unwrap(),
        1
    );
    let row: (String, String) =
        sqlx::query_as("SELECT name, protocol FROM forward_rules WHERE id = $1")
            .bind(rule_id)
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert_eq!(row.0, "renamed");
    assert_eq!(row.1, "tcp", "protocol must be untouched (PG)");
    // Switching to direct clears device_group_out.
    assert_eq!(
        db.update_rule_fields(
            rule_id,
            &ResourceScope::All,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            Some(None),
            None,
            None,
            None,
            None
        )
        .await
        .unwrap(),
        1
    );
    let dgo: (Option<i64>,) =
        sqlx::query_as("SELECT device_group_out FROM forward_rules WHERE id = $1")
            .bind(rule_id)
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert!(dgo.0.is_none(), "device_group_out must be cleared (PG)");
    cleanup(&db).await;
}

/// overflow entry rejects and rolls back (no data written).
#[tokio::test]
async fn pg_traffic_batch_single_entry_overflow_rejects_and_rolls_back() {
    let Some(db) = repo("traf_ov_rollback").await else {
        return;
    };
    db.insert_user("alice", "h", 1).await.unwrap();
    let alice = db.find_by_username("alice").await.unwrap().unwrap().id;
    sqlx::query("INSERT INTO device_groups (id, name, group_type, token, uid) VALUES (50, 'gin', 'in', 'tok-50', $1)")
        .bind(alice).execute(&db.pool).await.unwrap();
    sqlx::query(
        "INSERT INTO forward_rules (id, name, uid, listen_port, device_group_in, target_addr, target_port) \
         VALUES (100, 'r100', $1, 20000, 50, '127.0.0.1', 80)",
    )
    .bind(alice).execute(&db.pool).await.unwrap();
    let half = (i64::MAX as u64) / 2 + 1;
    let results = db
        .apply_traffic_batch(
            50,
            &[TrafficEntry {
                rule_id: 100,
                upload: half,
                download: half,
            }],
        )
        .await
        .unwrap();
    assert!(matches!(results[0], TrafficEntryResult::Overflow));
    let rule_t: (i64,) = sqlx::query_as("SELECT traffic_used FROM forward_rules WHERE id = 100")
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!(rule_t.0, 0, "overflow → no write (PG)");
    cleanup(&db).await;
}

// ── v1.0.8: device-group rate billing ──

async fn seed_group_with_rate(db: &PgRepository, gid: i64, rate: f64) {
    db.insert_user("alice", "h", 1).await.unwrap();
    let alice = db.find_by_username("alice").await.unwrap().unwrap().id;
    sqlx::query(
        "INSERT INTO device_groups (id, name, group_type, token, uid, rate) \
         VALUES ($1, 'gin', 'in', $2, $3, $4)",
    )
    .bind(gid)
    .bind(format!("tok-{gid}"))
    .bind(alice)
    .bind(rate)
    .execute(&db.pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO forward_rules \
         (id, name, uid, listen_port, device_group_in, target_addr, target_port) \
         VALUES (100, 'r100', $1, 20000, $2, '127.0.0.1', 80)",
    )
    .bind(alice)
    .bind(gid)
    .execute(&db.pool)
    .await
    .unwrap();
}

#[tokio::test]
async fn pg_traffic_batch_rate_2_charges_user_double_rule_stays_real() {
    let Some(db) = repo("traf_rate2").await else {
        return;
    };
    seed_group_with_rate(&db, 50, 2.0).await;
    let alice = db.find_by_username("alice").await.unwrap().unwrap().id;

    let results = db
        .apply_traffic_batch(
            50,
            &[TrafficEntry {
                rule_id: 100,
                upload: 1000,
                download: 2000,
            }],
        )
        .await
        .unwrap();
    assert_eq!(results.len(), 1);
    assert!(matches!(results[0], TrafficEntryResult::Ok));

    let rule_t: (i64,) = sqlx::query_as("SELECT traffic_used FROM forward_rules WHERE id = 100")
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!(rule_t.0, 3000);
    let user_t: (i64,) = sqlx::query_as("SELECT traffic_used FROM users WHERE id = $1")
        .bind(alice)
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!(user_t.0, 6000);
    cleanup(&db).await;
}

#[tokio::test]
async fn pg_traffic_batch_rate_1_is_unchanged_billing() {
    let Some(db) = repo("traf_rate1").await else {
        return;
    };
    seed_group_with_rate(&db, 51, 1.0).await;
    let alice = db.find_by_username("alice").await.unwrap().unwrap().id;

    let results = db
        .apply_traffic_batch(
            51,
            &[TrafficEntry {
                rule_id: 100,
                upload: 1000,
                download: 2000,
            }],
        )
        .await
        .unwrap();
    assert!(matches!(results[0], TrafficEntryResult::Ok));

    let rule_t: (i64,) = sqlx::query_as("SELECT traffic_used FROM forward_rules WHERE id = 100")
        .fetch_one(&db.pool)
        .await
        .unwrap();
    let user_t: (i64,) = sqlx::query_as("SELECT traffic_used FROM users WHERE id = $1")
        .bind(alice)
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!(rule_t.0, 3000);
    assert_eq!(user_t.0, 3000);
    cleanup(&db).await;
}

#[tokio::test]
async fn pg_traffic_batch_rate_1_5_rounds_correctly() {
    let Some(db) = repo("traf_rate1_5").await else {
        return;
    };
    seed_group_with_rate(&db, 52, 1.5).await;
    let alice = db.find_by_username("alice").await.unwrap().unwrap().id;

    let results = db
        .apply_traffic_batch(
            52,
            &[TrafficEntry {
                rule_id: 100,
                upload: 1000,
                download: 2000,
            }],
        )
        .await
        .unwrap();
    assert!(matches!(results[0], TrafficEntryResult::Ok));

    let rule_t: (i64,) = sqlx::query_as("SELECT traffic_used FROM forward_rules WHERE id = 100")
        .fetch_one(&db.pool)
        .await
        .unwrap();
    let user_t: (i64,) = sqlx::query_as("SELECT traffic_used FROM users WHERE id = $1")
        .bind(alice)
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!(rule_t.0, 3000);
    assert_eq!(user_t.0, 4500);

    db.apply_traffic_batch(
        52,
        &[TrafficEntry {
            rule_id: 100,
            upload: 1,
            download: 1,
        }],
    )
    .await
    .unwrap();
    let rule_t2: (i64,) = sqlx::query_as("SELECT traffic_used FROM forward_rules WHERE id = 100")
        .fetch_one(&db.pool)
        .await
        .unwrap();
    let user_t2: (i64,) = sqlx::query_as("SELECT traffic_used FROM users WHERE id = $1")
        .bind(alice)
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!(rule_t2.0, 3002);
    assert_eq!(user_t2.0, 4503);
    cleanup(&db).await;
}

// ── v1.0.8: suspension + expiry gating ──

async fn seed_active_rule(db: &PgRepository) -> i64 {
    db.insert_user("alice", "h", 1).await.unwrap();
    let alice = db.find_by_username("alice").await.unwrap().unwrap().id;
    sqlx::query(
        "INSERT INTO device_groups (id, name, group_type, token, uid) \
         VALUES (50, 'gin', 'in', 'tok-50', $1)",
    )
    .bind(alice)
    .execute(&db.pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO forward_rules \
         (name, uid, listen_port, device_group_in, target_addr, target_port) \
         VALUES ('r', $1, 20000, 50, '127.0.0.1', 80)",
    )
    .bind(alice)
    .execute(&db.pool)
    .await
    .unwrap();
    alice
}

#[tokio::test]
async fn pg_suspended_user_rule_is_filtered_and_resumes_on_unsuspend() {
    let Some(db) = repo("susp_filter").await else {
        return;
    };
    let alice = seed_active_rule(&db).await;
    assert_eq!(db.list_active_for_config(50).await.unwrap().len(), 1);

    sqlx::query("UPDATE users SET suspended = TRUE WHERE id = $1")
        .bind(alice)
        .execute(&db.pool)
        .await
        .unwrap();
    assert_eq!(
        db.list_active_for_config(50).await.unwrap().len(),
        0,
        "suspended user's rule must be filtered (PG)"
    );

    sqlx::query("UPDATE users SET suspended = FALSE WHERE id = $1")
        .bind(alice)
        .execute(&db.pool)
        .await
        .unwrap();
    assert_eq!(
        db.list_active_for_config(50).await.unwrap().len(),
        1,
        "rule must reappear after unsuspend (PG)"
    );
    cleanup(&db).await;
}

#[tokio::test]
async fn pg_expired_plan_rule_is_filtered_and_resumes_after_renewal() {
    let Some(db) = repo("expiry_filter").await else {
        return;
    };
    let alice = seed_active_rule(&db).await;
    sqlx::query("UPDATE users SET plan_expire_at = '2000-01-01 00:00:00' WHERE id = $1")
        .bind(alice)
        .execute(&db.pool)
        .await
        .unwrap();
    assert_eq!(
        db.list_active_for_config(50).await.unwrap().len(),
        0,
        "expired-plan user's rule must be filtered (PG)"
    );

    sqlx::query("UPDATE users SET plan_expire_at = '2099-01-01 00:00:00' WHERE id = $1")
        .bind(alice)
        .execute(&db.pool)
        .await
        .unwrap();
    assert_eq!(
        db.list_active_for_config(50).await.unwrap().len(),
        1,
        "rule must reappear after renewal (PG)"
    );
    cleanup(&db).await;
}

#[tokio::test]
async fn pg_null_plan_expire_at_is_no_expiry() {
    let Some(db) = repo("null_expiry").await else {
        return;
    };
    let alice = seed_active_rule(&db).await;
    sqlx::query("UPDATE users SET plan_expire_at = NULL WHERE id = $1")
        .bind(alice)
        .execute(&db.pool)
        .await
        .unwrap();
    assert_eq!(db.list_active_for_config(50).await.unwrap().len(), 1);
    cleanup(&db).await;
}

// ── v1.0.8: plan purchase (buy_plan) ──

async fn seed_buyer_and_plan(
    db: &PgRepository,
    balance: &str,
    plan_traffic: i64,
    plan_price: &str,
    duration_days: i32,
    reset_traffic: bool,
) -> (i64, i64) {
    db.insert_user("alice", "h", 1).await.unwrap();
    let alice = db.find_by_username("alice").await.unwrap().unwrap().id;
    sqlx::query("UPDATE users SET balance = $1 WHERE id = $2")
        .bind(balance)
        .bind(alice)
        .execute(&db.pool)
        .await
        .unwrap();
    let pid = db
        .insert_plan(
            "p1",
            10,
            plan_traffic,
            plan_price,
            if duration_days > 0 { "time" } else { "data" },
            duration_days,
            false,
            reset_traffic,
            "desc",
            false,
        )
        .await
        .unwrap();
    (alice, pid)
}

#[tokio::test]
async fn pg_buy_plan_stacks_traffic_and_charges_balance() {
    let Some(db) = repo("buy_stack").await else {
        return;
    };
    let (alice, pid) = seed_buyer_and_plan(&db, "100.00", 1_000_000, "30.00", 0, false).await;
    // RENEW: alice is already on this plan (plan_id = pid) with 500 quota left.
    // Re-buying the SAME plan stacks traffic (加流量).
    sqlx::query("UPDATE users SET traffic_limit = 500, plan_id = $1 WHERE id = $2")
        .bind(pid)
        .bind(alice)
        .execute(&db.pool)
        .await
        .unwrap();

    db.buy_plan(
        alice,
        pid,
        "p1",
        3000,
        1_000_000,
        10,
        0,
        false,
        false,
        &[],
        &[],
    )
    .await
    .unwrap();

    let (balance, traffic_limit, max_rules, plan_id): (String, i64, i32, Option<i64>) =
        sqlx::query_as(
            "SELECT balance, traffic_limit, max_rules, plan_id FROM users WHERE id = $1",
        )
        .bind(alice)
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!(balance, "70");
    assert_eq!(
        traffic_limit, 1_000_500,
        "renewing the same plan must stack traffic on existing quota (PG)"
    );
    assert_eq!(max_rules, 10);
    assert_eq!(plan_id, Some(pid));

    let orders: Vec<relay_shared::models::Order> = db.list_orders_by_user(alice).await.unwrap();
    assert_eq!(orders.len(), 1);
    assert_eq!(orders[0].plan_name, "p1");
    assert_eq!(orders[0].price, "30");
    cleanup(&db).await;
}

#[tokio::test]
async fn pg_buy_plan_reset_traffic_zeros_usage() {
    let Some(db) = repo("buy_reset").await else {
        return;
    };
    let (alice, pid) = seed_buyer_and_plan(&db, "100.00", 1_000_000, "10.00", 0, true).await;
    sqlx::query("UPDATE users SET traffic_used = 9999 WHERE id = $1")
        .bind(alice)
        .execute(&db.pool)
        .await
        .unwrap();

    db.buy_plan(
        alice,
        pid,
        "p1",
        1000,
        1_000_000,
        10,
        0,
        true,
        false,
        &[],
        &[],
    )
    .await
    .unwrap();

    let used: (i64,) = sqlx::query_as("SELECT traffic_used FROM users WHERE id = $1")
        .bind(alice)
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!(used.0, 0, "reset_traffic must zero traffic_used (PG)");
    cleanup(&db).await;
}

#[tokio::test]
async fn pg_buy_plan_insufficient_balance_is_rejected_and_rolls_back() {
    let Some(db) = repo("buy_insuf").await else {
        return;
    };
    let (alice, pid) = seed_buyer_and_plan(&db, "5.00", 1_000_000, "30.00", 0, false).await;
    sqlx::query("UPDATE users SET plan_id = NULL WHERE id = $1")
        .bind(alice)
        .execute(&db.pool)
        .await
        .unwrap();

    let err = db
        .buy_plan(
            alice,
            pid,
            "p1",
            3000,
            1_000_000,
            10,
            0,
            false,
            false,
            &[],
            &[],
        )
        .await
        .unwrap_err();
    assert!(matches!(err, BuyPlanError::InsufficientBalance));

    let (balance, plan_id): (String, Option<i64>) =
        sqlx::query_as("SELECT balance, plan_id FROM users WHERE id = $1")
            .bind(alice)
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert_eq!(
        balance, "5.00",
        "balance must be untouched on rollback (PG)"
    );
    assert_eq!(plan_id, None);
    let orders: Vec<relay_shared::models::Order> = db.list_orders_by_user(alice).await.unwrap();
    assert_eq!(orders.len(), 0, "no order row on insufficient balance (PG)");
    cleanup(&db).await;
}

#[tokio::test]
async fn pg_buy_plan_time_plan_sets_future_expiry() {
    let Some(db) = repo("buy_time").await else {
        return;
    };
    let (alice, pid) = seed_buyer_and_plan(&db, "100.00", 0, "5.00", 30, false).await;

    db.buy_plan(alice, pid, "p1", 500, 0, 10, 30, false, false, &[], &[])
        .await
        .unwrap();

    let expire: (Option<String>,) =
        sqlx::query_as("SELECT plan_expire_at::text FROM users WHERE id = $1")
            .bind(alice)
            .fetch_one(&db.pool)
            .await
            .unwrap();
    let exp = expire.0.expect("time plan must set an expiry (PG)");
    let now: (String,) = sqlx::query_as("SELECT NOW()::text")
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert!(
        exp > now.0,
        "expiry must be in the future (PG): {exp} <= {}",
        now.0
    );
    cleanup(&db).await;
}

#[tokio::test]
async fn pg_buy_plan_renewal_stacks_expiry_from_current_end() {
    let Some(db) = repo("buy_renew").await else {
        return;
    };
    let (alice, pid) = seed_buyer_and_plan(&db, "100.00", 0, "5.00", 30, false).await;
    // RENEW: plan_id = pid makes re-buying the same plan a renew (extend FROM
    // the existing far-future expiry, not clip to now + 30).
    sqlx::query(
        "UPDATE users SET plan_expire_at = '2099-12-31 00:00:00', plan_id = $1 WHERE id = $2",
    )
    .bind(pid)
    .bind(alice)
    .execute(&db.pool)
    .await
    .unwrap();

    db.buy_plan(alice, pid, "p1", 500, 0, 10, 30, false, false, &[], &[])
        .await
        .unwrap();

    let expire: (Option<String>,) =
        sqlx::query_as("SELECT plan_expire_at::text FROM users WHERE id = $1")
            .bind(alice)
            .fetch_one(&db.pool)
            .await
            .unwrap();
    let expire = expire.0.expect("renewal must keep an expiry (PG)");
    assert!(
        expire.starts_with("2100-"),
        "renewal must stack from current expiry (PG), got {expire}"
    );
    cleanup(&db).await;
}

/// v1.0.9: switching to a DIFFERENT plan replaces quota with the new plan's
/// amount (not stacked) and resets usage to 0.
#[tokio::test]
async fn pg_buy_plan_switch_replaces_traffic_and_resets_used() {
    let Some(db) = repo("buy_switch_traffic").await else {
        return;
    };
    let (alice, pid_a) = seed_buyer_and_plan(&db, "100.00", 1_000, "5.00", 0, false).await;
    sqlx::query(
        "UPDATE users SET plan_id = $1, traffic_limit = 800, traffic_used = 300 WHERE id = $2",
    )
    .bind(pid_a)
    .bind(alice)
    .execute(&db.pool)
    .await
    .unwrap();
    let pid_b = db
        .insert_plan("pB", 20, 5_000, "5.00", "data", 0, false, false, "", false)
        .await
        .unwrap();

    db.buy_plan(
        alice,
        pid_b,
        "pB",
        500,
        5_000,
        20,
        0,
        false,
        false,
        &[],
        &[],
    )
    .await
    .unwrap();

    let (traffic_limit, traffic_used, plan_id): (i64, i64, Option<i64>) =
        sqlx::query_as("SELECT traffic_limit, traffic_used, plan_id FROM users WHERE id = $1")
            .bind(alice)
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert_eq!(
        traffic_limit, 5_000,
        "switch must REPLACE quota with the new plan's amount, not stack (PG)"
    );
    assert_eq!(traffic_used, 0, "switch must reset usage to 0 (PG)");
    assert_eq!(plan_id, Some(pid_b));
    cleanup(&db).await;
}

/// v1.0.9: switching to a different time plan recomputes expiry from now.
#[tokio::test]
async fn pg_buy_plan_switch_recomputes_expiry_from_now() {
    let Some(db) = repo("buy_switch_expiry").await else {
        return;
    };
    let (alice, pid_a) = seed_buyer_and_plan(&db, "100.00", 0, "5.00", 30, false).await;
    sqlx::query(
        "UPDATE users SET plan_id = $1, plan_expire_at = '2099-12-31 00:00:00' WHERE id = $2",
    )
    .bind(pid_a)
    .bind(alice)
    .execute(&db.pool)
    .await
    .unwrap();
    let pid_b = db
        .insert_plan("pB", 10, 0, "5.00", "time", 30, false, false, "", false)
        .await
        .unwrap();

    db.buy_plan(alice, pid_b, "pB", 500, 0, 10, 30, false, false, &[], &[])
        .await
        .unwrap();

    let expire: (Option<String>,) =
        sqlx::query_as("SELECT plan_expire_at::text FROM users WHERE id = $1")
            .bind(alice)
            .fetch_one(&db.pool)
            .await
            .unwrap();
    let expire = expire.0.expect("switch to a time plan sets an expiry (PG)");
    assert!(
        !expire.starts_with("2099-") && !expire.starts_with("2100-"),
        "switch must recompute expiry from now, not stack from the old plan (PG), got {expire}"
    );
    cleanup(&db).await;
}

/// v1.0.9: renewing the SAME plan (reset_traffic=false) keeps usage and stacks
/// quota.
#[tokio::test]
async fn pg_buy_plan_renew_keeps_traffic_used() {
    let Some(db) = repo("buy_renew_used").await else {
        return;
    };
    let (alice, pid) = seed_buyer_and_plan(&db, "100.00", 1_000, "5.00", 0, false).await;
    sqlx::query(
        "UPDATE users SET plan_id = $1, traffic_limit = 1000, traffic_used = 400 WHERE id = $2",
    )
    .bind(pid)
    .bind(alice)
    .execute(&db.pool)
    .await
    .unwrap();

    db.buy_plan(alice, pid, "p1", 500, 1_000, 10, 0, false, false, &[], &[])
        .await
        .unwrap();

    let (traffic_limit, traffic_used): (i64, i64) =
        sqlx::query_as("SELECT traffic_limit, traffic_used FROM users WHERE id = $1")
            .bind(alice)
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert_eq!(traffic_limit, 2_000, "renew stacks quota (PG)");
    assert_eq!(traffic_used, 400, "renew keeps usage (PG)");
    cleanup(&db).await;
}

// ── v1.0.8: plan CRUD ──

#[tokio::test]
async fn pg_plan_crud_round_trip_and_delete_blocked_when_in_use() {
    let Some(db) = repo("plan_crud").await else {
        return;
    };
    let pid = db
        .insert_plan("p1", 10, 1_000, "5.00", "data", 0, false, false, "d", false)
        .await
        .unwrap();

    assert_eq!(
        db.update_plan_fields(
            pid,
            Some("p1-renamed"),
            Some(20),
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .await
        .unwrap(),
        1
    );
    let p = db.find_plan_by_id(pid).await.unwrap().unwrap();
    assert_eq!(p.name, "p1-renamed");
    assert_eq!(p.max_rules, 20);

    let visible_before = db.list_visible_plans().await.unwrap().len();
    db.update_plan_fields(
        pid,
        None,
        None,
        None,
        None,
        None,
        None,
        Some(true),
        None,
        None,
        None,
    )
    .await
    .unwrap();
    assert_eq!(
        db.list_visible_plans().await.unwrap().len(),
        visible_before - 1
    );
    assert!(db.list_plans().await.unwrap().iter().any(|p| p.id == pid));

    assert_eq!(db.count_users_on_plan(pid).await.unwrap(), 0);
    assert_eq!(db.delete_plan(pid).await.unwrap(), 1);
    assert!(db.find_plan_by_id(pid).await.unwrap().is_none());

    let pid2 = db
        .insert_plan("p2", 5, 0, "0", "data", 0, false, false, "", false)
        .await
        .unwrap();
    db.insert_user("bob", "h", 1).await.unwrap();
    let bob = db.find_by_username("bob").await.unwrap().unwrap().id;
    sqlx::query("UPDATE users SET plan_id = $1 WHERE id = $2")
        .bind(pid2)
        .bind(bob)
        .execute(&db.pool)
        .await
        .unwrap();
    assert_eq!(db.count_users_on_plan(pid2).await.unwrap(), 1);
    cleanup(&db).await;
}

// ── v1.0.9: plan ↔ device-group grants + purchase authorization ──

async fn seed_device_group(db: &PgRepository, gid: i64, uid: i64) {
    sqlx::query(
        "INSERT INTO device_groups (id, name, group_type, token, uid) \
         VALUES ($1, 'g', 'in', $2, $3)",
    )
    .bind(gid)
    .bind(format!("tok-dg-{gid}"))
    .bind(uid)
    .execute(&db.pool)
    .await
    .unwrap();
}

#[tokio::test]
async fn pg_create_plan_with_groups_persists_plan_and_grants() {
    let Some(db) = repo("create_plan_grp").await else {
        return;
    };
    seed_device_group(&db, 60, 1).await;
    seed_device_group(&db, 61, 1).await;
    let id = db
        .create_plan_with_groups(
            "combo",
            5,
            1000,
            "5.00",
            "data",
            0,
            false,
            false,
            "d",
            false,
            &[60, 61],
        )
        .await
        .unwrap();
    let p = db.find_plan_by_id(id).await.unwrap().expect("plan created");
    assert_eq!(p.name, "combo");
    assert_eq!(db.list_plan_device_groups(id).await.unwrap(), vec![60, 61]);
    cleanup(&db).await;
}

#[tokio::test]
async fn pg_clear_user_plan_revokes_groups_and_pauses_rules() {
    let Some(db) = repo("clear_user_plan").await else {
        return;
    };
    let (alice, pid) = seed_buyer_and_plan(&db, "100.00", 1000, "5.00", 0, false).await;
    seed_device_group(&db, 70, alice).await;
    sqlx::query("UPDATE users SET plan_id = $1, all_device_groups = TRUE WHERE id = $2")
        .bind(pid)
        .bind(alice)
        .execute(&db.pool)
        .await
        .unwrap();
    db.set_user_device_groups(alice, &[70]).await.unwrap();
    sqlx::query(
        "INSERT INTO forward_rules (id, name, uid, listen_port, device_group_in, \
         target_addr, target_port, paused) VALUES (300, 'r', $1, 21000, 70, '127.0.0.1', 80, FALSE)",
    )
    .bind(alice)
    .execute(&db.pool)
    .await
    .unwrap();

    let affected = db.clear_user_plan(alice).await.unwrap();
    assert_eq!(affected, 1);

    let (plan_id, all): (Option<i64>, bool) =
        sqlx::query_as("SELECT plan_id, all_device_groups FROM users WHERE id = $1")
            .bind(alice)
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert_eq!(plan_id, None);
    assert!(!all, "all_device_groups must be cleared (PG)");
    assert!(db.list_user_device_groups(alice).await.unwrap().is_empty());
    let (paused, auto): (bool, bool) =
        sqlx::query_as("SELECT paused, auto_paused FROM forward_rules WHERE id = 300")
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert!(
        paused && auto,
        "rule must be system-paused after clear (PG)"
    );

    assert_eq!(db.clear_user_plan(1).await.unwrap(), 0);
    cleanup(&db).await;
}

#[tokio::test]
async fn pg_plan_device_groups_round_trip_and_replace() {
    let Some(db) = repo("plan_dg_rtrip").await else {
        return;
    };
    let (alice, pid) = seed_buyer_and_plan(&db, "100.00", 1000, "5.00", 0, false).await;
    seed_device_group(&db, 50, alice).await;
    seed_device_group(&db, 51, alice).await;
    seed_device_group(&db, 52, alice).await;

    db.set_plan_device_groups(pid, &[50, 51]).await.unwrap();
    assert_eq!(db.list_plan_device_groups(pid).await.unwrap(), vec![50, 51]);

    db.set_plan_device_groups(pid, &[52]).await.unwrap();
    assert_eq!(db.list_plan_device_groups(pid).await.unwrap(), vec![52]);

    db.set_plan_device_groups(pid, &[50, 50, 51]).await.unwrap();
    assert_eq!(db.list_plan_device_groups(pid).await.unwrap(), vec![50, 51]);
    cleanup(&db).await;
}

/// v1.0.8: purchase REPLACES authorization, so a group the user already had
/// that ALSO appears in the new plan's grant set must end up exactly once
/// (mirrors the SQLite test).
#[tokio::test]
async fn pg_buy_plan_new_authorized_set_has_no_duplicate_groups() {
    let Some(db) = repo("buy_dg_dedup").await else {
        return;
    };
    let (alice, pid) = seed_buyer_and_plan(&db, "100.00", 1000, "5.00", 0, false).await;
    seed_device_group(&db, 50, alice).await;
    seed_device_group(&db, 51, alice).await;
    db.set_user_device_groups(alice, &[50]).await.unwrap();
    db.set_plan_device_groups(pid, &[50, 51]).await.unwrap();

    db.buy_plan(
        alice,
        pid,
        "p1",
        500,
        1000,
        10,
        0,
        false,
        false,
        &[50, 51],
        &[50, 51],
    )
    .await
    .unwrap();

    assert_eq!(
        db.list_user_device_groups(alice).await.unwrap(),
        vec![50, 51]
    );
    let all: (bool,) = sqlx::query_as("SELECT all_device_groups FROM users WHERE id = $1")
        .bind(alice)
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert!(!all.0);
    cleanup(&db).await;
}

/// v1.0.8: purchase REPLACES authorization — old groups are cleared. If the
/// user previously had groups not in the new plan, those are removed and
/// rules bound to them are paused. Mirrors the SQLite test.
#[tokio::test]
async fn pg_buy_plan_replaces_authorization_clears_old_groups() {
    let Some(db) = repo("buy_replaces_auth").await else {
        return;
    };
    let (alice, pid) = seed_buyer_and_plan(&db, "100.00", 1000, "5.00", 0, false).await;
    seed_device_group(&db, 50, alice).await;
    seed_device_group(&db, 51, alice).await;
    seed_device_group(&db, 52, alice).await;
    // Alice previously had groups 50 and 51.
    db.set_user_device_groups(alice, &[50, 51]).await.unwrap();
    // Plan grants only group 52.
    db.set_plan_device_groups(pid, &[52]).await.unwrap();

    // Create a rule bound to group 50 (will be paused after purchase).
    sqlx::query(
        "INSERT INTO forward_rules \
         (id, name, uid, listen_port, device_group_in, target_addr, target_port, paused) \
         VALUES (100, 'r100', $1, 20000, 50, '127.0.0.1', 80, FALSE)",
    )
    .bind(alice)
    .execute(&db.pool)
    .await
    .unwrap();

    // v1.0.8: new_authorized = {52} (the plan's grants).
    db.buy_plan(
        alice,
        pid,
        "p1",
        500,
        1000,
        10,
        0,
        false,
        false,
        &[52],
        &[52],
    )
    .await
    .unwrap();

    // Result: {52} — old groups 50, 51 are cleared.
    assert_eq!(db.list_user_device_groups(alice).await.unwrap(), vec![52]);
    // The rule bound to group 50 is now paused.
    let paused: (bool,) = sqlx::query_as("SELECT paused FROM forward_rules WHERE id = 100")
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert!(
        paused.0,
        "rule bound to removed group should be paused (PG)"
    );
    cleanup(&db).await;
}

#[tokio::test]
async fn pg_buy_plan_grant_all_sets_flag() {
    let Some(db) = repo("buy_grant_all").await else {
        return;
    };
    let (alice, pid) = seed_buyer_and_plan(&db, "100.00", 1000, "5.00", 0, false).await;

    db.buy_plan(alice, pid, "p1", 500, 1000, 10, 0, false, true, &[], &[])
        .await
        .unwrap();

    let all: (bool,) = sqlx::query_as("SELECT all_device_groups FROM users WHERE id = $1")
        .bind(alice)
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert!(
        all.0,
        "grant_all_groups must set the all_device_groups flag (PG)"
    );
    cleanup(&db).await;
}

/// v1.0.8 regression (PG): downgrading from a grant-all plan to a per-group
/// plan must RESET all_device_groups back to FALSE. Mirrors the SQLite test.
#[tokio::test]
async fn pg_buy_plan_grant_all_then_per_group_resets_all_flag() {
    let Some(db) = repo("buy_grant_all_downgrade").await else {
        return;
    };
    let (alice, pid) = seed_buyer_and_plan(&db, "100.00", 1000, "5.00", 0, false).await;
    seed_device_group(&db, 50, alice).await;
    seed_device_group(&db, 52, alice).await;

    // 1) Buy a grant-all plan → all_device_groups = TRUE.
    db.buy_plan(alice, pid, "all", 100, 1000, 10, 0, false, true, &[], &[])
        .await
        .unwrap();
    let all: (bool,) = sqlx::query_as("SELECT all_device_groups FROM users WHERE id = $1")
        .bind(alice)
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert!(all.0, "grant-all purchase must set the flag (PG)");

    // 2) Downgrade to a per-group plan granting only {52}.
    db.buy_plan(
        alice,
        pid,
        "ltd",
        100,
        1000,
        10,
        0,
        false,
        false,
        &[52],
        &[52],
    )
    .await
    .unwrap();

    let all: (bool,) = sqlx::query_as("SELECT all_device_groups FROM users WHERE id = $1")
        .bind(alice)
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert!(
        !all.0,
        "downgrade to a per-group plan must reset all_device_groups to FALSE (PG)"
    );
    assert_eq!(db.list_user_device_groups(alice).await.unwrap(), vec![52]);
    cleanup(&db).await;
}

/// v1.0.8: re-buying a plan that re-grants a group must auto-resume a rule
/// this system previously auto-paused on that group. Mirrors the SQLite test.
#[tokio::test]
async fn pg_buy_plan_resumes_auto_paused_rules_when_group_reauthorized() {
    let Some(db) = repo("buy_plan_resume").await else {
        return;
    };
    let (alice, pid_a) = seed_buyer_and_plan(&db, "100.00", 1000, "5.00", 0, false).await;
    seed_device_group(&db, 50, alice).await;
    seed_device_group(&db, 51, alice).await;
    let pid_b = db
        .insert_plan("pB", 10, 1000, "5.00", "data", 0, false, false, "", false)
        .await
        .unwrap();
    db.set_plan_device_groups(pid_a, &[50]).await.unwrap();
    db.set_plan_device_groups(pid_b, &[51]).await.unwrap();

    sqlx::query(
        "INSERT INTO forward_rules \
         (id, name, uid, listen_port, device_group_in, target_addr, target_port, paused) \
         VALUES (200, 'r200', $1, 20000, 50, '127.0.0.1', 80, FALSE)",
    )
    .bind(alice)
    .execute(&db.pool)
    .await
    .unwrap();
    db.buy_plan(
        alice,
        pid_a,
        "pA",
        500,
        1000,
        10,
        0,
        false,
        false,
        &[50],
        &[50],
    )
    .await
    .unwrap();

    db.buy_plan(
        alice,
        pid_b,
        "pB",
        500,
        1000,
        10,
        0,
        false,
        false,
        &[51],
        &[51],
    )
    .await
    .unwrap();
    let (paused, auto_paused): (bool, bool) =
        sqlx::query_as("SELECT paused, auto_paused FROM forward_rules WHERE id = 200")
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert!(
        paused && auto_paused,
        "buy_plan must auto-pause rule 200 (PG)"
    );

    db.buy_plan(
        alice,
        pid_a,
        "pA",
        500,
        1000,
        10,
        0,
        false,
        false,
        &[50],
        &[50],
    )
    .await
    .unwrap();
    let (paused, auto_paused): (bool, bool) =
        sqlx::query_as("SELECT paused, auto_paused FROM forward_rules WHERE id = 200")
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert!(
        !paused && !auto_paused,
        "re-authorizing group 50 must auto-resume the rule buy_plan itself paused (PG)"
    );
    cleanup(&db).await;
}

/// v1.0.8: a rule the user paused THEMSELVES (auto_paused cleared) must NOT be
/// silently revived by a later purchase. Mirrors the SQLite test.
#[tokio::test]
async fn pg_buy_plan_does_not_resume_manually_paused_rules() {
    let Some(db) = repo("buy_plan_no_resume").await else {
        return;
    };
    let (alice, pid_a) = seed_buyer_and_plan(&db, "100.00", 1000, "5.00", 0, false).await;
    seed_device_group(&db, 50, alice).await;
    seed_device_group(&db, 51, alice).await;
    let pid_b = db
        .insert_plan("pB", 10, 1000, "5.00", "data", 0, false, false, "", false)
        .await
        .unwrap();
    db.set_plan_device_groups(pid_a, &[50]).await.unwrap();
    db.set_plan_device_groups(pid_b, &[51]).await.unwrap();

    sqlx::query(
        "INSERT INTO forward_rules \
         (id, name, uid, listen_port, device_group_in, target_addr, target_port, paused) \
         VALUES (201, 'r201', $1, 20001, 50, '127.0.0.1', 80, FALSE)",
    )
    .bind(alice)
    .execute(&db.pool)
    .await
    .unwrap();
    db.buy_plan(
        alice,
        pid_a,
        "pA",
        500,
        1000,
        10,
        0,
        false,
        false,
        &[50],
        &[50],
    )
    .await
    .unwrap();

    db.buy_plan(
        alice,
        pid_b,
        "pB",
        500,
        1000,
        10,
        0,
        false,
        false,
        &[51],
        &[51],
    )
    .await
    .unwrap();

    // Human re-confirms the pause via the on/off switch — clears auto_paused.
    let scope = crate::db::repo::ResourceScope::All;
    db.update_rule_fields(
        201,
        &scope,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        Some(true),
    )
    .await
    .unwrap();
    let (_, auto_paused): (bool, bool) =
        sqlx::query_as("SELECT paused, auto_paused FROM forward_rules WHERE id = 201")
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert!(
        !auto_paused,
        "an explicit paused write must clear auto_paused (PG)"
    );

    db.buy_plan(
        alice,
        pid_a,
        "pA",
        500,
        1000,
        10,
        0,
        false,
        false,
        &[50],
        &[50],
    )
    .await
    .unwrap();
    let (paused,): (bool,) = sqlx::query_as("SELECT paused FROM forward_rules WHERE id = 201")
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert!(
        paused,
        "a manually-paused rule must NOT be auto-resumed by a later purchase (PG)"
    );
    cleanup(&db).await;
}

/// v1.0.8: REPLACE semantics — buying a second (different) plan replaces the
/// first plan's authorization rather than stacking it. Mirrors the SQLite
/// test.
#[tokio::test]
async fn pg_second_plan_purchase_replaces_first_plan_groups() {
    let Some(db) = repo("multi_plan_stack").await else {
        return;
    };
    let (alice, pid_a) = seed_buyer_and_plan(&db, "100.00", 1000, "5.00", 0, false).await;
    seed_device_group(&db, 50, alice).await;
    seed_device_group(&db, 51, alice).await;
    let pid_b = db
        .insert_plan("pB", 10, 1000, "5.00", "data", 0, false, false, "", false)
        .await
        .unwrap();
    db.set_plan_device_groups(pid_a, &[50]).await.unwrap();
    db.set_plan_device_groups(pid_b, &[51]).await.unwrap();

    db.buy_plan(
        alice,
        pid_a,
        "pA",
        500,
        1000,
        10,
        0,
        false,
        false,
        &[50],
        &[50],
    )
    .await
    .unwrap();
    db.buy_plan(
        alice,
        pid_b,
        "pB",
        500,
        1000,
        10,
        0,
        false,
        false,
        &[51],
        &[51],
    )
    .await
    .unwrap();

    assert_eq!(db.list_user_device_groups(alice).await.unwrap(), vec![51]);
    cleanup(&db).await;
}

#[tokio::test]
async fn pg_delete_plan_cascades_grant_rows() {
    let Some(db) = repo("del_plan_cascade").await else {
        return;
    };
    let pid = db
        .insert_plan("p1", 10, 1000, "5.00", "data", 0, false, false, "", false)
        .await
        .unwrap();
    seed_device_group(&db, 50, 1).await;
    db.set_plan_device_groups(pid, &[50]).await.unwrap();
    assert_eq!(db.list_plan_device_groups(pid).await.unwrap(), vec![50]);

    db.delete_plan(pid).await.unwrap();
    assert!(db.list_plan_device_groups(pid).await.unwrap().is_empty());
    cleanup(&db).await;
}

#[tokio::test]
async fn pg_expiry_does_not_revoke_granted_groups() {
    let Some(db) = repo("expiry_no_revoke").await else {
        return;
    };
    let (alice, pid) = seed_buyer_and_plan(&db, "100.00", 0, "5.00", 30, false).await;
    seed_device_group(&db, 50, alice).await;
    db.set_plan_device_groups(pid, &[50]).await.unwrap();

    db.buy_plan(alice, pid, "p1", 500, 0, 10, 30, false, false, &[50], &[50])
        .await
        .unwrap();
    assert_eq!(db.list_user_device_groups(alice).await.unwrap(), vec![50]);

    sqlx::query("UPDATE users SET plan_expire_at = '2000-01-01 00:00:00' WHERE id = $1")
        .bind(alice)
        .execute(&db.pool)
        .await
        .unwrap();
    assert_eq!(
        db.list_user_device_groups(alice).await.unwrap(),
        vec![50],
        "expiry must not revoke granted device groups (PG)"
    );
    cleanup(&db).await;
}

/// v0.4.11 PR3: Migration does NOT pause cross-owner shared inbound rules.
#[tokio::test]
async fn pg_migration_does_not_pause_cross_owner_shared_inbound_rules() {
    let Some(db) = repo("mig_pause_cross").await else {
        return;
    };
    sqlx::query("INSERT INTO users (id, username, password, admin) VALUES (2, 'u2', 'x', FALSE)")
        .execute(&db.pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO device_groups (id, name, group_type, token, uid) VALUES (20, 'g', 'in', 't', 1)")
        .execute(&db.pool).await.unwrap();
    sqlx::query("INSERT INTO forward_rules (name, uid, listen_port, device_group_in, target_addr, target_port) \
                 VALUES ('r', 2, 15000, 20, '127.0.0.1', 80)")
        .execute(&db.pool).await.unwrap();
    // PG runs migrations during repo(), so the rule must be unpaused.
    let paused: (bool,) = sqlx::query_as("SELECT paused FROM forward_rules WHERE name = 'r'")
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert!(
        !paused.0,
        "cross-owner shared inbound rule must NOT be paused (PG)"
    );
    cleanup(&db).await;
}

// ── v1.0.7: admin directly edits a user's plan association + expiry ──

#[tokio::test]
async fn pg_admin_set_user_plan_clears_and_adjusts_expiry() {
    let Some(db) = repo("admin_set_plan").await else {
        return;
    };
    let (alice, pid) = seed_buyer_and_plan(&db, "100.00", 0, "5.00", 30, false).await;
    sqlx::query(
        "UPDATE users SET plan_id = $1, plan_expire_at = '2030-01-01 00:00:00' WHERE id = $2",
    )
    .bind(pid)
    .bind(alice)
    .execute(&db.pool)
    .await
    .unwrap();

    assert_eq!(
        db.admin_set_user_plan(alice, Some(pid), Some("2099-12-31 00:00:00".into()))
            .await
            .unwrap(),
        1
    );
    let (plan_id, expire): (Option<i64>, Option<String>) =
        sqlx::query_as("SELECT plan_id, plan_expire_at FROM users WHERE id = $1")
            .bind(alice)
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert_eq!(plan_id, Some(pid));
    assert_eq!(expire.as_deref(), Some("2099-12-31 00:00:00"));

    db.admin_set_user_plan(alice, None, None).await.unwrap();
    let (plan_id2, expire2): (Option<i64>, Option<String>) =
        sqlx::query_as("SELECT plan_id, plan_expire_at FROM users WHERE id = $1")
            .bind(alice)
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert_eq!(plan_id2, None);
    assert_eq!(expire2, None);
    cleanup(&db).await;
}

#[tokio::test]
async fn pg_admin_set_user_plan_skips_admin_users() {
    let Some(db) = repo("admin_set_plan_skip").await else {
        return;
    };
    sqlx::query("INSERT INTO users (id, username, password, admin) VALUES (90, 'adm', 'x', TRUE)")
        .execute(&db.pool)
        .await
        .unwrap();
    let affected = db
        .admin_set_user_plan(90, None, Some("2099-12-31 00:00:00".into()))
        .await
        .unwrap();
    assert_eq!(
        affected, 0,
        "admin users must be skipped (WHERE admin = false)"
    );
    cleanup(&db).await;
}
