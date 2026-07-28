//! v1.3.0: site identity endpoints.
//!
//! Split across THREE auth levels on purpose:
//!
//!   * `GET /site` is public, because the login page renders the brand before
//!     anyone has a token.
//!   * `GET /user/site-notice` requires auth, because the announcement and the
//!     support contact are for this operator's users — not for anyone who can
//!     reach the port. Not displaying them on the login page would be hollow if
//!     the API served them to the world anyway.
//!   * `PUT /admin/settings/site` is admin-only, like every other setting.

use axum::extract::State;
use axum::Json;
use serde::{Deserialize, Serialize};

use super::middleware::{AdminOnly, AuthUser};
use super::AppState;
use crate::service::site::{SiteConfig, SITE_CONFIG_KEY};
use relay_shared::protocol::ApiResponse;

async fn load(state: &AppState) -> SiteConfig {
    let raw = state.db.get(SITE_CONFIG_KEY).await.ok().flatten();
    SiteConfig::from_json(raw.as_deref())
}

/// The public half: branding only. Deliberately does NOT carry `announcement`
/// or `contact` — see the module comment.
#[derive(Debug, Serialize)]
pub struct PublicSite {
    pub site_name: String,
    pub subtitle: String,
}

/// GET /api/v1/site — unauthenticated.
pub async fn get_public_site(State(state): State<AppState>) -> Json<ApiResponse<PublicSite>> {
    let cfg = load(&state).await;
    Json(ApiResponse::success(PublicSite {
        site_name: cfg.site_name,
        subtitle: cfg.subtitle,
    }))
}

/// The signed-in half.
#[derive(Debug, Serialize)]
pub struct SiteNotice {
    pub announcement: String,
    /// "info" | "success" | "warning" | "error" — already validated by
    /// SiteConfig::from_json, so the frontend can use it directly.
    pub announcement_type: String,
    pub contact: String,
}

/// GET /api/v1/user/site-notice — any authenticated user.
pub async fn get_site_notice(
    _user: AuthUser,
    State(state): State<AppState>,
) -> Json<ApiResponse<SiteNotice>> {
    let cfg = load(&state).await;
    Json(ApiResponse::success(SiteNotice {
        announcement: cfg.announcement,
        announcement_type: cfg.announcement_type,
        contact: cfg.contact,
    }))
}

/// GET /api/v1/admin/settings/site — the full row, for the edit form.
pub async fn get_site_settings(
    _admin: AdminOnly,
    State(state): State<AppState>,
) -> Json<ApiResponse<SiteConfig>> {
    Json(ApiResponse::success(load(&state).await))
}

#[derive(Debug, Deserialize)]
pub struct UpdateSiteRequest {
    #[serde(default)]
    pub site_name: String,
    #[serde(default)]
    pub subtitle: String,
    #[serde(default)]
    pub announcement: String,
    #[serde(default)]
    pub announcement_type: String,
    #[serde(default)]
    pub contact: String,
}

/// PUT /api/v1/admin/settings/site
pub async fn update_site_settings(
    _admin: AdminOnly,
    State(state): State<AppState>,
    Json(req): Json<UpdateSiteRequest>,
) -> Json<ApiResponse<SiteConfig>> {
    // Trim + clamp before storing, so every reader (including the public
    // endpoint hit on every login page load) gets a bounded value.
    let cfg = SiteConfig {
        site_name: req.site_name,
        subtitle: req.subtitle,
        announcement: req.announcement,
        announcement_type: req.announcement_type,
        contact: req.contact,
    }
    .sanitized();

    let json = match serde_json::to_string(&cfg) {
        Ok(j) => j,
        Err(e) => {
            tracing::error!("update_site_settings: serialize failed: {}", e);
            return Json(ApiResponse {
                code: 500,
                message: "配置序列化失败".into(),
                data: None,
            });
        }
    };
    if let Err(e) = state.db.set(SITE_CONFIG_KEY, &json).await {
        tracing::error!("update_site_settings: save failed: {}", e);
        return Json(ApiResponse {
            code: 500,
            message: "数据库错误".into(),
            data: None,
        });
    }

    tracing::info!(
        action = "update_site_settings",
        site_name = %cfg.site_name,
        "site settings updated"
    );
    // Records WHICH fields are now set, not their contents: the announcement can
    // be long, and the audit table is not the place to keep a copy of it.
    crate::service::audit::record(
        &state,
        Some(_admin.user_id),
        "update_site_settings",
        "settings",
        "site",
        &format!(
            "站点名称 {} / 公告 {} / 客服 {}",
            cfg.site_name,
            if cfg.announcement.is_empty() {
                "已清空"
            } else {
                "已设置"
            },
            if cfg.contact.is_empty() {
                "已清空"
            } else {
                "已设置"
            },
        ),
    )
    .await;

    Json(ApiResponse::success(cfg))
}
