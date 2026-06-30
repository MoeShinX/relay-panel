use super::{err, UserPublic};
use crate::api::middleware::AdminOnly;
use crate::api::AppState;
use crate::service::password::PasswordValidationError;
use crate::service::users::CreateUserError;
use axum::{
    extract::{Path, State},
    Json,
};
use relay_shared::protocol::{ApiResponse, UpdateUserRequest};
// === Users ===
pub async fn list_users(
    _admin: AdminOnly,
    State(state): State<AppState>,
) -> Json<ApiResponse<Vec<UserPublic>>> {
    // SELECT * is safe here — UserPublic has no `password` field, so sqlx
    // simply ignores that column. The hash never reaches the API response.
    let users: Vec<UserPublic> = state.db.list_users_public().await.unwrap_or_else(|e| {
        tracing::error!("list_users: db error: {}", e);
        Vec::new()
    });
    Json(ApiResponse::success(users))
}

/// Admin creates a NON-ADMIN user. Per the v0.4.4 two-tier model, admins can
/// only create regular users (never other admins) — `insert_user` always writes
/// admin=false (the schema default), so privilege escalation is impossible here.
/// The admin supplies the username + initial password.
#[derive(Debug, serde::Deserialize)]
pub struct CreateUserRequest {
    pub username: String,
    pub password: String,
}

pub async fn create_user(
    _admin: AdminOnly,
    State(state): State<AppState>,
    Json(req): Json<CreateUserRequest>,
) -> Json<ApiResponse<()>> {
    match crate::service::users::create_user(state.db.as_ref(), &req.username, &req.password).await
    {
        Ok(()) => {
            tracing::info!(
                action = "create_user",
                actor_admin_id = _admin.user_id,
                "admin created user {:?}",
                req.username
            );
            Json(ApiResponse::success(()))
        }
        Err(CreateUserError::InvalidUsername) => Json(err(
            400,
            "Username must be 1-64 chars, ASCII letters/digits/underscore only",
        )),
        Err(CreateUserError::Password(PasswordValidationError::TooShort)) => {
            Json(err(400, "Password must be at least 8 characters"))
        }
        Err(CreateUserError::Password(PasswordValidationError::TooLong)) => {
            Json(err(400, "Password must be at most 72 bytes"))
        }
        Err(CreateUserError::Hash(e)) => Json(err(500, format!("Failed to hash password: {}", e))),
        Err(CreateUserError::DuplicateUsername) => Json(err(409, "Username already exists")),
        Err(CreateUserError::DefaultPlanMissing) => {
            tracing::error!("create_user: default plan 1 is missing; no user created");
            Json(err(
                500,
                "Default plan is missing; contact an administrator",
            ))
        }
        Err(CreateUserError::Database(e)) => {
            tracing::error!("create_user: insert failed for {:?}: {}", req.username, e);
            Json(err(500, "database error"))
        }
    }
}

pub async fn delete_user(
    _admin: AdminOnly,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Json<ApiResponse<()>> {
    // Check the target first. Admin users are protected, and their associated
    // rules/groups must be protected too — do not clean anything up until the
    // target is known to be a deletable non-admin user.
    let is_admin = match state.db.is_admin(id).await {
        Ok(v) => v,
        Err(e) => {
            tracing::error!("delete_user {}: is_admin lookup failed: {}", id, e);
            return Json(err(500, "database error"));
        }
    };
    // Also need to confirm the row exists (is_admin returns false for both
    // "non-admin exists" and "doesn't exist" — distinguish via exists_by_id).
    let exists = match state.db.exists_by_id(id).await {
        Ok(v) => v,
        Err(e) => {
            tracing::error!("delete_user {}: exists lookup failed: {}", id, e);
            return Json(err(500, "database error"));
        }
    };
    if !exists || is_admin {
        return Json(err(
            404,
            "User not found (or is an admin and cannot be deleted)",
        ));
    }

    // Atomic cascade delete: removes the user's rules, tunnel_profiles and
    // device_groups, then the user row itself, all in ONE transaction with the
    // admin guard baked in. Returns 0 (and rolls back) if the target is an admin
    // or no longer exists — so we never leave a half-deleted account.
    match state.db.delete_user_cascade(id).await {
        Ok(0) => Json(err(
            404,
            "User not found (or is an admin and cannot be deleted)",
        )),
        Ok(_) => {
            tracing::warn!(
                action = "delete_user",
                target_user_id = id,
                actor_admin_id = _admin.user_id,
                "destructive admin op"
            );
            Json(ApiResponse::success(()))
        }
        Err(e) => {
            tracing::error!("delete_user {}: cascade delete failed: {}", id, e);
            Json(err(500, "database error"))
        }
    }
}

// === Update user (v0.3.4) ===
/// Admin edits a user's quota / balance / ban status. Deliberately cannot
/// change password, admin role, or id (see UpdateUserRequest doc).
pub async fn update_user(
    _admin: AdminOnly,
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(req): Json<UpdateUserRequest>,
) -> Json<ApiResponse<()>> {
    // v1.0.7: device-group authorization (all_device_groups / device_group_ids)
    // is handled ALONGSIDE the other fields (not early-return, which would drop
    // any balance/quota/banned submitted in the same request). All fields
    // optional — if nothing provided, bail early.
    if req.balance.is_none()
        && req.max_rules.is_none()
        && req.traffic_limit.is_none()
        && req.banned.is_none()
        && req.suspended.is_none()
        && req.all_device_groups.is_none()
        && req.device_group_ids.is_none()
    {
        return Json(err(400, "No fields to update"));
    }

    // Clamp numeric inputs to sane ranges (prevent overflow / absurd values).
    if let Some(mr) = req.max_rules {
        if !(0..=100_000).contains(&mr) {
            return Json(err(400, "max_rules must be between 0 and 100000"));
        }
    }
    if let Some(tl) = req.traffic_limit {
        if tl < 0 {
            return Json(err(400, "traffic_limit must be non-negative"));
        }
    }

    // v0.3.5: balance is still a TEXT column but admins can now edit it via
    // this endpoint. Validate the input shape strictly (non-negative decimal,
    // ≤ 2 fraction digits, ≤ 9999999999.99) and store the canonical form so
    // every row in the DB looks the same regardless of what the caller typed.
    // The check happens before we touch the SQL builder so a rejected value
    // never reaches the DB.
    let canonical_balance: Option<String> = match req.balance.as_deref() {
        None => None,
        Some(raw) => match relay_shared::money::parse_balance(raw) {
            Ok(c) => Some(c),
            Err(reason) => return Json(err(400, reason)),
        },
    };

    // Cannot ban an admin user (privilege protection).
    if req.banned == Some(true) {
        let is_admin = match state.db.is_admin(id).await {
            Ok(v) => v,
            Err(e) => {
                tracing::error!("update_user {}: is_admin lookup failed: {}", id, e);
                return Json(err(500, "database error"));
            }
        };
        if is_admin {
            return Json(err(400, "Cannot ban an admin user"));
        }
    }

    // v1.0.8: cannot suspend an admin user either (same privilege protection).
    if req.suspended == Some(true) {
        let is_admin = match state.db.is_admin(id).await {
            Ok(v) => v,
            Err(e) => {
                tracing::error!("update_user {}: is_admin lookup failed: {}", id, e);
                return Json(err(500, "database error"));
            }
        };
        if is_admin {
            return Json(err(400, "Cannot suspend an admin user"));
        }
    }

    // v1.0.4: apply field updates only when field-update args are present
    // (a group_id-only request must NOT hit update_user_fields, whose all-None
    // UPDATE would return 0 rows and be misread as "User not found").
    let has_field_update = req.balance.is_some()
        || req.max_rules.is_some()
        || req.traffic_limit.is_some()
        || req.banned.is_some()
        || req.suspended.is_some();

    if has_field_update {
        match state
            .db
            .update_user_fields(
                id,
                canonical_balance.as_deref(),
                req.max_rules,
                req.traffic_limit,
                req.banned,
                req.suspended,
            )
            .await
        {
            Ok(0) => return Json(err(404, "User not found")),
            Ok(_) => {
                if let Some(banned) = req.banned {
                    tracing::warn!(
                        action = if banned { "ban_user" } else { "unban_user" },
                        target_user_id = id,
                        actor_admin_id = _admin.user_id,
                        "destructive admin op"
                    );
                }
                if let Some(suspended) = req.suspended {
                    tracing::warn!(
                        action = if suspended { "suspend_user" } else { "unsuspend_user" },
                        target_user_id = id,
                        actor_admin_id = _admin.user_id,
                        "admin op"
                    );
                }
            }
            Err(e) => {
                tracing::error!("update_user {}: update_user_fields failed: {}", id, e);
                return Json(err(500, "database error"));
            }
        }
    }

    // v1.0.7: device-group authorization change. The per-user all_device_groups
    // flag and/or the explicit device-group assignments are applied here. After
    // re-authorizing, pause any of the user's rules whose inbound group is no
    // longer allowed — the rules + their data are kept so an admin can
    // re-authorize and resume. (set_user_all_device_groups is a no-op for admins,
    // who are always all-allowed.)
    let authz_changed = req.all_device_groups.is_some() || req.device_group_ids.is_some();
    if let Some(all) = req.all_device_groups {
        if let Err(e) = state.db.set_user_all_device_groups(id, all).await {
            tracing::error!("update_user {}: set_user_all_device_groups failed: {}", id, e);
            return Json(err(500, "database error"));
        }
    }
    if let Some(ref ids) = req.device_group_ids {
        if let Err(e) = state.db.set_user_device_groups(id, ids).await {
            tracing::error!("update_user {}: set_user_device_groups failed: {}", id, e);
            return Json(err(500, "database error"));
        }
    }
    if authz_changed {
        // Pause rules outside the user's NEW authorization.
        let allowed = match state.db.authorized_device_group_ids(id).await {
            Ok(a) => a,
            Err(e) => {
                tracing::error!("update_user {}: authz lookup for pause failed: {}", id, e);
                return Json(err(500, "database error"));
            }
        };
        match state.db.pause_rules_outside_groups(id, &allowed).await {
            Ok(n) if n > 0 => {
                tracing::warn!(
                    "update_user {}: paused {} rule(s) outside new authorization",
                    id,
                    n
                );
            }
            Ok(_) => {}
            Err(e) => {
                tracing::error!(
                    "update_user {}: pause_rules_outside_groups failed: {}",
                    id,
                    e
                );
                return Json(err(500, "database error"));
            }
        }
    }

    // A field update (ban) or an authorization change (pause) both alter what
    // nodes should forward, so refresh node config once at the end.
    state
        .node_connections
        .broadcast_all(r#"{"type":"config_changed"}"#)
        .await;
    Json(ApiResponse::success(()))
}

// === v1.0.7: per-user device-group authorization ===

/// A user's current device-group authorization, for preloading the admin
/// editor. `all_device_groups` short-circuits `device_group_ids` (when true the
/// user may use every group regardless of the explicit list).
#[derive(Debug, serde::Serialize)]
pub struct UserDeviceGroups {
    pub all_device_groups: bool,
    pub device_group_ids: Vec<i64>,
}

/// GET /users/{id}/device-groups — the explicit assignments + the all flag.
/// Updates go through PUT /users/{id} (update_user).
pub async fn get_user_device_groups(
    _admin: AdminOnly,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Json<ApiResponse<UserDeviceGroups>> {
    let all_device_groups = match crate::db::repo::UserRepository::find_by_id(state.db.as_ref(), id)
        .await
    {
        Ok(Some(u)) => u.all_device_groups,
        Ok(None) => return Json(err(404, "User not found")),
        Err(e) => {
            tracing::error!("get_user_device_groups {}: find_by_id failed: {}", id, e);
            return Json(err(500, "database error"));
        }
    };
    let device_group_ids = match state.db.list_user_device_groups(id).await {
        Ok(ids) => ids,
        Err(e) => {
            tracing::error!("get_user_device_groups {}: list failed: {}", id, e);
            return Json(err(500, "database error"));
        }
    };
    Json(ApiResponse::success(UserDeviceGroups {
        all_device_groups,
        device_group_ids,
    }))
}
