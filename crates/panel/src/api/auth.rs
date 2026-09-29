use crate::api::auth_throttle::{
    client_ip, run_bcrypt, IP_LIMITER, MAX_LOGIN_PASSWORD, MAX_LOGIN_USERNAME, USERNAME_LIMITER,
};
use crate::api::AppState;
use crate::service::password::{
    hash_password, validate_password, verify_password, PasswordValidationError,
};
use crate::service::users::validate_username;
use axum::extract::{ConnectInfo, State};
use axum::http::HeaderMap;
use axum::{Extension, Json};
use jsonwebtoken::{encode, EncodingKey, Header};
use relay_shared::models::User;
use relay_shared::protocol::*;
use serde::{Deserialize, Serialize};
use std::net::SocketAddr;

/// The TCP peer, when the server was started with connect info (main.rs does;
/// handler tests that build the router without it simply skip the per-IP
/// limit).
type Peer = Option<Extension<ConnectInfo<SocketAddr>>>;

fn too_many() -> ApiResponse<LoginResponse> {
    ApiResponse {
        code: 429,
        message: "Too many login attempts. Please wait a minute and try again.".into(),
        data: None,
    }
}

fn busy<T: Serialize>() -> ApiResponse<T> {
    ApiResponse {
        code: 429,
        message: "The server is busy. Please try again in a moment.".into(),
        data: None,
    }
}

fn invalid_credentials() -> ApiResponse<LoginResponse> {
    ApiResponse {
        code: 401,
        message: "Invalid credentials".into(),
        data: None,
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct Claims {
    sub: i64, // user id
    admin: bool,
    // v0.4.10 PR4: session-version counter copied from users.token_version at
    // sign time. The auth middleware rejects a token whose token_version != the
    // current DB value, so bumping the DB column instantly revokes this token.
    token_version: i64,
    exp: usize,
}

/// Pre-computed dummy bcrypt hash used when the username does not exist.
/// Verifying against this eliminates the timing side-channel that would
/// otherwise reveal whether a username is registered (~300 ms bcrypt vs ~1 ms
/// early return).
///
/// v1.2.12: the previous constant was not a valid bcrypt hash, so verifying
/// against it failed in microseconds and the side channel stayed wide open
/// (unknown username ≈ instant, known one ≈ 250 ms). This one is a real
/// cost-12 hash of a throwaway string; `dummy_hash_costs_a_full_bcrypt` pins it.
static DUMMY_HASH: &str = "$2b$12$TP5SNHPKxVJPX4LDVdMoUOyPDSwBf9JhE.WrdCmN5sMMhNhPLLsXm";

pub async fn login(
    State(state): State<AppState>,
    peer: Peer,
    headers: HeaderMap,
    Json(req): Json<LoginRequest>,
) -> Json<ApiResponse<LoginResponse>> {
    // v1.2.12: see api::auth_throttle. A login that cannot match costs
    // nothing — no bcrypt, no rate-limit entry.
    if req.username.is_empty()
        || req.username.len() > MAX_LOGIN_USERNAME
        || req.password.len() > MAX_LOGIN_PASSWORD
    {
        return Json(invalid_credentials());
    }
    if let Some(ip) = client_ip(peer.map(|Extension(ConnectInfo(a))| a), &headers) {
        if IP_LIMITER.hit(&ip) {
            return Json(too_many());
        }
    }
    // Max 5 attempts per username per 60s.
    if USERNAME_LIMITER.hit(&req.username) {
        return Json(too_many());
    }

    let user: Option<User> = match state.db.find_by_username_not_banned(&req.username).await {
        Ok(u) => u,
        Err(e) => {
            tracing::error!("login: db lookup failed for {:?}: {}", req.username, e);
            None
        }
    };

    // Always perform a bcrypt verification to prevent timing attacks that
    // reveal whether a username exists. When the user is None we verify
    // against a pre-computed dummy hash so the CPU cost is identical.
    // v1.2.12: on the blocking pool, under the global bcrypt permits.
    let hash = user
        .as_ref()
        .map_or_else(|| DUMMY_HASH.to_string(), |u| u.password.clone());
    let password = req.password.clone();
    let Some(ok) = run_bcrypt(move || verify_password(&password, &hash)).await else {
        return Json(busy());
    };
    let verified = ok && user.is_some();

    if verified {
        if let Some(user) = user {
            USERNAME_LIMITER.clear(&req.username);
            let claims = Claims {
                sub: user.id,
                admin: user.admin,
                token_version: user.token_version,
                exp: chrono::Utc::now().timestamp_millis() as usize / 1000 + 86400,
            };
            let token = encode(
                &Header::default(),
                &claims,
                &EncodingKey::from_secret(state.config.jwt_secret.as_bytes()),
            )
            .unwrap_or_default();

            return Json(ApiResponse::success(LoginResponse {
                token,
                admin: user.admin,
            }));
        }
    }

    Json(invalid_credentials())
}

pub async fn register(
    State(state): State<AppState>,
    peer: Peer,
    headers: HeaderMap,
    Json(req): Json<RegisterRequest>,
) -> Json<ApiResponse<()>> {
    // v1.2.12: registration also runs bcrypt for anyone, so it shares the
    // per-IP limit and the bcrypt permits with login.
    if let Some(ip) = client_ip(peer.map(|Extension(ConnectInfo(a))| a), &headers) {
        if IP_LIMITER.hit(&ip) {
            return Json(ApiResponse {
                code: 429,
                message: "Too many attempts. Please wait a minute and try again.".into(),
                data: None,
            });
        }
    }
    // v0.4.10 PR3: registration toggle now lives in app_settings (admin-managed),
    // NOT the REGISTRATION_ENABLED env var. The env var only seeds the row on
    // first boot; afterwards only the admin PUT can change it. A missing row
    // (unseeded) is treated as "disabled" (safe default).
    let settings =
        match crate::service::settings::get_registration_settings(state.db.as_ref()).await {
            Ok(s) => s,
            Err(e) => {
                tracing::error!("register: registration settings lookup failed: {}", e);
                return Json(ApiResponse {
                    code: 500,
                    message: "database error".into(),
                    data: None,
                });
            }
        };
    let enabled = settings.registration_enabled;
    if !enabled {
        return Json(ApiResponse {
            code: 403,
            message: "Registration is disabled. Ask an admin to create your account.".into(),
            data: None,
        });
    }

    // v0.4.21 PR2: resolve plan_id from request, falling back to the default.
    // Validate it is in the allowed list.
    let selected_plan_id = req.plan_id.unwrap_or(settings.default_registration_plan_id);
    if !settings.allowed_plan_ids.contains(&selected_plan_id) {
        return Json(ApiResponse {
            code: 400,
            message: "Selected plan is not available for registration.".into(),
            data: None,
        });
    }

    // Validate username: non-empty, ≤64 chars, ASCII alphanumeric + underscore.
    // Prevents table rendering breakage and DB bloat from absurd inputs.
    if !validate_username(&req.username) {
        return Json(ApiResponse {
            code: 400,
            message: "Username must be 1-64 chars, ASCII letters/digits/underscore only".into(),
            data: None,
        });
    }

    // v0.4.10 PR3: password length validation. bcrypt truncates at 72 bytes,
    // so anything longer is silently weakened; anything shorter than 8 is
    // trivially brute-forced. len() is UTF-8 bytes (matches bcrypt's boundary).
    if let Err(e) = validate_password(&req.password) {
        return Json(ApiResponse {
            code: 400,
            message: match e {
                PasswordValidationError::TooShort => "Password must be at least 8 characters",
                PasswordValidationError::TooLong => {
                    "Password must be at most 72 bytes (bcrypt limit)"
                }
            }
            .into(),
            data: None,
        });
    }

    let password = req.password.clone();
    let Some(hashed) = run_bcrypt(move || hash_password(&password)).await else {
        return Json(busy());
    };
    let hashed = match hashed {
        Ok(h) => h,
        Err(e) => {
            return Json(ApiResponse {
                code: 500,
                message: format!("Failed to hash password: {}", e),
                data: None,
            });
        }
    };

    // v0.4.10 PR3: insert_user_from_plan atomically copies the plan's quota
    // fields (max_rules/traffic_limit/speed_limit/ip_limit) via INSERT...SELECT,
    // closing the "validate plan then plan changes" race. The default plan_id
    // comes from app_settings. Match order matters:
    //   Ok(1)                    → registered
    //   Ok(0)                    → plan missing (deleted out from under us) → 500
    //   Err(UniqueViolation)     → concurrent same-username register → 409
    //   Err(other)               → 500
    let plan_id = selected_plan_id;
    match state
        .db
        .insert_user_from_plan(&req.username, &hashed, plan_id)
        .await
    {
        Ok(1) => Json(ApiResponse::success(())),
        Ok(0) => {
            tracing::error!(
                "register: default plan {} is missing; no user created",
                plan_id
            );
            Json(ApiResponse {
                code: 500,
                message: "Registration is misconfigured (default plan missing). \
                          Contact an administrator."
                    .into(),
                data: None,
            })
        }
        Ok(_) => Json(ApiResponse {
            // Should not happen for a single-row insert; defensive.
            code: 500,
            message: "database error".into(),
            data: None,
        }),
        Err(crate::db::error::DbError::UniqueViolation) => Json(ApiResponse {
            code: 409,
            message: "Username already exists".into(),
            data: None,
        }),
        Err(e) => {
            tracing::error!("register: insert failed for {:?}: {}", req.username, e);
            Json(ApiResponse {
                code: 500,
                message: "database error".into(),
                data: None,
            })
        }
    }
}

/// v0.4.10 PR3 / v0.4.21 PR2: public registration-status probe.
/// Unauthenticated (used by the login page to decide whether to show
/// the "create account" link and the registration page to render a plan
/// selector). Returns enabled flag, default_plan_id, and the list of
/// allowed plans (filtered from the full plans table).
///
/// A DB error is surfaced as 500 (NOT masqueraded as "disabled"), so a panel
/// outage doesn't make users think registration is closed.
pub async fn registration_status(
    State(state): State<AppState>,
) -> Json<ApiResponse<RegistrationStatus>> {
    let settings =
        match crate::service::settings::get_registration_settings(state.db.as_ref()).await {
            Ok(s) => s,
            Err(e) => {
                tracing::error!("registration_status: settings lookup failed: {}", e);
                return Json(ApiResponse {
                    code: 500,
                    message: "database error".into(),
                    data: None,
                });
            }
        };

    let allowed_set: std::collections::HashSet<i64> =
        settings.allowed_plan_ids.iter().copied().collect();

    let all_plans: Vec<relay_shared::models::Plan> = match state.db.list_plans().await {
        Ok(p) => p,
        Err(e) => {
            tracing::error!("registration_status: list_plans failed: {}", e);
            return Json(ApiResponse {
                code: 500,
                message: "database error".into(),
                data: None,
            });
        }
    };

    let plans: Vec<relay_shared::models::Plan> = all_plans
        .into_iter()
        .filter(|p| allowed_set.contains(&p.id))
        .collect();

    Json(ApiResponse::success(RegistrationStatus {
        enabled: settings.registration_enabled,
        default_plan_id: settings.default_registration_plan_id,
        plans,
        default_password_change_required: default_password_change_required(state.db.as_ref()).await,
    }))
}

/// v0.4.22: check whether the default admin (id=1) still has
/// must_change_password set. Used by the login page to decide whether
/// to show the security reminder banner.
async fn default_password_change_required(db: &dyn crate::db::repo::Repository) -> bool {
    match db.find_auth_state_by_id(1).await {
        Ok(Some((_banned, _version, must_change))) => must_change,
        // User doesn't exist (fresh DB before seed) or DB error → no banner.
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::DUMMY_HASH;
    use std::str::FromStr;

    /// v1.2.12: the dummy must be a well-formed cost-12 bcrypt hash, or an
    /// unknown username is answered in microseconds and the response time
    /// tells which usernames exist.
    #[test]
    fn dummy_hash_costs_a_full_bcrypt() {
        let parts = bcrypt::HashParts::from_str(DUMMY_HASH).expect("a valid bcrypt hash");
        assert_eq!(parts.get_cost(), 12, "same cost as real password hashes");
        assert_eq!(
            bcrypt::verify("not-the-password", DUMMY_HASH).ok(),
            Some(false)
        );
    }
}
