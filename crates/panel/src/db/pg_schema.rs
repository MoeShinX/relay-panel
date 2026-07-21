// v0.4.3 PR2: PostgreSQL schema — the PG equivalent of `schema::SCHEMA_SQL`.
//
// This is the schema for a FRESH PostgreSQL database. There is no migration
// suite here (unlike SQLite's `run_migrations`) because PostgreSQL support is
// new in v0.4.3 — there are no "old PG databases" to upgrade. The product
// contract is: the user installs PG, creates an empty database, fills in the
// connection string; the panel creates all tables + seed data on first boot.
// SQLite→PG data migration is the user's responsibility (not in scope).
//
// Translations from the SQLite SCHEMA_SQL:
//   - INTEGER PRIMARY KEY AUTOINCREMENT → BIGSERIAL PRIMARY KEY (id is i64)
//   - TEXT → TEXT (PG TEXT is unbounded, same semantics)
//   - INTEGER used as boolean (admin/banned/paused/is_builtin) → BOOLEAN
//   - DEFAULT (datetime('now')) → DEFAULT to_char(now() AT TIME ZONE 'UTC', ...)
//     (both backends store created_at as TEXT in 'YYYY-MM-DD HH:MM:SS' format
//     so the column type and the on-wire representation are identical)
//   - INSERT OR IGNORE → INSERT ... ON CONFLICT DO NOTHING
//   - CREATE [UNIQUE] INDEX IF NOT EXISTS → same syntax (PG supports it)
//   - REFERENCES (FK) → same syntax (PG enforces FK by default, no PRAGMA toggle)
//
// Table ordering: PG requires a referenced table to exist before the FK is
// declared (unlike SQLite, which resolves FKs lazily). The order below is the
// topological sort: plans → users → device_groups → tunnel_profiles →
// forward_rules. statistics and kvs are independent.

pub const PG_SCHEMA_SQL: &str = r#"
CREATE TABLE IF NOT EXISTS plans (
    id BIGSERIAL PRIMARY KEY,
    name TEXT NOT NULL,
    max_rules INTEGER NOT NULL DEFAULT 5,
    traffic BIGINT NOT NULL DEFAULT 0,
    speed_limit INTEGER NOT NULL DEFAULT 0,
    ip_limit INTEGER NOT NULL DEFAULT 3,
    price TEXT NOT NULL DEFAULT '0',
    -- v1.0.8: plan lifecycle + visibility (mirrors SQLite baseline + Migration 34).
    plan_type TEXT NOT NULL DEFAULT 'data',
    duration_days INTEGER NOT NULL DEFAULT 0,
    hidden BOOLEAN NOT NULL DEFAULT FALSE,
    reset_traffic BOOLEAN NOT NULL DEFAULT FALSE,
    description TEXT NOT NULL DEFAULT '',
    -- v1.0.9: grant ALL inbound groups on purchase (mirrors SQLite Migration 35).
    grant_all_groups BOOLEAN NOT NULL DEFAULT FALSE,
    created_at TEXT NOT NULL DEFAULT (to_char(now() AT TIME ZONE 'UTC', 'YYYY-MM-DD HH24:MI:SS'))
);

CREATE TABLE IF NOT EXISTS users (
    id BIGSERIAL PRIMARY KEY,
    username TEXT NOT NULL UNIQUE,
    password TEXT NOT NULL,
    balance TEXT NOT NULL DEFAULT '0',
    plan_id BIGINT REFERENCES plans(id),
    -- v1.0.7: replaces group_id. TRUE = user may use ALL device groups; FALSE =
    -- limited to user_device_groups (none = cannot forward). Admins always all.
    all_device_groups BOOLEAN NOT NULL DEFAULT FALSE,
    max_rules INTEGER NOT NULL DEFAULT 5,
    speed_limit INTEGER NOT NULL DEFAULT 0,
    ip_limit INTEGER NOT NULL DEFAULT 3,
    traffic_used BIGINT NOT NULL DEFAULT 0,
    traffic_limit BIGINT NOT NULL DEFAULT 0,
    admin BOOLEAN NOT NULL DEFAULT FALSE,
    banned BOOLEAN NOT NULL DEFAULT FALSE,
    created_at TEXT NOT NULL DEFAULT (to_char(now() AT TIME ZONE 'UTC', 'YYYY-MM-DD HH24:MI:SS')),
    -- v0.4.10 PR4: force-password-change flag + JWT session-version counter
    -- (see schema.rs for the rationale). token_version is BIGINT to match the
    -- i64 the JWT carries.
    must_change_password BOOLEAN NOT NULL DEFAULT FALSE,
    token_version BIGINT NOT NULL DEFAULT 0,
    -- v1.0.8: plan expiry (TEXT 'YYYY-MM-DD HH:MM:SS' UTC, NULL = no expiry)
    -- and admin suspension. Mirrors SQLite baseline + Migration 34.
    plan_expire_at TEXT,
    suspended BOOLEAN NOT NULL DEFAULT FALSE
);

CREATE TABLE IF NOT EXISTS device_groups (
    id BIGSERIAL PRIMARY KEY,
    name TEXT NOT NULL,
    group_type TEXT NOT NULL,
    token TEXT NOT NULL UNIQUE,
    uid BIGINT NOT NULL REFERENCES users(id),
    connect_host TEXT NOT NULL DEFAULT '',
    port_range TEXT NOT NULL DEFAULT '1-65535',
    fallback_group BIGINT REFERENCES device_groups(id),
    config TEXT NOT NULL DEFAULT '{}',
    capabilities TEXT NOT NULL DEFAULT '["tcp","udp"]',
    region TEXT,
    line_type TEXT,
    remark TEXT,
    -- v1.0.8: traffic billing multiplier for this line (REAL NOT NULL DEFAULT
    -- 1.0). Mirrors SQLite Migration 33 / baseline. Range 0.1..=100 enforced
    -- at the API.
    rate DOUBLE PRECISION NOT NULL DEFAULT 1.0,
    -- v1.0.7: hide from regular users' shared views (mirrors SQLite Migration 36).
    hidden BOOLEAN NOT NULL DEFAULT FALSE,
    created_at TEXT NOT NULL DEFAULT (to_char(now() AT TIME ZONE 'UTC', 'YYYY-MM-DD HH24:MI:SS'))
);

CREATE TABLE IF NOT EXISTS tunnel_profiles (
    id              BIGSERIAL PRIMARY KEY,
    name            TEXT NOT NULL UNIQUE,
    transport       TEXT NOT NULL DEFAULT 'direct',
    tls_mode        TEXT NOT NULL DEFAULT 'none',
    ws_path         TEXT NOT NULL DEFAULT '/relay',
    host_header     TEXT NOT NULL DEFAULT '',
    sni             TEXT NOT NULL DEFAULT '',
    cert_id         BIGINT,
    is_builtin      BOOLEAN NOT NULL DEFAULT FALSE,
    uid             BIGINT NOT NULL REFERENCES users(id),
    created_at      TEXT NOT NULL DEFAULT (to_char(now() AT TIME ZONE 'UTC', 'YYYY-MM-DD HH24:MI:SS'))
);

CREATE TABLE IF NOT EXISTS forward_rules (
    id BIGSERIAL PRIMARY KEY,
    name TEXT NOT NULL,
    uid BIGINT NOT NULL REFERENCES users(id),
    paused BOOLEAN NOT NULL DEFAULT FALSE,
    -- v1.0.8: 1 = system-auto-paused (buy_plan / plan removal revoking
    -- authorization), 0 = human-paused or never paused. Mirrors SQLite
    -- Migration 37. Lets a later re-authorization safely auto-resume only the
    -- rules IT paused.
    auto_paused BOOLEAN NOT NULL DEFAULT FALSE,
    listen_port INTEGER NOT NULL,
    protocol TEXT NOT NULL DEFAULT 'tcp',
    public_transport TEXT NOT NULL DEFAULT 'raw',
    node_transport TEXT NOT NULL DEFAULT 'raw',
    route_mode TEXT NOT NULL DEFAULT 'direct',
    entry_transport TEXT NOT NULL DEFAULT 'raw',
    device_group_in BIGINT NOT NULL REFERENCES device_groups(id),
    device_group_out BIGINT REFERENCES device_groups(id),
    forward_mode TEXT NOT NULL DEFAULT 'group',
    tunnel_profile_id BIGINT REFERENCES tunnel_profiles(id),
    domain TEXT,
    ws_path TEXT,
    ws_host TEXT,
    sni TEXT,
    target_addr TEXT NOT NULL,
    target_port INTEGER NOT NULL,
    load_balance_strategy TEXT NOT NULL DEFAULT 'first',
    upload_limit_mbps INTEGER NOT NULL DEFAULT 0,
    download_limit_mbps INTEGER NOT NULL DEFAULT 0,
    -- v1.2.0: cap on concurrent TCP connections, enforced PER NODE. 0 = unlimited.
    max_connections INTEGER NOT NULL DEFAULT 0,
    -- v1.2.0: restart the rule every N minutes to shed connections. 0 = off.
    auto_restart_minutes INTEGER NOT NULL DEFAULT 0,
    config TEXT NOT NULL DEFAULT '{}',
    traffic_used BIGINT NOT NULL DEFAULT 0,
    status TEXT NOT NULL DEFAULT 'active',
    created_at TEXT NOT NULL DEFAULT (to_char(now() AT TIME ZONE 'UTC', 'YYYY-MM-DD HH24:MI:SS'))
);

CREATE TABLE IF NOT EXISTS forward_rule_targets (
    id BIGSERIAL PRIMARY KEY,
    rule_id BIGINT NOT NULL REFERENCES forward_rules(id) ON DELETE CASCADE,
    host TEXT NOT NULL,
    port INTEGER NOT NULL CHECK (port >= 1 AND port <= 65535),
    position INTEGER NOT NULL CHECK (position >= 1),
    enabled BOOLEAN NOT NULL DEFAULT TRUE,
    created_at TEXT NOT NULL DEFAULT (to_char(now() AT TIME ZONE 'UTC', 'YYYY-MM-DD HH24:MI:SS'))
);

CREATE INDEX IF NOT EXISTS idx_forward_rule_targets_rule_position
    ON forward_rule_targets (rule_id, position);

CREATE TABLE IF NOT EXISTS statistics (
    id BIGSERIAL PRIMARY KEY,
    stat_type TEXT NOT NULL,
    stat_key TEXT NOT NULL,
    time TEXT NOT NULL,
    number BIGINT NOT NULL DEFAULT 0
);

-- v1.0.8: purchase history (mirrors SQLite baseline + Migration 34).
CREATE TABLE IF NOT EXISTS orders (
    id BIGSERIAL PRIMARY KEY,
    user_id BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    plan_id BIGINT,
    plan_name TEXT NOT NULL,
    price TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT (to_char(now() AT TIME ZONE 'UTC', 'YYYY-MM-DD HH24:MI:SS'))
);
CREATE INDEX IF NOT EXISTS idx_orders_user_id ON orders(user_id);

-- v1.2.0: redeem codes (balance top-up). Mirrors the SQLite baseline; see the
-- table comment there for why used_by is SET NULL (audit trail outlives the
-- account) and how the conditional-UPDATE redemption stays race-free.
CREATE TABLE IF NOT EXISTS redeem_codes (
    id BIGSERIAL PRIMARY KEY,
    code TEXT NOT NULL UNIQUE,
    amount TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'unused',
    used_by BIGINT REFERENCES users(id) ON DELETE SET NULL,
    used_at TEXT,
    expires_at TEXT,
    batch_id TEXT NOT NULL DEFAULT '',
    remark TEXT NOT NULL DEFAULT '',
    created_at TEXT NOT NULL DEFAULT (to_char(now() AT TIME ZONE 'UTC', 'YYYY-MM-DD HH24:MI:SS'))
);
CREATE INDEX IF NOT EXISTS idx_redeem_codes_status ON redeem_codes(status);
CREATE INDEX IF NOT EXISTS idx_redeem_codes_batch ON redeem_codes(batch_id);
CREATE INDEX IF NOT EXISTS idx_redeem_codes_used_by ON redeem_codes(used_by);

-- v1.0.9: plan ↔ device_group grant map (mirrors SQLite baseline + Migration 35).
CREATE TABLE IF NOT EXISTS plan_device_groups (
    plan_id BIGINT NOT NULL REFERENCES plans(id) ON DELETE CASCADE,
    device_group_id BIGINT NOT NULL REFERENCES device_groups(id) ON DELETE CASCADE,
    PRIMARY KEY (plan_id, device_group_id)
);

CREATE TABLE IF NOT EXISTS kvs (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL
);

-- v0.4.4: schema version tracking. PostgreSQL support shipped in v0.4.3 with no
-- migration mechanism — `CREATE TABLE IF NOT EXISTS` only creates missing tables
-- and can NEVER add a column to an existing one. This table records which schema
-- revision a database is at, so future releases can apply ordered, idempotent
-- migrations (ALTER TABLE ADD COLUMN, etc.) via `run_pg_migrations` instead of
-- silently leaving old databases stale. The baseline (everything above) is
-- revision 1. The row is seeded after the baseline schema is created.
CREATE TABLE IF NOT EXISTS schema_version (
    version INTEGER PRIMARY KEY,
    applied_at TEXT NOT NULL DEFAULT (to_char(now() AT TIME ZONE 'UTC', 'YYYY-MM-DD HH24:MI:SS'))
);

-- v0.4.11 PR4: shared-node port occupancy. Mirrors the SQLite partial indexes
-- (schema.rs). The old GLOBAL UNIQUE(listen_port) is replaced by two PARTIAL
-- unique indexes keyed on (device_group_in, listen_port), split by socket type
-- derived from `protocol`:
--   idx_fr_port_tcp → protocol IN ('tcp','tcp_udp')  (TCP socket)
--   idx_fr_port_udp → protocol IN ('udp','tcp_udp')  (UDP socket)
-- A fresh DB has no rows, so the indexes are always creatable. See migration
-- revision 11 (run_pg_migrations) for the upgrade path on existing DBs.
CREATE UNIQUE INDEX IF NOT EXISTS idx_fr_port_tcp
    ON forward_rules (device_group_in, listen_port)
    WHERE protocol IN ('tcp', 'tcp_udp');
CREATE UNIQUE INDEX IF NOT EXISTS idx_fr_port_udp
    ON forward_rules (device_group_in, listen_port)
    WHERE protocol IN ('udp', 'tcp_udp');

-- Default admin user (password: admin123, will be hashed on init via the
-- same UserRepository::replace_placeholder_admin_password path SQLite uses).
-- id=1 is pinned so FK references (device_groups.uid, forward_rules.uid, etc.)
-- resolve to a stable admin row across fresh deployments. ON CONFLICT (id) DO
-- NOTHING makes re-runs idempotent. admin=TRUE (PG boolean literal).
INSERT INTO users (id, username, password, admin, max_rules)
VALUES (1, 'admin', '$2b$12$PLACEHOLDER_WILL_BE_HASHED_ON_INIT', TRUE, 999)
ON CONFLICT (id) DO NOTHING;

-- Default plan. v1.1.0: seed ONLY when the plans table is empty (fresh DB), so
-- an admin who deletes the default plan doesn't see it reappear on every panel
-- update. (The old VALUES + ON CONFLICT DO NOTHING re-created it whenever id=1
-- was absent.)
INSERT INTO plans (id, name, max_rules, traffic, speed_limit, ip_limit, price)
SELECT 1, 'free', 5, 107374182400, 0, 3, '0'
WHERE NOT EXISTS (SELECT 1 FROM plans);

-- Builtin tunnel profiles (is_builtin=TRUE). Same five as the SQLite seed
-- (Migration 6 minus the deleted wss-via-caddy). name is UNIQUE so
-- ON CONFLICT (name) DO NOTHING makes this idempotent.
INSERT INTO tunnel_profiles (name, transport, tls_mode, ws_path, host_header, sni, is_builtin, uid)
VALUES
    ('direct',          'direct', 'none',        '',      '', '', TRUE, 1),
    ('ws-relay',        'ws',     'none',        '/relay','', '', TRUE, 1),
    ('tls-passthrough', 'tls',    'passthrough', '',      '', '', TRUE, 1),
    ('tls-terminate',   'tls',    'terminate',   '',      '', '', TRUE, 1),
    ('chain',           'chain',  'none',        '',      '', '', TRUE, 1)
ON CONFLICT (name) DO NOTHING;

-- Advance the BIGSERIAL sequences past the explicitly-seeded ids. The seed
-- rows above use literal id=1 (users, plans) so the FK references resolve to a
-- stable admin/plan. But inserting an explicit id does NOT advance the serial
-- sequence — it stays at 1. Without this, the next auto-id insert (e.g. the
-- first registered user) would generate id=1 and collide with the seed row's
-- primary key. setval to MAX(id) makes the next nextval return MAX+1.
-- pg_get_serial_sequence resolves the sequence name from the table/column so
-- we don't hardcode 'users_id_seq'. Idempotent: re-running on a populated DB
-- setval's to the current MAX, which is always correct.
-- tunnel_profiles is NOT setval'd here: its seed rows use auto-assigned ids
-- (no explicit id column), so its sequence already advanced normally.
SELECT setval(pg_get_serial_sequence('users', 'id'), (SELECT MAX(id) FROM users));
SELECT setval(pg_get_serial_sequence('plans', 'id'), (SELECT MAX(id) FROM plans));

-- v0.4.10 PR3: application settings (registration config). Single-row table.
-- The row is NOT seeded here — main.rs seeds it via insert_settings_if_absent.
CREATE TABLE IF NOT EXISTS app_settings (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    registration_enabled BOOLEAN NOT NULL DEFAULT FALSE,
    default_registration_plan_id BIGINT NOT NULL DEFAULT 1 REFERENCES plans(id),
    registration_allowed_plan_ids TEXT NOT NULL DEFAULT '[1]'
);

-- v1.0.7: per-user device-group authorization (replaces the user_groups layer).
CREATE TABLE IF NOT EXISTS user_device_groups (
    user_id BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    device_group_id BIGINT NOT NULL REFERENCES device_groups(id) ON DELETE CASCADE,
    PRIMARY KEY (user_id, device_group_id)
);

-- Record the baseline schema revision. ON CONFLICT DO NOTHING keeps re-runs
-- idempotent and never downgrades a database that later migrations advanced.
INSERT INTO schema_version (version) VALUES (1) ON CONFLICT (version) DO NOTHING;
"#;

/// The schema revision this build's baseline `PG_SCHEMA_SQL` represents. When a
/// future release adds a column/table, bump this and add a matching arm in
/// `run_pg_migrations`. `apply_pg_schema` seeds `schema_version` with revision 1.
pub const PG_SCHEMA_VERSION: i32 = 22;

/// Apply PG_SCHEMA_SQL to a pool. PostgreSQL's prepared-statement protocol
/// rejects multi-statement strings ("cannot insert multiple commands into a
/// prepared statement", SQLSTATE 42601), so we split on `;` and execute each
/// statement separately.
///
/// `--` line comments are stripped BEFORE splitting: a `;` inside a comment
/// (e.g. "revision 1; the row is seeded ...") must not be treated as a
/// statement separator, or the comment tail leaks out as a bogus statement
/// ("syntax error at or near ..."). Stripping comments is safe because no
/// string literal in PG_SCHEMA_SQL contains `--`, and no string literal
/// Split a multi-statement SQL string into individual statements.
///
/// PostgreSQL's prepared-statement protocol rejects multi-statement strings
/// ("cannot insert multiple commands into a prepared statement", SQLSTATE
/// 42601), so the baseline schema must be executed one statement at a time.
///
/// A naive `split(';')` is wrong: a `;` inside a `--` line comment (e.g.
/// "revision 1; the row is seeded ...") or inside a string literal (e.g.
/// `DEFAULT 'a;b'`) is NOT a statement separator. This walks the input once and
/// only treats a `;` as a separator when it is outside both single-quoted
/// strings and `--` line comments. Empty/whitespace-only statements are dropped.
pub fn split_sql_statements(sql: &str) -> Vec<String> {
    let mut statements = Vec::new();
    let mut current = String::new();
    let mut in_string = false; // inside a '...' single-quoted literal
    let mut in_comment = false; // inside a -- line comment (until newline)
    let bytes = sql.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i] as char;

        if in_comment {
            // A line comment ends at the newline. Drop the comment text itself.
            if c == '\n' {
                in_comment = false;
                current.push(c);
            }
            i += 1;
            continue;
        }

        if in_string {
            current.push(c);
            if c == '\'' {
                // SQL escapes a quote by doubling it ('') — stay in the string.
                if i + 1 < bytes.len() && bytes[i + 1] as char == '\'' {
                    current.push('\'');
                    i += 2;
                    continue;
                }
                in_string = false;
            }
            i += 1;
            continue;
        }

        // Outside string + comment.
        if c == '-' && i + 1 < bytes.len() && bytes[i + 1] as char == '-' {
            in_comment = true;
            i += 2;
            continue;
        }
        if c == '\'' {
            in_string = true;
            current.push(c);
            i += 1;
            continue;
        }
        if c == ';' {
            let trimmed = current.trim();
            if !trimmed.is_empty() {
                statements.push(trimmed.to_string());
            }
            current.clear();
            i += 1;
            continue;
        }
        current.push(c);
        i += 1;
    }
    let trimmed = current.trim();
    if !trimmed.is_empty() {
        statements.push(trimmed.to_string());
    }
    statements
}

/// Apply PG_SCHEMA_SQL to a pool, one statement at a time (see
/// `split_sql_statements` for why we can't send it as a single string).
///
/// Idempotent: every statement uses IF NOT EXISTS / ON CONFLICT DO NOTHING,
/// so re-runs on an already-initialized DB are no-ops.
///
/// v0.4.4: the whole baseline schema is applied inside ONE transaction. If any
/// statement fails (e.g. a permission error partway through), the entire schema
/// is rolled back rather than leaving a half-initialized database with some
/// tables present and others missing. DDL is transactional in PostgreSQL.
pub async fn apply_pg_schema(pool: &sqlx::PgPool) -> Result<(), sqlx::Error> {
    let mut tx = pool.begin().await?;
    for stmt in split_sql_statements(PG_SCHEMA_SQL) {
        sqlx::query(&stmt).execute(&mut *tx).await?;
    }
    tx.commit().await?;
    Ok(())
}

/// Apply any pending ordered migrations beyond the baseline. Runs after
/// `apply_pg_schema`, which guarantees `schema_version` exists and contains at
/// least revision 1.
///
/// v0.4.4 ships only the baseline (revision 1), so there are no migration arms
/// yet — this is the mechanism future releases use to evolve an EXISTING PG
/// database (e.g. `ALTER TABLE ... ADD COLUMN`). To add one: bump
/// `PG_SCHEMA_VERSION`, then add a `from == N` arm here that performs the
/// ALTERs and records the new revision. Each arm runs in its own transaction so
/// a partially-applied migration rolls back cleanly.
pub async fn run_pg_migrations(pool: &sqlx::PgPool) -> Result<(), sqlx::Error> {
    let current: i32 = sqlx::query_scalar("SELECT COALESCE(MAX(version), 0) FROM schema_version")
        .fetch_one(pool)
        .await?;

    // Already at (or beyond) the version this build understands — nothing to do.
    if current >= PG_SCHEMA_VERSION {
        return Ok(());
    }

    if current < 2 {
        let mut tx = pool.begin().await?;
        sqlx::query(
            r#"CREATE TABLE IF NOT EXISTS forward_rule_targets (
                id BIGSERIAL PRIMARY KEY,
                rule_id BIGINT NOT NULL REFERENCES forward_rules(id) ON DELETE CASCADE,
                host TEXT NOT NULL,
                port INTEGER NOT NULL CHECK (port >= 1 AND port <= 65535),
                position INTEGER NOT NULL CHECK (position >= 1),
                enabled BOOLEAN NOT NULL DEFAULT TRUE,
                created_at TEXT NOT NULL DEFAULT (to_char(now() AT TIME ZONE 'UTC', 'YYYY-MM-DD HH24:MI:SS'))
            )"#,
        )
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "CREATE INDEX IF NOT EXISTS idx_forward_rule_targets_rule_position \
             ON forward_rule_targets (rule_id, position)",
        )
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            r#"INSERT INTO forward_rule_targets (rule_id, host, port, position, enabled)
               SELECT fr.id, fr.target_addr, fr.target_port, 1, TRUE
               FROM forward_rules fr
               WHERE NOT EXISTS (
                   SELECT 1 FROM forward_rule_targets t WHERE t.rule_id = fr.id
               )"#,
        )
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "INSERT INTO schema_version (version) VALUES (2) ON CONFLICT (version) DO NOTHING",
        )
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
    }

    if current < 3 {
        let mut tx = pool.begin().await?;
        sqlx::query(
            "ALTER TABLE forward_rules \
             ADD COLUMN IF NOT EXISTS load_balance_strategy TEXT NOT NULL DEFAULT 'first'",
        )
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "INSERT INTO schema_version (version) VALUES (3) ON CONFLICT (version) DO NOTHING",
        )
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
    }

    if current < 4 {
        let mut tx = pool.begin().await?;
        sqlx::query(
            "ALTER TABLE forward_rules \
             ADD COLUMN IF NOT EXISTS upload_limit_mbps INTEGER NOT NULL DEFAULT 0",
        )
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "ALTER TABLE forward_rules \
             ADD COLUMN IF NOT EXISTS download_limit_mbps INTEGER NOT NULL DEFAULT 0",
        )
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "INSERT INTO schema_version (version) VALUES (4) ON CONFLICT (version) DO NOTHING",
        )
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
    }

    if current < 5 {
        // v0.4.7: chain removal + tls template consolidation. One transaction,
        // idempotent (ON CONFLICT / WHERE guards make re-runs no-ops).
        let mut tx = pool.begin().await?;
        let paused = sqlx::query(
            "UPDATE forward_rules SET paused = TRUE \
             WHERE route_mode = 'chain' AND paused = FALSE",
        )
        .execute(&mut *tx)
        .await?
        .rows_affected();
        let rewired = sqlx::query(
            "UPDATE device_groups SET group_type = 'out' \
             WHERE group_type = 'chained_outbound'",
        )
        .execute(&mut *tx)
        .await?
        .rows_affected();
        // v0.4.8 fix: NULL out tunnel_profile_id on rules that reference the
        // templates we're about to delete BEFORE the delete. A forward FK with
        // no ON DELETE clause would otherwise raise a constraint violation on
        // PostgreSQL and abort the migration. We match by name (not id) so the
        // lookup is stable regardless of auto-assigned ids.
        sqlx::query(
            "UPDATE forward_rules SET tunnel_profile_id = NULL \
             WHERE tunnel_profile_id IN ( \
                 SELECT id FROM tunnel_profiles \
                 WHERE name IN ('chain', 'tls-passthrough', 'tls-terminate') \
             )",
        )
        .execute(&mut *tx)
        .await?;
        // Now safe to delete the dead builtins — no rule references them.
        sqlx::query(
            "DELETE FROM tunnel_profiles \
             WHERE name IN ('chain', 'tls-passthrough', 'tls-terminate')",
        )
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "INSERT INTO tunnel_profiles \
                 (name, transport, tls_mode, ws_path, host_header, sni, is_builtin, uid) \
             VALUES ('tls-simple', 'tls_simple', 'none', '', '', '', TRUE, 1) \
             ON CONFLICT (name) DO NOTHING",
        )
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "INSERT INTO schema_version (version) VALUES (5) ON CONFLICT (version) DO NOTHING",
        )
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        tracing::info!(
            "PG migration 5: chain removed (paused {} chain rules, rewired {} chained_outbound groups)",
            paused,
            rewired
        );
    }

    if current < 6 {
        let mut tx = pool.begin().await?;
        sqlx::query(
            "CREATE INDEX IF NOT EXISTS idx_forward_rules_tunnel_profile \
             ON forward_rules (tunnel_profile_id)",
        )
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "INSERT INTO schema_version (version) VALUES (6) ON CONFLICT (version) DO NOTHING",
        )
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        tracing::info!("PG migration 6: forward_rules.tunnel_profile_id indexed");
    }

    // v0.4.10 revision 7: pause rules with inconsistent ownership.
    //
    // v0.4.11 PR3 revision 7 CHANGE: REMOVED uid-mismatch pause.
    // Cross-owner rules are now ALLOWED (shared inbound groups).
    // Only pause non-admin rules bound to custom (non-builtin) profiles.
    // PG boolean literals: is_builtin = FALSE, u.admin = FALSE, paused = FALSE.
    if current < 7 {
        let mut tx = pool.begin().await?;
        let paused_custom_profile = sqlx::query(
            "UPDATE forward_rules SET paused = TRUE \
             WHERE tunnel_profile_id IS NOT NULL AND paused = FALSE \
             AND EXISTS (SELECT 1 FROM tunnel_profiles tp, users u \
                         WHERE tp.id = forward_rules.tunnel_profile_id \
                           AND tp.is_builtin = FALSE \
                           AND u.id = forward_rules.uid AND u.admin = FALSE)",
        )
        .execute(&mut *tx)
        .await?;
        let paused_total = paused_custom_profile.rows_affected();

        sqlx::query(
            "INSERT INTO schema_version (version) VALUES (7) ON CONFLICT (version) DO NOTHING",
        )
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        if paused_total > 0 {
            tracing::warn!(
                "PG migration 7: paused {} rule(s) with regular-user bound to \
                 custom (non-builtin) tunnel profile. Admin review required — \
                 rebind to builtin profile or convert to admin.",
                paused_total
            );
        } else {
            tracing::info!("PG migration 7: no custom-profile violations found");
        }
    }

    // v0.4.10 PR3 revision 8: app_settings table for registration config.
    // Single-row table; the row itself is seeded by main.rs
    // (insert_settings_if_absent), not here. IF NOT EXISTS = no-op on fresh
    // installs where the baseline already created it.
    if current < 8 {
        let mut tx = pool.begin().await?;
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS app_settings (\
                 id INTEGER PRIMARY KEY CHECK (id = 1),\
                 registration_enabled BOOLEAN NOT NULL DEFAULT FALSE,\
                 default_registration_plan_id BIGINT NOT NULL DEFAULT 1 REFERENCES plans(id)\
             )",
        )
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "INSERT INTO schema_version (version) VALUES (8) ON CONFLICT (version) DO NOTHING",
        )
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        tracing::info!("PG migration 8: app_settings table present");
    }

    // v0.4.10 PR4 revision 9: users.must_change_password + token_version.
    // ADD COLUMN IF NOT EXISTS is a no-op on fresh installs where the baseline
    // already has the columns. token_version is BIGINT (i64), defaults 0.
    if current < 9 {
        let mut tx = pool.begin().await?;
        sqlx::query(
            "ALTER TABLE users ADD COLUMN IF NOT EXISTS must_change_password BOOLEAN NOT NULL DEFAULT FALSE",
        )
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "ALTER TABLE users ADD COLUMN IF NOT EXISTS token_version BIGINT NOT NULL DEFAULT 0",
        )
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "INSERT INTO schema_version (version) VALUES (9) ON CONFLICT (version) DO NOTHING",
        )
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        tracing::info!("PG migration 9: users.must_change_password + token_version present");
    }

    // v0.4.11 PR1 revision 10: tunnel profile semantics.
    // Non-destructive migration: tunnel templates represent only WS / TLS Simple.
    // Direct/raw are no longer tunnel template concepts.
    if current < 10 {
        let mut tx = pool.begin().await?;

        // Step 1: Ensure builtin templates exist (idempotent via ON CONFLICT DO NOTHING).
        sqlx::query(
            r#"INSERT INTO tunnel_profiles
                   (name, transport, tls_mode, ws_path, host_header, sni, is_builtin, uid)
                VALUES ('ws-relay', 'ws', 'none', '/relay', '', '', TRUE, 1)
                ON CONFLICT (name) DO NOTHING"#,
        )
        .execute(&mut *tx)
        .await?;

        sqlx::query(
            r#"INSERT INTO tunnel_profiles
                   (name, transport, tls_mode, ws_path, host_header, sni, is_builtin, uid)
                VALUES ('tls-simple', 'tls_simple', 'none', '', '', '', TRUE, 1)
                ON CONFLICT (name) DO NOTHING"#,
        )
        .execute(&mut *tx)
        .await?;

        // Step 2: Bind NULL-profile WS rules to builtin ws-relay.
        sqlx::query(
            r#"UPDATE forward_rules
                SET tunnel_profile_id = (
                    SELECT id FROM tunnel_profiles WHERE name = 'ws-relay' AND is_builtin = TRUE
                )
                WHERE tunnel_profile_id IS NULL
                  AND public_transport = 'ws'"#,
        )
        .execute(&mut *tx)
        .await?;

        // Step 3: Bind NULL-profile TLS Simple rules to builtin tls-simple.
        sqlx::query(
            r#"UPDATE forward_rules
                SET tunnel_profile_id = (
                    SELECT id FROM tunnel_profiles WHERE name = 'tls-simple' AND is_builtin = TRUE
                )
                WHERE tunnel_profile_id IS NULL
                  AND public_transport IN ('tls_simple', 'tls')"#,
        )
        .execute(&mut *tx)
        .await?;

        // Step 4: Unbind direct-profile templates and switch to Raw.
        sqlx::query(
            r#"UPDATE forward_rules
                SET tunnel_profile_id = NULL,
                    public_transport = 'raw',
                    node_transport = 'raw',
                    entry_transport = 'raw',
                    ws_path = NULL
                WHERE tunnel_profile_id IN (
                    SELECT id FROM tunnel_profiles WHERE transport = 'direct'
                )"#,
        )
        .execute(&mut *tx)
        .await?;

        sqlx::query(
            "INSERT INTO schema_version (version) VALUES (10) ON CONFLICT (version) DO NOTHING",
        )
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        tracing::info!("PG migration 10: tunnel profile semantics aligned");
    }

    // v0.4.11 PR4 revision 11: shared-node port occupancy. Replace the global
    // UNIQUE(listen_port) index with two partial unique indexes keyed on
    // (device_group_in, listen_port), split by socket type (TCP-bearing vs
    // UDP-bearing). The old global rule was strictly stricter, so a DB that
    // satisfied it also satisfies the new partial indexes. A DB that never had
    // the global index (or had duplicates) could still violate a partial index,
    // so we detect per-partition duplicates first and SKIP (keeping whatever
    // index exists) rather than fail the migration — mirroring SQLite
    // Migration 28 and PG-side caution.
    if current < 11 {
        let mut tx = pool.begin().await?;

        let tcp_dupes: (i64,) = sqlx::query_as(
            "SELECT COUNT(*) FROM (
                 SELECT device_group_in, listen_port FROM forward_rules
                 WHERE protocol IN ('tcp', 'tcp_udp')
                 GROUP BY device_group_in, listen_port HAVING COUNT(*) > 1
             ) d",
        )
        .fetch_one(&mut *tx)
        .await?;
        let udp_dupes: (i64,) = sqlx::query_as(
            "SELECT COUNT(*) FROM (
                 SELECT device_group_in, listen_port FROM forward_rules
                 WHERE protocol IN ('udp', 'tcp_udp')
                 GROUP BY device_group_in, listen_port HAVING COUNT(*) > 1
             ) d",
        )
        .fetch_one(&mut *tx)
        .await?;

        if tcp_dupes.0 > 0 || udp_dupes.0 > 0 {
            tracing::error!(
                "PG migration 11 SKIPPED: forward_rules has conflicting (device_group_in, \
                 listen_port) rows (tcp partition: {}, udp partition: {}). The new partial \
                 UNIQUE indexes were NOT created. Resolve the conflicts and restart.",
                tcp_dupes.0,
                udp_dupes.0
            );
            // Do NOT advance schema_version: leave revision < 11 so the
            // migration re-attempts on next boot once the operator fixes data.
            tx.rollback().await?;
        } else {
            sqlx::query("DROP INDEX IF EXISTS idx_forward_rules_listen_port")
                .execute(&mut *tx)
                .await?;
            sqlx::query(
                "CREATE UNIQUE INDEX IF NOT EXISTS idx_fr_port_tcp
                 ON forward_rules (device_group_in, listen_port)
                 WHERE protocol IN ('tcp', 'tcp_udp')",
            )
            .execute(&mut *tx)
            .await?;
            sqlx::query(
                "CREATE UNIQUE INDEX IF NOT EXISTS idx_fr_port_udp
                 ON forward_rules (device_group_in, listen_port)
                 WHERE protocol IN ('udp', 'tcp_udp')",
            )
            .execute(&mut *tx)
            .await?;
            sqlx::query(
                "INSERT INTO schema_version (version) VALUES (11) ON CONFLICT (version) DO NOTHING",
            )
            .execute(&mut *tx)
            .await?;
            tx.commit().await?;
            tracing::info!(
                "PG migration 11: replaced global listen_port index with per-group partial \
                 TCP/UDP indexes"
            );
        }
    }

    // ── Revision 12: v0.4.21 PR2 registration allowed plan ids ──
    if current < 12 {
        sqlx::query(
            "ALTER TABLE app_settings ADD COLUMN IF NOT EXISTS \
             registration_allowed_plan_ids TEXT NOT NULL DEFAULT '[1]'",
        )
        .execute(pool)
        .await?;
        // Seed existing rows from their default_registration_plan_id.
        sqlx::query(
            "UPDATE app_settings SET registration_allowed_plan_ids = \
             '[' || default_registration_plan_id || ']' \
             WHERE registration_allowed_plan_ids = '[1]' \
               AND default_registration_plan_id != 1",
        )
        .execute(pool)
        .await?;
        sqlx::query(
            "INSERT INTO schema_version (version) VALUES (12) ON CONFLICT (version) DO NOTHING",
        )
        .execute(pool)
        .await?;
        tracing::info!("PG migration 12: added registration_allowed_plan_ids to app_settings");
    }

    // ── Revision 13: v1.0.4 user permission groups ──
    if current < 13 {
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS user_groups (
                id BIGSERIAL PRIMARY KEY,
                name TEXT NOT NULL,
                remark TEXT NOT NULL DEFAULT '',
                allow_all_groups BOOLEAN NOT NULL DEFAULT FALSE,
                created_at TEXT NOT NULL DEFAULT (to_char(now() AT TIME ZONE 'UTC', 'YYYY-MM-DD HH24:MI:SS'))
            )",
        )
        .execute(pool)
        .await?;

        sqlx::query(
            "CREATE TABLE IF NOT EXISTS user_group_device_groups (
                user_group_id BIGINT NOT NULL REFERENCES user_groups(id) ON DELETE CASCADE,
                device_group_id BIGINT NOT NULL REFERENCES device_groups(id) ON DELETE CASCADE,
                PRIMARY KEY (user_group_id, device_group_id)
            )",
        )
        .execute(pool)
        .await?;

        sqlx::query(
            "INSERT INTO user_groups (id, name, remark, allow_all_groups) \
             VALUES (1, 'default', 'Default group - all device groups allowed', TRUE) \
             ON CONFLICT (id) DO NOTHING",
        )
        .execute(pool)
        .await?;

        // v1.0.4: advance the BIGSERIAL sequence past the explicit id=1 insert
        // so admin-created groups don't collide with the default group's id.
        sqlx::query(
            "SELECT setval(pg_get_serial_sequence('user_groups', 'id'), \
             GREATEST((SELECT MAX(id) FROM user_groups), 1))",
        )
        .execute(pool)
        .await?;

        // v1.0.7: guard the legacy group_id backfill — on a FRESH DB the baseline
        // schema no longer has users.group_id (it was replaced by
        // all_device_groups), yet this arm still replays. Skip the UPDATE when the
        // column is absent so fresh installs don't error here.
        sqlx::query(
            "DO $$ BEGIN \
               IF EXISTS (SELECT 1 FROM information_schema.columns \
                          WHERE table_name = 'users' AND column_name = 'group_id') THEN \
                 UPDATE users SET group_id = 1 WHERE group_id IS NULL; \
               END IF; \
             END $$",
        )
        .execute(pool)
        .await?;

        sqlx::query(
            "INSERT INTO schema_version (version) VALUES (13) ON CONFLICT (version) DO NOTHING",
        )
        .execute(pool)
        .await?;
        tracing::info!("PG migration 13: user permission groups tables created");
    }

    // ── Revision 14: v1.0.5 fix em-dash encoding in default user_group remark ──
    if current < 14 {
        sqlx::query(
            "UPDATE user_groups SET remark = 'Default group - all device groups allowed' \
             WHERE id = 1 AND remark != 'Default group - all device groups allowed'",
        )
        .execute(pool)
        .await?;
        sqlx::query(
            "INSERT INTO schema_version (version) VALUES (14) ON CONFLICT (version) DO NOTHING",
        )
        .execute(pool)
        .await?;
        tracing::info!("PG migration 14: default user_group remark normalized to ASCII");
    }

    // ── Revision 15: v1.0.7 drop the user_groups named-entity layer ──
    // Mirrors SQLite Migration 32: per-user all_device_groups flag + direct
    // user ↔ device_group link, no backfill (every non-admin starts unassigned;
    // admins are always all-allowed in code). The dormant users.group_id column
    // is left in place (unread) to avoid a risky table rewrite.
    if current < 15 {
        sqlx::query(
            "ALTER TABLE users ADD COLUMN IF NOT EXISTS all_device_groups BOOLEAN NOT NULL DEFAULT FALSE",
        )
        .execute(pool)
        .await?;

        sqlx::query(
            "CREATE TABLE IF NOT EXISTS user_device_groups (
                user_id BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
                device_group_id BIGINT NOT NULL REFERENCES device_groups(id) ON DELETE CASCADE,
                PRIMARY KEY (user_id, device_group_id)
            )",
        )
        .execute(pool)
        .await?;

        // Drop the legacy named-entity tables (child first for the FK).
        sqlx::query("DROP TABLE IF EXISTS user_group_device_groups")
            .execute(pool)
            .await?;
        sqlx::query("DROP TABLE IF EXISTS user_groups")
            .execute(pool)
            .await?;

        sqlx::query(
            "INSERT INTO schema_version (version) VALUES (15) ON CONFLICT (version) DO NOTHING",
        )
        .execute(pool)
        .await?;
        tracing::info!(
            "PG migration 15: user_groups layer replaced by user_device_groups + all_device_groups"
        );
    }

    // ── Revision 16: v1.0.8 device-group traffic billing rate ──
    // Mirrors SQLite Migration 33. ADD COLUMN IF NOT EXISTS so a FRESH database
    // (which already has rate from PG_SCHEMA_SQL baseline) replays this arm as
    // a no-op rather than erroring — same IF-NOT-EXISTS discipline as every
    // prior arm. Real bytes stay on forward_rules / users; users are CHARGED
    // real * rate (rounded) inside apply_traffic_batch.
    if current < 16 {
        sqlx::query(
            "ALTER TABLE device_groups \
             ADD COLUMN IF NOT EXISTS rate DOUBLE PRECISION NOT NULL DEFAULT 1.0",
        )
        .execute(pool)
        .await?;
        sqlx::query(
            "INSERT INTO schema_version (version) VALUES (16) ON CONFLICT (version) DO NOTHING",
        )
        .execute(pool)
        .await?;
        tracing::info!("PG migration 16: device_groups.rate column present");
    }

    // ── Revision 17: v1.0.8 plan management + user suspension ──
    // Mirrors SQLite Migration 34. Every ALTER uses ADD COLUMN IF NOT EXISTS
    // and the table uses CREATE ... IF NOT EXISTS so a FRESH database (which
    // already has all of these from PG_SCHEMA_SQL baseline) replays this arm as
    // a no-op — the same IF-NOT-EXISTS discipline that prevents the mig15-style
    // fresh-DB replay crash.
    if current < 17 {
        sqlx::query(
            "ALTER TABLE plans ADD COLUMN IF NOT EXISTS plan_type TEXT NOT NULL DEFAULT 'data'",
        )
        .execute(pool)
        .await?;
        sqlx::query(
            "ALTER TABLE plans ADD COLUMN IF NOT EXISTS duration_days INTEGER NOT NULL DEFAULT 0",
        )
        .execute(pool)
        .await?;
        sqlx::query(
            "ALTER TABLE plans ADD COLUMN IF NOT EXISTS hidden BOOLEAN NOT NULL DEFAULT FALSE",
        )
        .execute(pool)
        .await?;
        sqlx::query(
            "ALTER TABLE plans ADD COLUMN IF NOT EXISTS reset_traffic BOOLEAN NOT NULL DEFAULT FALSE",
        )
        .execute(pool)
        .await?;
        sqlx::query(
            "ALTER TABLE plans ADD COLUMN IF NOT EXISTS description TEXT NOT NULL DEFAULT ''",
        )
        .execute(pool)
        .await?;
        sqlx::query("ALTER TABLE users ADD COLUMN IF NOT EXISTS plan_expire_at TEXT")
            .execute(pool)
            .await?;
        sqlx::query(
            "ALTER TABLE users ADD COLUMN IF NOT EXISTS suspended BOOLEAN NOT NULL DEFAULT FALSE",
        )
        .execute(pool)
        .await?;

        sqlx::query(
            "CREATE TABLE IF NOT EXISTS orders (
                id BIGSERIAL PRIMARY KEY,
                user_id BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
                plan_id BIGINT,
                plan_name TEXT NOT NULL,
                price TEXT NOT NULL,
                created_at TEXT NOT NULL DEFAULT (to_char(now() AT TIME ZONE 'UTC', 'YYYY-MM-DD HH24:MI:SS'))
            )",
        )
        .execute(pool)
        .await?;
        sqlx::query("CREATE INDEX IF NOT EXISTS idx_orders_user_id ON orders(user_id)")
            .execute(pool)
            .await?;
        sqlx::query(
            "INSERT INTO schema_version (version) VALUES (17) ON CONFLICT (version) DO NOTHING",
        )
        .execute(pool)
        .await?;
        tracing::info!("PG migration 17: plans lifecycle cols + users suspension + orders table");
    }

    // ── Revision 18: v1.0.9 plan ↔ device-group grants ──
    // Mirrors SQLite Migration 35. ADD COLUMN IF NOT EXISTS + CREATE TABLE IF
    // NOT EXISTS so a FRESH database (which already has both from the baseline)
    // replays this arm as a no-op.
    if current < 18 {
        sqlx::query(
            "ALTER TABLE plans ADD COLUMN IF NOT EXISTS grant_all_groups BOOLEAN NOT NULL DEFAULT FALSE",
        )
        .execute(pool)
        .await?;
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS plan_device_groups (
                plan_id BIGINT NOT NULL REFERENCES plans(id) ON DELETE CASCADE,
                device_group_id BIGINT NOT NULL REFERENCES device_groups(id) ON DELETE CASCADE,
                PRIMARY KEY (plan_id, device_group_id)
            )",
        )
        .execute(pool)
        .await?;
        sqlx::query(
            "INSERT INTO schema_version (version) VALUES (18) ON CONFLICT (version) DO NOTHING",
        )
        .execute(pool)
        .await?;
        tracing::info!("PG migration 18: plans.grant_all_groups + plan_device_groups table");
    }

    // ── Revision 19: v1.0.7 device-group hidden flag ──
    // Mirrors SQLite Migration 36. ADD COLUMN IF NOT EXISTS so a FRESH database
    // (which already has hidden from the baseline) replays this arm as a no-op.
    if current < 19 {
        sqlx::query(
            "ALTER TABLE device_groups ADD COLUMN IF NOT EXISTS hidden BOOLEAN NOT NULL DEFAULT FALSE",
        )
        .execute(pool)
        .await?;
        sqlx::query(
            "INSERT INTO schema_version (version) VALUES (19) ON CONFLICT (version) DO NOTHING",
        )
        .execute(pool)
        .await?;
        tracing::info!("PG migration 19: device_groups.hidden column present");
    }

    // ── Revision 20: v1.0.8 forward_rules.auto_paused ──
    // Mirrors SQLite Migration 37. ADD COLUMN IF NOT EXISTS so a FRESH database
    // (which already has auto_paused from the baseline) replays this as a no-op.
    if current < 20 {
        sqlx::query(
            "ALTER TABLE forward_rules ADD COLUMN IF NOT EXISTS auto_paused BOOLEAN NOT NULL DEFAULT FALSE",
        )
        .execute(pool)
        .await?;
        sqlx::query(
            "INSERT INTO schema_version (version) VALUES (20) ON CONFLICT (version) DO NOTHING",
        )
        .execute(pool)
        .await?;
        tracing::info!("PG migration 20: forward_rules.auto_paused column present");
    }

    if current < 21 {
        // v1.2.0: connection cap + scheduled restart. Both default to 0 = off,
        // so an upgraded panel changes nothing about existing rules until the
        // operator opts one in. (0 = unlimited, not "admit zero" — the node maps
        // 0 → None.)
        sqlx::query(
            "ALTER TABLE forward_rules ADD COLUMN IF NOT EXISTS max_connections INTEGER NOT NULL DEFAULT 0",
        )
        .execute(pool)
        .await?;
        sqlx::query(
            "ALTER TABLE forward_rules ADD COLUMN IF NOT EXISTS auto_restart_minutes INTEGER NOT NULL DEFAULT 0",
        )
        .execute(pool)
        .await?;
        sqlx::query(
            "INSERT INTO schema_version (version) VALUES (21) ON CONFLICT (version) DO NOTHING",
        )
        .execute(pool)
        .await?;
        tracing::info!(
            "PG migration 21: forward_rules.max_connections + auto_restart_minutes present"
        );
    }

    if current < 22 {
        // v1.2.0: redeem codes. See the baseline comment for the SET NULL /
        // conditional-UPDATE reasoning.
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS redeem_codes (
                id BIGSERIAL PRIMARY KEY,
                code TEXT NOT NULL UNIQUE,
                amount TEXT NOT NULL,
                status TEXT NOT NULL DEFAULT 'unused',
                used_by BIGINT REFERENCES users(id) ON DELETE SET NULL,
                used_at TEXT,
                expires_at TEXT,
                batch_id TEXT NOT NULL DEFAULT '',
                remark TEXT NOT NULL DEFAULT '',
                created_at TEXT NOT NULL DEFAULT (to_char(now() AT TIME ZONE 'UTC', 'YYYY-MM-DD HH24:MI:SS'))
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
        sqlx::query(
            "INSERT INTO schema_version (version) VALUES (22) ON CONFLICT (version) DO NOTHING",
        )
        .execute(pool)
        .await?;
        tracing::info!("PG migration 22: redeem_codes table present");
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    // A `;` inside a -- line comment must NOT split a statement. This is the
    // exact bug that broke every PG test in v0.4.4: the schema_version comment
    // read "revision 1; the row is seeded ..." and the tail leaked out as a
    // bogus statement ("syntax error at or near the").
    #[test]
    fn semicolon_in_line_comment_is_not_a_separator() {
        let sql =
            "CREATE TABLE a (id INT); -- revision 1; the row is seeded\nCREATE TABLE b (id INT);";
        let stmts = split_sql_statements(sql);
        assert_eq!(
            stmts,
            vec!["CREATE TABLE a (id INT)", "CREATE TABLE b (id INT)"]
        );
    }

    // A `;` inside a single-quoted string literal must NOT split a statement.
    #[test]
    fn semicolon_in_string_literal_is_not_a_separator() {
        let sql = "INSERT INTO t (v) VALUES ('a;b'); INSERT INTO t (v) VALUES ('c');";
        let stmts = split_sql_statements(sql);
        assert_eq!(
            stmts,
            vec![
                "INSERT INTO t (v) VALUES ('a;b')",
                "INSERT INTO t (v) VALUES ('c')"
            ]
        );
    }

    // An escaped quote ('') keeps us inside the string, so a following `;`
    // inside it is still not a separator.
    #[test]
    fn escaped_quote_keeps_string_open() {
        let sql = "INSERT INTO t (v) VALUES ('it''s; ok'); SELECT 1;";
        let stmts = split_sql_statements(sql);
        assert_eq!(
            stmts,
            vec!["INSERT INTO t (v) VALUES ('it''s; ok')", "SELECT 1"]
        );
    }

    // Empty / whitespace-only statements (trailing `;`, blank lines) are dropped.
    #[test]
    fn empty_statements_are_dropped() {
        assert_eq!(
            split_sql_statements(";\n  ;\nSELECT 1;\n\n"),
            vec!["SELECT 1"]
        );
        assert!(split_sql_statements("   \n -- just a comment\n").is_empty());
    }

    // The real baseline schema must split into runnable statements, and no
    // fragment may start with a stray comment word like "the" (the symptom of
    // a mis-split comment). Every statement should start with a SQL keyword.
    #[test]
    fn real_schema_splits_into_clean_statements() {
        let stmts = split_sql_statements(PG_SCHEMA_SQL);
        assert!(
            stmts.len() >= 10,
            "expected many statements, got {}",
            stmts.len()
        );
        for s in &stmts {
            let head = s.split_whitespace().next().unwrap_or("").to_uppercase();
            assert!(
                matches!(head.as_str(), "CREATE" | "INSERT" | "SELECT" | "ALTER"),
                "statement does not start with a SQL keyword: {s:?}"
            );
        }
    }
}
