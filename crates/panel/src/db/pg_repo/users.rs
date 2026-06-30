use super::PgRepository;
use crate::db::error::DbError;
use crate::db::repo::*;
use async_trait::async_trait;
use relay_shared::models::User;

// ── UserRepository ──

#[async_trait]
impl UserRepository for PgRepository {
    async fn find_by_username_not_banned(&self, username: &str) -> Result<Option<User>, DbError> {
        let user = sqlx::query_as("SELECT * FROM users WHERE username = $1 AND banned = FALSE")
            .bind(username)
            .fetch_optional(&self.pool)
            .await?;
        Ok(user)
    }

    async fn find_by_username(&self, username: &str) -> Result<Option<User>, DbError> {
        let user = sqlx::query_as("SELECT * FROM users WHERE username = $1")
            .bind(username)
            .fetch_optional(&self.pool)
            .await?;
        Ok(user)
    }

    async fn find_by_id(&self, id: i64) -> Result<Option<User>, DbError> {
        let user = sqlx::query_as("SELECT * FROM users WHERE id = $1")
            .bind(id)
            .fetch_optional(&self.pool)
            .await?;
        Ok(user)
    }

    async fn find_password_by_id(&self, id: i64) -> Result<Option<String>, DbError> {
        let row: Option<(String,)> = sqlx::query_as("SELECT password FROM users WHERE id = $1")
            .bind(id)
            .fetch_optional(&self.pool)
            .await?;
        Ok(row.map(|(p,)| p))
    }

    async fn find_banned_by_id(&self, id: i64) -> Result<Option<bool>, DbError> {
        let row: Option<(bool,)> = sqlx::query_as("SELECT banned FROM users WHERE id = $1")
            .bind(id)
            .fetch_optional(&self.pool)
            .await?;
        Ok(row.map(|(b,)| b))
    }

    async fn find_auth_state_by_id(&self, id: i64) -> Result<Option<(bool, i64, bool)>, DbError> {
        let row: Option<(bool, i64, bool)> = sqlx::query_as(
            "SELECT banned, token_version, must_change_password FROM users WHERE id = $1",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row)
    }

    async fn is_admin(&self, id: i64) -> Result<bool, DbError> {
        let row: Option<(i32,)> =
            sqlx::query_as("SELECT 1 FROM users WHERE id = $1 AND admin = TRUE")
                .bind(id)
                .fetch_optional(&self.pool)
                .await?;
        Ok(row.is_some())
    }

    async fn exists_by_id(&self, id: i64) -> Result<bool, DbError> {
        let row: Option<(i32,)> = sqlx::query_as("SELECT 1 FROM users WHERE id = $1")
            .bind(id)
            .fetch_optional(&self.pool)
            .await?;
        Ok(row.is_some())
    }

    async fn insert_user(
        &self,
        username: &str,
        password_hash: &str,
        plan_id: i64,
    ) -> Result<(), DbError> {
        sqlx::query("INSERT INTO users (username, password, plan_id) VALUES ($1, $2, $3)")
            .bind(username)
            .bind(password_hash)
            .bind(plan_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    async fn insert_user_from_plan(
        &self,
        username: &str,
        password_hash: &str,
        plan_id: i64,
    ) -> Result<u64, DbError> {
        // Atomic INSERT...SELECT: copies the plan's quota fields into the new
        // user row in one statement. PG positional params can be reused, so
        // $3 serves both the plan_id column value AND the WHERE filter (unlike
        // SQLite's positional ? which must be bound once per occurrence).
        // 0 rows_affected = plan missing → caller fails the registration.
        let result = sqlx::query(
            "INSERT INTO users (username, password, plan_id, max_rules, traffic_limit, speed_limit, ip_limit) \
             SELECT $1, $2, $3, max_rules, traffic, speed_limit, ip_limit \
             FROM plans WHERE id = $3",
        )
        .bind(username)
        .bind(password_hash)
        .bind(plan_id)
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected())
    }

    async fn update_password(&self, id: i64, new_hash: &str) -> Result<u64, DbError> {
        let result = sqlx::query("UPDATE users SET password = $1 WHERE id = $2")
            .bind(new_hash)
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected())
    }

    async fn change_own_password(&self, id: i64, new_hash: &str) -> Result<u64, DbError> {
        let result = sqlx::query(
            "UPDATE users SET password = $1, token_version = token_version + 1, \
             must_change_password = FALSE WHERE id = $2",
        )
        .bind(new_hash)
        .bind(id)
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected())
    }

    async fn admin_reset_password(
        &self,
        id: i64,
        new_hash: &str,
        must_change_password: bool,
    ) -> Result<u64, DbError> {
        let result = sqlx::query(
            "UPDATE users SET password = $1, token_version = token_version + 1, \
             must_change_password = $2 WHERE id = $3",
        )
        .bind(new_hash)
        .bind(must_change_password)
        .bind(id)
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected())
    }

    async fn update_user_fields(
        &self,
        id: i64,
        balance: Option<&str>,
        max_rules: Option<i32>,
        traffic_limit: Option<i64>,
        banned: Option<bool>,
        suspended: Option<bool>,
    ) -> Result<u64, DbError> {
        // Build SET clause + bind values in the same field order as SQLite.
        // PG needs numbered placeholders; we accumulate binds in a Vec and
        // generate `$1, $2, ...` after we know how many there are.
        let mut sets: Vec<&str> = Vec::new();
        if balance.is_some() {
            sets.push("balance = ");
        }
        if max_rules.is_some() {
            sets.push("max_rules = ");
        }
        if traffic_limit.is_some() {
            sets.push("traffic_limit = ");
        }
        if banned.is_some() {
            sets.push("banned = ");
        }
        // v1.0.8: suspension (no token_version bump — user stays signed in).
        if suspended.is_some() {
            sets.push("suspended = ");
        }

        if sets.is_empty() {
            return Ok(0);
        }

        // Number the placeholders. id is always the last bind.
        let mut ph = 1;
        let mut sets_with_ph: Vec<String> = sets
            .iter()
            .map(|s| {
                let p = format!("{s}${ph}");
                ph += 1;
                p
            })
            .collect();
        // v0.4.10 PR4: banning revokes sessions via a token_version self-
        // increment (a literal expression, NOT a bound placeholder), appended
        // only when banning. Added after the numbered sets so placeholder
        // numbering is unaffected.
        if banned == Some(true) {
            sets_with_ph.push("token_version = token_version + 1".to_string());
        }
        let sql = format!(
            "UPDATE users SET {} WHERE id = ${}",
            sets_with_ph.join(", "),
            ph
        );

        let mut q = sqlx::query(&sql);
        if let Some(v) = balance {
            q = q.bind(v);
        }
        if let Some(v) = max_rules {
            q = q.bind(v);
        }
        if let Some(v) = traffic_limit {
            q = q.bind(v);
        }
        if let Some(v) = banned {
            q = q.bind(v);
        }
        if let Some(v) = suspended {
            q = q.bind(v);
        }
        q = q.bind(id);

        let result = q.execute(&self.pool).await?;
        Ok(result.rows_affected())
    }

    async fn increment_user_traffic(&self, id: i64, delta: i64) -> Result<(), DbError> {
        sqlx::query("UPDATE users SET traffic_used = traffic_used + $1 WHERE id = $2")
            .bind(delta)
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    async fn reset_traffic(&self, id: i64) -> Result<(), DbError> {
        let mut tx = self.pool.begin().await?;
        sqlx::query("UPDATE users SET traffic_used = 0 WHERE id = $1")
            .bind(id)
            .execute(&mut *tx)
            .await?;
        sqlx::query("UPDATE forward_rules SET traffic_used = 0 WHERE uid = $1")
            .bind(id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(())
    }

    async fn delete_non_admin(&self, id: i64) -> Result<u64, DbError> {
        let result = sqlx::query("DELETE FROM users WHERE id = $1 AND admin = FALSE")
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected())
    }

    async fn delete_user_cascade(&self, uid: i64) -> Result<u64, DbError> {
        // v0.4.4: one atomic transaction (was two un-transacted DELETEs that
        // missed tunnel_profiles). FK order: forward_rules → tunnel_profiles →
        // device_groups → users. The user delete carries the admin guard; if it
        // affects 0 rows (admin or gone) we roll back so the cascade is undone.
        let mut tx = self.pool.begin().await?;
        sqlx::query("DELETE FROM forward_rules WHERE uid = $1")
            .bind(uid)
            .execute(&mut *tx)
            .await?;
        sqlx::query("DELETE FROM tunnel_profiles WHERE uid = $1")
            .bind(uid)
            .execute(&mut *tx)
            .await?;
        sqlx::query("DELETE FROM device_groups WHERE uid = $1")
            .bind(uid)
            .execute(&mut *tx)
            .await?;
        let result = sqlx::query("DELETE FROM users WHERE id = $1 AND admin = FALSE")
            .bind(uid)
            .execute(&mut *tx)
            .await?;
        if result.rows_affected() == 0 {
            tx.rollback().await?;
            return Ok(0);
        }
        tx.commit().await?;
        Ok(result.rows_affected())
    }

    async fn list_users_public(&self) -> Result<Vec<crate::api::admin::UserPublic>, DbError> {
        let users: Vec<crate::api::admin::UserPublic> =
            sqlx::query_as("SELECT * FROM users ORDER BY id")
                .fetch_all(&self.pool)
                .await?;
        Ok(users)
    }

    async fn count_placeholder_admin_password(&self) -> Result<i64, DbError> {
        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM users WHERE id = 1 AND password LIKE '$2b$12$PLACEHOLDER%'",
        )
        .fetch_one(&self.pool)
        .await?;
        Ok(count)
    }

    async fn replace_placeholder_admin_password(&self, hash: &str) -> Result<(), DbError> {
        // Also set must_change_password so the seeded "admin123" forces a change
        // on first login. This fires ONLY while the password is still the
        // placeholder (first boot); once the admin sets a real password the LIKE
        // guard never matches again, so we never re-flag a real account.
        sqlx::query(
            "UPDATE users SET password = $1, must_change_password = TRUE \
             WHERE id = 1 AND password LIKE '$2b$12$PLACEHOLDER%'",
        )
        .bind(hash)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

}
