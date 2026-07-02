use super::SqliteRepository;
use crate::db::error::DbError;
use crate::db::repo::*;
use async_trait::async_trait;
use relay_shared::models::Plan;

// ── PlanRepository ──

#[async_trait]
impl PlanRepository for SqliteRepository {
    async fn list_plans(&self) -> Result<Vec<Plan>, DbError> {
        let plans: Vec<Plan> = sqlx::query_as("SELECT * FROM plans ORDER BY id")
            .fetch_all(&self.pool)
            .await?;
        Ok(plans)
    }

    async fn list_visible_plans(&self) -> Result<Vec<Plan>, DbError> {
        let plans: Vec<Plan> = sqlx::query_as("SELECT * FROM plans WHERE hidden = 0 ORDER BY id")
            .fetch_all(&self.pool)
            .await?;
        Ok(plans)
    }

    async fn find_plan_name_by_id(&self, id: i64) -> Result<Option<String>, DbError> {
        let row: Option<(String,)> = sqlx::query_as("SELECT name FROM plans WHERE id = ?")
            .bind(id)
            .fetch_optional(&self.pool)
            .await?;
        Ok(row.map(|(n,)| n))
    }

    async fn find_plan_by_id(&self, id: i64) -> Result<Option<Plan>, DbError> {
        let plan: Option<Plan> = sqlx::query_as("SELECT * FROM plans WHERE id = ?")
            .bind(id)
            .fetch_optional(&self.pool)
            .await?;
        Ok(plan)
    }

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
    ) -> Result<i64, DbError> {
        // INSERT-then-last_insert_rowid (SQLite). speed_limit/ip_limit keep
        // their defaults (placeholders, never enforced) — not exposed here.
        let result = sqlx::query(
            "INSERT INTO plans \
             (name, max_rules, traffic, price, plan_type, duration_days, hidden, reset_traffic, description, grant_all_groups) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(name)
        .bind(max_rules)
        .bind(traffic)
        .bind(price)
        .bind(plan_type)
        .bind(duration_days)
        .bind(hidden)
        .bind(reset_traffic)
        .bind(description)
        .bind(grant_all_groups)
        .execute(&self.pool)
        .await?;
        Ok(result.last_insert_rowid())
    }

    #[allow(clippy::too_many_arguments)]
    async fn create_plan_with_groups(
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
        device_group_ids: &[i64],
    ) -> Result<i64, DbError> {
        let mut tx = self.pool.begin().await?;
        let result = sqlx::query(
            "INSERT INTO plans \
             (name, max_rules, traffic, price, plan_type, duration_days, hidden, reset_traffic, description, grant_all_groups) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(name)
        .bind(max_rules)
        .bind(traffic)
        .bind(price)
        .bind(plan_type)
        .bind(duration_days)
        .bind(hidden)
        .bind(reset_traffic)
        .bind(description)
        .bind(grant_all_groups)
        .execute(&mut *tx)
        .await?;
        let id = result.last_insert_rowid();
        for dg in device_group_ids {
            sqlx::query(
                "INSERT OR IGNORE INTO plan_device_groups (plan_id, device_group_id) \
                 VALUES (?, ?)",
            )
            .bind(id)
            .bind(dg)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(id)
    }

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
    ) -> Result<u64, DbError> {
        let mut sets: Vec<&str> = Vec::new();
        if name.is_some() {
            sets.push("name = ?");
        }
        if max_rules.is_some() {
            sets.push("max_rules = ?");
        }
        if traffic.is_some() {
            sets.push("traffic = ?");
        }
        if price.is_some() {
            sets.push("price = ?");
        }
        if plan_type.is_some() {
            sets.push("plan_type = ?");
        }
        if duration_days.is_some() {
            sets.push("duration_days = ?");
        }
        if hidden.is_some() {
            sets.push("hidden = ?");
        }
        if reset_traffic.is_some() {
            sets.push("reset_traffic = ?");
        }
        if description.is_some() {
            sets.push("description = ?");
        }
        if grant_all_groups.is_some() {
            sets.push("grant_all_groups = ?");
        }

        if sets.is_empty() {
            return Ok(0);
        }

        let sql = format!("UPDATE plans SET {} WHERE id = ?", sets.join(", "));
        let mut q = sqlx::query(&sql);
        if let Some(v) = name {
            q = q.bind(v);
        }
        if let Some(v) = max_rules {
            q = q.bind(v);
        }
        if let Some(v) = traffic {
            q = q.bind(v);
        }
        if let Some(v) = price {
            q = q.bind(v);
        }
        if let Some(v) = plan_type {
            q = q.bind(v);
        }
        if let Some(v) = duration_days {
            q = q.bind(v);
        }
        if let Some(v) = hidden {
            q = q.bind(v);
        }
        if let Some(v) = reset_traffic {
            q = q.bind(v);
        }
        if let Some(v) = description {
            q = q.bind(v);
        }
        if let Some(v) = grant_all_groups {
            q = q.bind(v);
        }
        q = q.bind(id);

        let result = q.execute(&self.pool).await?;
        Ok(result.rows_affected())
    }

    async fn delete_plan(&self, id: i64) -> Result<u64, DbError> {
        let result = sqlx::query("DELETE FROM plans WHERE id = ?")
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected())
    }

    async fn count_users_on_plan(&self, plan_id: i64) -> Result<i64, DbError> {
        let row: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM users WHERE plan_id = ?")
            .bind(plan_id)
            .fetch_one(&self.pool)
            .await?;
        Ok(row.0)
    }

    // v1.0.8: atomic plan purchase. This opens a DEFERRED transaction
    // (pool.begin()); the write lock is acquired lazily on the first write
    // (the UPDATE below), not at BEGIN. SQLite serializes writers, so under
    // concurrent purchases the second writer either blocks briefly or fails
    // with SQLITE_BUSY (the caller retries) — never a double-deduction,
    // because the balance read + deduct + write all run inside this one tx
    // against a consistent snapshot. (PG uses an explicit SELECT ... FOR
    // UPDATE row lock instead; see the PG impl.)
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
    ) -> Result<(), BuyPlanError> {
        let mut tx = self.pool.begin().await?;

        // Read the user's current balance (canonical TEXT) + current expiry.
        let row: Option<(String, Option<String>)> =
            sqlx::query_as("SELECT balance, plan_expire_at FROM users WHERE id = ?")
                .bind(user_id)
                .fetch_optional(&mut *tx)
                .await?;
        let Some((balance_str, current_expire)) = row else {
            let _ = tx.rollback().await;
            // A missing user mid-purchase is a DB integrity issue, not a
            // balance issue — surface as a 500.
            return Err(BuyPlanError::Database(DbError::NotFound));
        };

        // Decimal math in integer cents (no floats). balance_to_cents returns
        // None on a non-canonical string — treat that as a data-integrity fault
        // and refuse the purchase (500) rather than mis-billing.
        let balance_cents =
            relay_shared::money::balance_to_cents(&balance_str).ok_or_else(|| {
                tracing::error!(
                    "buy_plan: user {} has non-canonical balance {:?}",
                    user_id,
                    balance_str
                );
                BuyPlanError::Database(DbError::NotFound)
            })?;
        if balance_cents < price_cents {
            let _ = tx.rollback().await;
            return Err(BuyPlanError::InsufficientBalance);
        }
        let new_balance = relay_shared::money::cents_to_balance(balance_cents - price_cents);

        // Compute the new expiry. duration_days=0 → NULL (no expiry). Otherwise
        // the new expiry = max(now, current) + duration_days, so renewals stack
        // rather than being clipped to "now + days" when the user still has
        // time left. Stored as 'YYYY-MM-DD HH:MM:SS' UTC (lexically comparable,
        // same format as created_at). SQLite's datetime() does the civil-
        // calendar math for us — the base is chosen via max() in SQL.
        let new_expire: Option<String> = if duration_days <= 0 {
            None
        } else {
            // base = max(now, current_expire). A NULL current_expire → now.
            // datetime(base, '+N days') yields 'YYYY-MM-DD HH:MM:SS' UTC.
            let row: (String,) = sqlx::query_as(
                "SELECT datetime(MAX(datetime('now'), COALESCE(?, datetime('now'))), ? || ' days')",
            )
            .bind(&current_expire)
            .bind(format!("+{}", duration_days))
            .fetch_one(&mut *tx)
            .await?;
            Some(row.0)
        };

        // Apply the user update. traffic_limit += traffic_to_add (stacks on top
        // of any remaining quota — the "购买=累加流量" contract). reset_traffic
        // zeros traffic_used in the same UPDATE.
        if reset_traffic {
            sqlx::query(
                "UPDATE users SET \
                 balance = ?, traffic_limit = traffic_limit + ?, max_rules = ?, \
                 plan_id = ?, plan_expire_at = ?, traffic_used = 0 \
                 WHERE id = ?",
            )
            .bind(&new_balance)
            .bind(traffic_to_add)
            .bind(plan_max_rules)
            .bind(plan_id)
            .bind(&new_expire)
            .bind(user_id)
            .execute(&mut *tx)
            .await?;
        } else {
            sqlx::query(
                "UPDATE users SET \
                 balance = ?, traffic_limit = traffic_limit + ?, max_rules = ?, \
                 plan_id = ?, plan_expire_at = ? \
                 WHERE id = ?",
            )
            .bind(&new_balance)
            .bind(traffic_to_add)
            .bind(plan_max_rules)
            .bind(plan_id)
            .bind(&new_expire)
            .bind(user_id)
            .execute(&mut *tx)
            .await?;
        }

        // Insert the order row (snapshots plan_name + the canonical price).
        let price_str = relay_shared::money::cents_to_balance(price_cents);
        sqlx::query("INSERT INTO orders (user_id, plan_id, plan_name, price) VALUES (?, ?, ?, ?)")
            .bind(user_id)
            .bind(plan_id)
            .bind(plan_name)
            .bind(&price_str)
            .execute(&mut *tx)
            .await?;

        // v1.0.9: grant device-group authorization in the SAME tx. Purchase is
        // ADDITIVE — the plan's grants are UNIONed into the user's existing
        // authorization; a purchase NEVER removes access (this supersedes
        // v1.0.8's replace semantics). Traffic/expiry stack independently above.
        //   - grant_all_groups → set all_device_groups=1 (union with "all" is
        //     "all"). Existing explicit rows are left in place — under the flag
        //     they're moot, and purchase never deletes.
        //   - else → INSERT the plan's groups (OR IGNORE dedups the overlap),
        //     leaving the all-groups flag and existing rows untouched. If the
        //     user was already unrestricted (all=1), the new rows are redundant.
        // The caller passes new_authorized_group_ids = the user's FULL set after
        // this purchase (existing ∪ plan), which drives the resume step below.
        if grant_all_groups {
            sqlx::query("UPDATE users SET all_device_groups = 1 WHERE id = ? AND admin = 0")
                .bind(user_id)
                .execute(&mut *tx)
                .await?;
        } else {
            for dg_id in device_group_ids {
                sqlx::query(
                    "INSERT OR IGNORE INTO user_device_groups (user_id, device_group_id) \
                     VALUES (?, ?)",
                )
                .bind(user_id)
                .bind(dg_id)
                .execute(&mut *tx)
                .await?;
            }
        }

        // Defensive: pause any active rule bound to a group the user is (still)
        // NOT authorized for. v1.0.9: under ADDITIVE purchase this is normally a
        // no-op — new_authorized ⊇ the user's previous set, so a purchase never
        // pauses a rule it didn't already invalidate. It stays as a safety net
        // for a rule left bound to a group the user was never authorized for.
        // Inline the pause logic inside the transaction (using &mut *tx) to
        // avoid acquiring a separate pool connection while the transaction is
        // still open — that would risk a pool-exhaustion deadlock.
        // auto_paused=1 marks these as SYSTEM pauses (see the resume step below
        // and the column doc on forward_rules.auto_paused).
        let n = if new_authorized_group_ids.is_empty() {
            let r = sqlx::query(
                "UPDATE forward_rules SET paused = 1, auto_paused = 1 \
                 WHERE uid = ? AND paused = 0",
            )
            .bind(user_id)
            .execute(&mut *tx)
            .await?;
            r.rows_affected()
        } else {
            let placeholders = vec!["?"; new_authorized_group_ids.len()].join(", ");
            let sql = format!(
                "UPDATE forward_rules SET paused = 1, auto_paused = 1 \
                 WHERE uid = ? AND paused = 0 AND device_group_in NOT IN ({})",
                placeholders
            );
            let mut q = sqlx::query(&sql).bind(user_id);
            for gid in new_authorized_group_ids {
                q = q.bind(gid);
            }
            let r = q.execute(&mut *tx).await?;
            r.rows_affected()
        };
        if n > 0 {
            tracing::warn!(
                "buy_plan: user {} purchased plan {}, {} rule(s) paused due to authorization change",
                user_id, plan_id, n
            );
        }

        // v1.0.8: symmetric auto-resume — a rule this system previously paused
        // (auto_paused=1) whose group is back in the new authorized set gets
        // un-paused here. A rule the user paused THEMSELVES (auto_paused=0,
        // e.g. via the on/off switch) is deliberately left alone even if its
        // group is authorized again — buying a plan must never silently revive
        // a rule the user turned off on purpose.
        if !new_authorized_group_ids.is_empty() {
            let placeholders = vec!["?"; new_authorized_group_ids.len()].join(", ");
            let sql = format!(
                "UPDATE forward_rules SET paused = 0, auto_paused = 0 \
                 WHERE uid = ? AND paused = 1 AND auto_paused = 1 \
                 AND device_group_in IN ({})",
                placeholders
            );
            let mut q = sqlx::query(&sql).bind(user_id);
            for gid in new_authorized_group_ids {
                q = q.bind(gid);
            }
            let resumed = q.execute(&mut *tx).await?.rows_affected();
            if resumed > 0 {
                tracing::info!(
                    "buy_plan: user {} purchased plan {}, {} previously auto-paused rule(s) resumed",
                    user_id, plan_id, resumed
                );
            }
        }

        tx.commit().await?;
        Ok(())
    }

    async fn list_plan_device_groups(&self, plan_id: i64) -> Result<Vec<i64>, DbError> {
        let rows: Vec<(i64,)> = sqlx::query_as(
            "SELECT device_group_id FROM plan_device_groups \
             WHERE plan_id = ? ORDER BY device_group_id",
        )
        .bind(plan_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.into_iter().map(|(id,)| id).collect())
    }

    async fn set_plan_device_groups(
        &self,
        plan_id: i64,
        device_group_ids: &[i64],
    ) -> Result<(), DbError> {
        // REPLACE the grant set (delete-then-insert, deduped via the PK).
        let mut tx = self.pool.begin().await?;
        sqlx::query("DELETE FROM plan_device_groups WHERE plan_id = ?")
            .bind(plan_id)
            .execute(&mut *tx)
            .await?;
        for dg_id in device_group_ids {
            sqlx::query(
                "INSERT OR IGNORE INTO plan_device_groups (plan_id, device_group_id) \
                 VALUES (?, ?)",
            )
            .bind(plan_id)
            .bind(dg_id)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(())
    }
}

// ── Helpers ──

/// Parse the `registration_allowed_plan_ids` TEXT column as a JSON `Vec<i64>`.
/// Falls back to `[default_plan_id]` on parse failure (dirty data).
fn parse_allowed_plan_ids(raw: &str, fallback: i64) -> Vec<i64> {
    serde_json::from_str::<Vec<i64>>(raw).unwrap_or_else(|_| vec![fallback])
}

/// Serialize a `Vec<i64>` to a JSON string.
fn serialize_allowed_plan_ids(ids: &[i64]) -> String {
    serde_json::to_string(ids).unwrap_or_else(|_| "[1]".to_string())
}

#[async_trait]
impl SettingsRepository for SqliteRepository {
    async fn get_registration_settings(&self) -> Result<Option<RegistrationSettings>, DbError> {
        let row: Option<(bool, i64, String)> = sqlx::query_as(
            "SELECT registration_enabled, default_registration_plan_id, \
             registration_allowed_plan_ids FROM app_settings WHERE id = 1",
        )
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(|(enabled, plan_id, raw_allowed)| {
            let allowed = parse_allowed_plan_ids(&raw_allowed, plan_id);
            RegistrationSettings {
                registration_enabled: enabled,
                default_registration_plan_id: plan_id,
                allowed_plan_ids: allowed,
            }
        }))
    }

    async fn insert_settings_if_absent(
        &self,
        enabled: bool,
        default_plan_id: i64,
        allowed_plan_ids: &[i64],
    ) -> Result<(), DbError> {
        let allowed_json = serialize_allowed_plan_ids(allowed_plan_ids);
        sqlx::query(
            "INSERT OR IGNORE INTO app_settings (id, registration_enabled, \
             default_registration_plan_id, registration_allowed_plan_ids) \
             VALUES (1, ?, ?, ?)",
        )
        .bind(enabled)
        .bind(default_plan_id)
        .bind(&allowed_json)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn set_registration_settings(
        &self,
        enabled: bool,
        default_plan_id: i64,
        allowed_plan_ids: &[i64],
    ) -> Result<(), DbError> {
        let allowed_json = serialize_allowed_plan_ids(allowed_plan_ids);
        sqlx::query(
            "INSERT INTO app_settings (id, registration_enabled, \
             default_registration_plan_id, registration_allowed_plan_ids) \
             VALUES (1, ?, ?, ?) \
             ON CONFLICT(id) DO UPDATE SET \
                 registration_enabled = excluded.registration_enabled, \
                 default_registration_plan_id = excluded.default_registration_plan_id, \
                 registration_allowed_plan_ids = excluded.registration_allowed_plan_ids",
        )
        .bind(enabled)
        .bind(default_plan_id)
        .bind(&allowed_json)
        .execute(&self.pool)
        .await?;
        Ok(())
    }
}
