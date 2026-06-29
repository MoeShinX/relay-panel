use super::err;
use crate::api::middleware::{AdminOnly, AuthUser};
use crate::api::AppState;
use crate::service::groups::{validate_rate, CreateGroupError, UpdateGroupError, RATE_DEFAULT};
use axum::{
    extract::{Path, State},
    Json,
};
use relay_shared::models::*;
use relay_shared::protocol::*;
// === Device Groups ===
pub async fn list_groups(
    user: AuthUser,
    State(state): State<AppState>,
) -> Json<ApiResponse<Vec<DeviceGroup>>> {
    let scope = user.resource_scope();
    let groups: Vec<DeviceGroup> = state.db.list_groups(&scope).await.unwrap_or_else(|e| {
        tracing::error!("list_groups: db error: {}", e);
        Vec::new()
    });
    Json(ApiResponse::success(groups))
}

pub async fn create_group(
    admin: AdminOnly,
    State(state): State<AppState>,
    Json(req): Json<CreateGroupRequest>,
) -> Json<ApiResponse<DeviceGroup>> {
    // v0.4.12 PR1: device groups are admin-managed shared infrastructure, and
    // only ADMIN-owned groups are ever shared to regular users. Creating a
    // group owned by a regular user would produce a "dead" group the user
    // can't manage (AdminOnly) and that is never shared. So `owner_uid` is
    // IGNORED — the group always belongs to the creating admin.
    // v1.0.8: clamp absent rate to the default (1.0) and reject out-of-range.
    let rate = match req.rate {
        Some(r) => match validate_rate(r) {
            Some(v) => v,
            None => {
                return Json(err(
                    400,
                    "rate must be between 0.1 and 100",
                ))
            }
        },
        None => RATE_DEFAULT,
    };
    match crate::service::groups::create_group(
        state.db.as_ref(),
        &req.name,
        &req.group_type,
        admin.user_id,
        &req.connect_host,
        &req.port_range,
        rate,
    )
    .await
    {
        Ok(g) => Json(ApiResponse::success(g)),
        Err(CreateGroupError::FetchFailed) => Json(err(500, "Failed to fetch created group")),
        Err(CreateGroupError::Database(e)) => {
            tracing::error!("create_group: db error: {}", e);
            Json(err(500, "database error"))
        }
    }
}

/// Rotate a device group's node token. Generates a fresh UUID, persists it,
/// and broadcasts `config_changed` so connected nodes drop the old token and
/// re-authenticate with the new one. This is the only way to revoke a leaked
/// node token without deleting the whole group (and its rules). Returns the
/// new token so the admin can hand it to the node operator.
#[derive(serde::Serialize)]
pub struct RotatedToken {
    token: String,
}

pub async fn rotate_group_token(
    _admin: AdminOnly,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Json<ApiResponse<RotatedToken>> {
    match crate::service::groups::rotate_group_token(state.db.as_ref(), id).await {
        Ok(None) => Json(err(404, "Group not found")),
        Ok(Some(new_token)) => {
            // v0.3.9: tear down every live WS connection for this group BEFORE
            // broadcasting. The old token just became invalid, but sockets that
            // authenticated at upgrade time stay open with the revoked credential
            // — close_group drops them so the node reconnects and re-auths with
            // the new token. (broadcast_all alone was insufficient: it pushed
            // config_changed, the node re-fetched config with the OLD token, the
            // panel returned an empty config, and the node tore down all its
            // listeners — a complete outage on every token rotation.)
            let closed = state.node_connections.close_group(id).await;
            // Notify any connections in OTHER groups too (harmless no-op for this
            // group since close_group already drained it).
            state
                .node_connections
                .broadcast_all(r#"{"type":"config_changed"}"#)
                .await;
            tracing::warn!(
                action = "rotate_group_token",
                group_id = id,
                closed_connections = closed,
                "admin rotated node token"
            );
            Json(ApiResponse::success(RotatedToken { token: new_token }))
        }
        Err(e) => {
            tracing::error!(
                "rotate_group_token {}: update_group_token failed: {}",
                id,
                e
            );
            Json(err(500, "database error"))
        }
    }
}

pub async fn update_group(
    _admin: AdminOnly,
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(req): Json<UpdateGroupRequest>,
) -> Json<ApiResponse<()>> {
    // v1.0.8: validate rate when present. Out-of-range → 400 (don't persist).
    let rate = match req.rate {
        Some(r) => match validate_rate(r) {
            Some(v) => Some(v),
            None => return Json(err(400, "rate must be between 0.1 and 100")),
        },
        None => None,
    };
    match crate::service::groups::update_group(
        state.db.as_ref(),
        id,
        req.name.as_deref(),
        req.group_type.as_ref(),
        req.connect_host.as_deref(),
        req.port_range.as_deref(),
        rate,
    )
    .await
    {
        Ok(()) => {
            state
                .node_connections
                .broadcast_all(r#"{"type":"config_changed"}"#)
                .await;
            Json(ApiResponse::success(()))
        }
        Err(UpdateGroupError::NoFields) => Json(err(400, "No fields to update")),
        // v0.3.6: 0 rows = group id didn't exist. 404 + no broadcast.
        Err(UpdateGroupError::NotFound) => Json(err(404, "Group not found")),
        Err(UpdateGroupError::Database(e)) => {
            tracing::error!("update_group {}: update_group_fields failed: {}", id, e);
            Json(err(500, "database error"))
        }
    }
}

pub async fn delete_group(
    admin: AdminOnly,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Json<ApiResponse<()>> {
    match crate::service::groups::delete_group(state.db.as_ref(), id).await {
        Ok(false) => Json(err(404, "Not found")),
        Ok(true) => {
            tracing::warn!(
                action = "delete_group",
                group_id = id,
                actor_id = admin.user_id,
                actor_admin = true,
                "destructive op"
            );
            // v1.0.4: close WS connections for the deleted group so nodes
            // stop reporting. Node status entries naturally expire via the
            // existing 2-minute timeout sweep.
            state.node_connections.close_group(id).await;
            state
                .node_connections
                .broadcast_all(r#"{"type":"config_changed"}"#)
                .await;
            Json(ApiResponse::success(()))
        }
        Err(e) => {
            if let Some(in_use) = e.downcast_ref::<crate::service::groups::GroupInUseError>() {
                return Json(err(
                    409,
                    format!(
                        "该分组仍被 {} 条规则使用，请先迁移规则。",
                        in_use.rule_count
                    ),
                ));
            }
            tracing::error!("delete_group {}: delete_group failed: {}", id, e);
            Json(err(500, "database error"))
        }
    }
}
