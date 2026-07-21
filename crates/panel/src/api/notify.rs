//! v1.2.0: notification settings API.
//!
//! Credentials follow a write-only convention: they go IN through PUT, and the
//! GET never returns them (only a boolean saying one is stored). An omitted /
//! empty credential on PUT means "keep what's there", so the UI can render a
//! masked field and submit the form without wiping the secret it never saw.

use axum::extract::State;
use axum::Json;
use relay_shared::protocol::ApiResponse;
use serde::{Deserialize, Serialize};

use crate::api::middleware::AdminOnly;
use crate::api::AppState;
use crate::service::notify::{self, NotifyConfig, NotifyConfigPublic, MIN_OFFLINE_ALERT_SECS};

fn err<T: Serialize>(code: i32, msg: &str) -> ApiResponse<T> {
    ApiResponse {
        code,
        message: msg.into(),
        data: None,
    }
}

async fn load(state: &AppState) -> NotifyConfig {
    let raw = state.db.get(notify::NOTIFY_CONFIG_KEY).await.ok().flatten();
    NotifyConfig::from_json(raw.as_deref())
}

/// GET /api/v1/admin/settings/notify
pub async fn get_notify_settings(
    _admin: AdminOnly,
    State(state): State<AppState>,
) -> Json<ApiResponse<NotifyConfigPublic>> {
    let cfg = load(&state).await;
    Json(ApiResponse::success(NotifyConfigPublic::from(&cfg)))
}

#[derive(Debug, Deserialize)]
pub struct UpdateNotifyRequest {
    pub enabled: bool,
    pub offline_alert_secs: i64,
    pub notify_recovery: bool,
    pub telegram_enabled: bool,
    pub telegram_chat_id: String,
    /// Empty / omitted = keep the stored token. The UI shows a masked field and
    /// never receives the real value, so it cannot echo it back.
    #[serde(default)]
    pub telegram_bot_token: Option<String>,
    pub email_enabled: bool,
    pub smtp_host: String,
    pub smtp_port: u16,
    pub smtp_username: String,
    /// Empty / omitted = keep the stored password.
    #[serde(default)]
    pub smtp_password: Option<String>,
    pub smtp_from: String,
    pub smtp_to: String,
    pub smtp_tls: bool,
}

/// PUT /api/v1/admin/settings/notify
pub async fn update_notify_settings(
    _admin: AdminOnly,
    State(state): State<AppState>,
    Json(req): Json<UpdateNotifyRequest>,
) -> Json<ApiResponse<NotifyConfigPublic>> {
    if req.offline_alert_secs < MIN_OFFLINE_ALERT_SECS {
        return Json(err(
            400,
            &format!("离线告警阈值最小 {MIN_OFFLINE_ALERT_SECS} 秒"),
        ));
    }
    let existing = load(&state).await;

    // Keep-if-empty: the browser was never given these values, so an empty
    // field means "unchanged", not "clear it". Clearing is done by disabling
    // the channel, not by blanking a field the user cannot see.
    let telegram_bot_token = match req.telegram_bot_token {
        Some(t) if !t.trim().is_empty() => t.trim().to_string(),
        _ => existing.telegram_bot_token.clone(),
    };
    let smtp_password = match req.smtp_password {
        Some(p) if !p.is_empty() => p,
        _ => existing.smtp_password.clone(),
    };

    // Refuse to enable a channel that cannot possibly work — otherwise the
    // operator turns it on, sees no error, and finds out it never sent
    // anything on the night a node dies.
    if req.telegram_enabled && (telegram_bot_token.is_empty() || req.telegram_chat_id.is_empty()) {
        return Json(err(400, "启用 Telegram 需要填写 Bot Token 和 Chat ID"));
    }
    if req.email_enabled && (req.smtp_host.is_empty() || req.smtp_to.is_empty()) {
        return Json(err(400, "启用邮件需要填写 SMTP 主机和收件人"));
    }

    let cfg = NotifyConfig {
        enabled: req.enabled,
        offline_alert_secs: req.offline_alert_secs,
        notify_recovery: req.notify_recovery,
        telegram_enabled: req.telegram_enabled,
        telegram_bot_token,
        telegram_chat_id: req.telegram_chat_id.trim().to_string(),
        email_enabled: req.email_enabled,
        smtp_host: req.smtp_host.trim().to_string(),
        smtp_port: req.smtp_port,
        smtp_username: req.smtp_username.trim().to_string(),
        smtp_password,
        smtp_from: req.smtp_from.trim().to_string(),
        smtp_to: req.smtp_to.trim().to_string(),
        smtp_tls: req.smtp_tls,
    };

    let json = match serde_json::to_string(&cfg) {
        Ok(j) => j,
        Err(e) => {
            tracing::error!("update_notify_settings: serialize failed: {}", e);
            return Json(err(500, "配置序列化失败"));
        }
    };
    if let Err(e) = state.db.set(notify::NOTIFY_CONFIG_KEY, &json).await {
        tracing::error!("update_notify_settings: save failed: {}", e);
        return Json(err(500, "数据库错误"));
    }
    tracing::info!(
        action = "update_notify_settings",
        enabled = cfg.enabled,
        telegram = cfg.telegram_enabled,
        email = cfg.email_enabled,
        "notification settings updated"
    );
    Json(ApiResponse::success(NotifyConfigPublic::from(&cfg)))
}

#[derive(Debug, Deserialize)]
pub struct TestNotifyRequest {
    /// "telegram" | "email" — which channel to exercise.
    pub channel: String,
}

#[derive(Debug, Serialize)]
pub struct TestNotifyResponse {
    pub ok: bool,
    /// The failure reason verbatim from the provider when ok = false.
    pub detail: String,
}

/// POST /api/v1/admin/settings/notify/test
///
/// Sends a real message on one channel using the STORED config. This exists
/// because notification config is the classic write-and-forget setting: a typo
/// in a chat id or an SMTP password is invisible until the night something
/// breaks, which is the worst possible moment to discover it.
///
/// It deliberately ignores the `enabled` master switch — you test before
/// turning it on, not after.
pub async fn test_notify(
    _admin: AdminOnly,
    State(state): State<AppState>,
    Json(req): Json<TestNotifyRequest>,
) -> Json<ApiResponse<TestNotifyResponse>> {
    let cfg = load(&state).await;
    let text = "✅ RelayPanel 测试消息\n\n如果你收到这条消息，说明通知配置正确。";

    let result = match req.channel.as_str() {
        "telegram" => notify::send_telegram(&cfg, text).await,
        "email" => notify::send_email(&cfg, "RelayPanel 测试消息", text).await,
        other => {
            return Json(err(400, &format!("未知的通知渠道: {other}")));
        }
    };

    match result {
        Ok(()) => {
            tracing::info!(action = "test_notify", channel = %req.channel, "test message sent");
            Json(ApiResponse::success(TestNotifyResponse {
                ok: true,
                detail: String::new(),
            }))
        }
        Err(detail) => {
            // 200 with ok=false, not an HTTP error: the REQUEST succeeded, the
            // delivery didn't, and the UI needs the provider's own words to be
            // able to fix it.
            tracing::warn!(action = "test_notify", channel = %req.channel, %detail, "test failed");
            Json(ApiResponse::success(TestNotifyResponse {
                ok: false,
                detail,
            }))
        }
    }
}
