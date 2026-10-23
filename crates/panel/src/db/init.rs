use crate::db::pg_schema::{apply_pg_schema, run_pg_migrations};
use crate::db::repo::UserRepository;
use crate::db::schema::{run_migrations, SCHEMA_SQL};
use crate::db::sqlite_repo::SqliteRepository;
use crate::service::password::{generate_password, validate_password};
use sqlx::sqlite::{SqliteConnectOptions, SqlitePool, SqlitePoolOptions};
use std::str::FromStr;

pub async fn init_db(database_url: &str) -> Result<SqlitePool, sqlx::Error> {
    // Parse the connection string and force three critical PRAGMAs that
    // prevent the most common SQLite pain points under concurrent load:
    //
    //   journal_mode=WAL  — allows concurrent readers + 1 writer without
    //     "database is locked" on multi-node traffic reporting.
    //   busy_timeout=5000 — if a write lock IS contended, wait up to 5s
    //     before erroring instead of failing instantly.
    //   foreign_keys=ON   — enques FK constraints (currently worked around
    //     by manual cascade in delete_user, but this catches future leaks).
    //
    // NOTE: foreign_keys is intentionally NOT set via SqliteConnectOptions here.
    // That pragma would be re-applied on every NEW pool connection, overriding
    // the temporary OFF we need during run_migrations (SQLite forbids dropping
    // a table referenced by an FK while enforcement is on, and several
    // migrations rebuild tables). Instead we toggle it explicitly below.
    //
    // SqliteConnectOptions::pragma() appends the others as `PRAGMA key=value;`
    // on every new connection, so they apply pool-wide.
    let opts = SqliteConnectOptions::from_str(database_url)?
        .journal_mode(sqlx::sqlite::SqliteJournalMode::Wal)
        .busy_timeout(std::time::Duration::from_secs(5))
        .create_if_missing(true);

    // Single-connection pool for migration: with max_connections=1 we are
    // guaranteed that the PRAGMA foreign_keys=OFF we set on it stays in effect
    // for every migration statement (no second connection reseting it to ON).
    let migrate_pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(opts.clone())
        .await?;
    sqlx::query(SCHEMA_SQL).execute(&migrate_pool).await?;

    // foreign_keys OFF during schema maintenance, per SQLite guidance:
    // https://www.sqlite.org/foreignkeys.html#fk_schemabugs
    sqlx::query("PRAGMA foreign_keys = OFF")
        .execute(&migrate_pool)
        .await?;
    run_migrations(&migrate_pool).await?;
    sqlx::query("PRAGMA foreign_keys = ON")
        .execute(&migrate_pool)
        .await?;
    migrate_pool.close().await;

    // Now build the real serving pool with max_connections=5. FK enforcement
    // is turned on per-connection via the connect_options pragma so it always
    // holds for runtime queries.
    let opts = opts.foreign_keys(true);
    let pool = SqlitePoolOptions::new()
        .max_connections(5)
        .connect_with(opts)
        .await?;

    // Replace the SCHEMA_SQL placeholder password with a real bcrypt hash of
    // the initial password — but ONLY when the stored password is still the
    // placeholder. See hash_default_admin_password_if_placeholder for the
    // safety reasoning (the short version: the WHERE clause matches ONLY the
    // literal placeholder, never an admin-changed bcrypt hash).
    hash_default_admin_password_if_placeholder(&pool).await?;

    Ok(pool)
}

/// The bcrypt hash for the first-boot admin password.
///
/// No fixed default: a published `admin/admin123` is a working login for
/// anyone who reaches a fresh panel before its owner does. The operator sets
/// `ADMIN_INITIAL_PASSWORD`; without it (or with one that fails the password
/// policy) a random password is generated and written to the log ONCE, at
/// WARN, so `docker logs` shows it. The account still has must_change_password.
fn initial_admin_password_hash() -> Result<String, sqlx::Error> {
    let from_env = std::env::var("ADMIN_INITIAL_PASSWORD")
        .ok()
        .filter(|v| !v.is_empty());
    let password = match from_env {
        Some(p) if validate_password(&p).is_ok() => p,
        other => {
            if other.is_some() {
                tracing::warn!(
                    "ADMIN_INITIAL_PASSWORD ignored: it must be 8..=72 bytes; generating one"
                );
            }
            let p = generate_password();
            tracing::warn!(
                "initial admin login: username `admin`, password `{p}` — shown only this once; \
                 you must change it at first login (or run `relay-panel reset-admin-password`)"
            );
            p
        }
    };
    bcrypt::hash(&password, 12).map_err(|e| sqlx::Error::Protocol(format!("bcrypt: {e}")))
}

/// Hash the initial admin password into users(id=1) IF AND ONLY IF the
/// stored value is still the SCHEMA_SQL placeholder. A password an admin has
/// already changed is NEVER touched.
///
/// Safety (this is the v0.3.1 audit item #3 regression guard):
///   - `WHERE password LIKE '$2b$12$PLACEHOLDER%'` matches only the seeded
///     placeholder string, never a real bcrypt hash (real hashes have random
///     salt where the literal "PLACEHOLDER" sits).
///   - The expensive bcrypt hash is computed lazily inside the branch, so a
///     normal restart (password already set) does NOT pay ~100ms on every boot.
///   - Migrations 1-16 never touch the password column; INSERT OR IGNORE in
///     SCHEMA_SQL only inserts when the row is absent. So no code path resets
///     a changed password — confirmed by `password_survives_init_when_changed`.
pub async fn hash_default_admin_password_if_placeholder(
    pool: &sqlx::SqlitePool,
) -> Result<(), sqlx::Error> {
    // v0.4.3: route through the UserRepository trait so the placeholder-hash
    // SQL lives in exactly one place (sqlite_repo.rs), and PR2's PgRepository
    // gets the same guard for free. init.rs still owns the SqlitePool because
    // migrations are SQLite-specific (PRAGMA toggles, schema rebuilds) and can't
    // be expressed through the Repository abstraction.
    let repo = SqliteRepository::new(pool.clone());
    let needs_hash = repo
        .count_placeholder_admin_password()
        .await
        .map_err(db_err_to_sqlx)?;
    if needs_hash > 0 {
        let hashed = initial_admin_password_hash()?;
        repo.replace_placeholder_admin_password(&hashed)
            .await
            .map_err(db_err_to_sqlx)?;
        tracing::info!("set the initial admin password (first boot placeholder -> bcrypt)");
    }
    Ok(())
}

/// Map a DbError back to sqlx::Error so init_db keeps its sqlx::Error return
/// type (callers expect it). The Other variant unwraps the wrapped sqlx::Error;
/// the structured variants (UniqueViolation etc.) are surfaced as generic
/// database errors — they cannot happen here, but if they did the right thing
/// is to fail init loudly.
fn db_err_to_sqlx(e: crate::db::error::DbError) -> sqlx::Error {
    match e {
        crate::db::error::DbError::Other(inner) => inner,
        other => sqlx::Error::Configuration(format!("init db error: {other}").into()),
    }
}

// ── PostgreSQL ──
//
// v0.4.3: PostgreSQL support. The user installs PG, creates an empty database,
// and fills in the connection string (e.g. `postgres://user:pass@host/db`).
// The panel creates all tables + seed data on first boot via PG_SCHEMA_SQL.
// There is NO migration suite — PG support is new, there are no old PG
// databases to upgrade.

/// Initialize a PostgreSQL pool + schema. Returns a `PgPool` the caller wraps
/// in `PgRepository::new` to get an `Arc<dyn Repository>`.
///
/// Fail-fast contract: any error (connection refused, auth failure, schema
/// creation failure) propagates as `Err` and the caller MUST terminate. There
/// is NO fallback to SQLite — silently switching backends would cause "data
/// written locally, user thinks it's in PG" data loss, which is strictly worse
/// than a startup crash.
pub async fn init_pg(database_url: &str) -> Result<sqlx::PgPool, sqlx::Error> {
    use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
    use std::str::FromStr;

    let opts = PgConnectOptions::from_str(database_url)?;

    // max_connections=5 matches the SQLite serving pool. PG handles
    // concurrent connections natively (MVCC), so no single-writer limit.
    let pool = PgPoolOptions::new()
        .max_connections(5)
        .connect_with(opts)
        .await?;

    // Create all tables + seed admin/plan/builtin profiles. apply_pg_schema
    // splits PG_SCHEMA_SQL into individual statements (PG's prepared-statement
    // protocol rejects multi-statement strings). Fully idempotent (CREATE
    // TABLE IF NOT EXISTS + ON CONFLICT DO NOTHING), so re-runs are no-ops.
    apply_pg_schema(&pool).await?;

    // Apply any ordered migrations beyond the baseline (v0.4.4: none yet, but
    // this is how future releases evolve an existing PG database since
    // CREATE TABLE IF NOT EXISTS can't alter existing tables).
    run_pg_migrations(&pool).await?;

    // Same placeholder-password hashing as SQLite: the seed admin row has the
    // PLACEHOLDER string, init replaces it with a real bcrypt hash of the initial password.
    // This runs through the Repository trait so the SQL lives in pg_repo.rs.
    hash_default_admin_password_if_placeholder_pg(&pool).await?;

    Ok(pool)
}

/// PG equivalent of `hash_default_admin_password_if_placeholder`. Same safety
/// contract: only hashes when the stored value is still the literal PLACEHOLDER.
async fn hash_default_admin_password_if_placeholder_pg(
    pool: &sqlx::PgPool,
) -> Result<(), sqlx::Error> {
    use crate::db::pg_repo::PgRepository;
    let repo = PgRepository::new(pool.clone());
    let needs_hash = repo
        .count_placeholder_admin_password()
        .await
        .map_err(db_err_to_sqlx)?;
    if needs_hash > 0 {
        let hashed = initial_admin_password_hash()?;
        repo.replace_placeholder_admin_password(&hashed)
            .await
            .map_err(db_err_to_sqlx)?;
        tracing::info!("set the initial admin password (first boot placeholder -> bcrypt)");
    }
    Ok(())
}

/// Detect whether a `database_url` string targets PostgreSQL. Recognized
/// prefixes: `postgres://`, `postgresql://`, `postgres+ssl://` (and the same
/// for `postgresql`). Anything else (including `sqlite:` and bare file paths)
/// is treated as SQLite — the default, zero-config backend.
pub fn is_postgres_url(database_url: &str) -> bool {
    database_url.starts_with("postgres://")
        || database_url.starts_with("postgresql://")
        || database_url.starts_with("postgres+ssl://")
        || database_url.starts_with("postgresql+ssl://")
}

#[cfg(test)]
mod initial_password_tests {
    use super::*;

    async fn admin_hash(dir: &std::path::Path, name: &str) -> String {
        let url = format!("sqlite:{}?mode=rwc", dir.join(name).display());
        let pool = init_db(&url).await.unwrap();
        sqlx::query_scalar("SELECT password FROM users WHERE id = 1")
            .fetch_one(&pool)
            .await
            .unwrap()
    }

    /// One test (env vars are process-global): no fixed default password, the
    /// operator's ADMIN_INITIAL_PASSWORD is honoured, and an invalid one falls
    /// back to a random password instead of a weak one.
    #[tokio::test]
    async fn first_boot_admin_password_is_not_a_fixed_default() {
        let dir = std::env::temp_dir().join(format!("relay-init-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();

        std::env::remove_var("ADMIN_INITIAL_PASSWORD");
        let random = admin_hash(&dir, "a.db").await;
        assert!(!bcrypt::verify("admin123", &random).unwrap());

        std::env::set_var("ADMIN_INITIAL_PASSWORD", "operator-chosen-1");
        let chosen = admin_hash(&dir, "b.db").await;
        assert!(bcrypt::verify("operator-chosen-1", &chosen).unwrap());

        std::env::set_var("ADMIN_INITIAL_PASSWORD", "short");
        let fallback = admin_hash(&dir, "c.db").await;
        assert!(!bcrypt::verify("short", &fallback).unwrap());

        std::env::remove_var("ADMIN_INITIAL_PASSWORD");
        std::fs::remove_dir_all(&dir).ok();
    }
}
