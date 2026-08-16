use super::err;
use crate::api::middleware::{AdminOnly, AuthUser};
use crate::api::AppState;
use crate::db::repo::ProfileScope;
use crate::service::profiles::{CreateProfileError, DeleteProfileError, UpdateProfileError};
use axum::{
    extract::{Path, State},
    Json,
};
use relay_shared::models::*;
use relay_shared::protocol::*;
// === Tunnel Profiles (v0.4.0) ===
// CRUD for user-defined tunnel profiles. Builtin profiles (is_builtin=1, seeded
// by Migration 6) are read-only: update/delete return 400. Clones the device
// groups CRUD pattern (INSERT-then-SELECT, dynamic SET builder, builtin guard).

pub async fn list_tunnel_profiles(
    _user: AuthUser,
    State(state): State<AppState>,
) -> Json<ApiResponse<Vec<TunnelProfile>>> {
    // v0.4.11 PR1: any logged-in user can see available templates (ws/tls_simple,
    // builtin + admin-created custom) for rule selection. No longer restricted
    // to builtin only.
    match state
        .db
        .list_profiles(&ProfileScope::AvailableTemplates)
        .await
    {
        Ok(profiles) => Json(ApiResponse::success(profiles)),
        Err(e) => {
            tracing::error!("list_tunnel_profiles: db error: {}", e);
            Json(err(500, "数据库错误"))
        }
    }
}

/// v0.4.11 PR1: admin-only endpoint for the tunnel profile management page.
/// Returns only custom WS/TLS Simple templates (is_builtin = false) that the
/// admin can edit/delete. Builtin profiles are not included.
pub async fn list_admin_tunnel_profiles(
    _admin: AdminOnly,
    State(state): State<AppState>,
) -> Json<ApiResponse<Vec<TunnelProfile>>> {
    match state
        .db
        .list_profiles(&ProfileScope::ManageableCustomTemplates)
        .await
    {
        Ok(profiles) => Json(ApiResponse::success(profiles)),
        Err(e) => {
            tracing::error!("list_admin_tunnel_profiles: db error: {}", e);
            Json(err(500, "数据库错误"))
        }
    }
}

pub async fn create_tunnel_profile(
    admin: AdminOnly,
    State(state): State<AppState>,
    Json(req): Json<CreateTunnelProfileRequest>,
) -> Json<ApiResponse<TunnelProfile>> {
    match crate::service::profiles::create_profile(
        state.db.as_ref(),
        &req.name,
        &req.transport,
        &req.tls_mode,
        &req.ws_path,
        &req.host_header,
        &req.sni,
        admin.user_id,
    )
    .await
    {
        Ok(p) => {
            state
                .node_connections
                .broadcast_all(r#"{"type":"config_changed"}"#)
                .await;
            Json(ApiResponse::success(p))
        }
        Err(CreateProfileError::EmptyName) => Json(err(400, "名称不能为空")),
        Err(CreateProfileError::InvalidTransport) => {
            Json(err(400, "传输协议必须是以下之一: ws, tls_simple"))
        }
        Err(CreateProfileError::FetchFailed) => Json(err(500, "获取创建的模板失败")),
        Err(CreateProfileError::Database(e)) => {
            tracing::error!("create_tunnel_profile: db error: {}", e);
            Json(err(500, "数据库错误"))
        }
    }
}

pub async fn update_tunnel_profile(
    _admin: AdminOnly,
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(req): Json<UpdateTunnelProfileRequest>,
) -> Json<ApiResponse<()>> {
    match crate::service::profiles::update_profile(
        state.db.as_ref(),
        id,
        req.name.as_deref(),
        req.transport.as_deref(),
        req.tls_mode.as_deref(),
        req.ws_path.as_deref(),
        req.host_header.as_deref(),
        req.sni.as_deref(),
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
        Err(UpdateProfileError::NotFound) => Json(err(404, "模板不存在")),
        Err(UpdateProfileError::BuiltinReadOnly) => Json(err(400, "内置模板不可编辑")),
        Err(UpdateProfileError::InvalidTransport) => {
            Json(err(400, "传输协议必须是以下之一: ws, tls_simple"))
        }
        // v0.4.8 fix: a transport change must stay compatible with every rule
        // already bound to this profile — surface a concrete count + protocol so
        // the admin knows what to rebind.
        Err(UpdateProfileError::TransportConflict { count, protocol }) => {
            let t = req.transport.as_deref().unwrap_or("");
            Json(err(
                400,
                format!(
                    "该模板被 {count} 条协议为 {protocol} 的规则使用，不能改为 {t}（ws/tls_simple 仅兼容 TCP）"
                ),
            ))
        }
        Err(UpdateProfileError::NoFields) => Json(err(400, "无需要更新的字段")),
        Err(UpdateProfileError::Database(e)) => {
            tracing::error!("update_tunnel_profile {}: db error: {}", id, e);
            Json(err(500, "数据库错误"))
        }
    }
}

pub async fn delete_tunnel_profile(
    _admin: AdminOnly,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Json<ApiResponse<()>> {
    match crate::service::profiles::delete_profile(state.db.as_ref(), id).await {
        Ok(()) => {
            tracing::warn!(
                action = "delete_tunnel_profile",
                profile_id = id,
                "admin op"
            );
            state
                .node_connections
                .broadcast_all(r#"{"type":"config_changed"}"#)
                .await;
            Json(ApiResponse::success(()))
        }
        Err(DeleteProfileError::NotFound) => Json(err(404, "模板不存在")),
        Err(DeleteProfileError::BuiltinReadOnly) => Json(err(400, "内置模板不可删除")),
        // HTTP 200 + body code (same convention as other err() returns) so the
        // frontend's res.code path surfaces the message.
        Err(DeleteProfileError::InUse(usage)) => {
            Json(err(409, format!("该模板正被 {usage} 条规则使用")))
        }
        Err(DeleteProfileError::Database(e)) => {
            tracing::error!("delete_tunnel_profile {}: db error: {}", id, e);
            Json(err(500, "数据库错误"))
        }
    }
}
