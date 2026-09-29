// v0.4.3 PR1: SQLite implementation of the Repository traits.
//
// All SQL that was previously inline in handler files (admin.rs, node.rs,
// config.rs, stats.rs, auth.rs, middleware.rs, ws.rs) lives here now.
// Handlers call `state.db.method()` and never write SQL directly.
//
// The SQL itself is UNCHANGED from the v0.4.2 codebase — same SQLite dialect,
// same ? placeholders, same statements. This is a pure mechanical move,
// not a rewrite. PR2 will add PgRepository with PostgreSQL-native SQL.

use sqlx::SqlitePool;

mod announcements;
mod groups;
mod kvs;
mod orders;
mod profiles;
mod redeem;
mod rules;
mod settings;
mod stats;
#[cfg(test)]
mod tests;
mod traffic;
mod user_groups;
mod users;

pub struct SqliteRepository {
    pub(super) pool: SqlitePool,
}

impl SqliteRepository {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    /// v1.2.12: open a transaction that WRITES. `pool.begin()` issues a
    /// deferred BEGIN: the first SELECT pins a WAL read snapshot, and if
    /// another connection commits before this transaction's first write, that
    /// write fails at once with SQLITE_BUSY (busy_timeout does not apply to a
    /// stale snapshot) — a 500 on a concurrent purchase / redeem / traffic
    /// report. BEGIN IMMEDIATE takes the write lock up front instead, where
    /// busy_timeout does apply: a concurrent writer waits its turn. Like any
    /// sqlx `Transaction`, it rolls back on drop.
    pub(super) async fn begin_write(
        &self,
    ) -> Result<sqlx::Transaction<'static, sqlx::Sqlite>, sqlx::Error> {
        self.pool.begin_with("BEGIN IMMEDIATE").await
    }
}

// ── Aggregate Repository ──
//
// The aggregate trait has no methods of its own — it just combines the domain
// traits. A blanket impl for any type satisfying all supertraits would work,
// but spelling it out keeps the impl block discoverable and avoids coherence
// surprises when PgRepository is added in PR2.
#[async_trait::async_trait]
impl super::repo::Repository for SqliteRepository {}
