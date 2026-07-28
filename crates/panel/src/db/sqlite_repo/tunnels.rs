use super::SqliteRepository;
use crate::db::error::DbError;
use crate::db::repo::*;
use async_trait::async_trait;
use relay_shared::models::Tunnels;

// ── TunnelRepository ──

#[async_trait]
impl TunnelRepository for SqliteRepository {
    async fn list_tunnels(&self, uid: i64) -> Result<Vec<Tunnels>, DbError> {
        let rows: Vec<Tunnels> = sqlx::query_as(
            "SELECT id, name, group_in, group_out, protocol, listen_port, \
             config_json, secret_json, enabled, uid, created_at \
             FROM tunnels WHERE uid = ? ORDER BY id",
        )
        .bind(uid)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }

    async fn find_by_id(&self, id: i64, uid: i64) -> Result<Option<Tunnels>, DbError> {
        let row: Option<Tunnels> = sqlx::query_as(
            "SELECT id, name, group_in, group_out, protocol, listen_port, \
             config_json, secret_json, enabled, uid, created_at \
             FROM tunnels WHERE id = ? AND uid = ?",
        )
        .bind(id)
        .bind(uid)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row)
    }

    async fn find_by_group_in(&self, group_in: i64) -> Result<Option<Tunnels>, DbError> {
        let row: Option<Tunnels> = sqlx::query_as(
            "SELECT id, name, group_in, group_out, protocol, listen_port, \
             config_json, secret_json, enabled, uid, created_at \
             FROM tunnels WHERE group_in = ?",
        )
        .bind(group_in)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row)
    }

    async fn create_tunnel(&self, req: &CreateTunnelRequest) -> Result<i64, DbError> {
        let result = sqlx::query(
            "INSERT INTO tunnels \
             (name, group_in, group_out, protocol, listen_port, config_json, secret_json, enabled, uid) \
             VALUES (?, ?, ?, ?, ?, ?, ?, 0, ?)",
        )
        .bind(&req.name)
        .bind(req.group_in)
        .bind(req.group_out)
        .bind(&req.protocol)
        .bind(req.listen_port)
        .bind(&req.config_json)
        .bind(&req.secret_json)
        .bind(req.uid)
        .execute(&self.pool)
        .await?;
        let id = result.last_insert_rowid();
        Ok(id)
    }

    async fn update_tunnel_fields(
        &self,
        id: i64,
        uid: i64,
        req: &UpdateTunnelRequest,
    ) -> Result<u64, DbError> {
        let mut sets: Vec<&str> = Vec::new();
        if req.name.is_some() {
            sets.push("name = ?");
        }
        if req.group_in.is_some() {
            sets.push("group_in = ?");
        }
        if req.group_out.is_some() {
            sets.push("group_out = ?");
        }
        if req.protocol.is_some() {
            sets.push("protocol = ?");
        }
        if req.listen_port.is_some() {
            sets.push("listen_port = ?");
        }
        if req.config_json.is_some() {
            sets.push("config_json = ?");
        }
        if req.secret_json.is_some() {
            sets.push("secret_json = ?");
        }
        if req.enabled.is_some() {
            sets.push("enabled = ?");
        }

        if sets.is_empty() {
            return Ok(0);
        }

        let sql = format!(
            "UPDATE tunnels SET {} WHERE id = ? AND uid = ?",
            sets.join(", "),
        );
        let mut q = sqlx::query(&sql);
        if let Some(v) = &req.name {
            q = q.bind(v);
        }
        if let Some(v) = req.group_in {
            q = q.bind(v);
        }
        if let Some(v) = req.group_out {
            q = q.bind(v);
        }
        if let Some(v) = &req.protocol {
            q = q.bind(v);
        }
        if let Some(v) = req.listen_port {
            q = q.bind(v);
        }
        if let Some(v) = &req.config_json {
            q = q.bind(v);
        }
        if let Some(v) = &req.secret_json {
            q = q.bind(v);
        }
        if let Some(v) = req.enabled {
            q = q.bind(if v { 1 } else { 0 });
        }
        q = q.bind(id).bind(uid);

        let result = q.execute(&self.pool).await?;
        Ok(result.rows_affected())
    }

    async fn delete_tunnel(&self, id: i64, uid: i64) -> Result<u64, DbError> {
        let result = sqlx::query("DELETE FROM tunnels WHERE id = ? AND uid = ?")
            .bind(id)
            .bind(uid)
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected())
    }
}
