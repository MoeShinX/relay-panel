//! v1.3.0: read side of the admin audit trail.
//!
//! Write side is `service::audit::record`, called from each destructive handler.

use axum::extract::{Query, State};
use axum::Json;
use serde::{Deserialize, Serialize};

use super::middleware::AdminOnly;
use super::AppState;
use relay_shared::protocol::ApiResponse;

/// Hard ceiling on page size. A caller asking for 100000 rows would pull the
/// whole year into one JSON body and stall the panel for everyone.
const MAX_LIMIT: i64 = 200;
const DEFAULT_LIMIT: i64 = 50;

#[derive(Debug, Deserialize)]
pub struct AuditQuery {
    /// Exact action name (`delete_user`, `rotate_group_token`, …). Absent or
    /// empty = all actions.
    #[serde(default)]
    pub action: Option<String>,
    #[serde(default)]
    pub limit: Option<i64>,
    #[serde(default)]
    pub offset: Option<i64>,
}

#[derive(Debug, Serialize)]
pub struct AuditResponse {
    pub items: Vec<crate::db::repo::AuditEntry>,
    /// Total matching the same filter, so the UI can page without guessing.
    pub total: i64,
}

/// GET /api/v1/admin/audit-log?action=&limit=&offset=
///
/// ADMIN ONLY, and deliberately never owner-scoped: the log records who acted
/// on whom, so exposing a filtered view to the target would leak other tenants'
/// actions through the same rows.
pub async fn get_audit_log(
    _admin: AdminOnly,
    State(state): State<AppState>,
    Query(q): Query<AuditQuery>,
) -> Json<ApiResponse<AuditResponse>> {
    let limit = q.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);
    let offset = q.offset.unwrap_or(0).max(0);
    // Treat an empty string like an absent filter — the frontend's "all" option
    // clears the select to "", and `action=""` would otherwise match nothing.
    let action = q.action.as_deref().map(str::trim).filter(|s| !s.is_empty());

    let total = match state.db.count_audit_log(action).await {
        Ok(t) => t,
        Err(e) => {
            tracing::error!("get_audit_log: count failed: {}", e);
            return Json(ApiResponse {
                code: 500,
                message: "数据库错误".into(),
                data: None,
            });
        }
    };

    match state.db.query_audit_log(action, limit, offset).await {
        Ok(items) => Json(ApiResponse::success(AuditResponse { items, total })),
        Err(e) => {
            tracing::error!("get_audit_log: query failed: {}", e);
            Json(ApiResponse {
                code: 500,
                message: "数据库错误".into(),
                data: None,
            })
        }
    }
}
