// v0.4.3 PR2: PostgreSQL implementation of the Repository traits.
//
// Mirror of `sqlite_repo` — same trait contracts, same method shapes, but
// PostgreSQL-native SQL. The SQL is a mechanical translation of the SQLite
// version, NOT a redesign:
//   - `?` placeholders → `$1, $2, ...` numbered placeholders (PG requirement).
//   - `INSERT OR REPLACE INTO kvs` → `INSERT ... ON CONFLICT (key) DO UPDATE
//     SET value = EXCLUDED.value` (PG upsert).
//   - boolean columns are BOOLEAN on PG (vs INTEGER 0/1 on SQLite). sqlx's
//     FromRow handles both transparently for the `bool` fields on our models.
//   - `LIKE 'prefix%'` stays — PG supports LIKE with the same wildcard.
//
// No migrations here (unlike SQLite). PG support is new in v0.4.3, so there
// are no "old PG databases" to upgrade; init_pg creates the full schema from
// pg_schema.rs on first boot. The contract: user installs PG, creates an empty
// database, fills in the connection string; the panel does the rest.
//
// Transaction isolation: PG defaults to READ COMMITTED (vs SQLite's
// effectively-SERIALIZABLE single-writer). apply_traffic_batch's ownership
// check + write run on the same tx handle, so a concurrent writer can't slip
// between the SELECT and UPDATE — but a concurrent transaction COULD update
// the same row after our SELECT and before our UPDATE, making our increment
// race. This matches the SQLite contract (which also isn't row-locked); the
// traffic counter is monotonic-additive so the worst case is a momentarily
// stale read, never a lost write (the UPDATE itself is atomic).

use sqlx::PgPool;

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

pub struct PgRepository {
    pub(super) pool: PgPool,
}

impl PgRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

// Helper: build a PG placeholder list `$1, $2, ..., $n` for n binds. Used by
// the dynamic SET builders (update_user_fields, update_rule_fields,
// update_group_fields, update_profile_fields) which construct SQL from the
// present fields and need to number the placeholders accordingly.
//
// PG's `format!`-built SQL is safe here exactly as in SQLite: the table +
// column names are compile-time literals (never user input), only the values
// are bound as parameters.
fn placeholders(start: usize, count: usize) -> String {
    (0..count)
        .map(|i| format!("${}", start + i))
        .collect::<Vec<_>>()
        .join(", ")
}

// ── Aggregate Repository ──

#[async_trait::async_trait]
impl super::repo::Repository for PgRepository {}

// NOTE: the `placeholders` helper above is currently unused (the dynamic SET
// builders inline the numbering instead), but kept for any future query that
// needs to build a placeholder list from a count (e.g. an IN (...) clause).
#[allow(dead_code)]
fn _placeholders_unused() {
    let _ = placeholders(1, 0);
}
