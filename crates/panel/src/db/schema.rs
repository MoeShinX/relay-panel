pub const SCHEMA_SQL: &str = r#"
CREATE TABLE IF NOT EXISTS users (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    username TEXT NOT NULL UNIQUE,
    password TEXT NOT NULL,
    balance TEXT NOT NULL DEFAULT '0',
    plan_id INTEGER REFERENCES plans(id),
    -- v1.0.7: replaces group_id. 1 = user may use ALL device groups; 0 = limited
    -- to the device groups in user_device_groups (none = cannot forward). Admins
    -- are always treated as all-allowed regardless of this flag.
    all_device_groups INTEGER NOT NULL DEFAULT 0,
    -- v0.3.0: SINGLE-TENANT. max_rules is advisory only (not enforced per-user;
    -- see the forward_rules.uid note above). Enforced only for the admin user
    -- (uid=1, max_rules=999) in practice.
    max_rules INTEGER NOT NULL DEFAULT 5,
    -- v0.3.0: PLACEHOLDER, NOT IMPLEMENTED. speed_limit / ip_limit are stored
    -- and accepted on input but never reach the node (ListenerConfig hardcodes
    -- None) and the forwarder has no limiter. Do not assume these take effect.
    speed_limit INTEGER NOT NULL DEFAULT 0,
    ip_limit INTEGER NOT NULL DEFAULT 3,
    traffic_used INTEGER NOT NULL DEFAULT 0,
    traffic_limit INTEGER NOT NULL DEFAULT 0,
    admin INTEGER NOT NULL DEFAULT 0,
    banned INTEGER NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    -- v0.4.10 PR4: force-password-change flag + JWT session-version counter.
    -- token_version is embedded in every JWT; the auth middleware rejects a
    -- token whose version != the DB value, so bumping it instantly revokes all
    -- of a user's existing sessions (admin reset, self change, ban). Appended
    -- after created_at to match the column order Migration 26 produces on
    -- upgraded databases.
    must_change_password INTEGER NOT NULL DEFAULT 0,
    token_version INTEGER NOT NULL DEFAULT 0,
    -- v1.0.8: plan expiry (TEXT 'YYYY-MM-DD HH:MM:SS' UTC, NULL = no expiry)
    -- and admin suspension. suspended does NOT block login or bump
    -- token_version (so the user stays signed in); it gates forwarding via
    -- list_active_for_config. Admins can never be suspended.
    plan_expire_at TEXT,
    suspended INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE IF NOT EXISTS plans (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    name TEXT NOT NULL,
    max_rules INTEGER NOT NULL DEFAULT 5,
    traffic INTEGER NOT NULL DEFAULT 0,
    -- v0.3.0: PLACEHOLDER, NOT IMPLEMENTED (see users.speed_limit note above).
    speed_limit INTEGER NOT NULL DEFAULT 0,
    ip_limit INTEGER NOT NULL DEFAULT 3,
    price TEXT NOT NULL DEFAULT '0',
    -- v1.0.8: plan lifecycle + visibility.
    -- plan_type: 'data' = traffic quota, 'time' = time-limited (duration_days).
    -- duration_days: 0 = unlimited (only meaningful for time plans).
    -- hidden: 1 = hidden from the public plan list + not self-purchasable.
    -- reset_traffic: 1 = buying resets traffic_used to 0.
    -- description: free-form line shown under the plan name in the shop.
    plan_type TEXT NOT NULL DEFAULT 'data',
    duration_days INTEGER NOT NULL DEFAULT 0,
    hidden INTEGER NOT NULL DEFAULT 0,
    reset_traffic INTEGER NOT NULL DEFAULT 0,
    description TEXT NOT NULL DEFAULT '',
    -- v1.0.9: when 1, buying this plan sets the user's all_device_groups flag
    -- (access to EVERY inbound group). When 0, buying grants only the groups
    -- in plan_device_groups. v1.0.8: purchase REPLACES the user's authorization
    -- (the plan's grant becomes the user's entire authorized set — old grants
    -- are cleared), not appended.
    grant_all_groups INTEGER NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE TABLE IF NOT EXISTS device_groups (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    name TEXT NOT NULL,
    group_type TEXT NOT NULL,
    token TEXT NOT NULL UNIQUE,
    uid INTEGER NOT NULL REFERENCES users(id),
    connect_host TEXT NOT NULL DEFAULT '',
    port_range TEXT NOT NULL DEFAULT '1-65535',
    fallback_group INTEGER REFERENCES device_groups(id),
    config TEXT NOT NULL DEFAULT '{}',
    -- v0.3.0: protocol capability declaration (JSON array, e.g.
    -- ["tcp","udp","tcp_udp","ws","wss","tls"]). Used for pre-create validation
    -- only; rules carry their own entry_transport. Older rows default to
    -- tcp/udp so existing groups behave exactly as before.
    capabilities TEXT NOT NULL DEFAULT '["tcp","udp"]',
    region TEXT,
    line_type TEXT,
    remark TEXT,
    -- v1.0.8: traffic billing multiplier for this line. Real bytes are stored
    -- on forward_rules / users; users are CHARGED `real * rate` (rounded).
    -- Default 1.0 = bill what you use. Range 0.1..=100 enforced at the API.
    rate REAL NOT NULL DEFAULT 1.0,
    -- v1.0.7: hide this group from regular users' shared views (node status /
    -- available lines). 1 = hidden. Admins are unaffected (they read /groups,
    -- not the shared endpoints). Default 0 keeps existing groups visible.
    hidden INTEGER NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
);

-- v0.3.0: reusable tunnel profiles describing how traffic flows between an
-- inbound node and an outbound node (NOT the user-facing entry protocol, which
-- lives on forward_rules.entry_transport). Seed rows are is_builtin=1.
CREATE TABLE IF NOT EXISTS tunnel_profiles (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    name            TEXT NOT NULL UNIQUE,
    transport       TEXT NOT NULL DEFAULT 'direct',  -- direct/ws/wss/tls/chain
    tls_mode        TEXT NOT NULL DEFAULT 'none',    -- none/terminate/passthrough
    ws_path         TEXT NOT NULL DEFAULT '/relay',
    host_header     TEXT NOT NULL DEFAULT '',
    sni             TEXT NOT NULL DEFAULT '',
    cert_id         INTEGER,                          -- certificates table (future)
    is_builtin      INTEGER NOT NULL DEFAULT 0,      -- 1 = seeded template, not deletable
    uid             INTEGER NOT NULL REFERENCES users(id),
    created_at      TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE TABLE IF NOT EXISTS forward_rules (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    name TEXT NOT NULL,
    -- v0.3.0: SINGLE-TENANT model. uid records WHO created the rule but is NOT
    -- used for access isolation in v0.3.x — all admin API read/write ignores it
    -- (any admin sees/edits every rule). Multi-tenant isolation (per-uid
    -- filtering + a restricted non-admin rule API) is a v1.0+ feature; until
    -- then max_rules / uid are effectively advisory, not enforced per-user.
    uid INTEGER NOT NULL REFERENCES users(id),
    paused INTEGER NOT NULL DEFAULT 0,
    -- v1.0.8: 1 = this rule was paused BY THE SYSTEM (buy_plan / plan removal
    -- revoking device-group authorization), 0 = paused by an explicit human
    -- action (the on/off switch, batch pause/resume) or never paused. Lets a
    -- later re-authorization (buying a plan that re-grants the group) safely
    -- auto-resume ONLY the rules IT paused, without reviving a rule the user
    -- deliberately turned off for unrelated reasons. Any explicit `paused`
    -- write via update_rule_fields resets this to 0 — a human touching the
    -- switch always overrides system bookkeeping.
    auto_paused INTEGER NOT NULL DEFAULT 0,
    listen_port INTEGER NOT NULL,
    protocol TEXT NOT NULL DEFAULT 'tcp',
    -- v0.4.0: three orthogonal fields replace the overloaded entry_transport.
    -- public_transport = user-facing ingress (raw|ws|wss|tls_simple);
    -- node_transport  = what the node listens on (raw|ws|tls_simple), derived
    --                    from public_transport at write time (wss→ws);
    -- route_mode      = forwarding topology (direct|group|chain).
    -- The legacy entry_transport column is kept (Migration 4) for old rows but
    -- new code reads ONLY these three columns.
    public_transport TEXT NOT NULL DEFAULT 'raw',
    node_transport TEXT NOT NULL DEFAULT 'raw',
    route_mode TEXT NOT NULL DEFAULT 'direct',
    -- Legacy v0.3.x column, superseded by public_transport/node_transport.
    -- Retained so old DBs don't lose data on migration; NOT read by v0.4.0 code.
    entry_transport TEXT NOT NULL DEFAULT 'raw',
    device_group_in INTEGER NOT NULL REFERENCES device_groups(id),
    device_group_out INTEGER REFERENCES device_groups(id),
    forward_mode TEXT NOT NULL DEFAULT 'group',
    -- v0.3.0: chain mode points here. NULL = fall back to the builtin 'direct'
    -- profile (id resolved at config-build time, not stored as a magic number
    -- so re-seeding stays safe).
    tunnel_profile_id INTEGER REFERENCES tunnel_profiles(id),
    -- v0.3.0: optional per-rule WS/TLS metadata. NULL means "use the tunnel
    -- profile default" (or "not applicable" for raw/tcp rules).
    domain TEXT,
    ws_path TEXT,
    ws_host TEXT,
    sni TEXT,
    target_addr TEXT NOT NULL,
    target_port INTEGER NOT NULL,
    -- v0.4.6: multi-target load-balancing strategy.
    -- 'first' (default) | 'round_robin' | 'failover'.
    load_balance_strategy TEXT NOT NULL DEFAULT 'first',
    -- v0.4.6: per-rule upload/download caps in decimal Mbps (0 = unlimited).
    -- Shared across all connections of the rule.
    upload_limit_mbps INTEGER NOT NULL DEFAULT 0,
    download_limit_mbps INTEGER NOT NULL DEFAULT 0,
    -- v1.2.0: cap on concurrent TCP connections, enforced PER NODE (nodes share
    -- no state, so a group-wide total would need a central allocator on the
    -- forwarding hot path). 0 = unlimited.
    max_connections INTEGER NOT NULL DEFAULT 0,
    -- v1.2.0: restart the rule every N minutes to shed accumulated connections.
    -- 0 = off. The API rejects non-zero values below MIN_AUTO_RESTART_MINUTES.
    auto_restart_minutes INTEGER NOT NULL DEFAULT 0,
    config TEXT NOT NULL DEFAULT '{}',
    traffic_used INTEGER NOT NULL DEFAULT 0,
    status TEXT NOT NULL DEFAULT 'active',
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE TABLE IF NOT EXISTS forward_rule_targets (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    rule_id INTEGER NOT NULL REFERENCES forward_rules(id) ON DELETE CASCADE,
    host TEXT NOT NULL,
    port INTEGER NOT NULL CHECK (port >= 1 AND port <= 65535),
    position INTEGER NOT NULL CHECK (position >= 1),
    enabled INTEGER NOT NULL DEFAULT 1,
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE INDEX IF NOT EXISTS idx_forward_rule_targets_rule_position
    ON forward_rule_targets (rule_id, position);

CREATE TABLE IF NOT EXISTS statistics (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    stat_type TEXT NOT NULL,
    stat_key TEXT NOT NULL,
    time TEXT NOT NULL,
    number INTEGER NOT NULL DEFAULT 0
);

-- v1.0.8: purchase history. plan_name + price are SNAPSHOTS at buy time so
-- the history stays accurate after a plan is later renamed/retired/deleted.
CREATE TABLE IF NOT EXISTS orders (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    plan_id INTEGER,
    plan_name TEXT NOT NULL,
    price TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
);
CREATE INDEX IF NOT EXISTS idx_orders_user_id ON orders(user_id);

-- v1.2.0: redeem codes (balance top-up). The panel has a shop + balance
-- deduction but no way for a user to ADD balance; this closes that loop
-- without depending on a payment gateway.
--
-- `used_by` is ON DELETE SET NULL, NOT CASCADE: deleting a user must not erase
-- the record that a code was spent. The row is the audit trail for money
-- entering the system, so it outlives the account that redeemed it.
--
-- `amount` is a canonical balance string (see relay_shared::money) — the same
-- representation as users.balance, so redemption is exact integer-cent math
-- with no float anywhere.
--
-- `status`: 'unused' | 'used' | 'void'. Redemption flips unused -> used with a
-- conditional UPDATE (WHERE status = 'unused') inside the balance transaction,
-- so two people racing on the same code can never both be credited.
CREATE TABLE IF NOT EXISTS redeem_codes (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    code TEXT NOT NULL UNIQUE,
    amount TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'unused',
    used_by INTEGER REFERENCES users(id) ON DELETE SET NULL,
    used_at TEXT,
    -- NULL = never expires. An expired code is REFUSED but keeps its 'unused'
    -- status, so an admin can extend the batch instead of regenerating it.
    expires_at TEXT,
    -- Groups one generation run so an admin can filter / export a batch.
    batch_id TEXT NOT NULL DEFAULT '',
    remark TEXT NOT NULL DEFAULT '',
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
);
CREATE INDEX IF NOT EXISTS idx_redeem_codes_status ON redeem_codes(status);
CREATE INDEX IF NOT EXISTS idx_redeem_codes_batch ON redeem_codes(batch_id);
CREATE INDEX IF NOT EXISTS idx_redeem_codes_used_by ON redeem_codes(used_by);

-- v1.2.0: hourly traffic history, written by apply_traffic_batch as an UPSERT
-- accumulate. Nodes report every ~10s (poll-frequency), so inserting a row per
-- report would explode; one row per (rule, hour) keeps 100 rules × 35 days at
-- ~84k rows.
--
-- `billed_total` is the SAME number charged to the user in that batch
-- (round((up+down) × group rate)), accumulated — so the history chart and the
-- quota deduction can never disagree. real_upload/real_download stay unrated
-- for the tooltip.
--
-- DELIBERATELY no FK on rule_id/uid: deleting a rule (or a user) must not
-- erase the usage that already happened — a cascade would make "last 7 days"
-- quietly shrink. Rows die by retention (35 days), not by parent deletion.
--
-- hour_ts is 'YYYY-MM-DD HH:00:00' UTC — the schema-wide TEXT timestamp format
-- whose lexicographic order is chronological.
--
-- `group_id` (v1.2.0) is the rule's inbound device group, stored as a SNAPSHOT
-- rather than resolved by joining forward_rules at query time. A join would
-- lose the line attribution the moment a rule is deleted — and this table
-- deliberately outlives its rules (see the no-FK note above), so "traffic per
-- line" would silently drop the history of deleted rules. Same reasoning as
-- orders.plan_name. 0 = unknown.
--
-- The primary key does NOT change: a rule belongs to exactly one inbound
-- group, so (rule_id, hour_ts) stays unique and the row count is unaffected.
CREATE TABLE IF NOT EXISTS traffic_history (
    rule_id INTEGER NOT NULL,
    uid INTEGER NOT NULL,
    group_id INTEGER NOT NULL DEFAULT 0,
    hour_ts TEXT NOT NULL,
    real_upload INTEGER NOT NULL DEFAULT 0,
    real_download INTEGER NOT NULL DEFAULT 0,
    billed_total INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (rule_id, hour_ts)
);
CREATE INDEX IF NOT EXISTS idx_traffic_history_uid ON traffic_history(uid, hour_ts);
CREATE INDEX IF NOT EXISTS idx_traffic_history_hour ON traffic_history(hour_ts);
CREATE INDEX IF NOT EXISTS idx_traffic_history_group ON traffic_history(group_id, hour_ts);

-- v1.0.9: plan ↔ device_group grant map. Buying a plan (with grant_all_groups=0)
-- REPLACES the user's user_device_groups with these groups (v1.0.8: purchase is
-- replace, not append — the plan's grant becomes the user's whole authorized
-- set). FK cascade so deleting a plan or device_group cleans up the mapping rows.
CREATE TABLE IF NOT EXISTS plan_device_groups (
    plan_id INTEGER NOT NULL REFERENCES plans(id) ON DELETE CASCADE,
    device_group_id INTEGER NOT NULL REFERENCES device_groups(id) ON DELETE CASCADE,
    PRIMARY KEY (plan_id, device_group_id)
);

CREATE TABLE IF NOT EXISTS kvs (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL
);

-- v0.4.11 PR4: shared-node port occupancy. The old GLOBAL UNIQUE(listen_port)
-- is removed. A port is now occupied per (device_group_in, listen_port, socket
-- type), where the socket type is derived from `protocol`:
--   tcp / tcp_udp  → occupy the TCP socket
--   udp / tcp_udp  → occupy the UDP socket
-- Conflict rules: TCP-bearing rules (tcp, tcp_udp) conflict with each other;
-- UDP-bearing rules (udp, tcp_udp) conflict with each other; a pure-TCP and a
-- pure-UDP rule may share the same port number; different device groups may
-- reuse the same port; different users selecting the same shared group share
-- one port pool (enforced naturally by the device_group_in dimension).
-- Two partial UNIQUE indexes express exactly these rules. See Migration 28 for
-- the upgrade path on existing DBs.
CREATE UNIQUE INDEX IF NOT EXISTS idx_fr_port_tcp
    ON forward_rules (device_group_in, listen_port)
    WHERE protocol IN ('tcp', 'tcp_udp');
CREATE UNIQUE INDEX IF NOT EXISTS idx_fr_port_udp
    ON forward_rules (device_group_in, listen_port)
    WHERE protocol IN ('udp', 'tcp_udp');

-- Default admin user (password: admin123, will be hashed on init)
INSERT OR IGNORE INTO users (id, username, password, admin, max_rules)
VALUES (1, 'admin', '$2b$12$PLACEHOLDER_WILL_BE_HASHED_ON_INIT', 1, 999);

-- Default plan. v1.1.0: seed ONLY when the plans table is completely empty
-- (fresh DB). The old `INSERT OR IGNORE ... VALUES (1, ...)` re-created the
-- 'free' plan on EVERY startup whenever id=1 was absent — so an admin who
-- deleted the default plan saw it reappear after every panel update. Seeding by
-- emptiness respects the deletion once any other plan exists.
INSERT INTO plans (id, name, max_rules, traffic, speed_limit, ip_limit, price)
SELECT 1, 'free', 5, 107374182400, 0, 3, '0'
WHERE NOT EXISTS (SELECT 1 FROM plans);

-- v0.4.10 PR3: application settings (registration config). Single-row table
-- (id is fixed to 1). The row is NOT seeded here — main.rs seeds it via
-- insert_settings_if_absent using REGISTRATION_ENABLED as the initial value.
-- Once the row exists the env var never overrides it. See SettingsRepository.
CREATE TABLE IF NOT EXISTS app_settings (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    registration_enabled INTEGER NOT NULL DEFAULT 0,
    default_registration_plan_id INTEGER NOT NULL DEFAULT 1 REFERENCES plans(id),
    registration_allowed_plan_ids TEXT NOT NULL DEFAULT '[1]'
);

-- v1.0.7: per-user device-group authorization. Replaces the user_groups /
-- user_group_device_groups named-entity layer with a direct user ↔ device_group
-- many-to-many. A user with all_device_groups=0 may only use the device groups
-- listed here; no rows = cannot forward.
CREATE TABLE IF NOT EXISTS user_device_groups (
    user_id INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    device_group_id INTEGER NOT NULL REFERENCES device_groups(id) ON DELETE CASCADE,
    PRIMARY KEY (user_id, device_group_id)
);
"#;

/// Run schema migrations for existing databases (v0.1.0/v0.1.1 → v0.1.2).
///
/// Two migrations needed for existing DBs:
/// 1. forward_rules: add `forward_mode` column (safe ALTER TABLE ADD COLUMN)
/// 2. forward_rules: make `device_group_out` nullable (requires table rebuild)
///
/// Both are idempotent — they detect whether the migration already ran.
/// The table rebuild follows the safe SQLite pattern:
///   create new → copy data → verify row count → drop old → rename new.
/// If verification fails, the migration aborts and leaves the old table intact.
pub async fn run_migrations(pool: &sqlx::SqlitePool) -> Result<(), sqlx::Error> {
    // ── Migration 1: add forward_mode column ──
    // For new databases, SCHEMA_SQL already creates it. For old databases,
    // ALTER TABLE ADD COLUMN is safe and doesn't require a rebuild.
    let needs_col: (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM pragma_table_info('forward_rules') WHERE name='forward_mode'",
    )
    .fetch_one(pool)
    .await?;

    if needs_col.0 == 0 {
        sqlx::query(
            "ALTER TABLE forward_rules ADD COLUMN forward_mode TEXT NOT NULL DEFAULT 'group'",
        )
        .execute(pool)
        .await?;
        tracing::info!("Migration: added forward_mode column to forward_rules");
    }

    // ── Migration 2: make device_group_out nullable ──
    // Check the original CREATE TABLE SQL from sqlite_master to see if
    // device_group_out was declared NOT NULL. If so, rebuild the table.
    let schema_row: Option<(String,)> =
        sqlx::query_as("SELECT sql FROM sqlite_master WHERE type='table' AND name='forward_rules'")
            .fetch_optional(pool)
            .await?;

    let needs_nullable_migration = schema_row
        .as_ref()
        .and_then(|(s,)| {
            if s.contains("device_group_out INTEGER NOT NULL") {
                Some(true)
            } else {
                None
            }
        })
        .unwrap_or(false);

    if needs_nullable_migration {
        // Column is still NOT NULL — rebuild the table to make it nullable.
        tracing::info!("Migration: rebuilding forward_rules to make device_group_out nullable");

        // Step 1: create the new table with the correct schema
        sqlx::query(
            r#"CREATE TABLE forward_rules_new (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    name TEXT NOT NULL,
                    uid INTEGER NOT NULL REFERENCES users(id),
                    paused INTEGER NOT NULL DEFAULT 0,
                    listen_port INTEGER NOT NULL,
                    protocol TEXT NOT NULL DEFAULT 'tcp',
                    device_group_in INTEGER NOT NULL REFERENCES device_groups(id),
                    device_group_out INTEGER REFERENCES device_groups(id),
                    forward_mode TEXT NOT NULL DEFAULT 'group',
                    target_addr TEXT NOT NULL,
                    target_port INTEGER NOT NULL,
                    config TEXT NOT NULL DEFAULT '{}',
                    traffic_used INTEGER NOT NULL DEFAULT 0,
                    status TEXT NOT NULL DEFAULT 'active',
                    created_at TEXT NOT NULL DEFAULT (datetime('now'))
                )"#,
        )
        .execute(pool)
        .await?;

        // Step 2: copy all data from old table to new.
        // forward_mode may not exist on old rows if migration 1 was just
        // applied — but ALTER TABLE ADD COLUMN backfills the default 'group',
        // so it's safe to SELECT it.
        sqlx::query(
            r#"INSERT INTO forward_rules_new
                   (id, name, uid, paused, listen_port, protocol,
                    device_group_in, device_group_out, forward_mode,
                    target_addr, target_port, config, traffic_used, status, created_at)
                   SELECT id, name, uid, paused, listen_port, protocol,
                          device_group_in, device_group_out,
                          COALESCE(forward_mode, 'group'),
                          target_addr, target_port, config, traffic_used, status, created_at
                   FROM forward_rules"#,
        )
        .execute(pool)
        .await?;

        // Step 3: verify row counts match — abort if mismatch
        let old_count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM forward_rules")
            .fetch_one(pool)
            .await?;
        let new_count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM forward_rules_new")
            .fetch_one(pool)
            .await?;

        if old_count.0 != new_count.0 {
            // Safety abort: drop the new table, leave old intact, return error
            sqlx::query("DROP TABLE forward_rules_new")
                .execute(pool)
                .await?;
            return Err(sqlx::Error::Protocol(format!(
                "Migration row count mismatch: old={} new={} — aborted, old table preserved",
                old_count.0, new_count.0
            )));
        }

        // Step 4: swap — drop old, rename new
        sqlx::query("DROP TABLE forward_rules")
            .execute(pool)
            .await?;
        sqlx::query("ALTER TABLE forward_rules_new RENAME TO forward_rules")
            .execute(pool)
            .await?;

        tracing::info!(
            "Migration: forward_rules rebuilt successfully ({} rows)",
            new_count.0
        );
    }

    // ── Migration 3: rebuild device_groups to remove CHECK constraint ──
    // Old v0.1.0/v0.1.1 had CHECK(group_type IN ('in','out')). New types
    // (monitor, chained_outbound) need it gone. Detect by trying to read the
    // schema SQL — if it contains "CHECK(group_type", rebuild.
    let schema_sql: Option<(String,)> =
        sqlx::query_as("SELECT sql FROM sqlite_master WHERE type='table' AND name='device_groups'")
            .fetch_optional(pool)
            .await?;

    if let Some((sql,)) = schema_sql {
        if sql.contains("CHECK(group_type") || sql.contains("CHECK (group_type") {
            tracing::info!("Migration: rebuilding device_groups to remove CHECK constraint");

            sqlx::query(
                r#"CREATE TABLE device_groups_new (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    name TEXT NOT NULL,
                    group_type TEXT NOT NULL,
                    token TEXT NOT NULL UNIQUE,
                    uid INTEGER NOT NULL REFERENCES users(id),
                    connect_host TEXT NOT NULL DEFAULT '',
                    port_range TEXT NOT NULL DEFAULT '1-65535',
                    fallback_group INTEGER REFERENCES device_groups(id),
                    config TEXT NOT NULL DEFAULT '{}',
                    created_at TEXT NOT NULL DEFAULT (datetime('now'))
                )"#,
            )
            .execute(pool)
            .await?;

            sqlx::query(
                r#"INSERT INTO device_groups_new
                   (id, name, group_type, token, uid, connect_host, port_range,
                    fallback_group, config, created_at)
                   SELECT id, name, group_type, token, uid, connect_host, port_range,
                          fallback_group, config, created_at
                   FROM device_groups"#,
            )
            .execute(pool)
            .await?;

            let old_count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM device_groups")
                .fetch_one(pool)
                .await?;
            let new_count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM device_groups_new")
                .fetch_one(pool)
                .await?;

            if old_count.0 != new_count.0 {
                sqlx::query("DROP TABLE device_groups_new")
                    .execute(pool)
                    .await?;
                return Err(sqlx::Error::Protocol(format!(
                    "Migration device_groups row count mismatch: old={} new={} — aborted",
                    old_count.0, new_count.0
                )));
            }

            sqlx::query("DROP TABLE device_groups")
                .execute(pool)
                .await?;
            sqlx::query("ALTER TABLE device_groups_new RENAME TO device_groups")
                .execute(pool)
                .await?;

            tracing::info!(
                "Migration: device_groups rebuilt successfully ({} rows)",
                new_count.0
            );
        }
    }

    // ── Migration 4: add entry_transport column ──
    // Same idempotent pattern as Migration 1: SCHEMA_SQL already has it on new
    // DBs; for old DBs, ALTER TABLE ADD COLUMN with a default backfills every
    // existing rule to "raw" (no behaviour change). Phase 1 only "raw" is ever
    // persisted; tls/ws/wss are rejected by the admin API.
    let needs_transport: (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM pragma_table_info('forward_rules') WHERE name='entry_transport'",
    )
    .fetch_one(pool)
    .await?;

    if needs_transport.0 == 0 {
        sqlx::query(
            "ALTER TABLE forward_rules ADD COLUMN entry_transport TEXT NOT NULL DEFAULT 'raw'",
        )
        .execute(pool)
        .await?;
        tracing::info!("Migration: added entry_transport column to forward_rules");
    }

    // ════════════════════════════════════════════════════════════════════
    // v0.3.0-alpha data model (Migrations 5–15).
    //
    // Ordering matters: the dependent column (forward_rules.tunnel_profile_id)
    // can only be added AFTER tunnel_profiles exists + is seeded. Everything
    // else is independent ALTER TABLE ADD COLUMN, which SQLite handles
    // idempotently by checking pragma_table_info first (the same pattern as
    // Migrations 1 & 4).
    //
    // Design reference: docs/TLS_WS_WSS_DESIGN.md §5.2.
    // ════════════════════════════════════════════════════════════════════

    // ── Migration 5: create tunnel_profiles table (if missing) ──
    // For new databases SCHEMA_SQL already creates it. For old databases that
    // predate v0.3.0, CREATE TABLE IF NOT EXISTS is a safe no-op if present.
    sqlx::query(
        r#"CREATE TABLE IF NOT EXISTS tunnel_profiles (
            id              INTEGER PRIMARY KEY AUTOINCREMENT,
            name            TEXT NOT NULL UNIQUE,
            transport       TEXT NOT NULL DEFAULT 'direct',
            tls_mode        TEXT NOT NULL DEFAULT 'none',
            ws_path         TEXT NOT NULL DEFAULT '/relay',
            host_header     TEXT NOT NULL DEFAULT '',
            sni             TEXT NOT NULL DEFAULT '',
            cert_id         INTEGER,
            is_builtin      INTEGER NOT NULL DEFAULT 0,
            uid             INTEGER NOT NULL REFERENCES users(id),
            created_at      TEXT NOT NULL DEFAULT (datetime('now'))
        )"#,
    )
    .execute(pool)
    .await?;
    tracing::debug!("Migration 5: tunnel_profiles table present");

    // ── Migration 6: seed builtin tunnel_profiles ──
    // Idempotent: name is UNIQUE, so INSERT OR IGNORE skips rows that already
    // exist (re-runs on an already-migrated DB are a no-op). uid=1 is the
    // default admin user created by SCHEMA_SQL; builtin templates are owned by
    // admin so every deployment has the same baseline set.
    sqlx::query(
        r#"INSERT OR IGNORE INTO tunnel_profiles
               (name, transport, tls_mode, ws_path, host_header, sni, is_builtin, uid)
           VALUES
               ('direct',          'direct', 'none',         '',      '', '', 1, 1),
               ('ws-relay',        'ws',     'none',         '/relay','', '', 1, 1),
               ('tls-passthrough', 'tls',    'passthrough',  '',      '', '', 1, 1),
               ('tls-terminate',   'tls',    'terminate',    '',      '', '', 1, 1),
               ('chain',           'chain',  'none',         '',      '', '', 1, 1)"#,
    )
    .execute(pool)
    .await?;
    tracing::debug!("Migration 6: builtin tunnel_profiles seeded");

    // ── Migration 7: forward_rules.tunnel_profile_id ──
    // References tunnel_profiles(id). MUST come after Migration 5 so the FK
    // target exists. Nullable: NULL means "no tunnel → fall back to builtin
    // 'direct'". Existing rules get NULL and behave exactly as before.
    add_column_if_missing(
        pool,
        "forward_rules",
        "tunnel_profile_id",
        "INTEGER REFERENCES tunnel_profiles(id)",
    )
    .await?;

    // ── Migrations 8–11: optional per-rule WS/TLS metadata ──
    // All nullable TEXT. NULL = "not applicable" (raw/tcp rules) or "use the
    // tunnel profile default". Added as plain ALTER TABLE ADD COLUMN.
    add_column_if_missing(pool, "forward_rules", "domain", "TEXT").await?; // Migration 8
    add_column_if_missing(pool, "forward_rules", "ws_path", "TEXT").await?; // Migration 9
    add_column_if_missing(pool, "forward_rules", "ws_host", "TEXT").await?; // Migration 10
    add_column_if_missing(pool, "forward_rules", "sni", "TEXT").await?; // Migration 11

    // ── Migration 12: device_groups.capabilities ──
    // JSON array of protocol capability strings. Default tcp+udp so existing
    // groups — which were always implicitly tcp/udp-only — keep their old
    // behaviour. NOT NULL with a default so every row always has a value.
    add_column_if_missing(
        pool,
        "device_groups",
        "capabilities",
        "TEXT NOT NULL DEFAULT '[\"tcp\",\"udp\"]'",
    )
    .await?; // Migration 12

    // ── Migrations 13–15: device_groups metadata columns ──
    // All nullable TEXT, purely descriptive (region / line / remark). NULL on
    // existing rows → frontend renders "-".
    add_column_if_missing(pool, "device_groups", "region", "TEXT").await?; // Migration 13
    add_column_if_missing(pool, "device_groups", "line_type", "TEXT").await?; // Migration 14
    add_column_if_missing(pool, "device_groups", "remark", "TEXT").await?; // Migration 15

    // ── Migration 16: listen_port uniqueness ──
    // Closes the TOCTOU window in auto_assign_port: even if two concurrent
    // create_rule requests pick the same free port, the UNIQUE INDEX rejects
    // the second INSERT at the DB layer (auto_assign_port retries on this).
    //
    // Safety on existing data: if a pre-v0.3.0 DB already has DUPLICATE
    // listen_port rows (only possible via the very race this fixes, or via
    // direct DB editing), CREATE UNIQUE INDEX would fail. We detect duplicates
    // first; if any exist we log a loud warning + leave the index unset rather
    // than silently deleting user rules. The operator must resolve duplicates
    // manually (the log names the offending ports) before the protection is
    // active — deleting data unattended is not this migration's job.
    let dupes: (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM (
            SELECT listen_port FROM forward_rules
            GROUP BY listen_port HAVING COUNT(*) > 1
        )",
    )
    .fetch_one(pool)
    .await?;
    if dupes.0 > 0 {
        // List the offending ports so the operator knows exactly what to fix.
        let dupe_ports: Vec<(i32,)> = sqlx::query_as(
            "SELECT listen_port FROM forward_rules
             GROUP BY listen_port HAVING COUNT(*) > 1 ORDER BY listen_port",
        )
        .fetch_all(pool)
        .await?;
        tracing::error!(
            "Migration 16 SKIPPED: forward_rules has {} duplicate listen_port value(s): {:?}. \
             The UNIQUE index was NOT created. Resolve these (keep one rule per port, delete the \
             rest) and restart to activate port-uniqueness protection.",
            dupes.0,
            dupe_ports.iter().map(|(p,)| p).collect::<Vec<_>>()
        );
    } else {
        sqlx::query(
            "CREATE UNIQUE INDEX IF NOT EXISTS idx_forward_rules_listen_port
             ON forward_rules (listen_port)",
        )
        .execute(pool)
        .await?;
        tracing::info!("Migration 16: created UNIQUE index on forward_rules.listen_port");
    }

    // ── Migration 17: v0.4.0 three-field transport split ──
    // Adds public_transport / node_transport / route_mode columns. The legacy
    // entry_transport column stays (not dropped — SQLite can't easily drop a
    // column without a table rebuild, and old data is harmless once unread).
    //
    // Backfill: existing rows have entry_transport ∈ {raw, ws, tls, wss} but
    // NULL/absent public/node/route columns. We set:
    //   public_transport = entry_transport (raw/ws/wss/tls carry over; "tls"
    //     is later mapped to "tls_simple" by PublicTransport::from_db_str)
    //   node_transport  = derived from entry_transport (wss→ws, else identity)
    //   route_mode      = forward_mode (direct/group carry over; "chain" rare)
    // This runs once (idempotent: add_column_if_missing skips if present; the
    // backfill UPDATE is guarded by "WHERE public_transport IS NULL"/empty so
    // re-runs don't clobber v0.4.0-written values).
    add_column_if_missing(
        pool,
        "forward_rules",
        "public_transport",
        "TEXT NOT NULL DEFAULT 'raw'",
    )
    .await?; // 17a
    add_column_if_missing(
        pool,
        "forward_rules",
        "node_transport",
        "TEXT NOT NULL DEFAULT 'raw'",
    )
    .await?; // 17b
    add_column_if_missing(
        pool,
        "forward_rules",
        "route_mode",
        "TEXT NOT NULL DEFAULT 'direct'",
    )
    .await?; // 17c

    // Backfill from the legacy columns for rows that pre-date this migration.
    // Only touches rows whose public_transport is still the column default
    // ('raw') AND whose legacy entry_transport is something else — i.e. a
    // pre-v0.4.0 ws/wss rule that hasn't been migrated yet. A genuinely raw
    // rule needs no update (defaults are already correct).
    sqlx::query(
        r#"UPDATE forward_rules
           SET public_transport = entry_transport,
               node_transport = CASE entry_transport
                   WHEN 'wss' THEN 'ws'
                   WHEN 'tls' THEN 'tls_simple'
                   ELSE entry_transport
               END,
               route_mode = COALESCE(NULLIF(forward_mode, ''), 'direct')
           WHERE public_transport = 'raw'
             AND entry_transport != 'raw'"#,
    )
    .execute(pool)
    .await?;
    tracing::info!("Migration 17: v0.4.0 transport split columns added + backfilled");

    // ── Migration 18: v0.4.1 WSS removal ──
    // Business WSS is cancelled (see ROADMAP-v0.4.md). Existing wss rules are
    // converted to plain ws (the node already runs a ws listener for them; only
    // the public label changes). The builtin wss-via-caddy tunnel profile is
    // deleted. Order matters: convert rules BEFORE deleting the profile, and
    // null out tunnel_profile_id references first to avoid FK issues.
    //
    // Idempotent: all statements are UPDATE/DELETE with WHERE clauses that match
    // nothing on a second run.
    // v0.4.1 fix: wrap all three statements in a transaction so a failure at
    // any step leaves the DB untouched (no half-migrated state).
    let mut tx = pool.begin().await?;
    sqlx::query(
        r#"UPDATE forward_rules
           SET public_transport = 'ws',
               node_transport = 'ws',
               entry_transport = 'ws'
           WHERE public_transport = 'wss'"#,
    )
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        r#"UPDATE forward_rules
           SET tunnel_profile_id = NULL
           WHERE tunnel_profile_id IN (
               SELECT id FROM tunnel_profiles WHERE name = 'wss-via-caddy'
           )"#,
    )
    .execute(&mut *tx)
    .await?;
    sqlx::query("DELETE FROM tunnel_profiles WHERE name = 'wss-via-caddy'")
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    tracing::info!("Migration 18: wss rules converted to ws, wss-via-caddy profile removed");

    // ── Migration 19: v0.4.6 multi-target rule table ──
    sqlx::query(
        r#"CREATE TABLE IF NOT EXISTS forward_rule_targets (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            rule_id INTEGER NOT NULL REFERENCES forward_rules(id) ON DELETE CASCADE,
            host TEXT NOT NULL,
            port INTEGER NOT NULL CHECK (port >= 1 AND port <= 65535),
            position INTEGER NOT NULL CHECK (position >= 1),
            enabled INTEGER NOT NULL DEFAULT 1,
            created_at TEXT NOT NULL DEFAULT (datetime('now'))
        )"#,
    )
    .execute(pool)
    .await?;
    sqlx::query(
        "CREATE INDEX IF NOT EXISTS idx_forward_rule_targets_rule_position \
         ON forward_rule_targets (rule_id, position)",
    )
    .execute(pool)
    .await?;
    sqlx::query(
        r#"INSERT INTO forward_rule_targets (rule_id, host, port, position, enabled)
           SELECT fr.id, fr.target_addr, fr.target_port, 1, 1
           FROM forward_rules fr
           WHERE NOT EXISTS (
               SELECT 1 FROM forward_rule_targets t WHERE t.rule_id = fr.id
           )"#,
    )
    .execute(pool)
    .await?;
    tracing::info!("Migration 19: forward_rule_targets created and backfilled");

    // ── Migration 20: v0.4.6 load_balance_strategy column ──
    add_column_if_missing(
        pool,
        "forward_rules",
        "load_balance_strategy",
        "TEXT NOT NULL DEFAULT 'first'",
    )
    .await?;
    tracing::info!("Migration 20: forward_rules.load_balance_strategy added");

    // ── Migration 21: v0.4.6 per-rule rate-limit columns ──
    add_column_if_missing(
        pool,
        "forward_rules",
        "upload_limit_mbps",
        "INTEGER NOT NULL DEFAULT 0",
    )
    .await?;
    add_column_if_missing(
        pool,
        "forward_rules",
        "download_limit_mbps",
        "INTEGER NOT NULL DEFAULT 0",
    )
    .await?;
    tracing::info!("Migration 21: forward_rules upload/download_limit_mbps added");

    // ── Migration 22: v0.4.7 chain removal + tls template consolidation ──
    // All steps are in ONE transaction so a failure leaves the DB untouched.
    // Idempotent: re-running is a no-op (deletes target already-gone rows;
    // updates match nothing once rewritten).
    //
    // 1. Historical chain rules → paused (NOT rewritten to direct, to avoid
    //    silently changing the forwarding path). The admin must reconfigure.
    // 2. chained_outbound groups → out (the only egress role now).
    // 3. Drop the dead builtin templates: chain, tls-passthrough, tls-terminate.
    // 4. Ensure a single canonical 'tls-simple' builtin exists (transport=
    //    'tls_simple'). INSERT OR IGNORE so it's only added once.
    // 5. Null out any forward_rules.tunnel_profile_id that pointed at a removed
    //    builtin (rules fall back to their stored public_transport, zero break).
    {
        let mut tx = pool.begin().await?;
        // Guard: only run the chain-rule pause if the forward_rules table has
        // the columns the UPDATE references. On a normal upgrade path they
        // exist (added by Migration 2); this guard only matters for the
        // minimal "ancient schema" fixtures used in migration tests, where
        // forward_rules may still be in a pre-Migration-2 shape.
        let has_columns: (i64,) = sqlx::query_as(
            "SELECT COUNT(*) FROM pragma_table_info('forward_rules') \
             WHERE name IN ('paused', 'route_mode')",
        )
        .fetch_one(&mut *tx)
        .await?;
        let paused = if has_columns.0 == 2 {
            sqlx::query(
                r#"UPDATE forward_rules SET paused = 1
                   WHERE route_mode = 'chain' AND paused = 0"#,
            )
            .execute(&mut *tx)
            .await?
            .rows_affected()
        } else {
            0
        };
        let rewired = sqlx::query(
            r#"UPDATE device_groups SET group_type = 'out'
               WHERE group_type = 'chained_outbound'"#,
        )
        .execute(&mut *tx)
        .await?
        .rows_affected();
        // v0.4.8 fix: NULL out tunnel_profile_id on rules referencing the
        // templates we're about to delete BEFORE the delete. SQLite doesn't
        // enforce FKs by default, but we keep the order correct and consistent
        // with PostgreSQL, and it prevents dangling references on
        // FK-enforced deployments.
        let has_tp: (i64,) = sqlx::query_as(
            "SELECT COUNT(*) FROM pragma_table_info('forward_rules') \
             WHERE name = 'tunnel_profile_id'",
        )
        .fetch_one(&mut *tx)
        .await?;
        if has_tp.0 == 1 {
            sqlx::query(
                r#"UPDATE forward_rules
                   SET tunnel_profile_id = NULL
                   WHERE tunnel_profile_id IN (
                       SELECT id FROM tunnel_profiles
                       WHERE name IN ('chain', 'tls-passthrough', 'tls-terminate')
                   )"#,
            )
            .execute(&mut *tx)
            .await?;
        }
        // Now safe to delete the dead builtins.
        sqlx::query(
            r#"DELETE FROM tunnel_profiles
               WHERE name IN ('chain', 'tls-passthrough', 'tls-terminate')"#,
        )
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            r#"INSERT OR IGNORE INTO tunnel_profiles
                   (name, transport, tls_mode, ws_path, host_header, sni, is_builtin, uid)
               VALUES ('tls-simple', 'tls_simple', 'none', '', '', '', 1, 1)"#,
        )
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        tracing::info!(
            "Migration 22: chain removed (paused {} chain rules, rewired {} chained_outbound groups)",
            paused,
            rewired
        );
    }

    // ── Migration 23: v0.4.7 index on forward_rules.tunnel_profile_id ──
    // Speeds up the delete-usage count and config-build profile lookups.
    sqlx::query(
        "CREATE INDEX IF NOT EXISTS idx_forward_rules_tunnel_profile \
         ON forward_rules (tunnel_profile_id)",
    )
    .execute(pool)
    .await?;
    tracing::info!("Migration 23: forward_rules.tunnel_profile_id indexed");

    // ── Migration 24: v0.4.10 pause rules bound to custom profiles (non-admin) ──
    //
    // v0.4.11 PR3 CHANGE: REMOVED the uid-mismatch pause checks.
    // The invariant forward_rules.uid == device_groups(in).uid is no longer
    // enforced as a hard rule — admin-shared inbound groups are allowed, and
    // a cross-user rule (rule.uid != group.uid) is a valid shared-inbound setup.
    //
    // The ONE invariant that remains enforced here:
    //   a regular user's rule may bind ONLY a builtin tunnel profile
    //
    // Pre-existing databases may contain regular-user rules bound to custom
    // profiles. These are paused so they stop receiving traffic; the operator
    // can rebind to a builtin profile or convert to admin.
    //
    // Idempotent: WHERE paused = 0 means already-paused rules are not recounted.
    //
    // Column guard: mirrors Migration 22's pattern. If any required column is
    // missing we skip Migration 24 entirely — the remaining invariant
    // (builtin-only for non-admin) is already enforced at write time from
    // v0.4.10 onward.
    {
        let mut tx = pool.begin().await?;
        let fr_cols: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM pragma_table_info('forward_rules') \
             WHERE name IN ('paused', 'tunnel_profile_id', 'device_group_in', 'device_group_out', 'uid')",
        )
        .fetch_one(&mut *tx)
        .await?;
        let dg_cols: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM pragma_table_info('device_groups') WHERE name = 'uid'",
        )
        .fetch_one(&mut *tx)
        .await?;
        let has_tp: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='tunnel_profiles'",
        )
        .fetch_one(&mut *tx)
        .await?;
        let tp_cols: i64 = if has_tp > 0 {
            sqlx::query_scalar(
                "SELECT COUNT(*) FROM pragma_table_info('tunnel_profiles') \
                 WHERE name IN ('is_builtin', 'uid')",
            )
            .fetch_one(&mut *tx)
            .await?
        } else {
            0
        };
        let u_cols: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM pragma_table_info('users') WHERE name = 'admin'",
        )
        .fetch_one(&mut *tx)
        .await?;
        // 5 forward_rules cols + 1 device_groups.uid + 2 tunnel_profiles cols
        // + 1 users.admin = 9. Anything less means the schema predates this
        // migration's assumptions — skip (write-path enforcement already covers).
        if fr_cols == 5 && dg_cols == 1 && tp_cols == 2 && u_cols == 1 {
            // v0.4.11 PR3: cross-owner rules are now ALLOWED (shared inbound).
            // Only pause non-admin rules bound to custom (non-builtin) profiles.
            let paused_custom_profile = sqlx::query(
                "UPDATE forward_rules SET paused = 1 \
                 WHERE tunnel_profile_id IS NOT NULL AND paused = 0 \
                 AND EXISTS (SELECT 1 FROM tunnel_profiles tp, users u \
                             WHERE tp.id = forward_rules.tunnel_profile_id \
                               AND tp.is_builtin = 0 \
                               AND u.id = forward_rules.uid AND u.admin = 0)",
            )
            .execute(&mut *tx)
            .await?;
            let paused_total = paused_custom_profile.rows_affected();

            if paused_total > 0 {
                tracing::warn!(
                    "Migration 24: paused {} rule(s) with regular-user bound to \
                     custom (non-builtin) tunnel profile. Admin review required — \
                     rebind to builtin profile or convert to admin.",
                    paused_total
                );
            } else {
                tracing::info!("Migration 24: no custom-profile violations found");
            }
        } else {
            tracing::info!(
                "Migration 24: skipped (schema predates required columns; \
                 write-path enforcement already covers)"
            );
        }
        tx.commit().await?;
    }

    // ── Migration 25: v0.4.10 PR3 app_settings table ──
    // Single-row table for registration config. IF NOT EXISTS so it's a no-op
    // on DBs where the baseline SCHEMA_SQL already created it (fresh installs).
    // The row itself is seeded by main.rs via insert_settings_if_absent (using
    // REGISTRATION_ENABLED as the initial value), NOT here — so this migration
    // only ensures the table exists on upgraded DBs.
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS app_settings (\
             id INTEGER PRIMARY KEY CHECK (id = 1),\
             registration_enabled INTEGER NOT NULL DEFAULT 0,\
             default_registration_plan_id INTEGER NOT NULL DEFAULT 1 REFERENCES plans(id)\
         )",
    )
    .execute(pool)
    .await?;
    tracing::info!("Migration 25: app_settings table present");

    // ── Migration 26: v0.4.10 PR4 must_change_password + token_version ──
    // Idempotent ADD COLUMN (add_column_if_missing checks pragma_table_info).
    // token_version drives JWT session revocation; must_change_password gates
    // a forced password change on next login. Both default to the no-op value
    // (0) so existing users are unaffected until an explicit reset/ban.
    add_column_if_missing(
        pool,
        "users",
        "must_change_password",
        "INTEGER NOT NULL DEFAULT 0",
    )
    .await?;
    add_column_if_missing(pool, "users", "token_version", "INTEGER NOT NULL DEFAULT 0").await?;
    tracing::info!("Migration 26: users.must_change_password + token_version present");

    // ── Migration 27: v0.4.11 PR1 tunnel profile semantics ──
    //
    // Non-destructive migration to align existing data with the new invariant:
    // tunnel templates represent only WS / TLS Simple. Direct/raw are no longer
    // tunnel template concepts.
    //
    // Strategy:
    // 1. Ensure builtin ws-relay and tls-simple templates exist (idempotent).
    // 2. Rules with NULL tunnel_profile_id and ws/tls_simple public_transport →
    //    bind to the corresponding builtin template.
    // 3. Rules bound to direct-profile templates → unbind and switch to Raw.
    // 4. Rules with WS/TLS transport whose bound profile has mismatched transport
    //    → sync rule transport to match the profile.

    // Step 1: Ensure builtin templates exist (idempotent via ON CONFLICT DO NOTHING).
    sqlx::query(
        r#"INSERT OR IGNORE INTO tunnel_profiles
               (name, transport, tls_mode, ws_path, host_header, sni, is_builtin, uid)
            VALUES ('ws-relay', 'ws', 'none', '/relay', '', '', 1, 1)"#,
    )
    .execute(pool)
    .await?;

    sqlx::query(
        r#"INSERT OR IGNORE INTO tunnel_profiles
               (name, transport, tls_mode, ws_path, host_header, sni, is_builtin, uid)
            VALUES ('tls-simple', 'tls_simple', 'none', '', '', '', 1, 1)"#,
    )
    .execute(pool)
    .await?;

    // Step 2: Bind NULL-profile WS rules to builtin ws-relay.
    // Only rules that don't already have a profile and have ws public_transport.
    sqlx::query(
        r#"UPDATE forward_rules
            SET tunnel_profile_id = (
                SELECT id FROM tunnel_profiles WHERE name = 'ws-relay' AND is_builtin = 1
            )
            WHERE tunnel_profile_id IS NULL
              AND public_transport = 'ws'"#,
    )
    .execute(pool)
    .await?;

    // Step 3: Bind NULL-profile TLS Simple rules to builtin tls-simple.
    sqlx::query(
        r#"UPDATE forward_rules
            SET tunnel_profile_id = (
                SELECT id FROM tunnel_profiles WHERE name = 'tls-simple' AND is_builtin = 1
            )
            WHERE tunnel_profile_id IS NULL
              AND public_transport IN ('tls_simple', 'tls')"#,
    )
    .execute(pool)
    .await?;

    // Step 4: Unbind direct-profile templates and switch to Raw.
    // Find rules bound to profiles with transport='direct'.
    let direct_profiles: Vec<(i64,)> =
        sqlx::query_as("SELECT id FROM tunnel_profiles WHERE transport = 'direct'")
            .fetch_all(pool)
            .await?;

    if !direct_profiles.is_empty() {
        let direct_ids: Vec<i64> = direct_profiles.iter().map(|(id,)| *id).collect();
        let placeholders: Vec<String> = direct_ids.iter().map(|_| "?".to_string()).collect();
        let sql = format!(
            r#"UPDATE forward_rules
                SET tunnel_profile_id = NULL,
                    public_transport = 'raw',
                    node_transport = 'raw',
                    entry_transport = 'raw',
                    ws_path = NULL
                WHERE tunnel_profile_id IN ({})"#,
            placeholders.join(", ")
        );
        let mut q = sqlx::query(&sql);
        for id in &direct_ids {
            q = q.bind(id);
        }
        q.execute(pool).await?;
    }

    tracing::info!("Migration 27: tunnel profile semantics aligned");

    // ── Migration 28: v0.4.11 PR4 shared-node port occupancy ──
    //
    // Replace the GLOBAL UNIQUE(listen_port) index with two PARTIAL unique
    // indexes keyed on (device_group_in, listen_port), split by socket type:
    //   idx_fr_port_tcp → protocol IN ('tcp','tcp_udp')   (TCP socket)
    //   idx_fr_port_udp → protocol IN ('udp','tcp_udp')   (UDP socket)
    //
    // This relaxes the old global rule: a pure-TCP and a pure-UDP rule may now
    // share a port number, different device groups may reuse ports, and users
    // sharing one inbound group share its port pool. The old global UNIQUE was
    // strictly stricter, so any DB that satisfied it also satisfies the new
    // partial indexes — the migration cannot lose data.
    //
    // Safety: a DB that SKIPPED Migration 16 (had duplicate listen_ports) could
    // still hold rows that violate a partial index. We detect per-partition
    // duplicates first and SKIP (keeping the old index) rather than error out,
    // mirroring Migration 16. Idempotent: CREATE/DROP ... IF [NOT] EXISTS.
    {
        let tcp_dupes: (i64,) = sqlx::query_as(
            "SELECT COUNT(*) FROM (
                 SELECT device_group_in, listen_port FROM forward_rules
                 WHERE protocol IN ('tcp', 'tcp_udp')
                 GROUP BY device_group_in, listen_port HAVING COUNT(*) > 1
             )",
        )
        .fetch_one(pool)
        .await?;
        let udp_dupes: (i64,) = sqlx::query_as(
            "SELECT COUNT(*) FROM (
                 SELECT device_group_in, listen_port FROM forward_rules
                 WHERE protocol IN ('udp', 'tcp_udp')
                 GROUP BY device_group_in, listen_port HAVING COUNT(*) > 1
             )",
        )
        .fetch_one(pool)
        .await?;

        if tcp_dupes.0 > 0 || udp_dupes.0 > 0 {
            tracing::error!(
                "Migration 28 SKIPPED: forward_rules has conflicting (device_group_in, \
                 listen_port) rows (tcp partition: {}, udp partition: {}). The new partial \
                 UNIQUE indexes were NOT created and the legacy global index is kept. Resolve \
                 the conflicts and restart to activate per-group port occupancy.",
                tcp_dupes.0,
                udp_dupes.0
            );
        } else {
            sqlx::query("DROP INDEX IF EXISTS idx_forward_rules_listen_port")
                .execute(pool)
                .await?;
            sqlx::query(
                "CREATE UNIQUE INDEX IF NOT EXISTS idx_fr_port_tcp
                 ON forward_rules (device_group_in, listen_port)
                 WHERE protocol IN ('tcp', 'tcp_udp')",
            )
            .execute(pool)
            .await?;
            sqlx::query(
                "CREATE UNIQUE INDEX IF NOT EXISTS idx_fr_port_udp
                 ON forward_rules (device_group_in, listen_port)
                 WHERE protocol IN ('udp', 'tcp_udp')",
            )
            .execute(pool)
            .await?;
            tracing::info!(
                "Migration 28: replaced global listen_port index with per-group partial \
                 TCP/UDP indexes"
            );
        }
    }

    // ── Migration 29: v0.4.21 PR2 registration allowed plan ids ──
    //
    // Add registration_allowed_plan_ids TEXT column to app_settings so the
    // admin can select which plans are available for self-registration. Stored
    // as a JSON array of plan ids (e.g. '[1,2,3]') for SQLite/PG parity.
    // Existing rows default to [default_registration_plan_id]; new rows default
    // to '[1]'.
    add_column_if_missing(
        pool,
        "app_settings",
        "registration_allowed_plan_ids",
        "TEXT NOT NULL DEFAULT '[1]'",
    )
    .await?;
    // For existing rows that predate this migration, seed allowed from the
    // existing default_registration_plan_id. A row where the default is already
    // 1 already has the right default — no-op.
    sqlx::query(
        "UPDATE app_settings SET registration_allowed_plan_ids = \
         '[' || default_registration_plan_id || ']' \
         WHERE registration_allowed_plan_ids = '[1]' \
           AND default_registration_plan_id != 1",
    )
    .execute(pool)
    .await?;
    tracing::info!("Migration 29: added registration_allowed_plan_ids to app_settings");

    // ── Migration 30: v1.0.4 user permission groups ──
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS user_groups (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            name TEXT NOT NULL,
            remark TEXT NOT NULL DEFAULT '',
            allow_all_groups INTEGER NOT NULL DEFAULT 0,
            created_at TEXT NOT NULL DEFAULT (datetime('now'))
        )",
    )
    .execute(pool)
    .await?;

    sqlx::query(
        "CREATE TABLE IF NOT EXISTS user_group_device_groups (
            user_group_id INTEGER NOT NULL REFERENCES user_groups(id) ON DELETE CASCADE,
            device_group_id INTEGER NOT NULL REFERENCES device_groups(id) ON DELETE CASCADE,
            PRIMARY KEY (user_group_id, device_group_id)
        )",
    )
    .execute(pool)
    .await?;

    sqlx::query(
        "INSERT OR IGNORE INTO user_groups (id, name, remark, allow_all_groups) \
         VALUES (1, 'default', 'Default group - all device groups allowed', 1)",
    )
    .execute(pool)
    .await?;

    // Migrate existing users to the default group, but only if the
    // `group_id` column exists (defensive for test schemas that may not
    // have it; real DBs always have it from baseline SCHEMA_SQL).
    let has_group_id: (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM pragma_table_info('users') WHERE name = 'group_id'")
            .fetch_one(pool)
            .await?;
    if has_group_id.0 > 0 {
        sqlx::query("UPDATE users SET group_id = 1 WHERE group_id IS NULL")
            .execute(pool)
            .await?;
    }

    tracing::info!("Migration 30: user permission groups tables created");

    // ── Migration 31: v1.0.5 fix em-dash encoding in default user_group remark ──
    // The seed in Migration 30 used an em dash (U+2014) which is garbled on
    // PostgreSQL connections with non-UTF-8 client_encoding. Replace with ASCII.
    sqlx::query(
        "UPDATE user_groups SET remark = 'Default group - all device groups allowed' \
         WHERE id = 1 AND remark != 'Default group - all device groups allowed'",
    )
    .execute(pool)
    .await?;
    tracing::info!("Migration 31: default user_group remark normalized to ASCII");

    // ── Migration 32: v1.0.7 drop the user_groups named-entity layer ──
    // Replaces user_groups / user_group_device_groups (a user → named group →
    // device-group allowlist chain) with a direct user ↔ device_group link plus
    // a per-user `all_device_groups` flag. Per the refactor decision, existing
    // authorizations are NOT backfilled: every non-admin starts unassigned
    // (all_device_groups=0, no rows) and admins re-assign. Admins are always
    // treated as all-allowed in code, so no flag flip is needed for them.
    //
    // The legacy `users.group_id` column is left in place (dormant, unread) —
    // dropping it would require a full users-table rebuild, which is not worth
    // the risk since nothing reads it anymore.
    add_column_if_missing(
        pool,
        "users",
        "all_device_groups",
        "INTEGER NOT NULL DEFAULT 0",
    )
    .await?;

    sqlx::query(
        "CREATE TABLE IF NOT EXISTS user_device_groups (
            user_id INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
            device_group_id INTEGER NOT NULL REFERENCES device_groups(id) ON DELETE CASCADE,
            PRIMARY KEY (user_id, device_group_id)
        )",
    )
    .execute(pool)
    .await?;

    // Drop the legacy named-entity tables (self-contained; FK-referenced only by
    // each other, so dropping is safe and loses no still-used data).
    sqlx::query("DROP TABLE IF EXISTS user_group_device_groups")
        .execute(pool)
        .await?;
    sqlx::query("DROP TABLE IF EXISTS user_groups")
        .execute(pool)
        .await?;

    tracing::info!(
        "Migration 32: user_groups layer replaced by user_device_groups + all_device_groups flag"
    );

    // ── Migration 33: v1.0.8 device-group traffic billing rate ──
    // Adds device_groups.rate (REAL NOT NULL DEFAULT 1.0). Real bytes stay on
    // forward_rules / users; users are CHARGED real * rate (rounded) inside
    // apply_traffic_batch. Existing rows backfill to 1.0 (unchanged billing).
    add_column_if_missing(pool, "device_groups", "rate", "REAL NOT NULL DEFAULT 1.0").await?;
    tracing::info!("Migration 33: device_groups.rate column present");

    // ── Migration 34: v1.0.8 plan management + user suspension ──
    // Adds:
    //   plans: plan_type / duration_days / hidden / reset_traffic / description
    //   users: plan_expire_at (TEXT, NULL = no expiry) / suspended (0/1)
    //   orders: purchase history (snapshots plan_name + price at buy time)
    // Every column + the table use add_column_if_missing / IF NOT EXISTS so the
    // arm is idempotent (re-runnable) and safe on a fresh-schema DB.
    add_column_if_missing(pool, "plans", "plan_type", "TEXT NOT NULL DEFAULT 'data'").await?;
    add_column_if_missing(pool, "plans", "duration_days", "INTEGER NOT NULL DEFAULT 0").await?;
    add_column_if_missing(pool, "plans", "hidden", "INTEGER NOT NULL DEFAULT 0").await?;
    add_column_if_missing(pool, "plans", "reset_traffic", "INTEGER NOT NULL DEFAULT 0").await?;
    add_column_if_missing(pool, "plans", "description", "TEXT NOT NULL DEFAULT ''").await?;
    add_column_if_missing(pool, "users", "plan_expire_at", "TEXT").await?;
    add_column_if_missing(pool, "users", "suspended", "INTEGER NOT NULL DEFAULT 0").await?;

    sqlx::query(
        "CREATE TABLE IF NOT EXISTS orders (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            user_id INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
            plan_id INTEGER,
            plan_name TEXT NOT NULL,
            price TEXT NOT NULL,
            created_at TEXT NOT NULL DEFAULT (datetime('now'))
        )",
    )
    .execute(pool)
    .await?;
    sqlx::query("CREATE INDEX IF NOT EXISTS idx_orders_user_id ON orders(user_id)")
        .execute(pool)
        .await?;
    tracing::info!("Migration 34: plans lifecycle cols + users suspension + orders table");

    // ── Migration 35: v1.0.9 plan ↔ device-group grants ──
    // Adds plans.grant_all_groups + the plan_device_groups map table. Buying a
    // plan grants these device groups to the user (see buy_plan). Idempotent:
    // add_column_if_missing + CREATE TABLE IF NOT EXISTS.
    add_column_if_missing(
        pool,
        "plans",
        "grant_all_groups",
        "INTEGER NOT NULL DEFAULT 0",
    )
    .await?;
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS plan_device_groups (
            plan_id INTEGER NOT NULL REFERENCES plans(id) ON DELETE CASCADE,
            device_group_id INTEGER NOT NULL REFERENCES device_groups(id) ON DELETE CASCADE,
            PRIMARY KEY (plan_id, device_group_id)
        )",
    )
    .execute(pool)
    .await?;
    tracing::info!("Migration 35: plans.grant_all_groups + plan_device_groups table");

    // ── Migration 36: v1.0.7 device-group hidden flag ──
    // Hides a group from regular users' shared views (node status / available
    // lines) without affecting admins. Idempotent: add_column_if_missing.
    // Default 0 keeps every existing group visible.
    add_column_if_missing(
        pool,
        "device_groups",
        "hidden",
        "INTEGER NOT NULL DEFAULT 0",
    )
    .await?;
    tracing::info!("Migration 36: device_groups.hidden column present");

    // ── Migration 37: v1.0.8 forward_rules.auto_paused ──
    // Distinguishes system-auto-paused (buy_plan / plan removal revoking
    // authorization) from human-paused (the on/off switch), so a later
    // re-authorization can safely auto-resume only rules it paused itself.
    // Default 0 keeps every existing paused rule as "human paused" (no
    // surprise auto-resume for pre-existing rows).
    add_column_if_missing(
        pool,
        "forward_rules",
        "auto_paused",
        "INTEGER NOT NULL DEFAULT 0",
    )
    .await?;
    tracing::info!("Migration 37: forward_rules.auto_paused column present");

    // ── Migration 38: v1.2.0 connection cap + scheduled restart ──
    // Both default to 0 = "off", which is exactly the pre-v1.2 behaviour: an
    // upgraded panel must not start capping or restarting anything until the
    // operator opts a rule in. (0 means unlimited, NOT "admit zero" — the node
    // maps 0 → None; see build_listeners_for_rule.)
    add_column_if_missing(
        pool,
        "forward_rules",
        "max_connections",
        "INTEGER NOT NULL DEFAULT 0",
    )
    .await?;
    add_column_if_missing(
        pool,
        "forward_rules",
        "auto_restart_minutes",
        "INTEGER NOT NULL DEFAULT 0",
    )
    .await?;
    tracing::info!("Migration 38: forward_rules.max_connections + auto_restart_minutes present");

    // ── Migration 39: v1.2.0 redeem codes (balance top-up) ──
    // See the table comment in SCHEMA_SQL for why used_by is SET NULL rather
    // than CASCADE, and why redemption is a conditional UPDATE.
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS redeem_codes (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            code TEXT NOT NULL UNIQUE,
            amount TEXT NOT NULL,
            status TEXT NOT NULL DEFAULT 'unused',
            used_by INTEGER REFERENCES users(id) ON DELETE SET NULL,
            used_at TEXT,
            expires_at TEXT,
            batch_id TEXT NOT NULL DEFAULT '',
            remark TEXT NOT NULL DEFAULT '',
            created_at TEXT NOT NULL DEFAULT (datetime('now'))
        )",
    )
    .execute(pool)
    .await?;
    for idx in [
        "CREATE INDEX IF NOT EXISTS idx_redeem_codes_status ON redeem_codes(status)",
        "CREATE INDEX IF NOT EXISTS idx_redeem_codes_batch ON redeem_codes(batch_id)",
        "CREATE INDEX IF NOT EXISTS idx_redeem_codes_used_by ON redeem_codes(used_by)",
    ] {
        sqlx::query(idx).execute(pool).await?;
    }
    tracing::info!("Migration 39: redeem_codes table present");

    // ── Migration 40: v1.2.0 hourly traffic history ──
    // See the SCHEMA_SQL comment for why there is no FK and why one row per
    // (rule, hour).
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS traffic_history (
            rule_id INTEGER NOT NULL,
            uid INTEGER NOT NULL,
            hour_ts TEXT NOT NULL,
            real_upload INTEGER NOT NULL DEFAULT 0,
            real_download INTEGER NOT NULL DEFAULT 0,
            billed_total INTEGER NOT NULL DEFAULT 0,
            PRIMARY KEY (rule_id, hour_ts)
        )",
    )
    .execute(pool)
    .await?;
    for idx in [
        "CREATE INDEX IF NOT EXISTS idx_traffic_history_uid ON traffic_history(uid, hour_ts)",
        "CREATE INDEX IF NOT EXISTS idx_traffic_history_hour ON traffic_history(hour_ts)",
    ] {
        sqlx::query(idx).execute(pool).await?;
    }
    tracing::info!("Migration 40: traffic_history table present");

    // ── Migration 41: v1.2.0 traffic_history.group_id (traffic per line) ──
    // See SCHEMA_SQL for why the group is stored as a snapshot rather than
    // joined at query time.
    add_column_if_missing(
        pool,
        "traffic_history",
        "group_id",
        "INTEGER NOT NULL DEFAULT 0",
    )
    .await?;
    sqlx::query(
        "CREATE INDEX IF NOT EXISTS idx_traffic_history_group \
         ON traffic_history(group_id, hour_ts)",
    )
    .execute(pool)
    .await?;
    // Backfill rows written before this column existed. Their rule usually
    // still exists, so the attribution is recoverable — without this, every
    // pre-upgrade hour would show up as "unknown line" on the chart. Rows whose
    // rule is already gone keep 0; that history is unattributable by
    // construction and inventing a group for it would be a lie.
    let backfilled = sqlx::query(
        "UPDATE traffic_history SET group_id = ( \
             SELECT fr.device_group_in FROM forward_rules fr \
             WHERE fr.id = traffic_history.rule_id) \
         WHERE group_id = 0 \
           AND EXISTS (SELECT 1 FROM forward_rules fr WHERE fr.id = traffic_history.rule_id)",
    )
    .execute(pool)
    .await?
    .rows_affected();
    tracing::info!(
        "Migration 41: traffic_history.group_id present ({} row(s) backfilled)",
        backfilled
    );

    Ok(())
}

/// Idempotent `ALTER TABLE ADD COLUMN`. SQLite has no IF NOT EXISTS for ADD
/// COLUMN, so we check `pragma_table_info` first — exactly the pattern used by
/// Migrations 1 & 4. Returns Ok(()) whether the column was just added or was
/// already present.
///
/// `type_def` is the full column definition after the name, e.g.
/// `"TEXT"` or `"INTEGER REFERENCES tunnel_profiles(id)"`.
async fn add_column_if_missing(
    pool: &sqlx::SqlitePool,
    table: &str,
    column: &str,
    type_def: &str,
) -> Result<(), sqlx::Error> {
    // pragma_table_info is parameterised by table name via the bound argument.
    // A column count of 0 means EITHER the column is absent (the normal case
    // → add it) OR the TABLE itself is absent (e.g. an ancient test DB that
    // pre-dates the table). In the latter case there's nothing to ALTER — the
    // table will be created elsewhere (SCHEMA_SQL on fresh boot) and the
    // column ships with it. Skip rather than error so migrations stay
    // idempotent on minimal test schemas.
    let count_sql = format!(
        "SELECT COUNT(*) FROM pragma_table_info('{}') WHERE name = ?",
        table
    );
    let exists: (i64,) = sqlx::query_as(&count_sql)
        .bind(column)
        .fetch_one(pool)
        .await?;

    if exists.0 == 0 {
        // Is the table itself present? If not, there's nothing to ALTER.
        let table_present: (i64,) = sqlx::query_as(&format!(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = '{}'",
            table
        ))
        .fetch_one(pool)
        .await?;
        if table_present.0 == 0 {
            return Ok(());
        }
        // NOTE: table/column/type_def are all compile-time literals from this
        // file — never user input — so the formatted SQL is safe from injection.
        let sql = format!("ALTER TABLE {} ADD COLUMN {} {}", table, column, type_def);
        sqlx::query(&sql).execute(pool).await?;
        tracing::info!("Migration: added {}.{} column", table, column);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::sqlite::SqlitePoolOptions;
    use sqlx::SqlitePool;

    /// Build a fresh in-memory database and run SCHEMA_SQL + migrations the way
    /// `init_db` does. This is the "new deployment" path — every v0.3.0 column
    /// should already exist from SCHEMA_SQL, and migrations must be no-ops.
    async fn fresh_pool() -> SqlitePool {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("connect memory db");
        sqlx::query(SCHEMA_SQL)
            .execute(&pool)
            .await
            .expect("schema");
        run_migrations(&pool).await.expect("migrations on fresh db");
        pool
    }

    /// Assert a column exists on a table (via pragma_table_info).
    async fn assert_column(pool: &SqlitePool, table: &str, column: &str) {
        let sql = format!(
            "SELECT COUNT(*) FROM pragma_table_info('{}') WHERE name = ?",
            table
        );
        let (n,): (i64,) = sqlx::query_as(&sql)
            .bind(column)
            .fetch_one(pool)
            .await
            .expect("pragma query");
        assert_eq!(
            n, 1,
            "expected column {}.{} to exist after migration",
            table, column
        );
    }

    /// Migration 38 must reach BOTH a fresh database (via SCHEMA_SQL) and an
    /// upgraded one (via ADD COLUMN), and existing rules must come out
    /// UNCAPPED.
    ///
    /// The value here is not "does ADD COLUMN work" — it's the default. 0 means
    /// unlimited, and the node maps 0 → None. If this ever defaulted to
    /// something the node read as a real cap, upgrading the panel would
    /// throttle every pre-existing rule to that number without anyone asking.
    #[tokio::test]
    async fn migration_38_defaults_existing_rules_to_uncapped_and_no_autorestart() {
        let pool = fresh_pool().await;
        assert_column(&pool, "forward_rules", "max_connections").await;
        assert_column(&pool, "forward_rules", "auto_restart_minutes").await;

        // Simulate a rule that predates v1.2: insert without mentioning either
        // new column, exactly as an older INSERT would have.
        sqlx::query(
            "INSERT INTO users (id, username, password) VALUES (1, 'u', 'h')
             ON CONFLICT DO NOTHING",
        )
        .execute(&pool)
        .await
        .expect("seed user");
        sqlx::query(
            "INSERT INTO device_groups (id, name, group_type, token, uid) \
             VALUES (1, 'g', 'in', 'tok', 1) ON CONFLICT DO NOTHING",
        )
        .execute(&pool)
        .await
        .expect("seed device group");
        sqlx::query(
            "INSERT INTO forward_rules (name, uid, listen_port, device_group_in, \
             target_addr, target_port) VALUES ('legacy', 1, 10001, 1, '1.2.3.4', 80)",
        )
        .execute(&pool)
        .await
        .expect("insert legacy rule");

        let (cap, restart): (i64, i64) = sqlx::query_as(
            "SELECT max_connections, auto_restart_minutes FROM forward_rules WHERE name = 'legacy'",
        )
        .fetch_one(&pool)
        .await
        .expect("read back");
        assert_eq!(cap, 0, "a pre-v1.2 rule must be UNCAPPED (0), never capped");
        assert_eq!(restart, 0, "a pre-v1.2 rule must not start auto-restarting");

        // Re-running migrations must not disturb the row (idempotency).
        run_migrations(&pool).await.expect("re-run migrations");
        let (cap, _): (i64, i64) = sqlx::query_as(
            "SELECT max_connections, auto_restart_minutes FROM forward_rules WHERE name = 'legacy'",
        )
        .fetch_one(&pool)
        .await
        .expect("read back after re-run");
        assert_eq!(
            cap, 0,
            "re-running migrations must not change existing data"
        );
    }

    /// All v0.3.0-alpha columns must be present on a fresh database (sourced
    /// from SCHEMA_SQL — the migration ADD COLUMN path must not be needed for
    /// a new deployment, but the columns must exist regardless).
    #[tokio::test]
    async fn fresh_db_has_all_v030_columns() {
        let pool = fresh_pool().await;

        // forward_rules additions
        for col in [
            "tunnel_profile_id",
            "domain",
            "ws_path",
            "ws_host",
            "sni",
            "entry_transport",
            // v0.4.0 three-field transport split
            "public_transport",
            "node_transport",
            "route_mode",
        ] {
            assert_column(&pool, "forward_rules", col).await;
        }
        // device_groups additions
        for col in ["capabilities", "region", "line_type", "remark"] {
            assert_column(&pool, "device_groups", col).await;
        }
        // tunnel_profiles table itself exists
        let (n,): (i64,) = sqlx::query_as(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='tunnel_profiles'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(n, 1, "tunnel_profiles table must exist");
    }

    /// The six builtin tunnel profiles must be seeded (is_builtin=1), owned by
    /// the default admin (uid=1). This is the fallback set every chain-mode
    /// rule depends on.
    #[tokio::test]
    async fn builtin_tunnel_profiles_seeded() {
        let pool = fresh_pool().await;
        let (n,): (i64,) =
            sqlx::query_as("SELECT COUNT(*) FROM tunnel_profiles WHERE is_builtin = 1")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(
            n, 3,
            "expected exactly 3 builtin tunnel profiles after v0.4.7 chain removal \
             (direct, ws-relay, tls-simple; chain/tls-passthrough/tls-terminate removed)"
        );

        // The 'direct' profile is the universal fallback for non-chain rules —
        // it MUST exist by name.
        let (has_direct,): (i64,) =
            sqlx::query_as("SELECT COUNT(*) FROM tunnel_profiles WHERE name='direct'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(has_direct, 1);
    }

    /// Migrations must be idempotent: running them again on an already-migrated
    /// database is a no-op (no error, no duplicate seed rows). This guards
    /// against the panel crashing on its second startup.
    #[tokio::test]
    async fn migrations_are_idempotent() {
        let pool = fresh_pool().await;
        let before: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM tunnel_profiles")
            .fetch_one(&pool)
            .await
            .unwrap();

        // Re-run on the already-migrated database.
        run_migrations(&pool).await.expect("second migration run");

        let after: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM tunnel_profiles")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(
            before, after,
            "second migration run must not duplicate seed rows"
        );
    }

    /// An "old" database that predates v0.3.0 (has forward_rules + device_groups
    /// but NONE of the v0.3.0 columns) must be fully upgraded by run_migrations.
    /// This simulates upgrading an existing v0.2.x deployment in place.
    ///
    /// Each DDL/DML statement is run separately — sqlx executes the first
    /// statement in a multi-statement string only when foreign_keys is on, so
    /// splitting removes any ambiguity about what ran.
    #[tokio::test]
    async fn old_db_gets_upgraded_with_all_v030_columns() {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        // Minimal "v0.2.x" schema WITHOUT any v0.3.0 columns, WITHOUT
        // entry_transport (simulating a pre-0.1.9 DB), and WITH the CHECK
        // constraint on group_type that Migration 3 removes — proving the full
        // chain runs together.
        for stmt in [
            r#"CREATE TABLE users (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                username TEXT NOT NULL UNIQUE,
                password TEXT NOT NULL,
                balance TEXT NOT NULL DEFAULT '0',
                plan_id INTEGER,
                group_id INTEGER,
                max_rules INTEGER NOT NULL DEFAULT 5,
                speed_limit INTEGER NOT NULL DEFAULT 0,
                ip_limit INTEGER NOT NULL DEFAULT 3,
                traffic_used INTEGER NOT NULL DEFAULT 0,
                traffic_limit INTEGER NOT NULL DEFAULT 0,
                admin INTEGER NOT NULL DEFAULT 0,
                banned INTEGER NOT NULL DEFAULT 0,
                created_at TEXT NOT NULL DEFAULT (datetime('now'))
            )"#,
            r#"CREATE TABLE device_groups (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                name TEXT NOT NULL,
                group_type TEXT NOT NULL CHECK(group_type IN ('in','out')),
                token TEXT NOT NULL UNIQUE,
                uid INTEGER NOT NULL REFERENCES users(id),
                connect_host TEXT NOT NULL DEFAULT '',
                port_range TEXT NOT NULL DEFAULT '1-65535',
                fallback_group INTEGER REFERENCES device_groups(id),
                config TEXT NOT NULL DEFAULT '{}',
                created_at TEXT NOT NULL DEFAULT (datetime('now'))
            )"#,
            r#"CREATE TABLE forward_rules (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                name TEXT NOT NULL,
                uid INTEGER NOT NULL REFERENCES users(id),
                paused INTEGER NOT NULL DEFAULT 0,
                listen_port INTEGER NOT NULL,
                protocol TEXT NOT NULL DEFAULT 'tcp',
                device_group_in INTEGER NOT NULL REFERENCES device_groups(id),
                device_group_out INTEGER NOT NULL REFERENCES device_groups(id),
                forward_mode TEXT NOT NULL DEFAULT 'group',
                target_addr TEXT NOT NULL,
                target_port INTEGER NOT NULL,
                config TEXT NOT NULL DEFAULT '{}',
                traffic_used INTEGER NOT NULL DEFAULT 0,
                status TEXT NOT NULL DEFAULT 'active',
                created_at TEXT NOT NULL DEFAULT (datetime('now'))
            )"#,
            "INSERT INTO users (id, username, password, admin) VALUES (1, 'admin', 'x', 1)",
        ] {
            sqlx::query(stmt).execute(&pool).await.unwrap();
        }

        // Insert one pre-existing rule + group so we can verify data survives
        // migration. Device_groups before forward_rules (FK order).
        sqlx::query(
            r#"INSERT INTO device_groups (id, name, group_type, token, uid, connect_host, port_range)
               VALUES (1, 'g1', 'in', 'tok1', 1, '10.0.0.1', '10000-20000')"#,
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            r#"INSERT INTO forward_rules (name, uid, listen_port, protocol, device_group_in, device_group_out, target_addr, target_port)
               VALUES ('r1', 1, 12345, 'tcp', 1, 1, '9.9.9.9', 53)"#,
        )
        .execute(&pool)
        .await
        .unwrap();

        // Now run the full migration suite on this old DB. FK is OFF during
        // migration (Migrations 2/3 rebuild tables referenced by FKs — SQLite
        // forbids dropping a referenced table while enforcement is on). The
        // production init_db path does the same; here we mirror it for the test.
        sqlx::query("PRAGMA foreign_keys = OFF")
            .execute(&pool)
            .await
            .unwrap();
        run_migrations(&pool).await.expect("migrate old db");
        sqlx::query("PRAGMA foreign_keys = ON")
            .execute(&pool)
            .await
            .unwrap();

        // Every v0.3.0 + v0.4.0 column must now exist.
        for (table, col) in [
            ("forward_rules", "entry_transport"),
            ("forward_rules", "tunnel_profile_id"),
            ("forward_rules", "domain"),
            ("forward_rules", "ws_path"),
            ("forward_rules", "ws_host"),
            ("forward_rules", "sni"),
            ("forward_rules", "public_transport"),
            ("forward_rules", "node_transport"),
            ("forward_rules", "route_mode"),
            ("device_groups", "capabilities"),
            ("device_groups", "region"),
            ("device_groups", "line_type"),
            ("device_groups", "remark"),
        ] {
            assert_column(&pool, table, col).await;
        }

        // The pre-existing rule must survive with its defaults backfilled:
        // entry_transport='raw' (Migration 4 default), public/node_transport
        // also 'raw' (v0.4.0 defaults — a raw rule needs no backfill UPDATE),
        // tunnel_profile_id=NULL, capabilities default on the group.
        let rule: (String, String, String, Option<i64>) = sqlx::query_as(
            "SELECT entry_transport, public_transport, node_transport, tunnel_profile_id \
             FROM forward_rules WHERE name='r1'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(rule.0, "raw", "existing rule must default to raw transport");
        assert_eq!(rule.1, "raw", "public_transport defaults to raw");
        assert_eq!(rule.2, "raw", "node_transport defaults to raw");
        assert!(
            rule.3.is_none(),
            "existing rule must have NULL tunnel_profile_id"
        );

        // device_groups.capabilities must be backfilled to the tcp/udp default.
        let caps: (String,) = sqlx::query_as("SELECT capabilities FROM device_groups WHERE id=1")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(caps.0, r#"["tcp","udp"]"#);

        // The CHECK constraint on group_type must be gone (Migration 3), so a
        // new group type insert must succeed.
        sqlx::query(
            "INSERT INTO device_groups (name, group_type, token, uid) VALUES ('mon','monitor','tok2',1)",
        )
        .execute(&pool)
        .await
        .expect("group_type CHECK should be removed post-migration");
    }

    /// v0.4.11 PR4: forward_rules port occupancy is per (device_group_in,
    /// listen_port, socket type) on a fresh DB. The partial unique indexes must:
    ///   - reject a second TCP-bearing rule on the SAME group + same port,
    ///   - ALLOW the same port on a DIFFERENT group,
    ///   - ALLOW a pure-UDP rule to share the port a pure-TCP rule holds.
    #[tokio::test]
    async fn fresh_db_enforces_per_group_port_occupancy() {
        let pool = fresh_pool().await;
        // Two groups so we can test cross-group reuse.
        for i in 1..=2 {
            sqlx::query("INSERT INTO device_groups (id, name, group_type, token, uid) VALUES (?, 'g', 'in', ?, 1)")
                .bind(i)
                .bind(format!("tok{i}"))
                .execute(&pool)
                .await
                .unwrap();
        }
        // First rule: group 1, port 12345, tcp — OK.
        sqlx::query("INSERT INTO forward_rules (name, uid, listen_port, protocol, device_group_in, target_addr, target_port) VALUES ('r1', 1, 12345, 'tcp', 1, '1.1.1.1', 53)")
            .execute(&pool)
            .await
            .expect("first tcp insert on group 1 / 12345 should succeed");

        // Same group + same port + TCP-bearing again → UNIQUE violation (2067).
        let dup = sqlx::query("INSERT INTO forward_rules (name, uid, listen_port, protocol, device_group_in, target_addr, target_port) VALUES ('r2', 1, 12345, 'tcp', 1, '2.2.2.2', 53)")
            .execute(&pool)
            .await;
        assert!(
            dup.is_err(),
            "same group + same port + TCP must be rejected by the partial unique index"
        );
        let err = dup.unwrap_err().into_database_error().unwrap();
        assert_eq!(
            err.code().as_deref(),
            Some("2067"),
            "expected SQLITE_CONSTRAINT_UNIQUE (2067)"
        );

        // DIFFERENT group, same port, tcp → allowed (independent port pool).
        sqlx::query("INSERT INTO forward_rules (name, uid, listen_port, protocol, device_group_in, target_addr, target_port) VALUES ('r3', 1, 12345, 'tcp', 2, '3.3.3.3', 53)")
            .execute(&pool)
            .await
            .expect("same port on a different group must be allowed");

        // SAME group + same port but PURE UDP → allowed (different socket type).
        sqlx::query("INSERT INTO forward_rules (name, uid, listen_port, protocol, device_group_in, target_addr, target_port) VALUES ('r4', 1, 12345, 'udp', 1, '4.4.4.4', 53)")
            .execute(&pool)
            .await
            .expect("pure-UDP may share the port a pure-TCP rule holds");

        // SAME group + same port + UDP again → UNIQUE violation on the udp index.
        let dup_udp = sqlx::query("INSERT INTO forward_rules (name, uid, listen_port, protocol, device_group_in, target_addr, target_port) VALUES ('r5', 1, 12345, 'udp', 1, '5.5.5.5', 53)")
            .execute(&pool)
            .await;
        assert!(
            dup_udp.is_err(),
            "same group + same port + UDP must be rejected by the udp partial index"
        );
    }

    /// Migration 16 safety: if an existing DB already has DUPLICATE listen_port
    /// rows, the migration must NOT delete data. It skips the index creation and
    /// logs instead, so the operator resolves duplicates manually.
    #[tokio::test]
    async fn migration_skips_index_when_duplicates_exist() {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        // Build an "old" DB with a duplicate listen_port (the very situation the
        // UNIQUE index is meant to prevent going forward).
        for stmt in [
            r#"CREATE TABLE users (id INTEGER PRIMARY KEY, username TEXT UNIQUE, password TEXT, admin INTEGER DEFAULT 0)"#,
            r#"CREATE TABLE device_groups (id INTEGER PRIMARY KEY, name TEXT, group_type TEXT, token TEXT UNIQUE, uid INTEGER)"#,
            r#"CREATE TABLE forward_rules (id INTEGER PRIMARY KEY, name TEXT, uid INTEGER, listen_port INTEGER, protocol TEXT, device_group_in INTEGER, target_addr TEXT, target_port INTEGER)"#,
            "INSERT INTO users (id, username, password, admin) VALUES (1,'a','x',1)",
        ] {
            sqlx::query(stmt).execute(&pool).await.unwrap();
        }
        // Two rules sharing listen_port 5000 — the duplicate.
        sqlx::query("INSERT INTO device_groups (id, name, group_type, token, uid) VALUES (1,'g','in','t',1)")
            .execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO forward_rules (name, uid, listen_port, protocol, device_group_in, target_addr, target_port) VALUES ('r1',1,5000,'tcp',1,'1.1.1.1',53)")
            .execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO forward_rules (name, uid, listen_port, protocol, device_group_in, target_addr, target_port) VALUES ('r2',1,5000,'tcp',1,'2.2.2.2',53)")
            .execute(&pool).await.unwrap();

        // Run migrations with FK off (same as production init_db does).
        sqlx::query("PRAGMA foreign_keys = OFF")
            .execute(&pool)
            .await
            .unwrap();
        run_migrations(&pool)
            .await
            .expect("migration must not error");
        sqlx::query("PRAGMA foreign_keys = ON")
            .execute(&pool)
            .await
            .unwrap();

        // BOTH duplicate rows must still exist — migration did NOT delete data.
        let (count,): (i64,) =
            sqlx::query_as("SELECT COUNT(*) FROM forward_rules WHERE listen_port = 5000")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(
            count, 2,
            "duplicate rows must survive migration (no data loss)"
        );

        // And the UNIQUE index must NOT exist (it was skipped).
        let (idx_count,): (i64,) = sqlx::query_as(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='index' AND name='idx_forward_rules_listen_port'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(
            idx_count, 0,
            "UNIQUE index must be skipped when duplicates exist"
        );
    }

    // ── Password-reset regression guards (audit item #3) ──
    // These pin the safety contract of init's password hashing: an admin who
    // changed their password must NEVER have it reverted by a restart, a
    // migration, or a container rebuild. The hashing function is in init.rs.

    /// The placeholder password IS replaced on first boot (the happy path — a
    /// fresh DB has the placeholder, init hashes it to a real bcrypt value).
    #[tokio::test]
    async fn placeholder_password_is_hashed_on_first_boot() {
        let pool = fresh_pool().await;
        // fresh_pool runs SCHEMA_SQL which seeds the placeholder.
        let before: (String,) = sqlx::query_as("SELECT password FROM users WHERE id=1")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert!(
            before.0.contains("PLACEHOLDER"),
            "fresh DB should still have the placeholder: {}",
            before.0
        );
        crate::db::init::hash_default_admin_password_if_placeholder(&pool)
            .await
            .unwrap();
        let after: (String,) = sqlx::query_as("SELECT password FROM users WHERE id=1")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert!(
            !after.0.contains("PLACEHOLDER"),
            "placeholder must be replaced with a real hash"
        );
        assert!(after.0.starts_with("$2b$12$"), "should be a bcrypt hash");
        // The seeded default password must force a change on first login.
        let mcp: (i64,) = sqlx::query_as("SELECT must_change_password FROM users WHERE id=1")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(
            mcp.0, 1,
            "first-boot default admin password must set must_change_password"
        );
    }

    /// THE critical safety test: an admin-changed password must survive init.
    /// We set a realistic bcrypt hash (NOT the placeholder), call the hashing
    /// function, and assert the password is byte-for-byte unchanged. If this
    /// ever fails, init is silently resetting changed passwords — a severe bug.
    #[tokio::test]
    async fn password_survives_init_when_changed() {
        let pool = fresh_pool().await;
        // Simulate "admin changed their password" with a real-looking bcrypt hash.
        let changed_hash = "$2b$12$abcdefghijklmnopqrstuvwxyz0123456789ABCDEFGHIJKLMNOPQR";
        sqlx::query("UPDATE users SET password = ? WHERE id = 1")
            .bind(changed_hash)
            .execute(&pool)
            .await
            .unwrap();
        // Run the init-time hashing (what every panel boot does).
        crate::db::init::hash_default_admin_password_if_placeholder(&pool)
            .await
            .unwrap();
        let after: (String,) = sqlx::query_as("SELECT password FROM users WHERE id=1")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(
            after.0, changed_hash,
            "an admin-changed password must NEVER be reset by init"
        );
        // And init must NOT flag an already-changed account for a forced change:
        // the LIKE guard doesn't match a real hash, so must_change_password stays
        // at its default (0).
        let mcp: (i64,) = sqlx::query_as("SELECT must_change_password FROM users WHERE id=1")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(
            mcp.0, 0,
            "init must not force a password change on an already-changed account"
        );
    }

    /// Idempotency: calling the hashing function twice must not re-hash (the
    /// second call should be a no-op since the placeholder is already gone).
    #[tokio::test]
    async fn password_hashing_is_idempotent() {
        let pool = fresh_pool().await;
        crate::db::init::hash_default_admin_password_if_placeholder(&pool)
            .await
            .unwrap();
        let after_first: (String,) = sqlx::query_as("SELECT password FROM users WHERE id=1")
            .fetch_one(&pool)
            .await
            .unwrap();
        // Second call — must not change anything.
        crate::db::init::hash_default_admin_password_if_placeholder(&pool)
            .await
            .unwrap();
        let after_second: (String,) = sqlx::query_as("SELECT password FROM users WHERE id=1")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(
            after_first.0, after_second.0,
            "re-running init must not re-hash"
        );
    }

    /// v0.4.7 Migration 22: chain removal + tls template consolidation.
    /// Verifies the migration on a DB that starts in the pre-v0.4.7 state:
    /// chain builtin present, a chain rule active, a chained_outbound group,
    /// and the old tls-passthrough/terminate builtins.
    #[tokio::test]
    async fn migration_22_removes_chain_and_consolidates_tls() {
        let pool = fresh_pool().await;
        // fresh_pool already ran all migrations, so the post-v0.4.7 state is in
        // place. Re-introduce the pre-v0.4.7 rows to simulate an upgrading DB.
        sqlx::query(
            "INSERT INTO tunnel_profiles (name, transport, tls_mode, ws_path, host_header, sni, is_builtin, uid) \
             VALUES ('chain','chain','none','','','',1,1), \
                    ('tls-passthrough','tls','passthrough','','','',1,1), \
                    ('tls-terminate','tls','terminate','','','',1,1)",
        )
        .execute(&pool)
        .await
        .unwrap();
        // A pre-v0.4.7 chain rule, currently active.
        sqlx::query(
            "INSERT INTO device_groups (id, name, group_type, token, uid) VALUES (1, 'gin', 'in', 'tok-1', 1)",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO forward_rules (name, uid, listen_port, protocol, public_transport, node_transport, \
             entry_transport, route_mode, forward_mode, target_addr, target_port, device_group_in, paused) \
             VALUES ('legacy-chain', 1, 30001, 'tcp', 'raw', 'raw', 'raw', 'chain', 'chain', '127.0.0.1', 80, 1, 0)",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO device_groups (id, name, group_type, token, uid) \
             VALUES (2, 'g-chain', 'chained_outbound', 'tok-2', 1)",
        )
        .execute(&pool)
        .await
        .unwrap();

        // Run migrations again — Migration 22 must rewrite the state.
        run_migrations(&pool).await.expect("re-run migrations");

        // 1. The chain rule is now paused (NOT rewritten to direct).
        let chain_rule: (i64,) =
            sqlx::query_as("SELECT paused FROM forward_rules WHERE name='legacy-chain'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(
            chain_rule.0, 1,
            "chain rule must be paused, not silently switched to direct"
        );
        let rm: (String,) =
            sqlx::query_as("SELECT route_mode FROM forward_rules WHERE name='legacy-chain'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(
            rm.0, "chain",
            "route_mode column is NOT rewritten (only paused)"
        );

        // 2. chained_outbound group → out.
        let gt: (String,) = sqlx::query_as("SELECT group_type FROM device_groups WHERE id=2")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(gt.0, "out");

        // 3. Dead builtins gone; canonical tls-simple present.
        let dead: (i64,) = sqlx::query_as(
            "SELECT COUNT(*) FROM tunnel_profiles WHERE name IN ('chain','tls-passthrough','tls-terminate')",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(dead.0, 0);
        let tls_simple: (i64,) =
            sqlx::query_as("SELECT COUNT(*) FROM tunnel_profiles WHERE name='tls-simple' AND transport='tls_simple'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(tls_simple.0, 1);

        // 4. Idempotent: running again changes nothing.
        run_migrations(&pool).await.expect("third migration run");
        let still_paused: (i64,) =
            sqlx::query_as("SELECT paused FROM forward_rules WHERE name='legacy-chain'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(still_paused.0, 1);
    }
}
