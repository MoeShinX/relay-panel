// ── Contract tests ──
//
// These tests exercise the SqliteRepository trait impls DIRECTLY (not via the
// HTTP handlers). They pin the contract each Repository method must satisfy so
// PR2's PgRepository can re-run the same assertions against its own impl.
//
// What they DON'T cover: handler wiring (covered by api::admin / api::node
// tests), SQL dialect specifics (the SQL strings themselves), or the
// transactional batch edge cases (already covered by api::node::tests).

use super::SqliteRepository;
use crate::db::error::DbError;
use crate::db::repo::*;
use crate::db::schema::SCHEMA_SQL;
use relay_shared::protocol::TrafficEntry;
use sqlx::sqlite::SqlitePoolOptions;

/// Build a fresh in-memory DB wrapped in a SqliteRepository. The schema is
/// created via SCHEMA_SQL so every table + seed row (admin user, plans,
/// builtin tunnel profiles) is present.
async fn repo() -> SqliteRepository {
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .unwrap();
    sqlx::query(SCHEMA_SQL).execute(&pool).await.unwrap();
    SqliteRepository::new(pool)
}

/// Seed an inbound device_group with the given id (rules reference
/// device_group_in via FK, so the group must exist before any rule).
async fn seed_group(db: &SqliteRepository, gid: i64) {
    sqlx::query(
        "INSERT INTO device_groups (id, name, group_type, token, uid) \
         VALUES (?, 'gin', 'in', ?, 1)",
    )
    .bind(gid)
    .bind(format!("tok-{gid}"))
    .execute(&db.pool)
    .await
    .unwrap();
}

/// Seed an inbound device_group with an explicit `port_range` (the auto-assign
/// pool). Mirrors `seed_group` but lets a test pin the range under test.
async fn seed_group_with_range(db: &SqliteRepository, gid: i64, range: &str) {
    sqlx::query(
        "INSERT INTO device_groups (id, name, group_type, token, uid, port_range) \
         VALUES (?, 'gin', 'in', ?, 1, ?)",
    )
    .bind(gid)
    .bind(format!("tok-{gid}"))
    .bind(range)
    .execute(&db.pool)
    .await
    .unwrap();
}

/// Insert a user row with the given id + admin flag (FK target for groups).
async fn seed_user(db: &SqliteRepository, id: i64, admin: bool) {
    sqlx::query("INSERT INTO users (id, username, password, admin) VALUES (?, ?, 'x', ?)")
        .bind(id)
        .bind(format!("u{id}"))
        .bind(admin as i64)
        .execute(&db.pool)
        .await
        .unwrap();
}

/// Insert a device group owned by `uid` with an explicit group_type.
async fn seed_group_typed(db: &SqliteRepository, gid: i64, uid: i64, gtype: &str) {
    sqlx::query(
        "INSERT INTO device_groups (id, name, group_type, token, connect_host, uid) \
         VALUES (?, ?, ?, ?, '1.2.3.4', ?)",
    )
    .bind(gid)
    .bind(format!("g{gid}"))
    .bind(gtype)
    .bind(format!("tok-{gid}"))
    .bind(uid)
    .execute(&db.pool)
    .await
    .unwrap();
}

/// v0.4.12 PR1 (scenario 1): an admin-owned `group_type='in'` group is
/// visible to a regular user even with NO rules. (scenario 8): the summary
/// DTO carries no token/uid/config columns.
#[tokio::test]
async fn shared_groups_lists_admin_inbound_for_user_without_rules() {
    let db = repo().await; // uid=1 admin is seeded
    seed_user(&db, 2, false).await; // alice (regular)
    seed_group_typed(&db, 10, 1, "in").await; // admin-owned inbound

    let shared = db.list_shared_groups(2, false).await.unwrap();
    assert_eq!(shared.len(), 1, "alice sees the admin inbound group");
    assert_eq!(shared[0].id, 10);
    // DTO is a SharedGroupSummary — it structurally cannot carry token/uid/
    // config/fallback_group (compile-time guarantee), so a positive id match
    // is sufficient here.
}

/// scenario 2: out / monitor groups never appear in the shared list.
#[tokio::test]
async fn shared_groups_excludes_non_inbound_types() {
    let db = repo().await;
    seed_user(&db, 2, false).await;
    seed_group_typed(&db, 10, 1, "in").await;
    seed_group_typed(&db, 11, 1, "out").await;
    seed_group_typed(&db, 12, 1, "monitor").await;

    let shared = db.list_shared_groups(2, false).await.unwrap();
    assert_eq!(shared.len(), 1);
    assert_eq!(shared[0].id, 10, "only the 'in' group is shared");
}

/// scenario 3: a regular user never sees ANOTHER regular user's group, even
/// if it's an inbound group. Only admin-owned groups are shared.
#[tokio::test]
async fn shared_groups_excludes_other_regular_users_groups() {
    let db = repo().await;
    seed_user(&db, 2, false).await; // alice
    seed_user(&db, 3, false).await; // bob (regular)
    seed_group_typed(&db, 20, 3, "in").await; // bob's inbound group

    let shared = db.list_shared_groups(2, false).await.unwrap();
    assert!(
        shared.is_empty(),
        "alice must NOT see bob's (regular user) inbound group"
    );
}

/// v1.0.7: list_shared_groups still RETURNS a hidden group (carrying the
/// `hidden` flag) — the node-status handler is the only place that drops it,
/// so the rule dropdown / shop keep listing hidden lines. Admins see it too.
#[tokio::test]
async fn shared_groups_carries_hidden_flag_and_still_lists_hidden() {
    let db = repo().await; // uid=1 admin is seeded
    seed_user(&db, 2, false).await; // alice (regular)
    seed_group_typed(&db, 10, 1, "in").await; // admin-owned inbound, visible
    seed_group_typed(&db, 11, 1, "in").await; // admin-owned inbound, to hide
    sqlx::query("UPDATE device_groups SET hidden = 1 WHERE id = 11")
        .execute(&db.pool)
        .await
        .unwrap();

    // Regular user: BOTH groups are listed; the hidden one carries hidden=true
    // so the node-status path can drop it while the rule dropdown keeps it.
    let shared = db.list_shared_groups(2, false).await.unwrap();
    assert_eq!(
        shared.len(),
        2,
        "hidden group must STILL be listed for rules"
    );
    assert!(shared.iter().any(|g| g.id == 11 && g.hidden));
    assert!(shared.iter().any(|g| g.id == 10 && !g.hidden));

    // Admin: list_groups (unscoped) returns BOTH — hidden does not affect the
    // admin management view.
    let all = db.list_groups(&ResourceScope::All).await.unwrap();
    assert!(
        all.iter().any(|g| g.id == 11 && g.hidden),
        "admin must still see the hidden group, flagged hidden=true"
    );
    assert!(all.iter().any(|g| g.id == 10 && !g.hidden));
}

/// An admin caller gets an empty shared list (admins manage groups directly).
#[tokio::test]
async fn shared_groups_empty_for_admin() {
    let db = repo().await;
    seed_group_typed(&db, 10, 1, "in").await;
    let shared = db.list_shared_groups(1, true).await.unwrap();
    assert!(shared.is_empty());
}

#[tokio::test]
async fn rule_targets_replace_and_list_enabled_in_order() {
    let db = repo().await;
    seed_group(&db, 1).await;
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
}

// ── UserRepository ──

#[tokio::test]
async fn user_find_by_username_distinguishes_banned() {
    let db = repo().await;
    // Seed a non-admin, non-banned user via the public API.
    db.insert_user("alice", "$2b$12$hash", 1).await.unwrap();

    // find_by_username finds her regardless of banned flag.
    assert!(db.find_by_username("alice").await.unwrap().is_some());

    // Ban her; find_by_username_not_banned should now skip her.
    sqlx::query("UPDATE users SET banned = 1 WHERE username = 'alice'")
        .execute(&db.pool)
        .await
        .unwrap();
    assert!(db
        .find_by_username_not_banned("alice")
        .await
        .unwrap()
        .is_none());
    // ...but find_by_username still returns the row.
    assert!(db.find_by_username("alice").await.unwrap().is_some());
}

#[tokio::test]
async fn user_insert_returns_unique_violation_on_duplicate() {
    let db = repo().await;
    db.insert_user("alice", "h1", 1).await.unwrap();
    // A second insert with the same username must surface as
    // DbError::UniqueViolation, not a raw sqlx::Error or a silent success.
    // This is the contract the register handler relies on to map to 409.
    match db.insert_user("alice", "h2", 1).await {
        Err(DbError::UniqueViolation) => {}
        other => panic!("expected UniqueViolation, got {:?}", other),
    }
}

#[tokio::test]
async fn user_update_password_and_find_password_by_id_round_trip() {
    let db = repo().await;
    db.insert_user("alice", "old-hash", 1).await.unwrap();
    let uid = db.find_by_username("alice").await.unwrap().unwrap().id;

    // Initially the stored hash is what we inserted.
    assert_eq!(
        db.find_password_by_id(uid).await.unwrap().as_deref(),
        Some("old-hash")
    );
    // Update and re-read.
    assert_eq!(db.update_password(uid, "new-hash").await.unwrap(), 1);
    assert_eq!(
        db.find_password_by_id(uid).await.unwrap().as_deref(),
        Some("new-hash")
    );
    // Update on a non-existent id affects 0 rows.
    assert_eq!(db.update_password(999_999, "x").await.unwrap(), 0);
}

#[tokio::test]
async fn user_update_fields_only_touches_present_columns() {
    let db = repo().await;
    db.insert_user("alice", "h", 1).await.unwrap();
    let uid = db.find_by_username("alice").await.unwrap().unwrap().id;

    // Update only max_rules; other fields must stay at their seeded values.
    assert_eq!(
        db.update_user_fields(uid, None, Some(7), None, None, None)
            .await
            .unwrap(),
        1
    );
    let row: (i32, i64, bool) =
        sqlx::query_as("SELECT max_rules, traffic_limit, banned FROM users WHERE id = ?")
            .bind(uid)
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert_eq!(row.0, 7);
    assert_eq!(row.1, 0, "traffic_limit must be untouched");
    assert!(!row.2, "banned must be untouched");

    // With no fields present, returns 0 and writes nothing.
    assert_eq!(
        db.update_user_fields(uid, None, None, None, None, None)
            .await
            .unwrap(),
        0
    );
}

#[tokio::test]
async fn user_is_admin_and_exists_by_id_distinguish_known_rows() {
    let db = repo().await;
    // SCHEMA_SQL seeds user id=1 as admin. Find him.
    assert!(db.exists_by_id(1).await.unwrap());
    assert!(db.is_admin(1).await.unwrap());

    // A non-existent id: exists=false, is_admin=false.
    assert!(!db.exists_by_id(999_999).await.unwrap());
    assert!(!db.is_admin(999_999).await.unwrap());

    // Insert a non-admin and confirm is_admin returns false but exists true.
    db.insert_user("alice", "h", 1).await.unwrap();
    let uid = db.find_by_username("alice").await.unwrap().unwrap().id;
    assert!(db.exists_by_id(uid).await.unwrap());
    assert!(!db.is_admin(uid).await.unwrap());
}

#[tokio::test]
async fn user_reset_traffic_zeros_user_and_owned_rules_atomically() {
    let db = repo().await;
    seed_group(&db, 1).await;
    db.insert_user("alice", "h", 1).await.unwrap();
    let uid = db.find_by_username("alice").await.unwrap().unwrap().id;
    // Pre-charge traffic on the user and one rule.
    sqlx::query("UPDATE users SET traffic_used = 500 WHERE id = ?")
        .bind(uid)
        .execute(&db.pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO forward_rules \
         (name, uid, listen_port, device_group_in, target_addr, target_port, traffic_used) \
         VALUES ('r1', ?, 20000, 1, '127.0.0.1', 80, 250)",
    )
    .bind(uid)
    .execute(&db.pool)
    .await
    .unwrap();

    db.reset_traffic(uid).await.unwrap();

    let user_t: (i64,) = sqlx::query_as("SELECT traffic_used FROM users WHERE id = ?")
        .bind(uid)
        .fetch_one(&db.pool)
        .await
        .unwrap();
    let rule_t: (i64,) = sqlx::query_as("SELECT traffic_used FROM forward_rules WHERE uid = ?")
        .bind(uid)
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!(user_t.0, 0);
    assert_eq!(rule_t.0, 0);
}

#[tokio::test]
async fn user_delete_non_admin_protects_admins() {
    let db = repo().await;
    db.insert_user("alice", "h", 1).await.unwrap();
    let alice = db.find_by_username("alice").await.unwrap().unwrap().id;

    // Alice (non-admin) is deletable.
    assert_eq!(db.delete_non_admin(alice).await.unwrap(), 1);
    assert!(!db.exists_by_id(alice).await.unwrap());

    // User id=1 is admin — delete_non_admin must refuse (0 rows affected).
    assert_eq!(db.delete_non_admin(1).await.unwrap(), 0);
    assert!(db.exists_by_id(1).await.unwrap(), "admin must still exist");
}

#[tokio::test]
async fn user_delete_cascade_clears_rules_groups_profiles_and_user() {
    let db = repo().await;
    db.insert_user("alice", "h", 1).await.unwrap();
    let uid = db.find_by_username("alice").await.unwrap().unwrap().id;
    sqlx::query(
        "INSERT INTO device_groups (name, group_type, token, uid) \
         VALUES ('g1', 'in', 'tok-1', ?)",
    )
    .bind(uid)
    .execute(&db.pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO forward_rules \
         (name, uid, listen_port, device_group_in, target_addr, target_port) \
         VALUES ('r1', ?, 20000, 1, '127.0.0.1', 80)",
    )
    .bind(uid)
    .execute(&db.pool)
    .await
    .unwrap();
    // A custom (non-builtin) tunnel profile owned by alice. This is the row
    // the pre-v0.4.4 cascade missed — it would FK-block the user delete AFTER
    // rules+groups were already gone, leaving a half-deleted account.
    sqlx::query(
        "INSERT INTO tunnel_profiles (name, transport, uid) \
         VALUES ('alice-custom', 'ws', ?)",
    )
    .bind(uid)
    .execute(&db.pool)
    .await
    .unwrap();

    let affected = db.delete_user_cascade(uid).await.unwrap();
    assert_eq!(affected, 1, "the user row must be deleted");

    let rules: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM forward_rules WHERE uid = ?")
        .bind(uid)
        .fetch_one(&db.pool)
        .await
        .unwrap();
    let groups: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM device_groups WHERE uid = ?")
        .bind(uid)
        .fetch_one(&db.pool)
        .await
        .unwrap();
    let profiles: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM tunnel_profiles WHERE uid = ?")
        .bind(uid)
        .fetch_one(&db.pool)
        .await
        .unwrap();
    let user: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM users WHERE id = ?")
        .bind(uid)
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!(rules.0, 0);
    assert_eq!(groups.0, 0);
    assert_eq!(profiles.0, 0, "custom tunnel profile must be deleted too");
    assert_eq!(user.0, 0, "user row must be gone");
}

#[tokio::test]
async fn user_delete_cascade_refuses_admin_and_rolls_back() {
    // Admin (id=1, seeded) with owned resources. The cascade must delete
    // NOTHING and return 0 — the admin guard + rollback protect the account.
    let db = repo().await;
    sqlx::query(
        "INSERT INTO device_groups (id, name, group_type, token, uid) \
         VALUES (1, 'admin-g', 'in', 'tok-admin', 1)",
    )
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
    assert_eq!(
        groups.0, 1,
        "admin group must be rolled back (still present)"
    );
    assert_eq!(rules.0, 1, "admin rule must be rolled back (still present)");
    assert!(db.exists_by_id(1).await.unwrap(), "admin must still exist");
}

#[tokio::test]
async fn user_placeholder_password_methods_round_trip() {
    let db = repo().await;
    // SCHEMA_SQL seeds user id=1 with the placeholder password, so the
    // count should start at 1.
    assert_eq!(db.count_placeholder_admin_password().await.unwrap(), 1);

    // Replace it with a real hash; count must drop to 0 and the row updates.
    db.replace_placeholder_admin_password("$2b$12$realhash")
        .await
        .unwrap();
    assert_eq!(db.count_placeholder_admin_password().await.unwrap(), 0);

    // A second replace is a no-op (the WHERE no longer matches).
    db.replace_placeholder_admin_password("$2b$12$other")
        .await
        .unwrap();
    let stored: (String,) = sqlx::query_as("SELECT password FROM users WHERE id = 1")
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!(
        stored.0, "$2b$12$realhash",
        "second replace must not overwrite"
    );
}

// ── RuleRepository ──

#[tokio::test]
async fn rule_insert_quota_guarded_respects_max_rules() {
    let db = repo().await;
    seed_group(&db, 1).await;
    // Use the seeded admin user (id=1). Cap his max_rules at 2.
    sqlx::query("UPDATE users SET max_rules = 2 WHERE id = 1")
        .execute(&db.pool)
        .await
        .unwrap();

    // Two inserts succeed.
    assert_eq!(
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
        .unwrap(),
        1
    );
    assert_eq!(
        db.insert_quota_guarded(
            "r2",
            1,
            20001,
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
    // Third insert hits the quota: WHERE rejects → 0 rows affected.
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

    // max_rules = 0 means unlimited.
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
}

#[tokio::test]
async fn rule_insert_quota_guarded_surfaces_port_unique_violation() {
    let db = repo().await;
    seed_group(&db, 1).await;
    // First insert on port 20000 succeeds.
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
    // Second insert on the SAME group + SAME port + overlapping socket type
    // hits the in-transaction port pre-check → DbError::PortConflict (NOT a
    // silent 0, NOT UniqueViolation). The handler relies on this to map to
    // a 409. (The partial unique index is the backstop if a concurrent
    // insert slips past the pre-check.)
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
}

/// v1.2 regression: the same listen_port may be reused across TWO different
/// inbound groups (the per-group partial unique index allows it). Before v1.2,
/// create_rule re-looked-up the new rule by (owner_uid, listen_port) — which
/// ignores device_group_in — so the second create_rule_full's targets / LB /
/// rate limits were written to the FIRST (wrong) rule. create_rule_full returns
/// the id straight from the INSERT, so the side-tables land on the right rule.
#[tokio::test]
async fn rule_create_full_cross_group_no_crosstalk() {
    let db = repo().await; // uid=1 admin seeded
    seed_group(&db, 1).await; // inbound group 1
    seed_group(&db, 2).await; // inbound group 2

    // Rule A on group 1, port 10000, ONE target, default LB ("first"), no caps.
    let id_a = db
        .create_rule_full(
            "ruleA",
            1,
            10000,
            "tcp_udp",
            "raw",
            "raw",
            "direct",
            "raw",
            None,
            1,
            None,
            "direct",
            "1.1.1.1",
            80,
            &[relay_shared::protocol::RuleTargetRequest {
                host: "a.example.com".into(),
                port: 1001,
                enabled: true,
            }],
            "first",
            0,
            0,
            None,
        )
        .await
        .unwrap()
        .expect("rule A created");

    // Rule B on group 2, SAME port 10000, THREE distinct targets + round_robin
    // LB + up/down rate caps. Under the old bug this clobbered rule A.
    let id_b = db
        .create_rule_full(
            "ruleB",
            1,
            10000,
            "tcp_udp",
            "raw",
            "raw",
            "direct",
            "raw",
            None,
            2,
            None,
            "direct",
            "2.2.2.2",
            80,
            &[
                relay_shared::protocol::RuleTargetRequest {
                    host: "b1.example.com".into(),
                    port: 2001,
                    enabled: true,
                },
                relay_shared::protocol::RuleTargetRequest {
                    host: "b2.example.com".into(),
                    port: 2002,
                    enabled: true,
                },
                relay_shared::protocol::RuleTargetRequest {
                    host: "b3.example.com".into(),
                    port: 2003,
                    enabled: true,
                },
            ],
            "round_robin",
            50,
            100,
            None,
        )
        .await
        .unwrap()
        .expect("rule B created");

    assert_ne!(id_a, id_b, "two distinct rules must have distinct ids");

    // Rule A is UNTOUCHED: still one target, LB "first", zero caps.
    let a = db
        .find_rule_by_id(id_a, &ResourceScope::All)
        .await
        .unwrap()
        .expect("rule A exists");
    assert_eq!(a.device_group_in, 1);
    assert_eq!(a.load_balance_strategy, "first");
    assert_eq!(a.upload_limit_mbps, 0);
    assert_eq!(a.download_limit_mbps, 0);
    let a_targets = db
        .list_rule_targets(id_a, &ResourceScope::All)
        .await
        .unwrap();
    assert_eq!(a_targets.len(), 1, "rule A keeps exactly its one target");
    assert_eq!(a_targets[0].host, "a.example.com");

    // Rule B got everything: three targets, round_robin, the rate caps.
    let b = db
        .find_rule_by_id(id_b, &ResourceScope::All)
        .await
        .unwrap()
        .expect("rule B exists");
    assert_eq!(b.device_group_in, 2);
    assert_eq!(b.load_balance_strategy, "round_robin");
    assert_eq!(b.upload_limit_mbps, 50);
    assert_eq!(b.download_limit_mbps, 100);
    let b_targets = db
        .list_rule_targets(id_b, &ResourceScope::All)
        .await
        .unwrap();
    assert_eq!(b_targets.len(), 3, "rule B got its three targets");
    assert_eq!(b_targets[0].host, "b1.example.com");
    assert_eq!(b_targets[2].host, "b3.example.com");
}

/// v1.2 regression: create_rule_full is one transaction, so a failure in the
/// targets write (here a port=0 target violates the table's CHECK constraint)
/// must roll back the rule-row INSERT — no half-rule left behind.
#[tokio::test]
async fn rule_create_full_rollback_on_target_failure() {
    let db = repo().await;
    seed_group(&db, 1).await;

    // port 0 is a valid u16 but violates forward_rule_targets CHECK (port >= 1),
    // so the target INSERT inside the transaction fails.
    let err = db
        .create_rule_full(
            "doomed",
            1,
            30000,
            "tcp",
            "raw",
            "raw",
            "direct",
            "raw",
            None,
            1,
            None,
            "direct",
            "1.1.1.1",
            80,
            &[relay_shared::protocol::RuleTargetRequest {
                host: "bad.example.com".into(),
                port: 0,
                enabled: true,
            }],
            "first",
            0,
            0,
            None,
        )
        .await;
    assert!(err.is_err(), "the bad target must fail the whole call");

    // No half-rule residue: zero rows on port 30000 for this owner.
    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM forward_rules WHERE uid = 1 AND listen_port = 30000",
    )
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(count, 0, "transaction must roll back, leaving no rule row");
    // And no orphan targets either.
    let target_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM forward_rule_targets WHERE rule_id IN \
             (SELECT id FROM forward_rules WHERE uid = 1)",
    )
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(target_count, 0, "no orphan target rows");
}

/// v1.2: create_rule_full reports Ok(None) (not an error, not a row) when the
/// owner's max_rules quota is exhausted, so the service can map that to a 400.
#[tokio::test]
async fn rule_create_full_quota_exhausted_returns_none() {
    let db = repo().await;
    seed_group(&db, 1).await;
    sqlx::query("UPDATE users SET max_rules = 1 WHERE id = 1")
        .execute(&db.pool)
        .await
        .unwrap();

    // First create succeeds with Some(id).
    assert!(
        db.create_rule_full(
            "r1",
            1,
            40000,
            "tcp",
            "raw",
            "raw",
            "direct",
            "raw",
            None,
            1,
            None,
            "direct",
            "1.1.1.1",
            80,
            &[relay_shared::protocol::RuleTargetRequest {
                host: "a.example.com".into(),
                port: 80,
                enabled: true,
            }],
            "first",
            0,
            0,
            None,
        )
        .await
        .unwrap()
        .is_some(),
        "first rule within quota returns Some(id)"
    );

    // Second create hits the quota guard → Ok(None), and crucially no new row.
    assert_eq!(
        db.create_rule_full(
            "r2",
            1,
            40001,
            "tcp",
            "raw",
            "raw",
            "direct",
            "raw",
            None,
            1,
            None,
            "direct",
            "1.1.1.1",
            80,
            &[relay_shared::protocol::RuleTargetRequest {
                host: "a.example.com".into(),
                port: 80,
                enabled: true,
            }],
            "first",
            0,
            0,
            None,
        )
        .await
        .unwrap(),
        None,
        "quota exhaustion returns Ok(None)"
    );

    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM forward_rules WHERE uid = 1")
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!(count, 1, "the over-quota create wrote no row");
}

/// v0.4.11 PR4: a pure-TCP and a pure-UDP rule may share the same port on
/// the same group; two TCP-bearing (or two UDP-bearing) rules may not.
#[tokio::test]
async fn rule_insert_quota_guarded_tcp_udp_share_port() {
    let db = repo().await;
    seed_group(&db, 1).await;
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
    // tcp on 20000 → OK.
    insert("r1", "tcp").await.unwrap();
    // udp on the SAME port + group → OK (different socket type).
    insert("r2", "udp").await.unwrap();
    // Another tcp on 20000 → PortConflict (TCP already held).
    match insert("r3", "tcp").await {
        Err(DbError::PortConflict) => {}
        other => panic!("expected PortConflict for second tcp, got {:?}", other),
    }
    // Another udp on 20000 → PortConflict (UDP already held).
    match insert("r4", "udp").await {
        Err(DbError::PortConflict) => {}
        other => panic!("expected PortConflict for second udp, got {:?}", other),
    }
    // tcp_udp on 20000 → PortConflict (overlaps both).
    match insert("r5", "tcp_udp").await {
        Err(DbError::PortConflict) => {}
        other => panic!("expected PortConflict for tcp_udp, got {:?}", other),
    }
}

/// v0.4.11 PR4: the same port on a DIFFERENT group is allowed (independent
/// pools). Different users sharing one group share its pool — modeled here
/// by inserting two rules with different uids into the same group.
#[tokio::test]
async fn rule_insert_quota_guarded_port_scoped_by_group() {
    let db = repo().await;
    seed_group(&db, 1).await;
    seed_group(&db, 2).await;
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
    // group 1, port 20000 → OK.
    insert("r1", 1, 1).await.unwrap();
    // group 2, same port → OK (different group).
    insert("r2", 1, 2).await.unwrap();
    // group 1 again from a DIFFERENT user → shared pool → PortConflict.
    match insert("r3", 2, 1).await {
        Err(DbError::PortConflict) => {}
        other => panic!(
            "expected PortConflict on shared group pool, got {:?}",
            other
        ),
    }
}

// ── v1.2.x: auto_assign_port honors the group's port_range ──

/// An explicit narrow range confines every auto-assigned port to that range.
#[tokio::test]
async fn auto_assign_port_stays_within_explicit_group_range() {
    use crate::service::rules::auto_assign_port;
    let db = repo().await;
    seed_group_with_range(&db, 1, "65000-65100").await;
    // Nothing is occupied, so every draw is valid — assert it stays in-range
    // across many random starts (the ring scan starts at a random offset).
    for _ in 0..30 {
        let p = auto_assign_port(&db, 1, "tcp").await.unwrap();
        assert!(
            (65000..=65100).contains(&p),
            "auto port {} escaped the configured 65000-65100 range",
            p
        );
    }
}

/// The `1-65535` schema default (seed_group leaves it unset → default) is the
/// "全可转发" sentinel and maps to the 10000-65535 pool, never a system port.
#[tokio::test]
async fn auto_assign_port_sentinel_uses_default_pool() {
    use crate::service::rules::auto_assign_port;
    let db = repo().await;
    seed_group(&db, 1).await; // port_range defaults to '1-65535'
    for _ in 0..30 {
        let p = auto_assign_port(&db, 1, "tcp").await.unwrap();
        assert!(
            (10000..=65535).contains(&p),
            "sentinel must map to 10000-65535, got {}",
            p
        );
    }
}

/// A full range errors (naming the real range) instead of spilling outside it —
/// and the fullness is socket-type scoped: a TCP occupant doesn't block UDP.
#[tokio::test]
async fn auto_assign_port_errors_when_range_full() {
    use crate::service::rules::auto_assign_port;
    let db = repo().await;
    seed_group_with_range(&db, 1, "50000-50000").await;
    // Occupy the pool's only port with a TCP rule on this group.
    db.insert_quota_guarded(
        "r1",
        1,
        50000,
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
    let err = auto_assign_port(&db, 1, "tcp").await.unwrap_err();
    assert!(
        err.contains("50000-50000"),
        "error must name the exhausted range, got {:?}",
        err
    );
    // A UDP-bearing rule doesn't conflict with the TCP occupant → still fits.
    let p = auto_assign_port(&db, 1, "udp").await.unwrap();
    assert_eq!(p, 50000, "udp must reuse the port held only by tcp");
}

/// A non-existent group id (None port_range) falls back to the default pool
/// rather than erroring.
#[tokio::test]
async fn auto_assign_port_missing_group_uses_default_pool() {
    use crate::service::rules::auto_assign_port;
    let db = repo().await;
    let p = auto_assign_port(&db, 999, "tcp").await.unwrap();
    assert!(
        (10000..=65535).contains(&p),
        "missing group must use the default pool, got {}",
        p
    );
}

#[tokio::test]
async fn rule_update_rule_fields_partial_update() {
    let db = repo().await;
    seed_group(&db, 1).await;
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

    // Rename only; protocol must be untouched.
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
            None,
        )
        .await
        .unwrap(),
        1
    );
    let row: (String, String) =
        sqlx::query_as("SELECT name, protocol FROM forward_rules WHERE id = ?")
            .bind(rule_id)
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert_eq!(row.0, "renamed");
    assert_eq!(row.1, "tcp", "protocol must be untouched");

    // Switching to direct clears device_group_out via Some(None) (the
    // outer-Some / inner-None shape), not a separate force flag.
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
            None,
        )
        .await
        .unwrap(),
        1
    );
    let dgo: (Option<i64>,) =
        sqlx::query_as("SELECT device_group_out FROM forward_rules WHERE id = ?")
            .bind(rule_id)
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert!(dgo.0.is_none(), "device_group_out must be cleared");
}

#[tokio::test]
async fn rule_list_active_for_config_filters_banned_paused_overquota() {
    let db = repo().await;
    // Seed a second user (non-admin) with a group + rule.
    db.insert_user("alice", "h", 1).await.unwrap();
    let alice = db.find_by_username("alice").await.unwrap().unwrap().id;
    sqlx::query(
        "INSERT INTO device_groups (id, name, group_type, token, uid) \
         VALUES (50, 'gin', 'in', 'tok-50', ?)",
    )
    .bind(alice)
    .execute(&db.pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO forward_rules \
         (name, uid, listen_port, device_group_in, target_addr, target_port) \
         VALUES ('r-active', ?, 20000, 50, '127.0.0.1', 80)",
    )
    .bind(alice)
    .execute(&db.pool)
    .await
    .unwrap();

    // Initially the rule appears in the active set for group 50.
    assert_eq!(db.list_active_for_config(50).await.unwrap().len(), 1);

    // Pause it → filtered out.
    sqlx::query("UPDATE forward_rules SET paused = 1 WHERE device_group_in = 50")
        .execute(&db.pool)
        .await
        .unwrap();
    assert_eq!(
        db.list_active_for_config(50).await.unwrap().len(),
        0,
        "paused rule must be filtered"
    );
    sqlx::query("UPDATE forward_rules SET paused = 0 WHERE device_group_in = 50")
        .execute(&db.pool)
        .await
        .unwrap();

    // Ban alice → filtered out.
    sqlx::query("UPDATE users SET banned = 1 WHERE id = ?")
        .bind(alice)
        .execute(&db.pool)
        .await
        .unwrap();
    assert_eq!(
        db.list_active_for_config(50).await.unwrap().len(),
        0,
        "banned user's rule must be filtered"
    );
    sqlx::query("UPDATE users SET banned = 0 WHERE id = ?")
        .bind(alice)
        .execute(&db.pool)
        .await
        .unwrap();

    // Over-quota → filtered (traffic_limit=100, traffic_used=100).
    sqlx::query("UPDATE users SET traffic_limit = 100, traffic_used = 100 WHERE id = ?")
        .bind(alice)
        .execute(&db.pool)
        .await
        .unwrap();
    assert_eq!(
        db.list_active_for_config(50).await.unwrap().len(),
        0,
        "over-quota user's rule must be filtered"
    );

    // traffic_limit = 0 means unlimited — must reappear even with high usage.
    sqlx::query("UPDATE users SET traffic_limit = 0 WHERE id = ?")
        .bind(alice)
        .execute(&db.pool)
        .await
        .unwrap();
    assert_eq!(
        db.list_active_for_config(50).await.unwrap().len(),
        1,
        "unlimited-quota rule must reappear"
    );
}

// ── GroupRepository ──

#[tokio::test]
async fn group_insert_then_find_by_token_round_trip() {
    let db = repo().await;
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

    // find_by_token_after_insert returns the same row.
    let g2 = db
        .find_by_token_after_insert("tok-abc")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(g2.id, g.id);

    // Unknown token → None.
    assert!(db.find_by_token("nope").await.unwrap().is_none());

    // find_name_by_id returns the name.
    assert_eq!(
        db.find_name_by_id(g.id, &ResourceScope::All)
            .await
            .unwrap()
            .as_deref(),
        Some("gin")
    );
}

#[tokio::test]
async fn group_update_token_returns_rows_affected() {
    let db = repo().await;
    db.insert_group("gin", "in", "tok-1", 1, "", "", 1.0, false)
        .await
        .unwrap();
    let g = db.find_by_token("tok-1").await.unwrap().unwrap();

    // Existing id → 1 row affected, and the new token now resolves.
    assert_eq!(
        db.update_group_token(g.id, &ResourceScope::All, "tok-2")
            .await
            .unwrap(),
        1
    );
    assert!(db.find_by_token("tok-1").await.unwrap().is_none());
    assert!(db.find_by_token("tok-2").await.unwrap().is_some());

    // Unknown id → 0 rows.
    assert_eq!(
        db.update_group_token(999_999, &ResourceScope::All, "tok-3")
            .await
            .unwrap(),
        0
    );
}

// ── TrafficRepository ──

#[tokio::test]
async fn traffic_batch_applies_to_rule_and_user() {
    let db = repo().await;
    // Seed alice + group 50 + rule 100 owned by alice on group 50.
    db.insert_user("alice", "h", 1).await.unwrap();
    let alice = db.find_by_username("alice").await.unwrap().unwrap().id;
    sqlx::query(
        "INSERT INTO device_groups (id, name, group_type, token, uid) \
         VALUES (50, 'gin', 'in', 'tok-50', ?)",
    )
    .bind(alice)
    .execute(&db.pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO forward_rules \
         (id, name, uid, listen_port, device_group_in, target_addr, target_port) \
         VALUES (100, 'r100', ?, 20000, 50, '127.0.0.1', 80)",
    )
    .bind(alice)
    .execute(&db.pool)
    .await
    .unwrap();

    let results = db
        .apply_traffic_batch(
            50,
            &[relay_shared::protocol::TrafficEntry {
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
    let user_t: (i64,) = sqlx::query_as("SELECT traffic_used FROM users WHERE id = ?")
        .bind(alice)
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!(rule_t.0, 3000);
    assert_eq!(user_t.0, 3000);
}

#[tokio::test]
async fn traffic_batch_other_group_rule_yields_othergrouprule_and_rolls_back() {
    let db = repo().await;
    db.insert_user("alice", "h", 1).await.unwrap();
    let alice = db.find_by_username("alice").await.unwrap().unwrap().id;
    // group 50 (alice's), group 60 (also alice's — same user, different
    // group, so the rule legitimately exists but is owned by group 60).
    for gid in [50, 60] {
        sqlx::query(
            "INSERT INTO device_groups (id, name, group_type, token, uid) \
             VALUES (?, 'g', 'in', ?, ?)",
        )
        .bind(gid)
        .bind(format!("tok-{gid}"))
        .bind(alice)
        .execute(&db.pool)
        .await
        .unwrap();
    }
    // rule 100 on group 50 (legitimate for token tok-50).
    sqlx::query(
        "INSERT INTO forward_rules \
         (id, name, uid, listen_port, device_group_in, target_addr, target_port) \
         VALUES (100, 'r100', ?, 20000, 50, '127.0.0.1', 80)",
    )
    .bind(alice)
    .execute(&db.pool)
    .await
    .unwrap();
    // rule 200 on group 60 — NOT owned by group 50.
    sqlx::query(
        "INSERT INTO forward_rules \
         (id, name, uid, listen_port, device_group_in, target_addr, target_port) \
         VALUES (200, 'r200', ?, 20001, 60, '127.0.0.1', 80)",
    )
    .bind(alice)
    .execute(&db.pool)
    .await
    .unwrap();

    // Batch contains BOTH rule 100 (legitimate) and rule 200 (foreign).
    // The contract: a foreign rule → Unavailable, entire batch rolled back.
    let results = db
        .apply_traffic_batch(
            50,
            &[
                relay_shared::protocol::TrafficEntry {
                    rule_id: 100,
                    upload: 500,
                    download: 0,
                },
                relay_shared::protocol::TrafficEntry {
                    rule_id: 200,
                    upload: 0,
                    download: 999,
                },
            ],
        )
        .await
        .unwrap();
    // v0.4.9: a foreign rule yields Unavailable (formerly OtherGroupRule).
    assert_eq!(results.len(), 1);
    assert!(matches!(
        results[0],
        crate::db::repo::TrafficEntryResult::Unavailable
    ));

    // Rollback: even rule 100's update must NOT have landed.
    let rule100_t: (i64,) = sqlx::query_as("SELECT traffic_used FROM forward_rules WHERE id = 100")
        .fetch_one(&db.pool)
        .await
        .unwrap();
    let user_t: (i64,) = sqlx::query_as("SELECT traffic_used FROM users WHERE id = ?")
        .bind(alice)
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!(rule100_t.0, 0, "legitimate entry must be rolled back too");
    assert_eq!(user_t.0, 0);
}

/// v0.4.9: a rule_id that does NOT exist must produce the SAME result as a
/// foreign rule (Unavailable) — NOT be silently skipped. This closes the
/// rule-id existence oracle: a node can no longer tell, from the response,
/// whether an id is missing vs owned by another group. The whole batch is
/// rolled back; the legitimate rule's traffic does NOT land.
#[tokio::test]
async fn traffic_batch_unknown_rule_is_unavailable_not_skipped() {
    let db = repo().await;
    db.insert_user("alice", "h", 1).await.unwrap();
    let alice = db.find_by_username("alice").await.unwrap().unwrap().id;
    sqlx::query(
        "INSERT INTO device_groups (id, name, group_type, token, uid) \
         VALUES (50, 'gin', 'in', 'tok-50', ?)",
    )
    .bind(alice)
    .execute(&db.pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO forward_rules \
         (id, name, uid, listen_port, device_group_in, target_addr, target_port) \
         VALUES (100, 'r100', ?, 20000, 50, '127.0.0.1', 80)",
    )
    .bind(alice)
    .execute(&db.pool)
    .await
    .unwrap();

    // Batch with rule 99999 (does not exist) + rule 100 (legitimate).
    // Pre-v0.4.9 the unknown one was skipped and rule 100 still applied.
    // Now the unknown id is treated identically to a foreign id → whole
    // batch rejected (Unavailable), rule 100 NOT applied.
    let results = db
        .apply_traffic_batch(
            50,
            &[
                relay_shared::protocol::TrafficEntry {
                    rule_id: 99999,
                    upload: 1,
                    download: 2,
                },
                relay_shared::protocol::TrafficEntry {
                    rule_id: 100,
                    upload: 10,
                    download: 20,
                },
            ],
        )
        .await
        .unwrap();
    assert_eq!(results.len(), 1);
    assert!(matches!(
        results[0],
        crate::db::repo::TrafficEntryResult::Unavailable
    ));
    let rule_t: (i64,) = sqlx::query_as("SELECT traffic_used FROM forward_rules WHERE id = 100")
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!(rule_t.0, 0, "batch rolled back → rule 100 must not apply");
}

/// v0.4.9 overflow: a single entry whose upload+download exceeds i64::MAX
/// → Overflow, whole batch rolled back.
#[tokio::test]
async fn traffic_batch_single_entry_overflow_rejects_and_rolls_back() {
    let db = repo().await;
    db.insert_user("alice", "h", 1).await.unwrap();
    let alice = db.find_by_username("alice").await.unwrap().unwrap().id;
    sqlx::query(
        "INSERT INTO device_groups (id, name, group_type, token, uid) \
         VALUES (50, 'gin', 'in', 'tok-50', ?)",
    )
    .bind(alice)
    .execute(&db.pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO forward_rules \
         (id, name, uid, listen_port, device_group_in, target_addr, target_port) \
         VALUES (100, 'r100', ?, 20000, 50, '127.0.0.1', 80)",
    )
    .bind(alice)
    .execute(&db.pool)
    .await
    .unwrap();
    // upload + download just over i64::MAX.
    let half = (i64::MAX as u64) / 2 + 1;
    let results = db
        .apply_traffic_batch(
            50,
            &[relay_shared::protocol::TrafficEntry {
                rule_id: 100,
                upload: half,
                download: half,
            }],
        )
        .await
        .unwrap();
    assert!(matches!(
        results[0],
        crate::db::repo::TrafficEntryResult::Overflow
    ));
    let rule_t: (i64,) = sqlx::query_as("SELECT traffic_used FROM forward_rules WHERE id = 100")
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!(rule_t.0, 0, "overflow → no write");
}

/// v0.4.9 overflow: duplicate rule_ids in one batch, each legal alone but
/// overflowing when summed → Overflow, rolled back.
#[tokio::test]
async fn traffic_batch_duplicate_rule_ids_cumulative_overflow() {
    let db = repo().await;
    db.insert_user("alice", "h", 1).await.unwrap();
    let alice = db.find_by_username("alice").await.unwrap().unwrap().id;
    sqlx::query(
        "INSERT INTO device_groups (id, name, group_type, token, uid) \
         VALUES (50, 'gin', 'in', 'tok-50', ?)",
    )
    .bind(alice)
    .execute(&db.pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO forward_rules \
         (id, name, uid, listen_port, device_group_in, target_addr, target_port) \
         VALUES (100, 'r100', ?, 20000, 50, '127.0.0.1', 80)",
    )
    .bind(alice)
    .execute(&db.pool)
    .await
    .unwrap();
    // Two entries for the SAME rule 100, each upload near i64::MAX/2.
    // Individually legal; their aggregated delta overflows.
    let half = (i64::MAX as u64) / 2 + 1;
    let results = db
        .apply_traffic_batch(
            50,
            &[
                relay_shared::protocol::TrafficEntry {
                    rule_id: 100,
                    upload: half,
                    download: 0,
                },
                relay_shared::protocol::TrafficEntry {
                    rule_id: 100,
                    upload: half,
                    download: 0,
                },
            ],
        )
        .await
        .unwrap();
    assert!(matches!(
        results[0],
        crate::db::repo::TrafficEntryResult::Overflow
    ));
    let rule_t: (i64,) = sqlx::query_as("SELECT traffic_used FROM forward_rules WHERE id = 100")
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!(rule_t.0, 0);
}

/// v0.4.9 overflow: a user owns two rules; each delta is legal, but their
/// cumulative effect on the USER's total overflows → Overflow, rolled back
/// (neither rule lands).
#[tokio::test]
async fn traffic_batch_user_cumulative_overflow_across_rules() {
    let db = repo().await;
    db.insert_user("alice", "h", 1).await.unwrap();
    let alice = db.find_by_username("alice").await.unwrap().unwrap().id;
    sqlx::query(
        "INSERT INTO device_groups (id, name, group_type, token, uid) \
         VALUES (50, 'gin', 'in', 'tok-50', ?)",
    )
    .bind(alice)
    .execute(&db.pool)
    .await
    .unwrap();
    // Two rules under the same user/group (distinct listen_ports — the
    // schema enforces listen_port uniqueness).
    for (rid, port) in [(100, 20000), (101, 20001)] {
        sqlx::query(
            "INSERT INTO forward_rules \
             (id, name, uid, listen_port, device_group_in, target_addr, target_port) \
             VALUES (?, 'r', ?, ?, 50, '127.0.0.1', 80)",
        )
        .bind(rid)
        .bind(alice)
        .bind(port)
        .execute(&db.pool)
        .await
        .unwrap();
    }
    // Pre-set the user's traffic near the ceiling so two legal deltas tip
    // the USER total over i64::MAX (the per-rule totals would be fine).
    sqlx::query("UPDATE users SET traffic_used = ? WHERE id = ?")
        .bind(i64::MAX - 100)
        .bind(alice)
        .execute(&db.pool)
        .await
        .unwrap();
    let results = db
        .apply_traffic_batch(
            50,
            &[
                relay_shared::protocol::TrafficEntry {
                    rule_id: 100,
                    upload: 60,
                    download: 0,
                },
                relay_shared::protocol::TrafficEntry {
                    rule_id: 101,
                    upload: 60,
                    download: 0,
                },
            ],
        )
        .await
        .unwrap();
    assert!(matches!(
        results[0],
        crate::db::repo::TrafficEntryResult::Overflow
    ));
    // Neither rule nor the user changed.
    let r100: (i64,) = sqlx::query_as("SELECT traffic_used FROM forward_rules WHERE id = 100")
        .fetch_one(&db.pool)
        .await
        .unwrap();
    let r101: (i64,) = sqlx::query_as("SELECT traffic_used FROM forward_rules WHERE id = 101")
        .fetch_one(&db.pool)
        .await
        .unwrap();
    let user_t: (i64,) = sqlx::query_as("SELECT traffic_used FROM users WHERE id = ?")
        .bind(alice)
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!(r100.0, 0);
    assert_eq!(r101.0, 0);
    assert_eq!(user_t.0, i64::MAX - 100, "user total unchanged");
}

/// v0.4.9: boundary — a delta that lands the rule's total EXACTLY on
/// i64::MAX is accepted (overflow is strictly > MAX).
#[tokio::test]
async fn traffic_batch_exactly_i64_max_is_accepted() {
    let db = repo().await;
    db.insert_user("alice", "h", 1).await.unwrap();
    let alice = db.find_by_username("alice").await.unwrap().unwrap().id;
    sqlx::query(
        "INSERT INTO device_groups (id, name, group_type, token, uid) \
         VALUES (50, 'gin', 'in', 'tok-50', ?)",
    )
    .bind(alice)
    .execute(&db.pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO forward_rules \
         (id, name, uid, listen_port, device_group_in, target_addr, target_port) \
         VALUES (100, 'r100', ?, 20000, 50, '127.0.0.1', 80)",
    )
    .bind(alice)
    .execute(&db.pool)
    .await
    .unwrap();
    // Pre-set rule + user to MAX-50, then add exactly 50 → lands on MAX.
    sqlx::query("UPDATE forward_rules SET traffic_used = ? WHERE id = 100")
        .bind(i64::MAX - 50)
        .execute(&db.pool)
        .await
        .unwrap();
    sqlx::query("UPDATE users SET traffic_used = ? WHERE id = ?")
        .bind(i64::MAX - 50)
        .bind(alice)
        .execute(&db.pool)
        .await
        .unwrap();
    let results = db
        .apply_traffic_batch(
            50,
            &[relay_shared::protocol::TrafficEntry {
                rule_id: 100,
                upload: 50,
                download: 0,
            }],
        )
        .await
        .unwrap();
    assert!(matches!(
        results[0],
        crate::db::repo::TrafficEntryResult::Ok
    ));
    let rule_t: (i64,) = sqlx::query_as("SELECT traffic_used FROM forward_rules WHERE id = 100")
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!(rule_t.0, i64::MAX);
}

/// v0.4.9: duplicate rule_ids in an otherwise-legal batch are AGGREGATED
/// (summed) and applied as ONE update — no double SQL, correct total.
#[tokio::test]
async fn traffic_batch_duplicate_rule_ids_are_aggregated() {
    let db = repo().await;
    db.insert_user("alice", "h", 1).await.unwrap();
    let alice = db.find_by_username("alice").await.unwrap().unwrap().id;
    sqlx::query(
        "INSERT INTO device_groups (id, name, group_type, token, uid) \
         VALUES (50, 'gin', 'in', 'tok-50', ?)",
    )
    .bind(alice)
    .execute(&db.pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO forward_rules \
         (id, name, uid, listen_port, device_group_in, target_addr, target_port) \
         VALUES (100, 'r100', ?, 20000, 50, '127.0.0.1', 80)",
    )
    .bind(alice)
    .execute(&db.pool)
    .await
    .unwrap();
    // Three entries for rule 100 → aggregated to upload 6, download 60.
    let results = db
        .apply_traffic_batch(
            50,
            &[
                relay_shared::protocol::TrafficEntry {
                    rule_id: 100,
                    upload: 1,
                    download: 10,
                },
                relay_shared::protocol::TrafficEntry {
                    rule_id: 100,
                    upload: 2,
                    download: 20,
                },
                relay_shared::protocol::TrafficEntry {
                    rule_id: 100,
                    upload: 3,
                    download: 30,
                },
            ],
        )
        .await
        .unwrap();
    assert!(matches!(
        results[0],
        crate::db::repo::TrafficEntryResult::Ok
    ));
    let rule_t: (i64,) = sqlx::query_as("SELECT traffic_used FROM forward_rules WHERE id = 100")
        .fetch_one(&db.pool)
        .await
        .unwrap();
    let user_t: (i64,) = sqlx::query_as("SELECT traffic_used FROM users WHERE id = ?")
        .bind(alice)
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!(rule_t.0, 66, "aggregated delta = 6+60");
    assert_eq!(user_t.0, 66);
}

// ── KvsRepository ──

#[tokio::test]
async fn kvs_set_get_delete_round_trip() {
    let db = repo().await;
    // Absent key → None.
    assert!(db.get("missing").await.unwrap().is_none());

    // Set then get.
    db.set("k", "v1").await.unwrap();
    assert_eq!(db.get("k").await.unwrap().as_deref(), Some("v1"));

    // Set again (INSERT OR REPLACE upsert).
    db.set("k", "v2").await.unwrap();
    assert_eq!(db.get("k").await.unwrap().as_deref(), Some("v2"));

    // Delete returns rows affected.
    assert_eq!(db.delete("k").await.unwrap(), 1);
    assert!(db.get("k").await.unwrap().is_none());

    // Delete of absent key returns 0.
    assert_eq!(db.delete("k").await.unwrap(), 0);
}

#[tokio::test]
async fn kvs_scan_prefix_returns_only_matching_keys() {
    let db = repo().await;
    db.set("node_status:1:a", "{}").await.unwrap();
    db.set("node_status:1:b", "{}").await.unwrap();
    db.set("node_status:2:c", "{}").await.unwrap();
    db.set("other_feature:1", "{}").await.unwrap();

    // scan_prefix matches the LIKE 'node_status:%' pattern.
    let rows = db.scan_prefix("node_status:").await.unwrap();
    assert_eq!(rows.len(), 3);
    assert!(rows.iter().all(|(k, _)| k.starts_with("node_status:")));

    // A more specific prefix narrows further.
    let rows = db.scan_prefix("node_status:1:").await.unwrap();
    assert_eq!(rows.len(), 2);
}

// ── v0.4.10 fix PR: ProfileScope + ownership-invariant tests ──

/// find_profile_by_id with BuiltinOnly must NOT return a custom profile.
#[tokio::test]
async fn find_profile_by_id_builtin_only_excludes_custom() {
    let db = repo().await;
    // Insert a custom (non-builtin) ws profile owned by admin (uid=1).
    // v0.4.11 PR1: custom profiles must be ws/tls_simple to be "available".
    sqlx::query(
        "INSERT INTO tunnel_profiles (name, transport, tls_mode, ws_path, host_header, sni, is_builtin, uid) \
         VALUES ('custom-x', 'ws', 'none', '/x', '', '', 0, 1)",
    )
    .execute(&db.pool)
    .await
    .unwrap();
    let custom_id: i64 =
        sqlx::query_scalar("SELECT id FROM tunnel_profiles WHERE name = 'custom-x'")
            .fetch_one(&db.pool)
            .await
            .unwrap();

    // AvailableTemplates → Some (custom ws/tls_simple visible).
    let r = TunnelProfileRepository::find_profile_by_id(
        &db,
        custom_id,
        &ProfileScope::AvailableTemplates,
    )
    .await
    .unwrap();
    assert!(
        r.is_some(),
        "AvailableTemplates must return custom ws/tls_simple profile"
    );

    // All → Some.
    let r = TunnelProfileRepository::find_profile_by_id(&db, custom_id, &ProfileScope::All)
        .await
        .unwrap();
    assert!(r.is_some(), "All must return custom profile");
}

/// v0.4.11 PR3: Migration 24 NO LONGER pauses cross-owner rules.
/// Shared inbound groups are now a valid use case (admin creates an inbound
/// group, regular users attach rules to it). We verify the migration does
/// NOT pause such rules.
#[tokio::test]
async fn migration_does_not_pause_cross_owner_shared_inbound_rules() {
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .unwrap();
    sqlx::query(SCHEMA_SQL).execute(&pool).await.unwrap();
    // user 2 (regular), group 20 owned by admin (user 1), rule owned by
    // user 2 pointing at group 20 → shared inbound scenario, MUST NOT pause.
    sqlx::query("INSERT INTO users (id, username, password, admin) VALUES (2, 'u2', 'x', 0)")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO device_groups (id, name, group_type, token, uid) VALUES (20, 'g', 'in', 't', 1)")
        .execute(&pool).await.unwrap();
    sqlx::query("INSERT INTO forward_rules (name, uid, listen_port, device_group_in, target_addr, target_port) \
                 VALUES ('r', 2, 15000, 20, '127.0.0.1', 80)")
        .execute(&pool).await.unwrap();

    sqlx::query("PRAGMA foreign_keys = OFF")
        .execute(&pool)
        .await
        .unwrap();
    crate::db::schema::run_migrations(&pool).await.unwrap();
    sqlx::query("PRAGMA foreign_keys = ON")
        .execute(&pool)
        .await
        .unwrap();

    let paused: (i64,) = sqlx::query_as("SELECT paused FROM forward_rules WHERE name = 'r'")
        .fetch_one(&pool)
        .await
        .unwrap();
    // v0.4.11 PR3: cross-owner shared inbound rules are ALLOWED
    assert_eq!(
        paused.0, 0,
        "cross-owner shared inbound rule must NOT be paused"
    );
}

/// Migration 24 pauses a regular user's rule bound to a non-builtin profile.
#[tokio::test]
async fn migration_pauses_non_admin_owner_custom_profile_rule() {
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .unwrap();
    sqlx::query(SCHEMA_SQL).execute(&pool).await.unwrap();
    sqlx::query("INSERT INTO users (id, username, password, admin) VALUES (2, 'u2', 'x', 0)")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO device_groups (id, name, group_type, token, uid) VALUES (20, 'g', 'in', 't', 2)")
        .execute(&pool).await.unwrap();
    sqlx::query("INSERT INTO tunnel_profiles (name, transport, tls_mode, ws_path, host_header, sni, is_builtin, uid) \
                 VALUES ('cust', 'direct', 'none', '/x', '', '', 0, 1)")
        .execute(&pool).await.unwrap();
    let pid: i64 = sqlx::query_scalar("SELECT id FROM tunnel_profiles WHERE name = 'cust'")
        .fetch_one(&pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO forward_rules (name, uid, listen_port, device_group_in, target_addr, target_port, tunnel_profile_id) \
                 VALUES ('r', 2, 15001, 20, '127.0.0.1', 80, ?)")
        .bind(pid)
        .execute(&pool).await.unwrap();

    sqlx::query("PRAGMA foreign_keys = OFF")
        .execute(&pool)
        .await
        .unwrap();
    crate::db::schema::run_migrations(&pool).await.unwrap();
    sqlx::query("PRAGMA foreign_keys = ON")
        .execute(&pool)
        .await
        .unwrap();

    let paused: (i64,) = sqlx::query_as("SELECT paused FROM forward_rules WHERE name = 'r'")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(
        paused.0, 1,
        "non-admin rule with custom profile must be paused"
    );
}

/// Migration 24 must NOT pause a legitimate rule (owner-consistent groups,
/// builtin-or-no profile). This is the false-positive guard.
#[tokio::test]
async fn migration_does_not_pause_valid_rules() {
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .unwrap();
    sqlx::query(SCHEMA_SQL).execute(&pool).await.unwrap();
    // Regular user 2, owns group 20, rule owned by 2 pointing at 20 → consistent.
    sqlx::query("INSERT INTO users (id, username, password, admin) VALUES (2, 'u2', 'x', 0)")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO device_groups (id, name, group_type, token, uid) VALUES (20, 'g', 'in', 't', 2)")
        .execute(&pool).await.unwrap();
    sqlx::query("INSERT INTO forward_rules (name, uid, listen_port, device_group_in, target_addr, target_port) \
                 VALUES ('r', 2, 15002, 20, '127.0.0.1', 80)")
        .execute(&pool).await.unwrap();

    sqlx::query("PRAGMA foreign_keys = OFF")
        .execute(&pool)
        .await
        .unwrap();
    crate::db::schema::run_migrations(&pool).await.unwrap();
    sqlx::query("PRAGMA foreign_keys = ON")
        .execute(&pool)
        .await
        .unwrap();

    let paused: (i64,) = sqlx::query_as("SELECT paused FROM forward_rules WHERE name = 'r'")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(paused.0, 0, "valid rule must NOT be paused");
}

/// v0.4.11 PR3: list_active_for_config INCLUDES cross-owner rules (shared inbound).
#[tokio::test]
async fn list_active_for_config_excludes_cross_owner_rule() {
    let db = repo().await;
    // user 2 owns the rule; group 20 is owned by user 1 (admin, seeded).
    // v0.4.11 PR3: this cross-owner rule IS returned by list_active_for_config
    // (shared inbound group scenario). The invariant is now enforced at
    // create_rule time via Migration 24, not filtered here.
    sqlx::query("INSERT INTO users (id, username, password, admin) VALUES (2, 'u2', 'x', 0)")
        .execute(&db.pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO device_groups (id, name, group_type, token, uid) VALUES (20, 'g', 'in', 't', 1)")
        .execute(&db.pool).await.unwrap();
    sqlx::query("INSERT INTO forward_rules (name, uid, listen_port, device_group_in, target_addr, target_port) \
                 VALUES ('r', 2, 15003, 20, '127.0.0.1', 80)")
        .execute(&db.pool).await.unwrap();

    let rules = db.list_active_for_config(20).await.unwrap();
    // v0.4.11 PR3: cross-owner rule is now included (shared inbound).
    assert_eq!(
        rules.len(),
        1,
        "shared inbound rule must be returned for config"
    );
}

// ── v0.4.10 PR3: app_settings + insert_user_from_plan ──

/// get_registration_settings returns None on a fresh DB (no row seeded).
#[tokio::test]
async fn settings_get_returns_none_when_unseeded() {
    let db = repo().await;
    let s = db.get_registration_settings().await.unwrap();
    assert!(s.is_none(), "fresh DB must have no app_settings row");
}

/// insert_settings_if_absent inserts on first call, and is a no-op on the
/// second call — the env-var seed value must NOT override an existing row.
#[tokio::test]
async fn settings_insert_if_absent_is_idempotent() {
    let db = repo().await;
    // First boot seed: enabled=true (simulating REGISTRATION_ENABLED=1).
    db.insert_settings_if_absent(true, 1, &[1]).await.unwrap();
    let s = db.get_registration_settings().await.unwrap().unwrap();
    assert!(s.registration_enabled);
    assert_eq!(s.default_registration_plan_id, 1);

    // Simulate a restart with env still =1. The row already exists, so the
    // insert_if_absent must NOT run — even though we pass true again. To
    // prove the row isn't touched, first flip it to false (admin action),
    // then call insert_if_absent(true) again and assert it stays false.
    db.set_registration_settings(false, 1, &[1]).await.unwrap();
    db.insert_settings_if_absent(true, 1, &[1]).await.unwrap(); // "restart"
    let s = db.get_registration_settings().await.unwrap().unwrap();
    assert!(
        !s.registration_enabled,
        "env-var seed must NOT re-enable registration after admin disabled it"
    );
}

/// set_registration_settings is an upsert: it creates the row if missing
/// (no need for a prior insert_settings_if_absent).
#[tokio::test]
async fn settings_set_upserts_when_no_row() {
    let db = repo().await;
    assert!(db.get_registration_settings().await.unwrap().is_none());
    // PUT directly on an unseeded DB — upsert creates the row.
    db.set_registration_settings(true, 1, &[1]).await.unwrap();
    let s = db.get_registration_settings().await.unwrap().unwrap();
    assert!(s.registration_enabled);
}

/// v0.4.21 PR2: allowed_plan_ids round-trips through set_registration_settings
/// and insert_settings_if_absent (multi-plan, order preserved). Mirrors the PG
/// test pg_settings_allowed_plan_ids_round_trip for two-backend parity.
#[tokio::test]
async fn settings_allowed_plan_ids_round_trip() {
    let db = repo().await;
    // Seed plan 2 for the multi-plan test.
    sqlx::query(
        "INSERT INTO plans (id, name, max_rules, traffic, speed_limit, ip_limit, price) \
         VALUES (2, 'premium', 10, 0, 0, 5, '9.99') ON CONFLICT (id) DO NOTHING",
    )
    .execute(&db.pool)
    .await
    .unwrap();

    // Multi-plan settings round-trip.
    db.set_registration_settings(true, 1, &[1, 2])
        .await
        .unwrap();
    let s = db.get_registration_settings().await.unwrap().unwrap();
    assert!(s.registration_enabled);
    assert_eq!(s.default_registration_plan_id, 1);
    assert_eq!(
        s.allowed_plan_ids,
        vec![1, 2],
        "SQLite multi-plan round-trip"
    );

    // Unseeded row insert must also carry allowed_plan_ids.
    sqlx::query("DELETE FROM app_settings WHERE id = 1")
        .execute(&db.pool)
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
        "SQLite unseeded round-trip (order preserved)"
    );
}

/// insert_user_from_plan atomically copies the plan's quota fields into the
/// new user, and returns 0 when the plan doesn't exist (no user created).
#[tokio::test]
async fn insert_user_from_plan_inherits_quota_and_handles_missing_plan() {
    let db = repo().await;
    // plan_id=1 is the seeded 'free' plan (max_rules=5, traffic=107374182400).
    let n = db.insert_user_from_plan("alice", "hash", 1).await.unwrap();
    assert_eq!(n, 1, "user should be created for an existing plan");

    let user = db.find_by_username("alice").await.unwrap().unwrap();
    assert_eq!(user.plan_id, Some(1));
    assert_eq!(user.max_rules, 5, "max_rules must be inherited from plan");
    assert_eq!(
        user.traffic_limit, 107374182400,
        "traffic_limit must be inherited from plan.traffic"
    );

    // A non-existent plan → 0 rows affected, no user created.
    let n = db.insert_user_from_plan("bob", "hash", 999).await.unwrap();
    assert_eq!(n, 0, "missing plan must yield 0 rows affected");
    assert!(
        db.find_by_username("bob").await.unwrap().is_none(),
        "no user should be created for a missing plan"
    );
}

/// v1.2 regression: a freshly-registered user has NO usable device groups by
/// product design — `all_device_groups` stays false and `user_device_groups`
/// stays empty, so a brand-new user cannot forward until a plan/admin grants
/// authorization. This pins the behaviour so a future change (e.g. auto-grant
/// on registration) can't silently flip it.
#[tokio::test]
async fn new_user_has_no_device_groups_by_default() {
    let db = repo().await; // plan_id=1 (free) is seeded
    db.insert_user_from_plan("carol", "hash", 1).await.unwrap();
    let carol = db
        .find_by_username("carol")
        .await
        .unwrap()
        .expect("carol registered");
    assert!(!carol.admin);
    assert!(
        !carol.all_device_groups,
        "all_device_groups must default to false — a new user may not use any group"
    );

    // No explicit authorization rows either.
    assert!(
        db.list_user_device_groups(carol.id)
            .await
            .unwrap()
            .is_empty(),
        "user_device_groups must be empty for a new user"
    );
    assert!(
        db.authorized_device_group_ids(carol.id)
            .await
            .unwrap()
            .is_empty(),
        "authorized_device_group_ids must be empty for a new user"
    );
    assert!(
        db.is_user_restricted(carol.id).await.unwrap(),
        "a non-admin without all_device_groups is restricted (cannot forward)"
    );
}

/// Migration 25 is idempotent: re-running run_migrations on a DB whose
/// baseline SCHEMA_SQL already created app_settings must not error (the
/// CREATE TABLE IF NOT EXISTS is a no-op). This pins the upgrade path for
/// old databases that reach app_settings only via Migration 25.
#[tokio::test]
async fn migration_creates_app_settings_table() {
    let db = repo().await;
    // repo() ran SCHEMA_SQL (app_settings already present). Re-running
    // migrations must succeed (Migration 25's IF NOT EXISTS is a no-op).
    sqlx::query("PRAGMA foreign_keys = OFF")
        .execute(&db.pool)
        .await
        .unwrap();
    crate::db::schema::run_migrations(&db.pool)
        .await
        .expect("migrations must be idempotent on a baseline-schema DB");
    sqlx::query("PRAGMA foreign_keys = ON")
        .execute(&db.pool)
        .await
        .unwrap();

    // The table exists and is queryable (repo() did not seed a row).
    let count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM app_settings")
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!(count.0, 0, "table present but no row seeded by schema");
}

// ── v0.4.10 PR4: token_version + must_change_password ──

/// find_auth_state_by_id returns (banned, token_version, must_change) in one
/// query; None for a missing user.
#[tokio::test]
async fn find_auth_state_returns_all_three_or_none() {
    let db = repo().await;
    sqlx::query(
        "INSERT INTO users (id, username, password, admin, banned, token_version, must_change_password) \
         VALUES (2, 'u2', 'x', 0, 1, 7, 1)",
    )
    .execute(&db.pool)
    .await
    .unwrap();
    let s = db.find_auth_state_by_id(2).await.unwrap().unwrap();
    assert_eq!(s, (true, 7, true));
    assert!(db.find_auth_state_by_id(999).await.unwrap().is_none());
}

/// change_own_password bumps token_version and clears must_change_password.
#[tokio::test]
async fn change_own_password_bumps_version_and_clears_must_change() {
    let db = repo().await;
    sqlx::query(
        "INSERT INTO users (id, username, password, admin, token_version, must_change_password) \
         VALUES (2, 'u2', 'old', 0, 3, 1)",
    )
    .execute(&db.pool)
    .await
    .unwrap();
    let n = db.change_own_password(2, "newhash").await.unwrap();
    assert_eq!(n, 1);
    let s = db.find_auth_state_by_id(2).await.unwrap().unwrap();
    assert_eq!(s.1, 4, "token_version must increment");
    assert!(!s.2, "must_change_password must be cleared");
    let pw = db.find_password_by_id(2).await.unwrap().unwrap();
    assert_eq!(pw, "newhash");
}

/// admin_reset_password bumps token_version and sets must_change_password
/// to the requested value.
#[tokio::test]
async fn admin_reset_password_bumps_version_and_sets_must_change() {
    let db = repo().await;
    sqlx::query(
        "INSERT INTO users (id, username, password, admin, token_version, must_change_password) \
         VALUES (2, 'u2', 'old', 0, 0, 0)",
    )
    .execute(&db.pool)
    .await
    .unwrap();
    let n = db.admin_reset_password(2, "temphash", true).await.unwrap();
    assert_eq!(n, 1);
    let s = db.find_auth_state_by_id(2).await.unwrap().unwrap();
    assert_eq!(s.1, 1, "token_version must increment");
    assert!(s.2, "must_change_password must be set true");
}

/// Banning a user (update_user_fields banned=true) bumps token_version so
/// the ban revokes their existing JWTs.
#[tokio::test]
async fn ban_bumps_token_version() {
    let db = repo().await;
    sqlx::query(
        "INSERT INTO users (id, username, password, admin, banned, token_version) \
         VALUES (2, 'u2', 'x', 0, 0, 5)",
    )
    .execute(&db.pool)
    .await
    .unwrap();
    db.update_user_fields(2, None, None, None, Some(true), None)
        .await
        .unwrap();
    let s = db.find_auth_state_by_id(2).await.unwrap().unwrap();
    assert!(s.0, "user must be banned");
    assert_eq!(s.1, 6, "ban must bump token_version");
}

/// Unbanning (banned=false) does NOT bump token_version (only banning does).
#[tokio::test]
async fn unban_does_not_bump_token_version() {
    let db = repo().await;
    sqlx::query(
        "INSERT INTO users (id, username, password, admin, banned, token_version) \
         VALUES (2, 'u2', 'x', 0, 1, 5)",
    )
    .execute(&db.pool)
    .await
    .unwrap();
    db.update_user_fields(2, None, None, None, Some(false), None)
        .await
        .unwrap();
    let s = db.find_auth_state_by_id(2).await.unwrap().unwrap();
    assert!(!s.0, "user must be unbanned");
    assert_eq!(s.1, 5, "unban must NOT bump token_version");
}

/// Migration 26 is idempotent on a baseline-schema DB (columns already
/// present from SCHEMA_SQL).
#[tokio::test]
async fn migration_adds_password_columns() {
    let db = repo().await;
    sqlx::query("PRAGMA foreign_keys = OFF")
        .execute(&db.pool)
        .await
        .unwrap();
    crate::db::schema::run_migrations(&db.pool)
        .await
        .expect("migrations idempotent");
    sqlx::query("PRAGMA foreign_keys = ON")
        .execute(&db.pool)
        .await
        .unwrap();
    // Both columns must be queryable.
    let row: (i64, bool) =
        sqlx::query_as("SELECT token_version, must_change_password FROM users WHERE id = 1")
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert_eq!(row.0, 0, "default token_version is 0");
    assert!(!row.1, "default must_change_password is false");
}

// ── v0.4.18 PR8: Owner-scope authorization tests ──
//
// These pin the contract that ResourceScope::Owner filters by uid.
// The tested methods (delete_rule, find_rule_by_id, update_group_fields,
// delete_group) all accept a scope parameter and must reject operations
// on resources owned by a different user under Owner scope.

/// Owner scope: delete_rule succeeds for own rule, fails for another user's rule.
#[tokio::test]
async fn delete_rule_owner_scope_rejects_wrong_owner() {
    let db = repo().await;
    // User 2 owns the rule, user 3 does not.
    seed_user(&db, 2, false).await;
    seed_user(&db, 3, false).await;
    seed_group_typed(&db, 10, 2, "in").await;
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
    assert_eq!(n, 1, "owner 2 must be able to delete their rule");

    // Recreate the rule for the negative case.
    seed_group_typed(&db, 11, 2, "in").await;
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

    // User 3 must NOT be able to delete user 2's rule.
    let n = db
        .delete_rule(rule_id2, &ResourceScope::Owner(3))
        .await
        .unwrap();
    assert_eq!(n, 0, "user 3 must NOT delete user 2's rule");

    // Rule must still exist (DELETE was rejected).
    let still_there = db
        .find_rule_by_id(rule_id2, &ResourceScope::All)
        .await
        .unwrap();
    assert!(still_there.is_some(), "rule must survive rejected DELETE");
}

/// Owner scope: find_rule_by_id returns None for another user's rule.
#[tokio::test]
async fn find_rule_by_id_owner_scope_filters_other_owner() {
    let db = repo().await;
    seed_user(&db, 2, false).await;
    seed_user(&db, 3, false).await;
    seed_group_typed(&db, 10, 2, "in").await;
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

    // Owner sees their rule.
    let own = db
        .find_rule_by_id(rule_id, &ResourceScope::Owner(2))
        .await
        .unwrap();
    assert!(own.is_some(), "owner 2 must see own rule");

    // Another user gets None (indistinguishable from "doesn't exist").
    let other = db
        .find_rule_by_id(rule_id, &ResourceScope::Owner(3))
        .await
        .unwrap();
    assert!(other.is_none(), "user 3 must NOT see user 2's rule");
}

/// Owner scope: update_group_fields succeeds for own group, fails for another user's group.
#[tokio::test]
async fn update_group_fields_owner_scope_rejects_wrong_owner() {
    let db = repo().await;
    seed_user(&db, 2, false).await;
    seed_user(&db, 3, false).await;
    seed_group_typed(&db, 10, 2, "in").await;

    // Owner can rename their group.
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
    assert_eq!(n, 1, "owner 2 must be able to rename their group");

    // User 3 must NOT be able to rename user 2's group.
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
    assert_eq!(n, 0, "user 3 must NOT rename user 2's group");

    // Verify name unchanged after rejected update.
    let name = db
        .find_name_by_id(10, &ResourceScope::All)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(name, "renamed", "name must survive rejected update");
}

/// Owner scope: delete_group succeeds for own group, fails for another user's group.
#[tokio::test]
async fn delete_group_owner_scope_rejects_wrong_owner() {
    let db = repo().await;
    seed_user(&db, 2, false).await;
    seed_user(&db, 3, false).await;
    seed_group_typed(&db, 10, 2, "in").await;

    // User 3 must NOT be able to delete user 2's group.
    let n = db.delete_group(10, &ResourceScope::Owner(3)).await.unwrap();
    assert_eq!(n, 0, "user 3 must NOT delete user 2's group");

    // Group must still exist.
    let name = db.find_name_by_id(10, &ResourceScope::All).await.unwrap();
    assert!(name.is_some(), "group must survive rejected DELETE");

    // Owner CAN delete.
    let n = db.delete_group(10, &ResourceScope::Owner(2)).await.unwrap();
    assert_eq!(n, 1, "owner 2 must be able to delete their group");
}

// ── v0.4.18 PR8: SQLite parity gap fill — tests ported from pg_repo ──

/// Cascade deletes rules, groups, profiles, and the user in one tx.
/// Regression for v0.4.4: the cascade must delete custom tunnel_profiles.
#[tokio::test]
async fn delete_user_cascade_removes_rules_groups_profiles_and_user() {
    let db = repo().await;
    db.insert_user("alice", "h", 1).await.unwrap();
    let uid = db.find_by_username("alice").await.unwrap().unwrap().id;
    sqlx::query("INSERT INTO device_groups (id, name, group_type, token, uid) VALUES (1, 'gin', 'in', 'tok-1', ?)")
        .bind(uid).execute(&db.pool).await.unwrap();
    sqlx::query(
        "INSERT INTO forward_rules \
         (name, uid, listen_port, device_group_in, target_addr, target_port) \
         VALUES ('r1', ?, 20000, 1, '127.0.0.1', 80)",
    )
    .bind(uid)
    .execute(&db.pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO tunnel_profiles (name, transport, uid) VALUES ('alice-custom', 'ws', ?)",
    )
    .bind(uid)
    .execute(&db.pool)
    .await
    .unwrap();

    let affected = db.delete_user_cascade(uid).await.unwrap();
    assert_eq!(affected, 1, "the user row must be deleted");

    for (table, col) in [
        ("forward_rules", "uid"),
        ("device_groups", "uid"),
        ("tunnel_profiles", "uid"),
    ] {
        let n: (i64,) =
            sqlx::query_as(&format!("SELECT COUNT(*) FROM {} WHERE {} = ?", table, col))
                .bind(uid)
                .fetch_one(&db.pool)
                .await
                .unwrap();
        assert_eq!(n.0, 0, "{} rows for user must be deleted", table);
    }
    assert!(!db.exists_by_id(uid).await.unwrap(), "user must be gone");
}

/// Switching a rule to "direct" clears device_group_out.
/// Regression: SQLite tolerated duplicate column assignments; the fix ensures
/// device_group_out is always set exactly once.
#[tokio::test]
async fn rule_update_switch_to_direct_clears_device_group_out() {
    let db = repo().await;
    seed_group(&db, 1).await;
    seed_group_typed(&db, 2, 1, "out").await;
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
        .expect("update must succeed");
    assert_eq!(affected, 1);

    let dgo: (Option<i64>,) =
        sqlx::query_as("SELECT device_group_out FROM forward_rules WHERE id = ?")
            .bind(rule_id)
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert!(dgo.0.is_none(), "device_group_out must be cleared to NULL");
}

/// v0.4.12: PG revision 7 migration SQL — cross-owner rules must be paused.
/// Group owned by user 3, rule owned by user 2 → mismatch → paused.
#[tokio::test]
async fn migration_pauses_cross_owner_rules() {
    let db = repo().await;
    sqlx::query("INSERT INTO users (id, username, password, admin) VALUES (2, 'u2', 'x', 0)")
        .execute(&db.pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO users (id, username, password, admin) VALUES (3, 'u3', 'x', 0)")
        .execute(&db.pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO device_groups (id, name, group_type, token, uid) VALUES (20, 'g', 'in', 't', 3)")
        .execute(&db.pool).await.unwrap();
    sqlx::query("INSERT INTO forward_rules (name, uid, listen_port, device_group_in, target_addr, target_port) \
                 VALUES ('r', 2, 15000, 20, '127.0.0.1', 80)")
        .execute(&db.pool).await.unwrap();
    // The exact UPDATE from PG revision 7 (cross-owner mismatch arm).
    sqlx::query(
        "UPDATE forward_rules SET paused = 1 \
         WHERE paused = 0 \
         AND EXISTS (SELECT 1 FROM device_groups dg \
                     WHERE dg.id = forward_rules.device_group_in \
                       AND dg.uid <> forward_rules.uid)",
    )
    .execute(&db.pool)
    .await
    .unwrap();

    let paused: (i64,) = sqlx::query_as("SELECT paused FROM forward_rules WHERE name = 'r'")
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!(
        paused.0, 1,
        "cross-owner rule must be paused by migration SQL"
    );
}

/// v0.4.12 PR1 (SQLite parity): combined shared_groups test — admin inbound
/// is visible, out/monitor excluded, other regular users' groups excluded,
/// admin caller gets empty list.
#[tokio::test]
async fn shared_groups_admin_inbound_only() {
    let db = repo().await;
    seed_user(&db, 2, false).await; // alice (regular)
    seed_user(&db, 3, false).await; // bob (regular)
    seed_group_typed(&db, 10, 1, "in").await;
    sqlx::query("INSERT INTO device_groups (id, name, group_type, token, uid) VALUES (11, 'g11', 'out', 'tok11', 1)")
        .execute(&db.pool).await.unwrap();
    sqlx::query("INSERT INTO device_groups (id, name, group_type, token, uid) VALUES (12, 'g12', 'monitor', 'tok12', 1)")
        .execute(&db.pool).await.unwrap();
    seed_group_typed(&db, 20, 3, "in").await; // bob's inbound

    // alice (regular) sees ONLY admin inbound group 10.
    let shared = db.list_shared_groups(2, false).await.unwrap();
    assert_eq!(shared.len(), 1, "only admin 'in' group is shared");
    assert_eq!(shared[0].id, 10);

    // admin caller gets empty list.
    let admin_shared = db.list_shared_groups(1, true).await.unwrap();
    assert!(admin_shared.is_empty(), "admin gets no shared groups");
}

/// overflow entry without rollback check — minimal parity with PG's version.
#[tokio::test]
async fn traffic_batch_single_entry_overflow() {
    let db = repo().await;
    db.insert_user("alice", "h", 1).await.unwrap();
    let alice = db.find_by_username("alice").await.unwrap().unwrap().id;
    sqlx::query("INSERT INTO device_groups (id, name, group_type, token, uid) VALUES (50, 'gin', 'in', 'tok-50', ?)")
        .bind(alice).execute(&db.pool).await.unwrap();
    sqlx::query(
        "INSERT INTO forward_rules \
         (id, name, uid, listen_port, device_group_in, target_addr, target_port) \
         VALUES (100, 'r100', ?, 20000, 50, '127.0.0.1', 80)",
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
}

// ── v1.0.8: device-group billing rate ──
// rate multiplies the bytes CHARGED to the user; the rule's own counter keeps
// REAL bytes (upload+download). Default 1.0 = bill what you use.

/// Helper: seed alice + group `gid` with `rate` + rule `rid` on group `gid`
/// owned by alice, with both rule and user counters starting at 0.
async fn seed_group_with_rate(db: &crate::db::sqlite_repo::SqliteRepository, gid: i64, rate: f64) {
    db.insert_user("alice", "h", 1).await.unwrap();
    let alice = db.find_by_username("alice").await.unwrap().unwrap().id;
    sqlx::query(
        "INSERT INTO device_groups (id, name, group_type, token, uid, rate) \
         VALUES (?, 'gin', 'in', ?, ?, ?)",
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
         VALUES (100, 'r100', ?, 20000, ?, '127.0.0.1', 80)",
    )
    .bind(alice)
    .bind(gid)
    .execute(&db.pool)
    .await
    .unwrap();
}

#[tokio::test]
async fn traffic_batch_rate_2_charges_user_double_rule_stays_real() {
    let db = repo().await;
    seed_group_with_rate(&db, 50, 2.0).await;
    let alice = db.find_by_username("alice").await.unwrap().unwrap().id;

    // 1000 up + 2000 down = 3000 real bytes.
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

    // Rule counter: REAL bytes (unchanged by rate).
    let rule_t: (i64,) = sqlx::query_as("SELECT traffic_used FROM forward_rules WHERE id = 100")
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!(rule_t.0, 3000);
    // User counter: BILLED = round(3000 * 2.0) = 6000.
    let user_t: (i64,) = sqlx::query_as("SELECT traffic_used FROM users WHERE id = ?")
        .bind(alice)
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!(user_t.0, 6000);
}

#[tokio::test]
async fn traffic_batch_rate_1_is_unchanged_billing() {
    // Regression: rate=1.0 must charge exactly the real bytes (the historical
    // behavior). This guards against a future refactor that double-applies rate
    // or skips the multiply when rate is 1.0.
    let db = repo().await;
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
    let user_t: (i64,) = sqlx::query_as("SELECT traffic_used FROM users WHERE id = ?")
        .bind(alice)
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!(rule_t.0, 3000);
    assert_eq!(user_t.0, 3000);
}

#[tokio::test]
async fn traffic_batch_rate_1_5_rounds_correctly() {
    // 1000 + 2000 = 3000 real; 3000 * 1.5 = 4500.0 → round → 4500.
    // Also covers the non-integer-input path (rate stored as REAL).
    let db = repo().await;
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
    let user_t: (i64,) = sqlx::query_as("SELECT traffic_used FROM users WHERE id = ?")
        .bind(alice)
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!(rule_t.0, 3000);
    assert_eq!(user_t.0, 4500);

    // Second batch: 1 up + 1 down = 2 real; 2 * 1.5 = 3.0 → 3 billed.
    // Verifies round() (not truncation) and accumulation across batches.
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
    let user_t2: (i64,) = sqlx::query_as("SELECT traffic_used FROM users WHERE id = ?")
        .bind(alice)
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!(rule_t2.0, 3002); // 3000 + 2 real
    assert_eq!(user_t2.0, 4503); // 4500 + 3 billed
}

// ── v1.0.8: suspension + expiry gating (list_active_for_config) ──

/// Seed alice (non-admin) + group 50 + one active rule owned by alice.
async fn seed_active_rule(db: &SqliteRepository) -> i64 {
    db.insert_user("alice", "h", 1).await.unwrap();
    let alice = db.find_by_username("alice").await.unwrap().unwrap().id;
    sqlx::query(
        "INSERT INTO device_groups (id, name, group_type, token, uid) \
         VALUES (50, 'gin', 'in', 'tok-50', ?)",
    )
    .bind(alice)
    .execute(&db.pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO forward_rules \
         (name, uid, listen_port, device_group_in, target_addr, target_port) \
         VALUES ('r', ?, 20000, 50, '127.0.0.1', 80)",
    )
    .bind(alice)
    .execute(&db.pool)
    .await
    .unwrap();
    alice
}

#[tokio::test]
async fn suspended_user_rule_is_filtered_and_resumes_on_unsuspend() {
    let db = repo().await;
    let alice = seed_active_rule(&db).await;
    // Active by default.
    assert_eq!(db.list_active_for_config(50).await.unwrap().len(), 1);

    // Suspend → rule filtered (gate 2 of 4).
    sqlx::query("UPDATE users SET suspended = 1 WHERE id = ?")
        .bind(alice)
        .execute(&db.pool)
        .await
        .unwrap();
    assert_eq!(
        db.list_active_for_config(50).await.unwrap().len(),
        0,
        "suspended user's rule must be filtered"
    );

    // Unsuspend → rule reappears (auto-recovery, no manual re-publish).
    sqlx::query("UPDATE users SET suspended = 0 WHERE id = ?")
        .bind(alice)
        .execute(&db.pool)
        .await
        .unwrap();
    assert_eq!(
        db.list_active_for_config(50).await.unwrap().len(),
        1,
        "rule must reappear after unsuspend"
    );
}

#[tokio::test]
async fn expired_plan_rule_is_filtered_and_resumes_after_renewal() {
    let db = repo().await;
    let alice = seed_active_rule(&db).await;
    // Set an expiry in the past → rule filtered (gate 4 of 4).
    sqlx::query("UPDATE users SET plan_expire_at = '2000-01-01 00:00:00' WHERE id = ?")
        .bind(alice)
        .execute(&db.pool)
        .await
        .unwrap();
    assert_eq!(
        db.list_active_for_config(50).await.unwrap().len(),
        0,
        "expired-plan user's rule must be filtered"
    );

    // Renew to a future expiry → rule reappears.
    sqlx::query("UPDATE users SET plan_expire_at = '2099-01-01 00:00:00' WHERE id = ?")
        .bind(alice)
        .execute(&db.pool)
        .await
        .unwrap();
    assert_eq!(
        db.list_active_for_config(50).await.unwrap().len(),
        1,
        "rule must reappear after renewal"
    );
}

#[tokio::test]
async fn null_plan_expire_at_is_no_expiry() {
    let db = repo().await;
    let alice = seed_active_rule(&db).await;
    // NULL expiry (the default) must mean "never expires" — rule stays active.
    sqlx::query("UPDATE users SET plan_expire_at = NULL WHERE id = ?")
        .bind(alice)
        .execute(&db.pool)
        .await
        .unwrap();
    assert_eq!(db.list_active_for_config(50).await.unwrap().len(), 1);
}

// ── v1.0.8: plan purchase (buy_plan) ──

/// Seed alice with a starting balance + an empty plan row, returning alice's id.
async fn seed_buyer_and_plan(
    db: &SqliteRepository,
    balance: &str,
    plan_traffic: i64,
    plan_price: &str,
    duration_days: i32,
    reset_traffic: bool,
) -> (i64, i64) {
    db.insert_user("alice", "h", 1).await.unwrap();
    let alice = db.find_by_username("alice").await.unwrap().unwrap().id;
    sqlx::query("UPDATE users SET balance = ? WHERE id = ?")
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
async fn buy_plan_stacks_traffic_and_charges_balance() {
    let db = repo().await;
    // alice has 100.00 balance, 500 existing traffic_limit; plan costs 30.00,
    // adds 1_000_000 traffic. After purchase: balance 70.00, traffic_limit
    // 1_000_500 (stacked), max_rules 10, plan_id set.
    let (alice, pid) = seed_buyer_and_plan(&db, "100.00", 1_000_000, "30.00", 0, false).await;
    // RENEW: alice is already on this plan (plan_id = pid) with 500 quota left.
    // Re-buying the SAME plan stacks traffic (加流量).
    sqlx::query("UPDATE users SET traffic_limit = 500, plan_id = ? WHERE id = ?")
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
        sqlx::query_as("SELECT balance, traffic_limit, max_rules, plan_id FROM users WHERE id = ?")
            .bind(alice)
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert_eq!(balance, "70");
    assert_eq!(
        traffic_limit, 1_000_500,
        "renewing the same plan must stack traffic on existing quota"
    );
    assert_eq!(max_rules, 10);
    assert_eq!(plan_id, Some(pid));

    // An order row was recorded.
    let orders: Vec<relay_shared::models::Order> = db.list_orders_by_user(alice).await.unwrap();
    assert_eq!(orders.len(), 1);
    assert_eq!(orders[0].plan_name, "p1");
    assert_eq!(orders[0].price, "30");
}

#[tokio::test]
async fn buy_plan_reset_traffic_zeros_usage() {
    let db = repo().await;
    let (alice, pid) = seed_buyer_and_plan(&db, "100.00", 1_000_000, "10.00", 0, true).await;
    sqlx::query("UPDATE users SET traffic_used = 9999 WHERE id = ?")
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

    let used: (i64,) = sqlx::query_as("SELECT traffic_used FROM users WHERE id = ?")
        .bind(alice)
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!(used.0, 0, "reset_traffic must zero traffic_used");
}

#[tokio::test]
async fn buy_plan_insufficient_balance_is_rejected_and_rolls_back() {
    let db = repo().await;
    // alice has 5.00; plan costs 30.00 → must refuse and leave state untouched.
    let (alice, pid) = seed_buyer_and_plan(&db, "5.00", 1_000_000, "30.00", 0, false).await;
    // Clear the seeded plan_id so we can assert it stays NULL on rollback.
    sqlx::query("UPDATE users SET plan_id = NULL WHERE id = ?")
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

    // Nothing changed: balance intact, no order, plan_id still NULL.
    let (balance, plan_id): (String, Option<i64>) =
        sqlx::query_as("SELECT balance, plan_id FROM users WHERE id = ?")
            .bind(alice)
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert_eq!(balance, "5.00", "balance must be untouched on rollback");
    assert_eq!(plan_id, None);
    let orders: Vec<relay_shared::models::Order> = db.list_orders_by_user(alice).await.unwrap();
    assert_eq!(orders.len(), 0, "no order row on insufficient balance");
}

#[tokio::test]
async fn buy_plan_time_plan_sets_future_expiry() {
    let db = repo().await;
    // 30-day time plan. Expiry must be ~30 days in the future (we check it's
    // after now and within 31 days — avoids a flaky exact-timestamp assert).
    let (alice, pid) = seed_buyer_and_plan(&db, "100.00", 0, "5.00", 30, false).await;

    db.buy_plan(alice, pid, "p1", 500, 0, 10, 30, false, false, &[], &[])
        .await
        .unwrap();

    let expire: (Option<String>,) = sqlx::query_as("SELECT plan_expire_at FROM users WHERE id = ?")
        .bind(alice)
        .fetch_one(&db.pool)
        .await
        .unwrap();
    let exp = expire.0.expect("time plan must set an expiry");
    let now = sqlx::query_as::<_, (String,)>("SELECT datetime('now')")
        .fetch_one(&db.pool)
        .await
        .unwrap()
        .0;
    assert!(exp > now, "expiry must be in the future ({exp} <= {now})");
}

#[tokio::test]
async fn buy_plan_renewal_stacks_expiry_from_current_end() {
    let db = repo().await;
    let (alice, pid) = seed_buyer_and_plan(&db, "100.00", 0, "5.00", 30, false).await;
    // RENEW: alice is already on this plan with an expiry far in the future.
    // Re-buying the SAME plan must extend FROM that date (now + 30 would clip
    // it). plan_id = pid makes this a renew (not a switch).
    let future = "2099-12-31 00:00:00";
    sqlx::query("UPDATE users SET plan_expire_at = ?, plan_id = ? WHERE id = ?")
        .bind(future)
        .bind(pid)
        .bind(alice)
        .execute(&db.pool)
        .await
        .unwrap();

    db.buy_plan(alice, pid, "p1", 500, 0, 10, 30, false, false, &[], &[])
        .await
        .unwrap();

    let expire: (Option<String>,) = sqlx::query_as("SELECT plan_expire_at FROM users WHERE id = ?")
        .bind(alice)
        .fetch_one(&db.pool)
        .await
        .unwrap();
    let expire = expire.0.expect("renewal must keep an expiry");
    // 2099-12-31 + 30 days = 2100-01-30. If it had clipped to now+30, it would
    // be ~2026. The exact arithmetic is SQLite's datetime(), so assert the
    // year is 2100 (proves it extended from the existing end, not from now).
    assert!(
        expire.starts_with("2100-"),
        "renewal must stack from current expiry, got {expire}"
    );
}

/// v1.0.9: switching to a DIFFERENT plan replaces the quota with the new plan's
/// amount (not stacked) and resets usage to 0 — the new plan starts fresh.
#[tokio::test]
async fn buy_plan_switch_replaces_traffic_and_resets_used() {
    let db = repo().await;
    // alice is on plan A with 800 quota and 300 already used.
    let (alice, pid_a) = seed_buyer_and_plan(&db, "100.00", 1_000, "5.00", 0, false).await;
    sqlx::query(
        "UPDATE users SET plan_id = ?, traffic_limit = 800, traffic_used = 300 WHERE id = ?",
    )
    .bind(pid_a)
    .bind(alice)
    .execute(&db.pool)
    .await
    .unwrap();
    // A DIFFERENT plan B grants 5000 traffic.
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
        sqlx::query_as("SELECT traffic_limit, traffic_used, plan_id FROM users WHERE id = ?")
            .bind(alice)
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert_eq!(
        traffic_limit, 5_000,
        "switch must REPLACE quota with the new plan's amount, not stack"
    );
    assert_eq!(traffic_used, 0, "switch must reset usage to 0");
    assert_eq!(plan_id, Some(pid_b));
}

/// v1.0.9: switching to a different time plan recomputes expiry from now — the
/// old plan's remaining time does NOT carry over.
#[tokio::test]
async fn buy_plan_switch_recomputes_expiry_from_now() {
    let db = repo().await;
    // alice is on time plan A with a far-future expiry.
    let (alice, pid_a) = seed_buyer_and_plan(&db, "100.00", 0, "5.00", 30, false).await;
    sqlx::query(
        "UPDATE users SET plan_id = ?, plan_expire_at = '2099-12-31 00:00:00' WHERE id = ?",
    )
    .bind(pid_a)
    .bind(alice)
    .execute(&db.pool)
    .await
    .unwrap();
    // Switch to a DIFFERENT 30-day plan B.
    let pid_b = db
        .insert_plan("pB", 10, 0, "5.00", "time", 30, false, false, "", false)
        .await
        .unwrap();

    db.buy_plan(alice, pid_b, "pB", 500, 0, 10, 30, false, false, &[], &[])
        .await
        .unwrap();

    let expire: (Option<String>,) = sqlx::query_as("SELECT plan_expire_at FROM users WHERE id = ?")
        .bind(alice)
        .fetch_one(&db.pool)
        .await
        .unwrap();
    let expire = expire.0.expect("switch to a time plan sets an expiry");
    // Recomputed from now, NOT stacked onto 2099 → must not land in 2099/2100.
    assert!(
        !expire.starts_with("2099-") && !expire.starts_with("2100-"),
        "switch must recompute expiry from now, not stack from the old plan, got {expire}"
    );
}

/// v1.0.9: renewing the SAME plan (reset_traffic=false) keeps usage and stacks
/// quota — the counterpart to the switch-resets-usage rule.
#[tokio::test]
async fn buy_plan_renew_keeps_traffic_used() {
    let db = repo().await;
    let (alice, pid) = seed_buyer_and_plan(&db, "100.00", 1_000, "5.00", 0, false).await;
    sqlx::query(
        "UPDATE users SET plan_id = ?, traffic_limit = 1000, traffic_used = 400 WHERE id = ?",
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
        sqlx::query_as("SELECT traffic_limit, traffic_used FROM users WHERE id = ?")
            .bind(alice)
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert_eq!(traffic_limit, 2_000, "renew stacks quota");
    assert_eq!(traffic_used, 400, "renew keeps usage");
}

// ── v1.1.0: default plan is not re-seeded on restart ──

/// Regression (v1.1.0): once an admin deletes the seeded "free" plan and other
/// plans exist, re-applying the schema (what every panel start/update does) must
/// NOT bring it back. The seed now runs only when the plans table is empty.
#[tokio::test]
async fn default_plan_not_reseeded_on_restart() {
    let db = repo().await;
    assert!(
        db.list_plans()
            .await
            .unwrap()
            .iter()
            .any(|p| p.name == "free"),
        "a fresh DB seeds the default free plan"
    );

    // Admin adds a real plan, repoints registration off the default, deletes it.
    let keep = db
        .insert_plan(
            "keep", 10, 1_000, "5.00", "data", 0, false, false, "d", false,
        )
        .await
        .unwrap();
    sqlx::query("UPDATE app_settings SET default_registration_plan_id = ?")
        .bind(keep)
        .execute(&db.pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM plans WHERE id = 1")
        .execute(&db.pool)
        .await
        .unwrap();

    // Simulate a restart: re-apply the baseline schema.
    sqlx::query(SCHEMA_SQL).execute(&db.pool).await.unwrap();

    let names: Vec<String> = db
        .list_plans()
        .await
        .unwrap()
        .into_iter()
        .map(|p| p.name)
        .collect();
    assert!(
        !names.contains(&"free".to_string()),
        "deleted default plan must not reappear after restart"
    );
    assert!(names.contains(&"keep".to_string()), "other plans survive");
}

// ── v1.0.8: plan CRUD ──

#[tokio::test]
async fn plan_crud_round_trip_and_delete_blocked_when_in_use() {
    let db = repo().await;
    let pid = db
        .insert_plan("p1", 10, 1_000, "5.00", "data", 0, false, false, "d", false)
        .await
        .unwrap();

    // Update.
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

    // list_visible_plans excludes hidden. The baseline seeds a 'free' plan
    // (hidden=0), so the visible count drops by exactly one when we hide ours.
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
    // list_plans (admin, includes hidden) still has ours + the seed.
    assert!(db.list_plans().await.unwrap().iter().any(|p| p.id == pid));

    // count_users_on_plan = 0 → delete succeeds.
    assert_eq!(db.count_users_on_plan(pid).await.unwrap(), 0);
    assert_eq!(db.delete_plan(pid).await.unwrap(), 1);
    assert!(db.find_plan_by_id(pid).await.unwrap().is_none());

    // Recreate + assign a user → delete still 0 rows would be wrong; instead
    // count_users_on_plan > 0 signals 409 at the handler.
    let pid2 = db
        .insert_plan("p2", 5, 0, "0", "data", 0, false, false, "", false)
        .await
        .unwrap();
    db.insert_user("bob", "h", 1).await.unwrap();
    let bob = db.find_by_username("bob").await.unwrap().unwrap().id;
    sqlx::query("UPDATE users SET plan_id = ? WHERE id = ?")
        .bind(pid2)
        .bind(bob)
        .execute(&db.pool)
        .await
        .unwrap();
    assert_eq!(db.count_users_on_plan(pid2).await.unwrap(), 1);
}

// ── v1.0.9: plan ↔ device-group grants + purchase authorization ──

/// Insert a device group with an explicit id owned by `uid`.
async fn seed_device_group(db: &SqliteRepository, gid: i64, uid: i64) {
    sqlx::query(
        "INSERT INTO device_groups (id, name, group_type, token, uid) \
         VALUES (?, 'g', 'in', ?, ?)",
    )
    .bind(gid)
    .bind(format!("tok-dg-{gid}"))
    .bind(uid)
    .execute(&db.pool)
    .await
    .unwrap();
}

/// v1.0.9: create_plan_with_groups writes the plan row AND its grant set in one
/// transaction; both must be present afterward.
#[tokio::test]
async fn create_plan_with_groups_persists_plan_and_grants() {
    let db = repo().await;
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
}

/// v1.0.9: clear_user_plan nulls the plan, revokes device-group authorization
/// (flag + explicit rows) and system-pauses the user's active rules — all in
/// one transaction. Admin targets are a no-op.
#[tokio::test]
async fn clear_user_plan_revokes_groups_and_pauses_rules() {
    let db = repo().await;
    let (alice, pid) = seed_buyer_and_plan(&db, "100.00", 1000, "5.00", 0, false).await;
    seed_device_group(&db, 70, alice).await;
    sqlx::query("UPDATE users SET plan_id = ?, all_device_groups = 1 WHERE id = ?")
        .bind(pid)
        .bind(alice)
        .execute(&db.pool)
        .await
        .unwrap();
    db.set_user_device_groups(alice, &[70]).await.unwrap();
    sqlx::query(
        "INSERT INTO forward_rules (id, name, uid, listen_port, device_group_in, \
         target_addr, target_port, paused) VALUES (300, 'r', ?, 21000, 70, '127.0.0.1', 80, 0)",
    )
    .bind(alice)
    .execute(&db.pool)
    .await
    .unwrap();

    let affected = db.clear_user_plan(alice).await.unwrap();
    assert_eq!(affected, 1);

    let (plan_id, all): (Option<i64>, bool) =
        sqlx::query_as("SELECT plan_id, all_device_groups FROM users WHERE id = ?")
            .bind(alice)
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert_eq!(plan_id, None);
    assert!(!all, "all_device_groups must be cleared");
    assert!(db.list_user_device_groups(alice).await.unwrap().is_empty());
    let (paused, auto): (bool, bool) =
        sqlx::query_as("SELECT paused, auto_paused FROM forward_rules WHERE id = 300")
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert!(paused && auto, "rule must be system-paused after clear");

    // Admin target (id 1) is a no-op.
    assert_eq!(db.clear_user_plan(1).await.unwrap(), 0);
}

#[tokio::test]
async fn plan_device_groups_round_trip_and_replace() {
    let db = repo().await;
    let (alice, pid) = seed_buyer_and_plan(&db, "100.00", 1000, "5.00", 0, false).await;
    seed_device_group(&db, 50, alice).await;
    seed_device_group(&db, 51, alice).await;
    seed_device_group(&db, 52, alice).await;

    db.set_plan_device_groups(pid, &[50, 51]).await.unwrap();
    assert_eq!(db.list_plan_device_groups(pid).await.unwrap(), vec![50, 51]);

    // REPLACE semantics: a second set overwrites, not appends.
    db.set_plan_device_groups(pid, &[52]).await.unwrap();
    assert_eq!(db.list_plan_device_groups(pid).await.unwrap(), vec![52]);

    // Duplicate ids in the input are deduped by the PK.
    db.set_plan_device_groups(pid, &[50, 50, 51]).await.unwrap();
    assert_eq!(db.list_plan_device_groups(pid).await.unwrap(), vec![50, 51]);
}

/// v1.0.8: purchase REPLACES authorization, so a group the user already had
/// that ALSO appears in the new plan's grant set must end up exactly once
/// (the replace is a clean delete-then-insert of the new set, not a
/// dedup-on-append) — no duplicate row, no unique-constraint error.
#[tokio::test]
async fn buy_plan_new_authorized_set_has_no_duplicate_groups() {
    let db = repo().await;
    let (alice, pid) = seed_buyer_and_plan(&db, "100.00", 1000, "5.00", 0, false).await;
    seed_device_group(&db, 50, alice).await;
    seed_device_group(&db, 51, alice).await;
    // Alice already has group 50 from a prior purchase.
    db.set_user_device_groups(alice, &[50]).await.unwrap();
    // Plan grants 50 (overlaps the existing grant) + 51 (new).
    db.set_plan_device_groups(pid, &[50, 51]).await.unwrap();

    // v1.0.8: new_authorized = {50, 51} (the plan's grants).
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

    // Result: exactly {50, 51} — the overlapping id 50 appears once, not
    // duplicated by the replace's delete-then-insert.
    assert_eq!(
        db.list_user_device_groups(alice).await.unwrap(),
        vec![50, 51]
    );
    // The all-groups flag is NOT set for a per-group grant.
    let all: (bool,) = sqlx::query_as("SELECT all_device_groups FROM users WHERE id = ?")
        .bind(alice)
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert!(!all.0);
}

/// v1.0.8: purchase REPLACES authorization — old groups are cleared.
/// If the user previously had groups not in the new plan, those are removed
/// and rules bound to them are paused.
#[tokio::test]
async fn buy_plan_replaces_authorization_clears_old_groups() {
    let db = repo().await;
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
        "INSERT INTO forward_rules (id, name, uid, listen_port, device_group_in, \
         target_addr, target_port, paused) VALUES (100, 'r100', ?, 20000, 50, '127.0.0.1', 80, 0)",
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
    assert!(paused.0, "rule bound to removed group should be paused");
}

/// v1.0.8 regression: downgrading from a grant-all plan to a per-group plan
/// must RESET all_device_groups back to 0. Without the reset the user stays
/// effectively unrestricted (all_device_groups=1 overrides the explicit set),
/// so the "replace to only the new plan's lines" never takes effect.
#[tokio::test]
async fn buy_plan_grant_all_then_per_group_resets_all_flag() {
    let db = repo().await;
    let (alice, pid) = seed_buyer_and_plan(&db, "100.00", 1000, "5.00", 0, false).await;
    seed_device_group(&db, 50, alice).await;
    seed_device_group(&db, 52, alice).await;

    // 1) Buy a grant-all plan → all_device_groups = 1.
    db.buy_plan(alice, pid, "all", 100, 1000, 10, 0, false, true, &[], &[])
        .await
        .unwrap();
    let all: (bool,) = sqlx::query_as("SELECT all_device_groups FROM users WHERE id = ?")
        .bind(alice)
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert!(all.0, "grant-all purchase must set the flag");

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

    // The flag must be cleared, and the authorized set is exactly {52}.
    let all: (bool,) = sqlx::query_as("SELECT all_device_groups FROM users WHERE id = ?")
        .bind(alice)
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert!(
        !all.0,
        "downgrade to a per-group plan must reset all_device_groups to 0"
    );
    assert_eq!(db.list_user_device_groups(alice).await.unwrap(), vec![52]);
}

/// v1.0.8: re-buying a plan that re-grants a group must auto-resume a rule
/// this system previously auto-paused on that group (not just fix the
/// authorization set) — otherwise the user pays for the line again but it
/// stays dark until someone manually clicks resume.
#[tokio::test]
async fn buy_plan_resumes_auto_paused_rules_when_group_reauthorized() {
    let db = repo().await;
    let (alice, pid_a) = seed_buyer_and_plan(&db, "100.00", 1000, "5.00", 0, false).await;
    seed_device_group(&db, 50, alice).await;
    seed_device_group(&db, 51, alice).await;
    let pid_b = db
        .insert_plan("pB", 10, 1000, "5.00", "data", 0, false, false, "", false)
        .await
        .unwrap();
    db.set_plan_device_groups(pid_a, &[50]).await.unwrap();
    db.set_plan_device_groups(pid_b, &[51]).await.unwrap();

    // 1) Buy plan A (grants 50) — rule on group 50 is unpaused.
    sqlx::query(
        "INSERT INTO forward_rules (id, name, uid, listen_port, device_group_in, \
         target_addr, target_port, paused) VALUES (200, 'r200', ?, 20000, 50, '127.0.0.1', 80, 0)",
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

    // 2) Buy plan B (grants only 51) — REPLACE revokes group 50, so buy_plan
    // itself auto-pauses rule 200 (auto_paused=1).
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
    assert!(paused && auto_paused, "buy_plan must auto-pause rule 200");

    // 3) Buy plan A again (re-grants 50) — the rule must AUTO-RESUME.
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
        "re-authorizing group 50 must auto-resume the rule buy_plan itself paused"
    );
}

/// v1.0.8: a rule the user paused THEMSELVES (via the on/off switch, which
/// clears auto_paused) must NOT be silently revived by a later purchase, even
/// if that purchase happens to re-grant the rule's group. Only rules the
/// SYSTEM paused (auto_paused=1) are eligible for buy_plan's auto-resume.
#[tokio::test]
async fn buy_plan_does_not_resume_manually_paused_rules() {
    let db = repo().await;
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
        "INSERT INTO forward_rules (id, name, uid, listen_port, device_group_in, \
         target_addr, target_port, paused) VALUES (201, 'r201', ?, 20001, 50, '127.0.0.1', 80, 0)",
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

    // buy_plan B auto-pauses rule 201 (group 50 revoked).
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

    // The user explicitly re-confirms the pause via the on/off switch
    // (update_rule_fields) — this clears auto_paused, marking it as a HUMAN
    // decision regardless of the rule already being paused.
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
        "an explicit paused write must clear auto_paused"
    );

    // Buy plan A again (re-grants 50) — the rule must STAY paused, since it is
    // no longer flagged as a system pause.
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
        "a manually-paused rule must NOT be auto-resumed by a later purchase"
    );
}

#[tokio::test]
async fn buy_plan_grant_all_sets_flag() {
    let db = repo().await;
    let (alice, pid) = seed_buyer_and_plan(&db, "100.00", 1000, "5.00", 0, false).await;

    // grant_all_groups=true → set all_device_groups, ignore the explicit list.
    // new_authorized = all inbound groups (in this test, the only inbound group
    // is the one created by seed_buyer_and_plan).
    db.buy_plan(alice, pid, "p1", 500, 1000, 10, 0, false, true, &[], &[])
        .await
        .unwrap();

    let all: (bool,) = sqlx::query_as("SELECT all_device_groups FROM users WHERE id = ?")
        .bind(alice)
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert!(
        all.0,
        "grant_all_groups must set the all_device_groups flag"
    );
}

/// v1.0.8: REPLACE semantics — buying a second (different) plan replaces the
/// first plan's authorization rather than stacking it. After both purchases
/// the user is left with ONLY plan B's groups.
#[tokio::test]
async fn second_plan_purchase_replaces_first_plan_groups() {
    let db = repo().await;
    let (alice, pid_a) = seed_buyer_and_plan(&db, "100.00", 1000, "5.00", 0, false).await;
    seed_device_group(&db, 50, alice).await;
    seed_device_group(&db, 51, alice).await;
    // Second plan.
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

    // User now has only the groups from plan B — plan A's grant was replaced,
    // not stacked.
    assert_eq!(db.list_user_device_groups(alice).await.unwrap(), vec![51]);
}

#[tokio::test]
async fn delete_plan_cascades_grant_rows() {
    let db = repo().await;
    // Cascade requires FK enforcement, which is OFF by default on a bare pool.
    sqlx::query("PRAGMA foreign_keys = ON")
        .execute(&db.pool)
        .await
        .unwrap();
    let pid = db
        .insert_plan("p1", 10, 1000, "5.00", "data", 0, false, false, "", false)
        .await
        .unwrap();
    seed_device_group(&db, 50, 1).await;
    db.set_plan_device_groups(pid, &[50]).await.unwrap();
    assert_eq!(db.list_plan_device_groups(pid).await.unwrap(), vec![50]);

    // Deleting the plan cascades to plan_device_groups.
    db.delete_plan(pid).await.unwrap();
    assert!(db.list_plan_device_groups(pid).await.unwrap().is_empty());
}

#[tokio::test]
async fn expiry_does_not_revoke_granted_groups() {
    let db = repo().await;
    let (alice, pid) = seed_buyer_and_plan(&db, "100.00", 0, "5.00", 30, false).await;
    seed_device_group(&db, 50, alice).await;
    db.set_plan_device_groups(pid, &[50]).await.unwrap();

    // Buy a 30-day time plan that grants group 50.
    db.buy_plan(alice, pid, "p1", 500, 0, 10, 30, false, false, &[50], &[50])
        .await
        .unwrap();
    assert_eq!(db.list_user_device_groups(alice).await.unwrap(), vec![50]);

    // Force the plan to expire — the spec says expiry only gates forwarding
    // (list_active_for_config), it must NOT revoke the device-group grant.
    sqlx::query("UPDATE users SET plan_expire_at = '2000-01-01 00:00:00' WHERE id = ?")
        .bind(alice)
        .execute(&db.pool)
        .await
        .unwrap();
    assert_eq!(
        db.list_user_device_groups(alice).await.unwrap(),
        vec![50],
        "expiry must not revoke granted device groups"
    );
}

// ── v1.0.7: admin directly edits a user's plan association + expiry ──

#[tokio::test]
async fn admin_set_user_plan_clears_and_adjusts_expiry() {
    let db = repo().await;
    let (alice, pid) = seed_buyer_and_plan(&db, "100.00", 0, "5.00", 30, false).await;
    // Start with a plan + expiry on alice.
    sqlx::query(
        "UPDATE users SET plan_id = ?, plan_expire_at = '2030-01-01 00:00:00' WHERE id = ?",
    )
    .bind(pid)
    .bind(alice)
    .execute(&db.pool)
    .await
    .unwrap();

    // Adjust expiry, keep the plan_id.
    assert_eq!(
        db.admin_set_user_plan(alice, Some(pid), Some("2099-12-31 00:00:00".into()))
            .await
            .unwrap(),
        1
    );
    let (plan_id, expire): (Option<i64>, Option<String>) =
        sqlx::query_as("SELECT plan_id, plan_expire_at FROM users WHERE id = ?")
            .bind(alice)
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert_eq!(plan_id, Some(pid));
    assert_eq!(expire.as_deref(), Some("2099-12-31 00:00:00"));

    // Clear: both columns go NULL.
    db.admin_set_user_plan(alice, None, None).await.unwrap();
    let (plan_id2, expire2): (Option<i64>, Option<String>) =
        sqlx::query_as("SELECT plan_id, plan_expire_at FROM users WHERE id = ?")
            .bind(alice)
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert_eq!(plan_id2, None);
    assert_eq!(expire2, None);
}

#[tokio::test]
async fn admin_set_user_plan_skips_admin_users() {
    let db = repo().await;
    // The baseline seeds admin user id 1. Editing an admin's plan must be a no-op.
    let affected = db
        .admin_set_user_plan(1, None, Some("2099-12-31 00:00:00".into()))
        .await
        .unwrap();
    assert_eq!(affected, 0, "admin users must be skipped (WHERE admin = 0)");
}

/// v1.2.0: the auto-restart scheduler's query. It must return ONLY rules that
/// opted in (`auto_restart_minutes > 0`) AND are not paused.
///
/// The paused filter belongs in SQL, not the scheduler: a paused rule has no
/// listener on any node, so restarting it is a guaranteed no-op that still
/// costs a WS round-trip per node, every tick, forever.
#[tokio::test]
async fn rule_list_auto_restart_rules_excludes_off_and_paused() {
    let db = repo().await;
    seed_group(&db, 1).await;
    for (name, port) in [("off", 20001), ("on", 20002), ("paused", 20003)] {
        db.insert_quota_guarded(
            name,
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
        .unwrap();
    }
    let on_id: i64 = sqlx::query_scalar("SELECT id FROM forward_rules WHERE name = 'on'")
        .fetch_one(&db.pool)
        .await
        .unwrap();
    let paused_id: i64 = sqlx::query_scalar("SELECT id FROM forward_rules WHERE name = 'paused'")
        .fetch_one(&db.pool)
        .await
        .unwrap();

    // "off" keeps the default 0 → never scheduled.
    db.set_rule_connection_controls(on_id, &ResourceScope::All, 0, 10)
        .await
        .unwrap();
    db.set_rule_connection_controls(paused_id, &ResourceScope::All, 0, 10)
        .await
        .unwrap();
    sqlx::query("UPDATE forward_rules SET paused = 1 WHERE id = ?")
        .bind(paused_id)
        .execute(&db.pool)
        .await
        .unwrap();

    let got = db.list_auto_restart_rules().await.unwrap();
    assert_eq!(
        got.len(),
        1,
        "only the enabled, unpaused rule is scheduled; got {got:?}"
    );
    assert_eq!(got[0].0, on_id);
    assert_eq!(got[0].1, 1, "device_group_in is carried for the fan-out");
    assert_eq!(got[0].2, 10, "the interval is carried");
}

// ── v1.2.0: redeem codes ──

/// Seed one unused code. Returns its id.
async fn seed_code(db: &SqliteRepository, code: &str, amount: &str, expires: Option<&str>) -> i64 {
    db.create_redeem_codes(&[NewRedeemCode {
        code: code.into(),
        amount: amount.into(),
        expires_at: expires.map(str::to_string),
        batch_id: "b1".into(),
        remark: String::new(),
    }])
    .await
    .unwrap();
    sqlx::query_scalar::<_, i64>("SELECT id FROM redeem_codes WHERE code = ?")
        .bind(code)
        .fetch_one(&db.pool)
        .await
        .unwrap()
}

async fn balance_of(db: &SqliteRepository, uid: i64) -> String {
    sqlx::query_scalar::<_, String>("SELECT balance FROM users WHERE id = ?")
        .bind(uid)
        .fetch_one(&db.pool)
        .await
        .unwrap()
}

#[tokio::test]
async fn redeem_credits_balance_and_marks_code_used() {
    let db = repo().await;
    seed_user(&db, 10, false).await;
    seed_code(&db, "AAAA1111BBBB2222", "10.50", None).await;

    let (amount, new_balance) = db
        .redeem_code("AAAA1111BBBB2222", 10, "2026-01-01 00:00:00")
        .await
        .expect("redeem must succeed");
    assert_eq!(amount, "10.50");
    assert_eq!(new_balance, "10.50", "credited onto a zero balance");
    assert_eq!(balance_of(&db, 10).await, "10.50");

    let (status, used_by): (String, Option<i64>) =
        sqlx::query_as("SELECT status, used_by FROM redeem_codes WHERE code = 'AAAA1111BBBB2222'")
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert_eq!(status, "used");
    assert_eq!(used_by, Some(10));
}

/// THE money test: one code can only ever be credited ONCE.
///
/// The second attempt must fail AND leave the balance untouched. If the claim
/// were not conditional on status, a retry (or two users sharing a leaked code)
/// would mint balance out of nothing.
#[tokio::test]
async fn redeem_twice_credits_only_once() {
    let db = repo().await;
    seed_user(&db, 10, false).await;
    seed_user(&db, 11, false).await;
    seed_code(&db, "CCCC3333DDDD4444", "25", None).await;

    db.redeem_code("CCCC3333DDDD4444", 10, "2026-01-01 00:00:00")
        .await
        .expect("first redeem succeeds");

    // Same user retrying, and a different user trying the same code.
    for uid in [10, 11] {
        let err = db
            .redeem_code("CCCC3333DDDD4444", uid, "2026-01-01 00:00:01")
            .await
            .expect_err("a spent code must never credit again");
        assert!(matches!(err, RedeemCodeError::NotRedeemable), "got {err:?}");
    }
    assert_eq!(balance_of(&db, 10).await, "25", "no double credit");
    assert_eq!(balance_of(&db, 11).await, "0", "loser gets nothing");
}

/// An expired code is refused but stays 'unused', so an admin can push
/// expires_at out instead of regenerating the whole batch.
#[tokio::test]
async fn redeem_expired_is_refused_and_stays_unused() {
    let db = repo().await;
    seed_user(&db, 10, false).await;
    seed_code(&db, "EEEE5555FFFF6666", "5", Some("2026-01-01 00:00:00")).await;

    let err = db
        .redeem_code("EEEE5555FFFF6666", 10, "2026-01-02 00:00:00")
        .await
        .expect_err("past expiry must be refused");
    assert!(matches!(err, RedeemCodeError::Expired), "got {err:?}");
    assert_eq!(balance_of(&db, 10).await, "0");

    let status: String =
        sqlx::query_scalar("SELECT status FROM redeem_codes WHERE code = 'EEEE5555FFFF6666'")
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert_eq!(status, "unused", "expiry must not consume the code");

    // Exactly at the deadline is still valid (expiry is "after", not "at").
    db.redeem_code("EEEE5555FFFF6666", 10, "2026-01-01 00:00:00")
        .await
        .expect("redeem AT the expiry instant is allowed");
}

/// Crediting must never push a balance past the ceiling — that would persist a
/// value parse_balance rejects, i.e. a balance the panel can no longer write.
#[tokio::test]
async fn redeem_refuses_to_overflow_the_balance_ceiling() {
    let db = repo().await;
    seed_user(&db, 10, false).await;
    sqlx::query("UPDATE users SET balance = ? WHERE id = 10")
        .bind(relay_shared::money::MAX_BALANCE)
        .execute(&db.pool)
        .await
        .unwrap();
    seed_code(&db, "GGGG7777HHHH8888", "1", None).await;

    let err = db
        .redeem_code("GGGG7777HHHH8888", 10, "2026-01-01 00:00:00")
        .await
        .expect_err("overflow must be refused");
    assert!(
        matches!(err, RedeemCodeError::BalanceOverflow),
        "got {err:?}"
    );
    assert_eq!(
        balance_of(&db, 10).await,
        relay_shared::money::MAX_BALANCE,
        "balance unchanged"
    );
    let status: String =
        sqlx::query_scalar("SELECT status FROM redeem_codes WHERE code = 'GGGG7777HHHH8888'")
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert_eq!(
        status, "unused",
        "a refused redeem must not consume the code"
    );
}

/// Voiding is for UNUSED codes only. A used code already moved money — letting
/// it be voided (or deleted) would falsify the record of that.
#[tokio::test]
async fn void_and_delete_never_touch_a_used_code() {
    let db = repo().await;
    seed_user(&db, 10, false).await;
    let unused_id = seed_code(&db, "JJJJ9999KKKK0000", "1", None).await;
    let used_id = seed_code(&db, "MMMM2222NNNN3333", "1", None).await;
    db.redeem_code("MMMM2222NNNN3333", 10, "2026-01-01 00:00:00")
        .await
        .unwrap();

    assert_eq!(db.void_redeem_code(unused_id).await.unwrap(), 1);
    assert_eq!(
        db.void_redeem_code(used_id).await.unwrap(),
        0,
        "a used code must not be voidable"
    );
    // Voided codes can no longer be redeemed.
    let err = db
        .redeem_code("JJJJ9999KKKK0000", 10, "2026-01-01 00:00:00")
        .await
        .expect_err("voided code must be refused");
    assert!(matches!(err, RedeemCodeError::NotRedeemable), "got {err:?}");

    // Deletion: the voided row goes, the used row stays.
    assert_eq!(
        db.delete_unused_redeem_codes(&[unused_id, used_id])
            .await
            .unwrap(),
        1,
        "only the non-used row is deletable"
    );
    let survivor: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM redeem_codes WHERE status = 'used'")
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert_eq!(survivor, 1, "the redemption record survives");
}

/// Deleting the user who redeemed a code must NOT erase the code row — it is
/// the record of money entering the system (FK is ON DELETE SET NULL).
#[tokio::test]
async fn deleting_the_redeemer_keeps_the_code_record() {
    let db = repo().await;
    seed_user(&db, 5, false).await;
    seed_code(&db, "PPPP4444QQQQ5555", "9.99", None).await;
    db.redeem_code("PPPP4444QQQQ5555", 5, "2026-01-01 00:00:00")
        .await
        .unwrap();

    sqlx::query("DELETE FROM users WHERE id = 5")
        .execute(&db.pool)
        .await
        .unwrap();

    let (status, used_by, amount): (String, Option<i64>, String) = sqlx::query_as(
        "SELECT status, used_by, amount FROM redeem_codes WHERE code = 'PPPP4444QQQQ5555'",
    )
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(status, "used", "the redemption still happened");
    assert_eq!(used_by, None, "FK nulls the reference, not the row");
    assert_eq!(amount, "9.99", "the amount stays auditable");
}

#[tokio::test]
async fn list_and_count_filter_by_status() {
    let db = repo().await;
    seed_user(&db, 10, false).await;
    seed_code(&db, "AAAA0000AAAA0001", "1", None).await;
    seed_code(&db, "AAAA0000AAAA0002", "1", None).await;
    db.redeem_code("AAAA0000AAAA0002", 10, "2026-01-01 00:00:00")
        .await
        .unwrap();

    let all = RedeemCodeFilter {
        limit: 50,
        ..Default::default()
    };
    assert_eq!(db.count_redeem_codes(&all).await.unwrap(), 2);

    let unused = RedeemCodeFilter {
        status: Some("unused".into()),
        limit: 50,
        ..Default::default()
    };
    let rows = db.list_redeem_codes(&unused).await.unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].code, "AAAA0000AAAA0001");
    assert_eq!(db.count_redeem_codes(&unused).await.unwrap(), 1);
}

// ── v1.2.0: traffic history ──

/// Seed a user + inbound group + rule, mirroring the traffic-batch fixtures.
/// Returns (uid, group_id, rule_id).
async fn seed_history_fixture(
    db: &SqliteRepository,
    username: &str,
    group_id: i64,
    rule_id: i64,
    rate: f64,
) -> i64 {
    db.insert_user(username, "h", 1).await.unwrap();
    let uid = db.find_by_username(username).await.unwrap().unwrap().id;
    sqlx::query(
        "INSERT INTO device_groups (id, name, group_type, token, uid, rate) \
         VALUES (?, 'gin', 'in', ?, ?, ?)",
    )
    .bind(group_id)
    .bind(format!("tok-{group_id}"))
    .bind(uid)
    .bind(rate)
    .execute(&db.pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO forward_rules \
         (id, name, uid, listen_port, device_group_in, target_addr, target_port) \
         VALUES (?, ?, ?, 20000, ?, '127.0.0.1', 80)",
    )
    .bind(rule_id)
    .bind(format!("r{rule_id}"))
    .bind(uid)
    .bind(group_id)
    .execute(&db.pool)
    .await
    .unwrap();
    uid
}

/// THE agreement invariant: the history's billed_total is the SAME number that
/// was charged against the user's quota — written in the same transaction.
/// With rate 3.0, real 1000+2000 bytes bill 9000; history must say 9000 too.
/// If these ever diverge, the chart calls the billing a liar (or vice versa).
#[tokio::test]
async fn traffic_history_agrees_with_quota_charge() {
    let db = repo().await;
    let uid = seed_history_fixture(&db, "hist_a", 60, 200, 3.0).await;

    let res = db
        .apply_traffic_batch(
            60,
            &[relay_shared::protocol::TrafficEntry {
                rule_id: 200,
                upload: 1000,
                download: 2000,
            }],
        )
        .await
        .unwrap();
    assert!(matches!(res[0], TrafficEntryResult::Ok));

    let user_used: i64 = sqlx::query_scalar("SELECT traffic_used FROM users WHERE id = ?")
        .bind(uid)
        .fetch_one(&db.pool)
        .await
        .unwrap();
    // SUM over buckets, not one row: a batch straddling an hour rollover may
    // legitimately produce two rows.
    let (h_up, h_down, h_billed): (i64, i64, i64) = sqlx::query_as(
        "SELECT COALESCE(SUM(real_upload),0), COALESCE(SUM(real_download),0), \
                COALESCE(SUM(billed_total),0) \
         FROM traffic_history WHERE rule_id = 200",
    )
    .fetch_one(&db.pool)
    .await
    .unwrap();

    assert_eq!(user_used, 9000, "user is billed real × rate");
    assert_eq!(h_billed, user_used, "history MUST equal the quota charge");
    assert_eq!((h_up, h_down), (1000, 2000), "real bytes stay unrated");
}

/// Reports arrive every ~10s; the second batch in the same hour must FOLD into
/// the existing (rule, hour) row, not add one. If this regresses, the table
/// grows ~8.6k rows per rule per day and the retention sweeper can't save it.
#[tokio::test]
async fn traffic_history_upserts_within_the_hour() {
    let db = repo().await;
    seed_history_fixture(&db, "hist_b", 61, 201, 1.0).await;

    for _ in 0..3 {
        db.apply_traffic_batch(
            61,
            &[relay_shared::protocol::TrafficEntry {
                rule_id: 201,
                upload: 100,
                download: 0,
            }],
        )
        .await
        .unwrap();
    }

    let rows: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM traffic_history WHERE rule_id = 201")
        .fetch_one(&db.pool)
        .await
        .unwrap();
    let total: i64 =
        sqlx::query_scalar("SELECT SUM(real_upload) FROM traffic_history WHERE rule_id = 201")
            .fetch_one(&db.pool)
            .await
            .unwrap();
    // ≤ 2 not == 1: the test could legitimately straddle an hour boundary.
    assert!(
        rows <= 2,
        "3 batches must fold into hour buckets, got {rows} rows"
    );
    assert_eq!(total, 300, "accumulation must not lose bytes");
}

/// Owner scoping + daily aggregation. Alice must never see Bob's buckets, and
/// the daily view must sum a day's hours into one bucket.
#[tokio::test]
async fn traffic_history_query_scopes_and_aggregates() {
    let db = repo().await;
    let alice = seed_history_fixture(&db, "hist_c", 62, 202, 1.0).await;
    let bob = seed_history_fixture(&db, "hist_d", 63, 203, 1.0).await;

    // Hand-written buckets with controlled timestamps (yesterday, two hours).
    for (uid, rule, hour, up) in [
        (alice, 202, "2026-07-20 10:00:00", 100i64),
        (alice, 202, "2026-07-20 11:00:00", 200),
        (bob, 203, "2026-07-20 10:00:00", 999),
    ] {
        sqlx::query(
            "INSERT INTO traffic_history (rule_id, uid, hour_ts, real_upload, real_download, billed_total) \
             VALUES (?, ?, ?, ?, 0, ?)",
        )
        .bind(rule)
        .bind(uid)
        .bind(hour)
        .bind(up)
        .bind(up)
        .execute(&db.pool)
        .await
        .unwrap();
    }

    // Alice's daily view: ONE bucket for 2026-07-20, summing her two hours,
    // with zero contamination from Bob.
    let daily = db
        .query_traffic_history(Some(alice), None, "2026-07-01 00:00:00", true)
        .await
        .unwrap();
    assert_eq!(daily.len(), 1, "two hours of one day fold into one bucket");
    assert_eq!(daily[0].bucket, "2026-07-20");
    assert_eq!(daily[0].real_upload, 300, "alice's hours summed");
    assert_eq!(daily[0].billed_total, 300);

    // Hourly view keeps the two hours distinct.
    let hourly = db
        .query_traffic_history(Some(alice), None, "2026-07-01 00:00:00", false)
        .await
        .unwrap();
    assert_eq!(hourly.len(), 2);
    assert_eq!(hourly[0].bucket, "2026-07-20 10:00:00");

    // The unscoped (admin) view sees both users.
    let all = db
        .query_traffic_history(None, None, "2026-07-01 00:00:00", true)
        .await
        .unwrap();
    assert_eq!(all[0].real_upload, 300 + 999, "admin sees everyone");

    // rule_id drill-down on a FOREIGN rule returns nothing for alice — the
    // uid pin makes the endpoint useless as a rule-id existence probe.
    let foreign = db
        .query_traffic_history(Some(alice), Some(203), "2026-07-01 00:00:00", true)
        .await
        .unwrap();
    assert!(foreign.is_empty(), "a foreign rule matches nothing");
}

/// Retention: prune removes strictly-older rows and leaves the rest.
#[tokio::test]
async fn traffic_history_prune_respects_cutoff() {
    let db = repo().await;
    let uid = seed_history_fixture(&db, "hist_e", 64, 204, 1.0).await;
    for hour in ["2026-05-01 00:00:00", "2026-07-20 00:00:00"] {
        sqlx::query(
            "INSERT INTO traffic_history (rule_id, uid, hour_ts, real_upload, real_download, billed_total) \
             VALUES (204, ?, ?, 1, 1, 1)",
        )
        .bind(uid)
        .bind(hour)
        .execute(&db.pool)
        .await
        .unwrap();
    }

    let deleted = db
        .prune_traffic_history("2026-06-15 00:00:00")
        .await
        .unwrap();
    assert_eq!(deleted, 1, "only the pre-cutoff row dies");
    let left: String =
        sqlx::query_scalar("SELECT hour_ts FROM traffic_history WHERE rule_id = 204")
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert_eq!(left, "2026-07-20 00:00:00");
}
