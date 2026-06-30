use super::{err, UserSelf};
use crate::api::middleware::{AdminOnly, AuthUser};
use crate::api::AppState;
use crate::service::password::{
    hash_password, validate_password, verify_password, PasswordValidationError,
};
use axum::{
    extract::{Path, State},
    Json,
};
use relay_shared::protocol::ApiResponse;
// === v0.4.10 PR4: admin password reset ===
/// PUT /admin/users/{id}/password — an admin sets a (temporary) password for
/// another user. Atomically bumps the target's token_version (revoking ALL
/// their sessions) and optionally sets must_change_password so the temporary
/// password forces a change on first login.
///
/// Refuses to reset ANOTHER admin's password (privilege protection): an admin
/// changes their own password via /user/password, never another admin's here.
pub async fn reset_user_password(
    admin: AdminOnly,
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(req): Json<ResetPasswordRequest>,
) -> Json<ApiResponse<()>> {
    // Unified password policy: 8..=72 UTF-8 bytes.
    if let Err(e) = validate_password(&req.new_password) {
        return Json(err(
            400,
            match e {
                PasswordValidationError::TooShort => "New password must be at least 8 characters",
                PasswordValidationError::TooLong => "New password must be at most 72 bytes",
            },
        ));
    }

    // The target must exist; and an admin cannot reset ANOTHER admin's password.
    let exists = match state.db.exists_by_id(id).await {
        Ok(v) => v,
        Err(e) => {
            tracing::error!("reset_user_password {}: exists lookup failed: {}", id, e);
            return Json(err(500, "database error"));
        }
    };
    if !exists {
        return Json(err(404, "User not found"));
    }
    let target_is_admin = match state.db.is_admin(id).await {
        Ok(v) => v,
        Err(e) => {
            tracing::error!("reset_user_password {}: is_admin lookup failed: {}", id, e);
            return Json(err(500, "database error"));
        }
    };
    if target_is_admin && id != admin.user_id {
        return Json(err(403, "Cannot reset another admin's password"));
    }

    let new_hash = match hash_password(&req.new_password) {
        Ok(h) => h,
        Err(e) => return Json(err(500, format!("Failed to hash password: {}", e))),
    };

    match state
        .db
        .admin_reset_password(id, &new_hash, req.must_change_password)
        .await
    {
        Ok(0) => Json(err(404, "User not found")),
        Ok(_) => {
            // Audit: actor + target + must_change flag. NEVER log the password
            // or its hash.
            tracing::warn!(
                action = "admin_reset_password",
                actor_admin_id = admin.user_id,
                target_user_id = id,
                must_change_password = req.must_change_password,
                "admin reset a user's password (sessions revoked)"
            );
            Json(ApiResponse::success(()))
        }
        Err(e) => {
            tracing::error!(
                "reset_user_password {}: admin_reset_password failed: {}",
                id,
                e
            );
            Json(err(500, "database error"))
        }
    }
}

// === Reset traffic (v0.3.4) ===
/// Zero out a user's traffic_used AND all their forward_rules.traffic_used in
/// one transaction, so the user total and per-rule detail stay consistent.
/// Admin can reset anyone including themselves.
pub async fn reset_user_traffic(
    _admin: AdminOnly,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Json<ApiResponse<()>> {
    // Verify the user exists first (404, not silent success).
    let exists = match state.db.exists_by_id(id).await {
        Ok(v) => v,
        Err(e) => {
            tracing::error!("reset_user_traffic {}: exists lookup failed: {}", id, e);
            return Json(err(500, "database error"));
        }
    };
    if !exists {
        return Json(err(404, "User not found"));
    }

    // Atomic: both updates in one transaction inside the repository. If either
    // fails, neither lands — prevents the "user total zeroed but rules still
    // show old traffic" split.
    match state.db.reset_traffic(id).await {
        Ok(()) => {
            tracing::warn!(
                action = "reset_user_traffic",
                target_user_id = id,
                actor_admin_id = _admin.user_id,
                "destructive admin op"
            );
            Json(ApiResponse::success(()))
        }
        Err(e) => {
            tracing::error!("reset_user_traffic {}: reset_traffic failed: {}", id, e);
            Json(err(500, "database error"))
        }
    }
}

// === Change Password ===
#[derive(Debug, serde::Deserialize)]
pub struct ChangePasswordRequest {
    pub current_password: String,
    pub new_password: String,
}

// === v0.4.10 PR4: admin password reset ===
#[derive(Debug, serde::Deserialize)]
pub struct ResetPasswordRequest {
    pub new_password: String,
    /// When true, the target user must change this (temporary) password on
    /// first login. Defaults to true client-side; required here.
    pub must_change_password: bool,
}

/// GET /user/me — the calling user's own account info (no password hash).
/// Any authenticated user (admin or not) can read their own row. The response
/// is the [`UserSelf`] projection, which deliberately omits the password column
/// (and plan_id/group_id/speed_limit/ip_limit/banned — not needed by the
/// account page). 404 only if the user was deleted between JWT issue and now.
pub async fn get_me(user: AuthUser, State(state): State<AppState>) -> Json<ApiResponse<UserSelf>> {
    // Load the user row first — everything else depends on it.
    let u = match crate::db::repo::UserRepository::find_by_id(state.db.as_ref(), user.user_id).await
    {
        Ok(Some(u)) => u,
        Ok(None) => {
            return Json(ApiResponse {
                code: 404,
                message: "User not found".into(),
                data: None,
            })
        }
        Err(e) => {
            tracing::error!("get_me {}: find_by_id failed: {}", user.user_id, e);
            return Json(ApiResponse {
                code: 500,
                message: "database error".into(),
                data: None,
            });
        }
    };

    // v0.4.10: resolve the two derived fields (rule count + plan name) in
    // parallel. DB errors are NOT swallowed — a failed count or plan lookup
    // returns 500 rather than masquerading as "0 rules" or "no plan", which
    // would hide a real outage from the account page.
    let plan_id = u.plan_id;
    let (current_rules, plan_name) =
        match tokio::try_join!(state.db.count_by_uid(user.user_id), async {
            match plan_id {
                Some(pid) => state.db.find_plan_name_by_id(pid).await,
                None => Ok(None),
            }
        },)
        {
            Ok(v) => v,
            Err(e) => {
                tracing::error!(
                    "get_me {}: account projection query failed: {}",
                    user.user_id,
                    e
                );
                return Json(ApiResponse {
                    code: 500,
                    message: "database error".into(),
                    data: None,
                });
            }
        };

    Json(ApiResponse::success(UserSelf {
        id: u.id,
        username: u.username,
        admin: u.admin,
        balance: u.balance,
        plan_id: u.plan_id,
        plan_name,
        max_rules: u.max_rules,
        current_rules,
        traffic_used: u.traffic_used,
        traffic_limit: u.traffic_limit,
        registered_at: u.created_at,
        must_change_password: u.must_change_password,
        plan_expire_at: u.plan_expire_at,
        suspended: u.suspended,
    }))
}

/// Change the calling user's own password. Requires the current password to
/// be supplied (re-authentication) so a stolen JWT alone can't change it.
/// Any authenticated user can change their own password — not just admins.
pub async fn change_password(
    user: crate::api::middleware::AuthUser,
    State(state): State<AppState>,
    Json(req): Json<ChangePasswordRequest>,
) -> Json<ApiResponse<()>> {
    // v0.4.10 PR4: unified password policy — 8..=72 UTF-8 bytes (bcrypt limit).
    // len() is bytes, matching the frontend's TextEncoder byte check.
    if let Err(e) = validate_password(&req.new_password) {
        return Json(err(
            400,
            match e {
                PasswordValidationError::TooShort => "New password must be at least 8 characters",
                PasswordValidationError::TooLong => "New password must be at most 72 bytes",
            },
        ));
    }

    // Fetch the user's current password hash
    let current_hash = match state.db.find_password_by_id(user.user_id).await {
        Ok(Some(h)) => h,
        Ok(None) => return Json(err(404, "User not found")),
        Err(e) => {
            tracing::error!(
                "change_password {}: find_password_by_id failed: {}",
                user.user_id,
                e
            );
            return Json(err(500, "database error"));
        }
    };

    // Verify current password
    if !verify_password(&req.current_password, &current_hash) {
        return Json(err(401, "Current password is incorrect"));
    }

    // Hash and update
    let new_hash = match hash_password(&req.new_password) {
        Ok(h) => h,
        Err(e) => return Json(err(500, format!("Failed to hash password: {}", e))),
    };

    // v0.4.10 PR4: change_own_password atomically bumps token_version (revoking
    // ALL of this user's sessions, including the current one — the frontend
    // then re-logs in) and clears must_change_password.
    match state.db.change_own_password(user.user_id, &new_hash).await {
        Ok(_) => {
            tracing::info!(
                "user {} changed their password (sessions revoked)",
                user.user_id
            );
            Json(ApiResponse::success(()))
        }
        Err(e) => {
            tracing::error!(
                "change_password {}: change_own_password failed: {}",
                user.user_id,
                e
            );
            Json(err(500, "database error"))
        }
    }
}
