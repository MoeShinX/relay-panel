use super::PgRepository;
use crate::db::error::DbError;
use crate::db::repo::*;
use async_trait::async_trait;
use relay_shared::models::Plan;

// ── PlanRepository ──

#[async_trait]
impl PlanRepository for PgRepository {
    async fn list_plans(&self) -> Result<Vec<Plan>, DbError> {
        let plans: Vec<Plan> = sqlx::query_as("SELECT * FROM plans ORDER BY id")
            .fetch_all(&self.pool)
            .await?;
        Ok(plans)
    }

    async fn list_visible_plans(&self) -> Result<Vec<Plan>, DbError> {
        let plans: Vec<Plan> =
            sqlx::query_as("SELECT * FROM plans WHERE hidden = FALSE ORDER BY id")
                .fetch_all(&self.pool)
                .await?;
        Ok(plans)
    }

    async fn find_plan_name_by_id(&self, id: i64) -> Result<Option<String>, DbError> {
        let row: Option<(String,)> = sqlx::query_as("SELECT name FROM plans WHERE id = $1")
            .bind(id)
            .fetch_optional(&self.pool)
            .await?;
        Ok(row.map(|(n,)| n))
    }

    async fn find_plan_by_id(&self, id: i64) -> Result<Option<Plan>, DbError> {
        let plan: Option<Plan> = sqlx::query_as("SELECT * FROM plans WHERE id = $1")
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
        // RETURNING id (PG); speed_limit/ip_limit keep their defaults.
        let row: (i64,) = sqlx::query_as(
            "INSERT INTO plans \
             (name, max_rules, traffic, price, plan_type, duration_days, hidden, reset_traffic, description, grant_all_groups) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10) RETURNING id",
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
        .fetch_one(&self.pool)
        .await?;
        Ok(row.0)
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
        let row: (i64,) = sqlx::query_as(
            "INSERT INTO plans \
             (name, max_rules, traffic, price, plan_type, duration_days, hidden, reset_traffic, description, grant_all_groups) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10) RETURNING id",
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
        .fetch_one(&mut *tx)
        .await?;
        let id = row.0;
        for dg in device_group_ids {
            sqlx::query(
                "INSERT INTO plan_device_groups (plan_id, device_group_id) \
                 VALUES ($1, $2) ON CONFLICT DO NOTHING",
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
            sets.push("name = ");
        }
        if max_rules.is_some() {
            sets.push("max_rules = ");
        }
        if traffic.is_some() {
            sets.push("traffic = ");
        }
        if price.is_some() {
            sets.push("price = ");
        }
        if plan_type.is_some() {
            sets.push("plan_type = ");
        }
        if duration_days.is_some() {
            sets.push("duration_days = ");
        }
        if hidden.is_some() {
            sets.push("hidden = ");
        }
        if reset_traffic.is_some() {
            sets.push("reset_traffic = ");
        }
        if description.is_some() {
            sets.push("description = ");
        }
        if grant_all_groups.is_some() {
            sets.push("grant_all_groups = ");
        }

        if sets.is_empty() {
            return Ok(0);
        }

        let mut ph = 1;
        let sets_with_ph: Vec<String> = sets
            .iter()
            .map(|s| {
                let p = format!("{s}${ph}");
                ph += 1;
                p
            })
            .collect();
        let id_ph = ph;
        let sql = format!(
            "UPDATE plans SET {} WHERE id = ${}",
            sets_with_ph.join(", "),
            id_ph
        );

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
        let result = sqlx::query("DELETE FROM plans WHERE id = $1")
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected())
    }

    async fn count_users_on_plan(&self, plan_id: i64) -> Result<i64, DbError> {
        let row: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM users WHERE plan_id = $1")
            .bind(plan_id)
            .fetch_one(&self.pool)
            .await?;
        Ok(row.0)
    }

    // v1.0.8: atomic plan purchase. PG defaults to READ COMMITTED, so the
    // SELECT ... FOR UPDATE row lock is what prevents 防双花: a concurrent tx
    // trying to lock the same user row blocks until this commits, then reads
    // the post-deduction balance. The lock + the UPDATE run on the same tx.
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
        new_authorized_group_ids: &[i64],
    ) -> Result<(), BuyPlanError> {
        let mut tx = self.pool.begin().await?;

        // FOR UPDATE locks the user row for the tx's duration.
        let row: Option<(String, Option<String>)> =
            sqlx::query_as("SELECT balance, plan_expire_at FROM users WHERE id = $1 FOR UPDATE")
                .bind(user_id)
                .fetch_optional(&mut *tx)
                .await?;
        let Some((balance_str, current_expire)) = row else {
            let _ = tx.rollback().await;
            return Err(BuyPlanError::Database(DbError::NotFound));
        };

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

        // Compute the new expiry. duration_days=0 → NULL. Otherwise
        // max(now, current) + duration_days (renewals stack). PG:
        //   GREATEST(now_utc, COALESCE(current, now_utc)) + N * interval '1 day'
        // cast to TEXT in the canonical 'YYYY-MM-DD HH:MM:SS' format so it
        // compares lexically (same as created_at).
        let new_expire: Option<String> = if duration_days <= 0 {
            None
        } else {
            let row: (String,) = sqlx::query_as(
                "SELECT to_char( \
                   GREATEST(now() AT TIME ZONE 'UTC', \
                            COALESCE($1::timestamptz, now() AT TIME ZONE 'UTC')) \
                   + make_interval(days => $2), \
                   'YYYY-MM-DD HH24:MI:SS')",
            )
            .bind(&current_expire)
            .bind(duration_days)
            .fetch_one(&mut *tx)
            .await?;
            Some(row.0)
        };

        if reset_traffic {
            sqlx::query(
                "UPDATE users SET \
                 balance = $1, traffic_limit = traffic_limit + $2, max_rules = $3, \
                 plan_id = $4, plan_expire_at = $5, traffic_used = 0 \
                 WHERE id = $6",
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
                 balance = $1, traffic_limit = traffic_limit + $2, max_rules = $3, \
                 plan_id = $4, plan_expire_at = $5 \
                 WHERE id = $6",
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

        let price_str = relay_shared::money::cents_to_balance(price_cents);
        sqlx::query(
            "INSERT INTO orders (user_id, plan_id, plan_name, price) VALUES ($1, $2, $3, $4)",
        )
        .bind(user_id)
        .bind(plan_id)
        .bind(plan_name)
        .bind(&price_str)
        .execute(&mut *tx)
        .await?;

        // v1.0.9: grant device-group authorization in the SAME tx (mirrors the
        // SQLite impl). Purchase is ADDITIVE — the plan's grants are UNIONed into
        // the user's existing authorization; a purchase NEVER removes access
        // (this supersedes v1.0.8's replace semantics):
        //   - grant_all_groups → set all_device_groups=TRUE (union with "all" is
        //     "all"). Existing explicit rows are left in place (moot under the
        //     flag; purchase never deletes).
        //   - else → INSERT the plan's groups (ON CONFLICT DO NOTHING dedups the
        //     overlap), leaving the all-groups flag and existing rows untouched.
        // The caller passes new_authorized_group_ids = the user's FULL set after
        // this purchase (existing ∪ plan), which drives the resume step below.
        if grant_all_groups {
            sqlx::query(
                "UPDATE users SET all_device_groups = TRUE WHERE id = $1 AND admin = FALSE",
            )
            .bind(user_id)
            .execute(&mut *tx)
            .await?;
        } else {
            for dg_id in device_group_ids {
                sqlx::query(
                    "INSERT INTO user_device_groups (user_id, device_group_id) VALUES ($1, $2) \
                     ON CONFLICT DO NOTHING",
                )
                .bind(user_id)
                .bind(dg_id)
                .execute(&mut *tx)
                .await?;
            }
        }

        // Defensive: pause any active rule bound to a group the user is (still)
        // NOT authorized for. v1.0.9: under ADDITIVE purchase this is normally a
        // no-op (new_authorized ⊇ the previous set). It stays as a safety net.
        // Inline the pause logic inside the transaction (using &mut *tx) to
        // avoid acquiring a separate pool connection while the transaction is
        // still open — that would risk a pool-exhaustion deadlock.
        // auto_paused=TRUE marks these as SYSTEM pauses (see the resume step
        // below and the column doc on forward_rules.auto_paused).
        let n = if new_authorized_group_ids.is_empty() {
            let r = sqlx::query(
                "UPDATE forward_rules SET paused = TRUE, auto_paused = TRUE \
                 WHERE uid = $1 AND paused = FALSE",
            )
            .bind(user_id)
            .execute(&mut *tx)
            .await?;
            r.rows_affected()
        } else {
            let placeholders = (1..=new_authorized_group_ids.len())
                .map(|i| format!("${}", i + 1))
                .collect::<Vec<_>>()
                .join(", ");
            let sql = format!(
                "UPDATE forward_rules SET paused = TRUE, auto_paused = TRUE \
                 WHERE uid = $1 AND paused = FALSE AND device_group_in NOT IN ({})",
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
        // (auto_paused=TRUE) whose group is back in the new authorized set gets
        // un-paused here. A rule the user paused THEMSELVES (auto_paused=FALSE,
        // e.g. via the on/off switch) is deliberately left alone even if its
        // group is authorized again — buying a plan must never silently revive
        // a rule the user turned off on purpose.
        if !new_authorized_group_ids.is_empty() {
            let placeholders = (1..=new_authorized_group_ids.len())
                .map(|i| format!("${}", i + 1))
                .collect::<Vec<_>>()
                .join(", ");
            let sql = format!(
                "UPDATE forward_rules SET paused = FALSE, auto_paused = FALSE \
                 WHERE uid = $1 AND paused = TRUE AND auto_paused = TRUE \
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
             WHERE plan_id = $1 ORDER BY device_group_id",
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
        let mut tx = self.pool.begin().await?;
        sqlx::query("DELETE FROM plan_device_groups WHERE plan_id = $1")
            .bind(plan_id)
            .execute(&mut *tx)
            .await?;
        for dg_id in device_group_ids {
            sqlx::query(
                "INSERT INTO plan_device_groups (plan_id, device_group_id) \
                 VALUES ($1, $2) ON CONFLICT DO NOTHING",
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

fn parse_allowed_plan_ids(raw: &str, fallback: i64) -> Vec<i64> {
    serde_json::from_str::<Vec<i64>>(raw).unwrap_or_else(|_| vec![fallback])
}

fn serialize_allowed_plan_ids(ids: &[i64]) -> String {
    serde_json::to_string(ids).unwrap_or_else(|_| "[1]".to_string())
}

#[async_trait]
impl SettingsRepository for PgRepository {
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
            "INSERT INTO app_settings (id, registration_enabled, \
             default_registration_plan_id, registration_allowed_plan_ids) \
             VALUES (1, $1, $2, $3) \
             ON CONFLICT (id) DO NOTHING",
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
             VALUES (1, $1, $2, $3) \
             ON CONFLICT (id) DO UPDATE SET \
                 registration_enabled = EXCLUDED.registration_enabled, \
                 default_registration_plan_id = EXCLUDED.default_registration_plan_id, \
                 registration_allowed_plan_ids = EXCLUDED.registration_allowed_plan_ids",
        )
        .bind(enabled)
        .bind(default_plan_id)
        .bind(&allowed_json)
        .execute(&self.pool)
        .await?;
        Ok(())
    }
}
