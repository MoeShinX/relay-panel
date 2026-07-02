use super::SqliteRepository;
use crate::db::error::DbError;
use crate::db::repo::*;
use async_trait::async_trait;
use relay_shared::models::User;

// ── UserRepository ──

#[async_trait]
impl UserRepository for SqliteRepository {
    async fn find_by_username_not_banned(&self, username: &str) -> Result<Option<User>, DbError> {
        let user = sqlx::query_as("SELECT * FROM users WHERE username = ? AND banned = 0")
            .bind(username)
            .fetch_optional(&self.pool)
            .await?;
        Ok(user)
    }

    async fn find_by_username(&self, username: &str) -> Result<Option<User>, DbError> {
        let user = sqlx::query_as("SELECT * FROM users WHERE username = ?")
            .bind(username)
            .fetch_optional(&self.pool)
            .await?;
        Ok(user)
    }

    async fn find_by_id(&self, id: i64) -> Result<Option<User>, DbError> {
        let user = sqlx::query_as("SELECT * FROM users WHERE id = ?")
            .bind(id)
            .fetch_optional(&self.pool)
            .await?;
        Ok(user)
    }

    async fn find_password_by_id(&self, id: i64) -> Result<Option<String>, DbError> {
        let row: Option<(String,)> = sqlx::query_as("SELECT password FROM users WHERE id = ?")
            .bind(id)
            .fetch_optional(&self.pool)
            .await?;
        Ok(row.map(|(p,)| p))
    }

    async fn find_banned_by_id(&self, id: i64) -> Result<Option<bool>, DbError> {
        let row: Option<(bool,)> = sqlx::query_as("SELECT banned FROM users WHERE id = ?")
            .bind(id)
            .fetch_optional(&self.pool)
            .await?;
        Ok(row.map(|(b,)| b))
    }

    async fn find_auth_state_by_id(&self, id: i64) -> Result<Option<(bool, i64, bool)>, DbError> {
        let row: Option<(bool, i64, bool)> = sqlx::query_as(
            "SELECT banned, token_version, must_change_password FROM users WHERE id = ?",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row)
    }

    async fn is_admin(&self, id: i64) -> Result<bool, DbError> {
        let row: Option<(i64,)> = sqlx::query_as("SELECT 1 FROM users WHERE id = ? AND admin = 1")
            .bind(id)
            .fetch_optional(&self.pool)
            .await?;
        Ok(row.is_some())
    }

    async fn exists_by_id(&self, id: i64) -> Result<bool, DbError> {
        let row: Option<(i64,)> = sqlx::query_as("SELECT 1 FROM users WHERE id = ?")
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
        sqlx::query("INSERT INTO users (username, password, plan_id) VALUES (?, ?, ?)")
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
        // user row in one statement. If the plan doesn't exist the SELECT
        // yields no row → 0 rows_affected (caller fails the registration).
        // Note the column mapping: plans.traffic → users.traffic_limit.
        let result = sqlx::query(
            "INSERT INTO users (username, password, plan_id, max_rules, traffic_limit, speed_limit, ip_limit) \
             SELECT ?, ?, ?, max_rules, traffic, speed_limit, ip_limit \
             FROM plans WHERE id = ?",
        )
        .bind(username)
        .bind(password_hash)
        .bind(plan_id)
        .bind(plan_id)
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected())
    }

    async fn update_password(&self, id: i64, new_hash: &str) -> Result<u64, DbError> {
        let result = sqlx::query("UPDATE users SET password = ? WHERE id = ?")
            .bind(new_hash)
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected())
    }

    async fn change_own_password(&self, id: i64, new_hash: &str) -> Result<u64, DbError> {
        // Atomic: new hash + bump token_version (revoke all sessions) + clear
        // must_change_password, in one UPDATE.
        let result = sqlx::query(
            "UPDATE users SET password = ?, token_version = token_version + 1, \
             must_change_password = 0 WHERE id = ?",
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
            "UPDATE users SET password = ?, token_version = token_version + 1, \
             must_change_password = ? WHERE id = ?",
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
        let mut sets: Vec<&str> = Vec::new();
        if balance.is_some() {
            sets.push("balance = ?");
        }
        if max_rules.is_some() {
            sets.push("max_rules = ?");
        }
        if traffic_limit.is_some() {
            sets.push("traffic_limit = ?");
        }
        if banned.is_some() {
            sets.push("banned = ?");
        }
        // v1.0.8: suspension. Unlike banned, suspended does NOT bump
        // token_version (the user stays signed in; forwarding is gated by
        // list_active_for_config).
        if suspended.is_some() {
            sets.push("suspended = ?");
        }
        // v0.4.10 PR4: banning a user revokes their sessions. token_version is
        // a self-increment expression (no bind), appended only when banning.
        if banned == Some(true) {
            sets.push("token_version = token_version + 1");
        }

        if sets.is_empty() {
            return Ok(0);
        }

        let sql = format!("UPDATE users SET {} WHERE id = ?", sets.join(", "));
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

    async fn admin_set_user_plan(
        &self,
        id: i64,
        plan_id: Option<i64>,
        plan_expire_at: Option<String>,
    ) -> Result<u64, DbError> {
        let result = sqlx::query(
            "UPDATE users SET plan_id = ?, plan_expire_at = ? WHERE id = ? AND admin = 0",
        )
        .bind(plan_id)
        .bind(plan_expire_at)
        .bind(id)
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected())
    }

    async fn clear_user_plan(&self, user_id: i64) -> Result<u64, DbError> {
        let mut tx = self.pool.begin().await?;
        // Clear plan association + revoke the all-groups flag in one UPDATE.
        let r = sqlx::query(
            "UPDATE users SET plan_id = NULL, plan_expire_at = NULL, all_device_groups = 0 \
             WHERE id = ? AND admin = 0",
        )
        .bind(user_id)
        .execute(&mut *tx)
        .await?;
        let affected = r.rows_affected();
        if affected == 0 {
            // Missing user or an admin target — nothing to clear.
            let _ = tx.rollback().await;
            return Ok(0);
        }
        // Drop explicit group grants, then system-pause every active rule
        // (auto_paused=1) since the user now has no authorized groups.
        sqlx::query("DELETE FROM user_device_groups WHERE user_id = ?")
            .bind(user_id)
            .execute(&mut *tx)
            .await?;
        sqlx::query(
            "UPDATE forward_rules SET paused = 1, auto_paused = 1 WHERE uid = ? AND paused = 0",
        )
        .bind(user_id)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(affected)
    }

    async fn increment_user_traffic(&self, id: i64, delta: i64) -> Result<(), DbError> {
        sqlx::query("UPDATE users SET traffic_used = traffic_used + ? WHERE id = ?")
            .bind(delta)
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    async fn reset_traffic(&self, id: i64) -> Result<(), DbError> {
        let mut tx = self.pool.begin().await?;
        sqlx::query("UPDATE users SET traffic_used = 0 WHERE id = ?")
            .bind(id)
            .execute(&mut *tx)
            .await?;
        sqlx::query("UPDATE forward_rules SET traffic_used = 0 WHERE uid = ?")
            .bind(id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(())
    }

    async fn delete_non_admin(&self, id: i64) -> Result<u64, DbError> {
        let result = sqlx::query("DELETE FROM users WHERE id = ? AND admin = 0")
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected())
    }

    async fn delete_user_cascade(&self, uid: i64) -> Result<u64, DbError> {
        // v0.4.4: one atomic transaction. Previously this deleted rules + groups
        // in two un-transacted statements, MISSED tunnel_profiles entirely, and
        // left the user row to a separate delete_non_admin call — so a user with
        // a custom tunnel profile would have rules+groups permanently deleted and
        // THEN fail the FK check on the user delete, leaving the account half-gone.
        //
        // Delete order respects the FK graph: forward_rules references both
        // tunnel_profiles and device_groups, so it goes first; tunnel_profiles and
        // device_groups both reference users, so the user row goes last. The user
        // delete carries the `admin = 0` guard, and if it affects 0 rows (admin or
        // already gone) we roll the whole thing back by returning before commit.
        let mut tx = self.pool.begin().await?;
        sqlx::query("DELETE FROM forward_rules WHERE uid = ?")
            .bind(uid)
            .execute(&mut *tx)
            .await?;
        sqlx::query("DELETE FROM tunnel_profiles WHERE uid = ?")
            .bind(uid)
            .execute(&mut *tx)
            .await?;
        sqlx::query("DELETE FROM device_groups WHERE uid = ?")
            .bind(uid)
            .execute(&mut *tx)
            .await?;
        let result = sqlx::query("DELETE FROM users WHERE id = ? AND admin = 0")
            .bind(uid)
            .execute(&mut *tx)
            .await?;
        if result.rows_affected() == 0 {
            // Admin or non-existent: roll back so the cascade above is undone.
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
            "UPDATE users SET password = ?, must_change_password = 1 \
             WHERE id = 1 AND password LIKE '$2b$12$PLACEHOLDER%'",
        )
        .bind(hash)
        .execute(&self.pool)
        .await?;
        Ok(())
    }
}
