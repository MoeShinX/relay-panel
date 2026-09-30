use super::PgRepository;
use crate::db::error::DbError;
use crate::db::repo::*;
use async_trait::async_trait;
use sqlx::PgConnection;

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
        replace_device_groups(&mut tx, user_id, device_group_ids).await?;
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
        let mut conn = self.pool.acquire().await?;
        Ok(authorized_ids(&mut conn, user_id).await?)
    }

    async fn pause_rules_outside_groups(
        &self,
        user_id: i64,
        allowed_group_ids: &[i64],
    ) -> Result<u64, DbError> {
        let mut conn = self.pool.acquire().await?;
        Ok(pause_outside(&mut conn, user_id, allowed_group_ids).await?)
    }

    async fn update_user_authorization(
        &self,
        user_id: i64,
        all_device_groups: Option<bool>,
        device_group_ids: Option<&[i64]>,
    ) -> Result<u64, DbError> {
        let mut tx = self.pool.begin().await?;
        if let Some(all) = all_device_groups {
            // Admins are always all-allowed in code, so leave their flag alone.
            sqlx::query("UPDATE users SET all_device_groups = $1 WHERE id = $2 AND admin = FALSE")
                .bind(all)
                .bind(user_id)
                .execute(&mut *tx)
                .await?;
        }
        if let Some(ids) = device_group_ids {
            replace_device_groups(&mut tx, user_id, ids).await?;
        }
        let allowed = authorized_ids(&mut tx, user_id).await?;
        let paused = pause_outside(&mut tx, user_id, &allowed).await?;
        tx.commit().await?;
        Ok(paused)
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

// The SQL behind the authorization methods, on a caller-supplied connection so
// update_user_authorization can run every step inside ONE transaction.

/// Replace a user's explicit device-group assignments (clear + re-insert).
async fn replace_device_groups(
    conn: &mut PgConnection,
    user_id: i64,
    device_group_ids: &[i64],
) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM user_device_groups WHERE user_id = $1")
        .bind(user_id)
        .execute(&mut *conn)
        .await?;
    for dg_id in device_group_ids {
        sqlx::query(
            "INSERT INTO user_device_groups (user_id, device_group_id) \
             VALUES ($1, $2) ON CONFLICT DO NOTHING",
        )
        .bind(user_id)
        .bind(dg_id)
        .execute(&mut *conn)
        .await?;
    }
    Ok(())
}

/// Effective inbound group ids for the user (see authorized_device_group_ids).
async fn authorized_ids(conn: &mut PgConnection, user_id: i64) -> Result<Vec<i64>, sqlx::Error> {
    // Admins and all_device_groups users get EVERY inbound group.
    let flags: Option<(bool, bool)> =
        sqlx::query_as("SELECT admin, all_device_groups FROM users WHERE id = $1")
            .bind(user_id)
            .fetch_optional(&mut *conn)
            .await?;
    let (is_admin, all) = match flags {
        Some(f) => f,
        None => return Ok(Vec::new()),
    };
    if is_admin || all {
        let all_in: Vec<(i64,)> =
            sqlx::query_as("SELECT id FROM device_groups WHERE group_type = 'in' ORDER BY id")
                .fetch_all(&mut *conn)
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
    .fetch_all(&mut *conn)
    .await?;
    Ok(rows.into_iter().map(|(id,)| id).collect())
}

/// Pause the user's active rules outside `allowed_group_ids` (all of them when
/// it is empty). Returns the number newly paused.
async fn pause_outside(
    conn: &mut PgConnection,
    user_id: i64,
    allowed_group_ids: &[i64],
) -> Result<u64, sqlx::Error> {
    // v1.0.8: auto_paused=TRUE marks this as a SYSTEM pause (vs. a human
    // using the on/off switch), so a later re-authorization can safely
    // auto-resume it.
    if allowed_group_ids.is_empty() {
        let r = sqlx::query(
            "UPDATE forward_rules SET paused = TRUE, auto_paused = TRUE \
             WHERE uid = $1 AND paused = FALSE",
        )
        .bind(user_id)
        .execute(&mut *conn)
        .await?;
        return Ok(r.rows_affected());
    }
    // Build "device_group_in NOT IN ($2, $3, ...)" with bound params.
    let placeholders = (0..allowed_group_ids.len())
        .map(|i| format!("${}", i + 2))
        .collect::<Vec<_>>()
        .join(", ");
    let sql = format!(
        "UPDATE forward_rules SET paused = TRUE, auto_paused = TRUE \
         WHERE uid = $1 AND paused = FALSE AND device_group_in NOT IN ({})",
        placeholders
    );
    let mut q = sqlx::query(&sql).bind(user_id);
    for gid in allowed_group_ids {
        q = q.bind(gid);
    }
    let r = q.execute(&mut *conn).await?;
    Ok(r.rows_affected())
}
