use super::err;
use crate::api::middleware::AdminOnly;
use crate::api::AppState;
use crate::db::repo::RegistrationSettings;
use crate::service::settings::RegistrationSettingsError;
use axum::{extract::State, Json};
use relay_shared::protocol::*;

/// v0.4.21 PR2: read the registration settings (admin-only). Returns the full
/// row { registration_enabled, default_registration_plan_id, allowed_plan_ids }.
pub async fn get_registration_settings(
    _admin: AdminOnly,
    State(state): State<AppState>,
) -> Json<ApiResponse<RegistrationSettings>> {
    let settings =
        match crate::service::settings::get_registration_settings(state.db.as_ref()).await {
            Ok(s) => s,
            Err(e) => {
                tracing::error!("get_registration_settings: db error: {}", e);
                return Json(err(500, "数据库错误"));
            }
        };
    Json(ApiResponse::success(settings))
}

/// v0.4.21 PR2: update the registration settings (admin-only). Validates
/// allowed_plan_ids and default_plan_id before writing.
pub async fn update_registration_settings(
    _admin: AdminOnly,
    State(state): State<AppState>,
    Json(req): Json<RegistrationSettingsRequest>,
) -> Json<ApiResponse<RegistrationSettings>> {
    match crate::service::settings::update_registration_settings(
        state.db.as_ref(),
        req.enabled,
        req.default_plan_id,
        &req.allowed_plan_ids,
    )
    .await
    {
        Ok(settings) => {
            crate::service::audit::record(
                &state,
                Some(_admin.user_id),
                "update_registration_settings",
                "settings",
                "registration",
                &format!(
                    "公开注册 {} / 默认套餐 #{}",
                    if settings.registration_enabled {
                        "开启"
                    } else {
                        "关闭"
                    },
                    settings.default_registration_plan_id
                ),
            )
            .await;
            Json(ApiResponse::success(settings))
        }
        Err(e) => {
            let (code, msg): (i32, String) = match e {
                RegistrationSettingsError::DefaultPlanMissing => (400, "默认套餐不存在".into()),
                RegistrationSettingsError::AllowedPlansEmpty => {
                    (400, "允许注册的套餐列表不能为空".into())
                }
                RegistrationSettingsError::DefaultPlanNotInAllowed => {
                    (400, "默认套餐必须在允许注册的套餐列表中".into())
                }
                RegistrationSettingsError::AllowedPlanNotFound(id) => {
                    tracing::error!(
                        "update_registration_settings: allowed plan {} does not exist",
                        id
                    );
                    (400, format!("套餐 {} 不存在", id))
                }
                RegistrationSettingsError::Database(e) => {
                    tracing::error!("update_registration_settings: db error: {}", e);
                    (500, "数据库错误".into())
                }
            };
            Json(err(code, msg))
        }
    }
}
