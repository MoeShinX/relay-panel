use crate::api::AppState;
use axum::extract::{FromRef, FromRequestParts};
use axum::http::request::Parts;
use axum::http::{Method, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use jsonwebtoken::{decode, Algorithm, DecodingKey, Validation};
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Claims {
    pub sub: i64,
    pub admin: bool,
    // v0.4.10 PR4: session-version. Must match users.token_version or the token
    // is rejected (revoked by a password change / admin reset / ban). Kept in
    // sync manually with the signing-side Claims in auth.rs.
    #[serde(default)]
    pub token_version: i64,
    pub exp: usize,
}

pub struct AuthUser {
    pub user_id: i64,
    pub admin: bool,
}

impl AuthUser {
    /// The ownership scope this caller's resource queries are restricted to.
    /// An admin sees everything (`All`); anyone else only their own rows
    /// (`Owner(user_id)`). The db layer uses this to fold the owner filter into
    /// its SQL — it never imports the api layer, so this is the single bridge.
    pub fn resource_scope(&self) -> crate::db::repo::ResourceScope {
        if self.admin {
            crate::db::repo::ResourceScope::All
        } else {
            crate::db::repo::ResourceScope::Owner(self.user_id)
        }
    }
}

/// Auth extraction failure. Distinguishes:
///   - genuine "not authenticated" (HTTP 401) — missing/invalid token,
///     deleted or banned user;
///   - "authenticated but forbidden" (HTTP 403) — a valid logged-in
///     non-admin hitting an admin-only endpoint;
///   - a transient backend failure (HTTP 500).
///
/// The 401-vs-403 split matters for the frontend: the axios interceptor treats
/// a 401 as "token invalid → force logout". Before v0.4.9 a logged-in
/// non-admin on an admin endpoint also got 401, so opening the app (Dashboard
/// hits /admin/*) immediately logged them out — the "登录闪退" bug. Now they
/// get 403, which the frontend handles gracefully (redirect to /account).
#[derive(Debug)]
pub enum AuthError {
    Unauthorized,
    /// Authenticated but lacking the required role (e.g. non-admin on an
    /// AdminOnly endpoint). Maps to HTTP 403.
    Forbidden,
    /// v0.4.10 PR4: the user must change their password before using any
    /// non-whitelisted endpoint. Maps to HTTP 403 with a STRUCTURED body
    /// `{code:"PASSWORD_CHANGE_REQUIRED"}` so the frontend can distinguish it
    /// from an ordinary role-based 403 (which it must NOT — it redirects to the
    /// force-password-change page instead of showing a 403/logging out).
    PasswordChangeRequired,
    /// A transient DB error while re-checking the user (banned/exists).
    Db,
}

impl IntoResponse for AuthError {
    fn into_response(self) -> Response {
        match self {
            AuthError::Unauthorized => (StatusCode::UNAUTHORIZED, "Unauthorized").into_response(),
            AuthError::Forbidden => (StatusCode::FORBIDDEN, "Forbidden").into_response(),
            AuthError::PasswordChangeRequired => (
                StatusCode::FORBIDDEN,
                Json(serde_json::json!({
                    "code": "PASSWORD_CHANGE_REQUIRED",
                    "message": "Password change required",
                    "data": null,
                })),
            )
                .into_response(),
            AuthError::Db => (StatusCode::INTERNAL_SERVER_ERROR, "Database error").into_response(),
        }
    }
}

async fn extract_auth_user(parts: &mut Parts, state: &AppState) -> Result<AuthUser, AuthError> {
    let auth_header = parts
        .headers
        .get("Authorization")
        .and_then(|v| v.to_str().ok())
        .ok_or(AuthError::Unauthorized)?;

    let token = auth_header
        .strip_prefix("Bearer ")
        .ok_or(AuthError::Unauthorized)?;

    let token_data = decode::<Claims>(
        token,
        &DecodingKey::from_secret(state.config.jwt_secret.as_bytes()),
        &Validation::new(Algorithm::HS256),
    )
    .map_err(|_| AuthError::Unauthorized)?;

    // Re-check the database on every request (one indexed PK lookup). v0.4.10
    // PR4: find_auth_state_by_id returns (banned, token_version,
    // must_change_password) in ONE query, replacing the old banned-only check.
    //
    //   - missing row → deleted user → 401
    //   - banned → 401
    //   - claims.token_version != db.token_version → 401 (token revoked by a
    //     password change / admin reset / ban; the only way to invalidate a JWT
    //     before its 24h exp)
    //   - must_change_password → block every endpoint EXCEPT the password-change
    //     whitelist (see below), returning PasswordChangeRequired (403 + code)
    //   - the admin flag comes from this same row, NOT from the JWT claim, so
    //     a demoted admin loses privileges immediately (and a promoted user
    //     gains them without re-login). `claims.admin` is deliberately ignored.
    let (banned, db_token_version, must_change_password, db_admin) =
        match state.db.find_auth_state_by_id(token_data.claims.sub).await {
            Ok(Some(auth_state)) => auth_state,
            Ok(None) => return Err(AuthError::Unauthorized), // user deleted
            Err(e) => {
                tracing::error!("auth db lookup failed: {}", e);
                return Err(AuthError::Db);
            }
        };

    if banned {
        return Err(AuthError::Unauthorized);
    }
    if token_data.claims.token_version != db_token_version {
        // Token issued before a version bump → revoked. 401 so the frontend
        // forces a fresh login.
        return Err(AuthError::Unauthorized);
    }

    if must_change_password && !is_password_change_whitelisted(parts) {
        return Err(AuthError::PasswordChangeRequired);
    }

    Ok(AuthUser {
        user_id: token_data.claims.sub,
        admin: db_admin,
    })
}

/// Whitelist for must_change_password users: only `GET /user/me` and
/// `PUT /user/password` are allowed; everything else returns
/// PasswordChangeRequired. Matches by METHOD + exact path.
///
/// axum's `.nest("/api/v1", ...)` strips the prefix from `parts.uri.path()`
/// inside extractors, so the path seen here is normally `/user/me`. We match
/// BOTH the stripped and full forms defensively: if the whitelist failed to
/// match, a must_change_password user could not even reach the password-change
/// endpoint and would be permanently locked out — so robustness here is a
/// safety requirement, not just tidiness.
fn is_password_change_whitelisted(parts: &Parts) -> bool {
    let path = parts.uri.path();
    match parts.method {
        Method::GET => path == "/user/me" || path == "/api/v1/user/me",
        Method::PUT => path == "/user/password" || path == "/api/v1/user/password",
        _ => false,
    }
}

impl<S: Send + Sync> FromRequestParts<S> for AuthUser
where
    AppState: axum::extract::FromRef<S>,
{
    type Rejection = AuthError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let state = AppState::from_ref(state);
        extract_auth_user(parts, &state).await
    }
}

/// Admin-only guard that also carries the authenticated admin's user id,
/// so admin endpoints can attribute created resources (rules/groups) to the
/// actual caller instead of hardcoding uid=1.
pub struct AdminOnly {
    pub user_id: i64,
}

impl<S: Send + Sync> FromRequestParts<S> for AdminOnly
where
    AppState: axum::extract::FromRef<S>,
{
    type Rejection = AuthError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let state = AppState::from_ref(state);
        let user = extract_auth_user(parts, &state).await?;
        if user.admin {
            Ok(AdminOnly {
                user_id: user.user_id,
            })
        } else {
            // v0.4.9: a logged-in non-admin is FORBIDDEN (403), not
            // Unauthorized (401). Returning 401 here was the root cause of
            // the "登录闪退" bug: the frontend's 401 interceptor cleared the
            // token + redirected to /login the moment a non-admin hit any
            // /admin/* endpoint (e.g. Dashboard's on-mount calls).
            Err(AuthError::Forbidden)
        }
    }
}
