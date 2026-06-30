use super::PgRepository;
use crate::db::error::DbError;
use crate::db::repo::*;
use async_trait::async_trait;

#[async_trait]
impl DeviceGroupAuthRepository for PgRepository {
    async fn list_user_device_groups(&self, user_id: i64) -> Result<Vec<i64>, DbError> {
        let rows: Vec<(i64,)> = sqlx::query_as(
            "SELECT device_group_id FROM user_device_groups \
             WHERE user_id = $1 ORDER BY device_group_id",
        )
        .bind(user_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.into_iter().map(|(id,)| id).collect())
    }

    async fn set_user_device_groups(
        &self,
        user_id: i64,
        device_group_ids: &[i64],
    ) -> Result<(), DbError> {
        let mut tx = self.pool.begin().await?;
        sqlx::query("DELETE FROM user_device_groups WHERE user_id = $1")
            .bind(user_id)
            .execute(&mut *tx)
            .await?;
        for dg_id in device_group_ids {
            sqlx::query(
                "INSERT INTO user_device_groups (user_id, device_group_id) \
                 VALUES ($1, $2) ON CONFLICT DO NOTHING",
            )
            .bind(user_id)
            .bind(dg_id)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(())
    }

    async fn set_user_all_device_groups(&self, user_id: i64, all: bool) -> Result<u64, DbError> {
        // Admins are always all-allowed in code, so leave their flag alone.
        let r =
            sqlx::query("UPDATE users SET all_device_groups = $1 WHERE id = $2 AND admin = FALSE")
                .bind(all)
                .bind(user_id)
                .execute(&self.pool)
                .await?;
        Ok(r.rows_affected())
    }

    async fn authorized_device_group_ids(&self, user_id: i64) -> Result<Vec<i64>, DbError> {
        // Admins and all_device_groups users get EVERY inbound group.
        let flags: Option<(bool, bool)> =
            sqlx::query_as("SELECT admin, all_device_groups FROM users WHERE id = $1")
                .bind(user_id)
                .fetch_optional(&self.pool)
                .await?;
        let (is_admin, all) = match flags {
            Some(f) => f,
            None => return Ok(Vec::new()),
        };
        if is_admin || all {
            let all_in: Vec<(i64,)> =
                sqlx::query_as("SELECT id FROM device_groups WHERE group_type = 'in' ORDER BY id")
                    .fetch_all(&self.pool)
                    .await?;
            return Ok(all_in.into_iter().map(|(id,)| id).collect());
        }
        // Otherwise only the user's explicit assignments (inbound groups only —
        // the authorized set is compared against rule.device_group_in).
        let rows: Vec<(i64,)> = sqlx::query_as(
            "SELECT dg.id FROM device_groups dg \
             JOIN user_device_groups udg ON udg.device_group_id = dg.id \
             WHERE udg.user_id = $1 AND dg.group_type = 'in' \
             ORDER BY dg.id",
        )
        .bind(user_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.into_iter().map(|(id,)| id).collect())
    }

    async fn pause_rules_outside_groups(
        &self,
        user_id: i64,
        allowed_group_ids: &[i64],
    ) -> Result<u64, DbError> {
        // Empty allowed list → pause ALL of the user's currently-active rules.
        if allowed_group_ids.is_empty() {
            let r =
                sqlx::query("UPDATE forward_rules SET paused = TRUE WHERE uid = $1 AND paused = FALSE")
                    .bind(user_id)
                    .execute(&self.pool)
                    .await?;
            return Ok(r.rows_affected());
        }
        // Build "device_group_in NOT IN ($2, $3, ...)" with bound params.
        let placeholders = (0..allowed_group_ids.len())
            .map(|i| format!("${}", i + 2))
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!(
            "UPDATE forward_rules SET paused = TRUE \
             WHERE uid = $1 AND paused = FALSE AND device_group_in NOT IN ({})",
            placeholders
        );
        let mut q = sqlx::query(&sql).bind(user_id);
        for gid in allowed_group_ids {
            q = q.bind(gid);
        }
        let r = q.execute(&self.pool).await?;
        Ok(r.rows_affected())
    }

    async fn is_user_restricted(&self, user_id: i64) -> Result<bool, DbError> {
        let row: Option<(bool, bool)> =
            sqlx::query_as("SELECT admin, all_device_groups FROM users WHERE id = $1")
                .bind(user_id)
                .fetch_optional(&self.pool)
                .await?;
        // Restricted = a non-admin without the all-device-groups flag.
        Ok(matches!(row, Some((false, false))))
    }
}
