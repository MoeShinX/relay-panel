use super::SqliteRepository;
use crate::db::error::DbError;
use crate::db::repo::*;
use async_trait::async_trait;
use relay_shared::models::{DeviceGroup, SharedGroupSummary};

// ── GroupRepository ──

#[async_trait]
impl GroupRepository for SqliteRepository {
    async fn list_groups(&self, scope: &ResourceScope) -> Result<Vec<DeviceGroup>, DbError> {
        let groups: Vec<DeviceGroup> = match scope.owner_id() {
            None => sqlx::query_as("SELECT * FROM device_groups ORDER BY id"),
            Some(uid) => {
                sqlx::query_as("SELECT * FROM device_groups WHERE uid = ? ORDER BY id").bind(uid)
            }
        }
        .fetch_all(&self.pool)
        .await?;
        Ok(groups)
    }

    async fn list_shared_groups(
        &self,
        uid: i64,
        is_admin: bool,
    ) -> Result<Vec<SharedGroupSummary>, DbError> {
        // v0.4.11 PR3: admins manage groups directly — no shared infrastructure needed.
        if is_admin {
            return Ok(vec![]);
        }
        // v0.4.12 PR1: regular users see ALL ADMIN-owned inbound groups,
        // independent of whether they already have rules. The JOIN to users
        // enforces admin ownership so a regular user's group is never exposed
        // as "shared". group_type uses the stable machine value 'in' (the old
        // LIKE 'inbound%' never matched — group_type is 'in' / 'out' / 'monitor').
        // v1.0.7: `g.hidden` is SELECTED (not filtered here) so the caller
        // decides. Only the node-status path (`list_shared_node_summary`) hides
        // it; the rule dropdown / shop still list hidden groups so existing and
        // new rules keep working. Admins get [] above and are unaffected.
        let groups: Vec<SharedGroupSummary> = sqlx::query_as(
            "SELECT g.id, g.name, g.group_type, g.connect_host, g.capabilities, g.region, g.line_type, g.hidden \
             FROM device_groups g \
             JOIN users u ON u.id = g.uid \
             WHERE g.uid != ? AND u.admin = 1 AND g.group_type = 'in' \
             ORDER BY g.id",
        )
        .bind(uid)
        .fetch_all(&self.pool)
        .await?;
        Ok(groups)
    }

    async fn find_by_token(&self, token: &str) -> Result<Option<DeviceGroup>, DbError> {
        let group: Option<DeviceGroup> =
            sqlx::query_as("SELECT * FROM device_groups WHERE token = ?")
                .bind(token)
                .fetch_optional(&self.pool)
                .await?;
        Ok(group)
    }

    async fn find_by_id(
        &self,
        id: i64,
        scope: &ResourceScope,
    ) -> Result<Option<DeviceGroup>, DbError> {
        let group: Option<DeviceGroup> = match scope.owner_id() {
            None => sqlx::query_as("SELECT * FROM device_groups WHERE id = ?").bind(id),
            Some(uid) => sqlx::query_as("SELECT * FROM device_groups WHERE id = ? AND uid = ?")
                .bind(id)
                .bind(uid),
        }
        .fetch_optional(&self.pool)
        .await?;
        Ok(group)
    }

    async fn find_name_by_id(
        &self,
        id: i64,
        scope: &ResourceScope,
    ) -> Result<Option<String>, DbError> {
        let row: Option<(String,)> = match scope.owner_id() {
            None => sqlx::query_as("SELECT name FROM device_groups WHERE id = ?").bind(id),
            Some(uid) => sqlx::query_as("SELECT name FROM device_groups WHERE id = ? AND uid = ?")
                .bind(id)
                .bind(uid),
        }
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(|(n,)| n))
    }

    async fn insert_group(
        &self,
        name: &str,
        group_type: &str,
        token: &str,
        uid: i64,
        connect_host: &str,
        port_range: &str,
        rate: f64,
        hidden: bool,
    ) -> Result<(), DbError> {
        sqlx::query(
            "INSERT INTO device_groups (name, group_type, token, uid, connect_host, port_range, rate, hidden) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(name)
        .bind(group_type)
        .bind(token)
        .bind(uid)
        .bind(connect_host)
        .bind(port_range)
        .bind(rate)
        .bind(hidden)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn find_by_token_after_insert(
        &self,
        token: &str,
    ) -> Result<Option<DeviceGroup>, DbError> {
        // INSERT-then-SELECT-by-token pattern: token is freshly generated
        // (UUID v4), so the SELECT is guaranteed to hit the just-inserted row.
        let group: Option<DeviceGroup> =
            sqlx::query_as("SELECT * FROM device_groups WHERE token = ?")
                .bind(token)
                .fetch_optional(&self.pool)
                .await?;
        Ok(group)
    }

    async fn update_group_fields(
        &self,
        id: i64,
        scope: &ResourceScope,
        name: Option<&str>,
        group_type: Option<&str>,
        connect_host: Option<&str>,
        port_range: Option<&str>,
        rate: Option<f64>,
        hidden: Option<bool>,
    ) -> Result<u64, DbError> {
        // Token is NOT updatable here (rotation is a separate endpoint). Build
        // the SET clause from the present fields; binding order matches below.
        let mut sets: Vec<&str> = Vec::new();
        if name.is_some() {
            sets.push("name = ?");
        }
        if group_type.is_some() {
            sets.push("group_type = ?");
        }
        if connect_host.is_some() {
            sets.push("connect_host = ?");
        }
        if port_range.is_some() {
            sets.push("port_range = ?");
        }
        if rate.is_some() {
            sets.push("rate = ?");
        }
        if hidden.is_some() {
            sets.push("hidden = ?");
        }

        if sets.is_empty() {
            return Ok(0);
        }

        let sql = match scope.owner_id() {
            None => format!("UPDATE device_groups SET {} WHERE id = ?", sets.join(", ")),
            Some(_) => format!(
                "UPDATE device_groups SET {} WHERE id = ? AND uid = ?",
                sets.join(", ")
            ),
        };
        let mut q = sqlx::query(&sql);
        if let Some(v) = name {
            q = q.bind(v);
        }
        if let Some(v) = group_type {
            q = q.bind(v);
        }
        if let Some(v) = connect_host {
            q = q.bind(v);
        }
        if let Some(v) = port_range {
            q = q.bind(v);
        }
        if let Some(v) = rate {
            q = q.bind(v);
        }
        if let Some(v) = hidden {
            q = q.bind(v);
        }
        q = q.bind(id);
        if let Some(uid) = scope.owner_id() {
            q = q.bind(uid);
        }

        let result = q.execute(&self.pool).await?;
        Ok(result.rows_affected())
    }

    async fn update_group_token(
        &self,
        id: i64,
        scope: &ResourceScope,
        new_token: &str,
    ) -> Result<u64, DbError> {
        let result = match scope.owner_id() {
            None => sqlx::query("UPDATE device_groups SET token = ? WHERE id = ?")
                .bind(new_token)
                .bind(id),
            Some(uid) => sqlx::query("UPDATE device_groups SET token = ? WHERE id = ? AND uid = ?")
                .bind(new_token)
                .bind(id)
                .bind(uid),
        }
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected())
    }

    async fn count_rules_by_group(&self, id: i64) -> Result<i64, DbError> {
        // Check if fallback_group column exists (defensive: test schemas may not have it).
        let has_fallback: (i64,) = sqlx::query_as(
            "SELECT COUNT(*) FROM pragma_table_info('forward_rules') WHERE name = 'fallback_group'",
        )
        .fetch_one(&self.pool)
        .await?;
        let sql = if has_fallback.0 > 0 {
            "SELECT COUNT(*) FROM forward_rules \
             WHERE device_group_in = ? OR device_group_out = ? OR fallback_group = ?"
        } else {
            "SELECT COUNT(*) FROM forward_rules \
             WHERE device_group_in = ? OR device_group_out = ?"
        };
        let mut q = sqlx::query_as(sql).bind(id).bind(id);
        if has_fallback.0 > 0 {
            q = q.bind(id);
        }
        let row: (i64,) = q.fetch_one(&self.pool).await?;
        Ok(row.0)
    }

    async fn delete_group(&self, id: i64, scope: &ResourceScope) -> Result<u64, DbError> {
        let result = match scope.owner_id() {
            None => sqlx::query("DELETE FROM device_groups WHERE id = ?").bind(id),
            Some(uid) => sqlx::query("DELETE FROM device_groups WHERE id = ? AND uid = ?")
                .bind(id)
                .bind(uid),
        }
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected())
    }

    async fn delete_groups_by_uid(&self, uid: i64) -> Result<u64, DbError> {
        let result = sqlx::query("DELETE FROM device_groups WHERE uid = ?")
            .bind(uid)
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected())
    }
}
