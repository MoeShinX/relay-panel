use super::err;
use crate::api::middleware::AdminOnly;
use crate::api::AppState;
use crate::db::repo::{CreateTunnelRequest, TunnelRepository, UpdateTunnelRequest};
use crate::service::tunnels::{
    create_tunnel as svc_create_tunnel, delete_tunnel as svc_delete_tunnel,
    regenerate_config as svc_regenerate_config, update_tunnel as svc_update_tunnel,
    CreateTunnelError, UpdateTunnelError,
};
use axum::{
    extract::{Path, State},
    Json,
};
use relay_shared::models::Tunnels;
use relay_shared::protocol::ApiResponse;
use serde_json::Value;

// === Tunnel Forwarding (v1.3) ===
// CRUD for tunnel forwarding rules. Each tunnel maps one ingress group
// (group_in) to an optional egress group (group_out) via sing-box.

pub async fn list_tunnels(
    _admin: AdminOnly,
    State(state): State<AppState>,
) -> Json<ApiResponse<Vec<Tunnels>>> {
    let tunnels: Vec<Tunnels> = TunnelRepository::list_tunnels(state.db.as_ref(), _admin.user_id)
        .await
        .unwrap_or_else(|e| {
            tracing::error!("list_tunnels: db error: {}", e);
            Vec::new()
        });
    Json(ApiResponse::success(tunnels))
}

pub async fn get_tunnel(
    _admin: AdminOnly,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Json<ApiResponse<Tunnels>> {
    let tunnel = match TunnelRepository::find_by_id(state.db.as_ref(), id, _admin.user_id).await {
        Ok(Some(t)) => t,
        Ok(None) => return Json(err(404, "隧道不存在")),
        Err(e) => {
            tracing::error!("get_tunnel: db error: {}", e);
            return Json(err(500, "数据库错误"));
        }
    };
    Json(ApiResponse::success(tunnel))
}

pub async fn create_tunnel(
    admin: AdminOnly,
    State(state): State<AppState>,
    Json(req): Json<CreateTunnelRequest>,
) -> Json<ApiResponse<Tunnels>> {
    // Auto-populate config_json and secret_json if empty.
    let mut req = req;
    if req.config_json.is_empty() {
        req.config_json =
            serde_json::to_string(&crate::service::tunnels::generate_tunnel_config(&Tunnels {
                id: 0,
                name: req.name.clone(),
                group_in: req.group_in,
                group_out: req.group_out,
                protocol: req.protocol.clone(),
                listen_port: req.listen_port,
                config_json: String::new(),
                secret_json: String::new(),
                enabled: false,
                uid: admin.user_id,
                created_at: String::new(),
            }))
            .unwrap_or_else(|_| "{}".to_string());
    }
    if req.secret_json.is_empty() {
        req.secret_json = serde_json::to_string(&crate::service::tunnels::generate_tunnel_secret())
            .unwrap_or_else(|_| "{}".to_string());
    }

    let id = match svc_create_tunnel(state.db.as_ref(), &req).await {
        Ok(id) => id,
        Err(CreateTunnelError::DuplicateGroupIn) => {
            return Json(err(400, "该入口组已绑定隧道"));
        }
        Err(e) => {
            tracing::error!("create_tunnel: db error: {}", e);
            return Json(err(500, "数据库错误"));
        }
    };

    let tunnel = match TunnelRepository::find_by_id(state.db.as_ref(), id, admin.user_id).await {
        Ok(Some(t)) => t,
        Ok(None) => return Json(err(500, "创建隧道后无法读取")),
        Err(e) => {
            tracing::error!("create_tunnel: find_by_id failed: {}", e);
            return Json(err(500, "数据库错误"));
        }
    };
    Json(ApiResponse::success(tunnel))
}

pub async fn update_tunnel(
    _admin: AdminOnly,
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(req): Json<UpdateTunnelRequest>,
) -> Json<ApiResponse<()>> {
    let affected = match svc_update_tunnel(state.db.as_ref(), id, _admin.user_id, &req).await {
        Ok(n) => n,
        Err(UpdateTunnelError::DuplicateGroupIn) => {
            return Json(err(400, "该入口组已绑定隧道"));
        }
        Err(e) => {
            tracing::error!("update_tunnel: db error: {}", e);
            return Json(err(500, "数据库错误"));
        }
    };

    if affected == 0 {
        return Json(err(404, "隧道不存在"));
    }
    Json(ApiResponse::success(()))
}

pub async fn delete_tunnel(
    _admin: AdminOnly,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Json<ApiResponse<()>> {
    match svc_delete_tunnel(state.db.as_ref(), id, _admin.user_id).await {
        Ok(n) if n > 0 => Json(ApiResponse::success(())),
        Ok(_) => Json(err(404, "隧道不存在")),
        Err(e) => {
            tracing::error!("delete_tunnel: db error: {}", e);
            Json(err(500, "数据库错误"))
        }
    }
}

/// Regenerate the sing-box config and secret for a tunnel.
/// Replaces both config_json and secret_json with fresh generated values.
pub async fn regenerate_tunnel_config(
    _admin: AdminOnly,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Json<ApiResponse<(Value, Value)>> {
    match svc_regenerate_config(state.db.as_ref(), id, _admin.user_id).await {
        Ok((config, secret)) => Json(ApiResponse::success((config, secret))),
        Err(e) => {
            tracing::error!("regenerate_tunnel_config: {}", e);
            Json(err(500, "生成配置失败"))
        }
    }
}
