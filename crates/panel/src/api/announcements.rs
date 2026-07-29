//! v1.2.4: site announcement endpoints.
//!
//! Auth split mirrors the site config: signed-in users read (the archive is for
//! them), admins write. Nothing here is public — the announcement was
//! deliberately kept off the login page, and serving it unauthenticated would
//! undo that.

use axum::extract::{Path, Query, State};
use axum::Json;
use serde::{Deserialize, Serialize};

use super::middleware::{AdminOnly, AuthUser};
use super::AppState;
use crate::db::repo::{Announcement, NewAnnouncement};
use crate::service::announcements as svc;
use relay_shared::protocol::ApiResponse;

fn now_utc() -> String {
    chrono::Utc::now().format("%Y-%m-%d %H:%M:%S").to_string()
}

fn err<T: Serialize>(code: i32, msg: impl Into<String>) -> ApiResponse<T> {
    ApiResponse {
        code,
        message: msg.into(),
        data: None,
    }
}

const MAX_LIMIT: i64 = 100;
const DEFAULT_LIMIT: i64 = 20;

#[derive(Debug, Deserialize)]
pub struct ListQuery {
    #[serde(default)]
    pub limit: Option<i64>,
    #[serde(default)]
    pub offset: Option<i64>,
}

#[derive(Debug, Serialize)]
pub struct ListResponse {
    pub items: Vec<Announcement>,
    pub total: i64,
}

/// GET /api/v1/user/announcements — the archive, for any signed-in user.
///
/// Includes expired notices: reading what was announced last month is the
/// entire reason this page exists.
pub async fn list_for_user(
    _user: AuthUser,
    State(state): State<AppState>,
    Query(q): Query<ListQuery>,
) -> Json<ApiResponse<ListResponse>> {
    let limit = q.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);
    let offset = q.offset.unwrap_or(0).max(0);
    let now = now_utc();

    let total = match state.db.count_announcements(true, &now).await {
        Ok(t) => t,
        Err(e) => {
            tracing::error!("list_announcements: count failed: {}", e);
            return Json(err(500, "数据库错误"));
        }
    };
    match state.db.list_announcements(true, &now, limit, offset).await {
        Ok(items) => Json(ApiResponse::success(ListResponse { items, total })),
        Err(e) => {
            tracing::error!("list_announcements: query failed: {}", e);
            Json(err(500, "数据库错误"))
        }
    }
}

/// GET /api/v1/user/announcements/active — the one the banner shows.
///
/// `data: null` when there is nothing live, which the frontend renders as no
/// banner at all rather than an empty box.
pub async fn active_for_user(
    _user: AuthUser,
    State(state): State<AppState>,
) -> Json<ApiResponse<Option<Announcement>>> {
    match state.db.active_announcement(&now_utc()).await {
        Ok(a) => Json(ApiResponse::success(a)),
        Err(e) => {
            tracing::error!("active_announcement: {}", e);
            Json(err(500, "数据库错误"))
        }
    }
}

#[derive(Debug, Serialize)]
pub struct LatestId {
    pub latest_id: i64,
}

/// GET /api/v1/user/announcements/latest-id
///
/// Feeds the header bell. Returns 0 when there are no announcements at all, so
/// a fresh install shows no dot rather than a permanent one.
pub async fn latest_id(
    _user: AuthUser,
    State(state): State<AppState>,
) -> Json<ApiResponse<LatestId>> {
    match state.db.latest_announcement_id().await {
        Ok(latest_id) => Json(ApiResponse::success(LatestId { latest_id })),
        Err(e) => {
            tracing::error!("latest_announcement_id: {}", e);
            Json(err(500, "数据库错误"))
        }
    }
}

/// GET /api/v1/admin/announcements — same archive, for the management page.
pub async fn list_for_admin(
    _admin: AdminOnly,
    State(state): State<AppState>,
    Query(q): Query<ListQuery>,
) -> Json<ApiResponse<ListResponse>> {
    let limit = q.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);
    let offset = q.offset.unwrap_or(0).max(0);
    let now = now_utc();

    let total = match state.db.count_announcements(true, &now).await {
        Ok(t) => t,
        Err(e) => {
            tracing::error!("admin list_announcements: count failed: {}", e);
            return Json(err(500, "数据库错误"));
        }
    };
    match state.db.list_announcements(true, &now, limit, offset).await {
        Ok(items) => Json(ApiResponse::success(ListResponse { items, total })),
        Err(e) => {
            tracing::error!("admin list_announcements: query failed: {}", e);
            Json(err(500, "数据库错误"))
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct WriteRequest {
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub content: String,
    #[serde(default)]
    pub kind: String,
    #[serde(default)]
    pub pinned: bool,
    /// "YYYY-MM-DD HH:MM:SS" (UTC), or empty/absent for never.
    #[serde(default)]
    pub expires_at: Option<String>,
}

/// Shared validation. Content is the one required field — a notice with no body
/// would render an empty banner.
fn build(
    req: &WriteRequest,
    published_at: String,
    actor: Option<(i64, String)>,
) -> Result<NewAnnouncement, String> {
    let content = svc::clamp(&req.content, svc::MAX_CONTENT);
    if content.is_empty() {
        return Err("公告内容不能为空".into());
    }
    let expires_at = svc::parse_expiry(req.expires_at.as_deref()).map_err(|e| e.to_string())?;
    let (author_id, author_name) = match actor {
        Some((id, name)) => (Some(id), name),
        None => (None, String::new()),
    };
    Ok(NewAnnouncement {
        title: svc::clamp(&req.title, svc::MAX_TITLE),
        content,
        kind: svc::normalize_kind(req.kind.trim()),
        pinned: req.pinned,
        published_at,
        expires_at,
        author_id,
        author_name,
    })
}

/// Resolve the acting admin's name for the author snapshot — same reasoning as
/// the audit log: deleting the account must not erase who posted the notice.
async fn actor_name(state: &AppState, id: i64) -> String {
    match crate::db::repo::UserRepository::find_by_id(state.db.as_ref(), id).await {
        Ok(Some(u)) => u.username,
        _ => format!("#{id}"),
    }
}

/// POST /api/v1/admin/announcements
pub async fn create(
    admin: AdminOnly,
    State(state): State<AppState>,
    Json(req): Json<WriteRequest>,
) -> Json<ApiResponse<i64>> {
    let name = actor_name(&state, admin.user_id).await;
    let a = match build(&req, now_utc(), Some((admin.user_id, name))) {
        Ok(a) => a,
        Err(m) => return Json(err(400, m)),
    };
    match state.db.create_announcement(&a).await {
        Ok(id) => {
            crate::service::audit::record(
                &state,
                Some(admin.user_id),
                "create_announcement",
                "announcement",
                id,
                &a.title,
            )
            .await;
            Json(ApiResponse::success(id))
        }
        Err(e) => {
            tracing::error!("create_announcement: {}", e);
            Json(err(500, "数据库错误"))
        }
    }
}

/// PUT /api/v1/admin/announcements/{id}
pub async fn update(
    admin: AdminOnly,
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(req): Json<WriteRequest>,
) -> Json<ApiResponse<()>> {
    // published_at and author are carried from the stored row — editing a typo
    // must not re-date the notice or reassign who posted it. The repo UPDATE
    // does not touch those columns; this only satisfies the shared builder.
    let a = match build(&req, String::new(), None) {
        Ok(a) => a,
        Err(m) => return Json(err(400, m)),
    };
    match state.db.update_announcement(id, &a).await {
        Ok(0) => Json(err(404, "公告不存在")),
        Ok(_) => {
            crate::service::audit::record(
                &state,
                Some(admin.user_id),
                "update_announcement",
                "announcement",
                id,
                &a.title,
            )
            .await;
            Json(ApiResponse::success(()))
        }
        Err(e) => {
            tracing::error!("update_announcement {}: {}", id, e);
            Json(err(500, "数据库错误"))
        }
    }
}

/// DELETE /api/v1/admin/announcements/{id}
pub async fn delete(
    admin: AdminOnly,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Json<ApiResponse<()>> {
    // Read the title before deleting: afterwards the audit entry could only say
    // "announcement 7".
    let title = match state.db.find_announcement(id).await {
        Ok(Some(a)) => a.title,
        _ => String::new(),
    };
    match state.db.delete_announcement(id).await {
        Ok(0) => Json(err(404, "公告不存在")),
        Ok(_) => {
            crate::service::audit::record(
                &state,
                Some(admin.user_id),
                "delete_announcement",
                "announcement",
                id,
                &title,
            )
            .await;
            Json(ApiResponse::success(()))
        }
        Err(e) => {
            tracing::error!("delete_announcement {}: {}", id, e);
            Json(err(500, "数据库错误"))
        }
    }
}
