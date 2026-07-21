//! v1.2.0: redeem codes — admin generation/management + user redemption.
//!
//! Closes the loop the shop already assumed: the panel could deduct balance
//! (buy_plan) but had no way for a user to ADD any, so balance could only be
//! typed in by an admin. Codes need no payment gateway, no merchant account and
//! no compliance work — an admin generates a batch and sells or gifts it.

use axum::extract::{Path, Query, State};
use axum::Json;
use relay_shared::models::{RedeemCode, MAX_REDEEM_BATCH};
use relay_shared::money;
use relay_shared::protocol::ApiResponse;
use serde::{Deserialize, Serialize};

use crate::api::middleware::{AdminOnly, AuthUser};
use crate::api::AppState;
use crate::db::repo::{NewRedeemCode, RedeemCodeError, RedeemCodeFilter};
use crate::service::redeem;

fn err<T: Serialize>(code: i32, msg: &str) -> ApiResponse<T> {
    ApiResponse {
        code,
        message: msg.into(),
        data: None,
    }
}

/// UTC 'YYYY-MM-DD HH:MM:SS' — the timestamp format every other table uses, and
/// the one whose lexicographic order is chronological (expiry compares as TEXT).
fn now_utc() -> String {
    chrono::Utc::now().format("%Y-%m-%d %H:%M:%S").to_string()
}

#[derive(Debug, Deserialize)]
pub struct CreateCodesRequest {
    /// How many codes to generate (1..=MAX_REDEEM_BATCH).
    pub count: i64,
    /// Face value, e.g. "10" or "10.50".
    pub amount: String,
    /// Optional 'YYYY-MM-DD HH:MM:SS' UTC expiry. Omitted = never expires.
    #[serde(default)]
    pub expires_at: Option<String>,
    #[serde(default)]
    pub remark: String,
}

#[derive(Debug, Serialize)]
pub struct CreateCodesResponse {
    pub batch_id: String,
    /// How many rows actually landed (a duplicate code is skipped, not fatal).
    pub created: u64,
    /// The generated codes in DISPLAY form (dashed). Returned ONCE, here — the
    /// list endpoint returns them too, so this is convenience rather than a
    /// last chance, but it lets the admin copy the batch straight away.
    pub codes: Vec<String>,
}

/// POST /api/v1/admin/redeem-codes — generate a batch.
pub async fn create_codes(
    _admin: AdminOnly,
    State(state): State<AppState>,
    Json(req): Json<CreateCodesRequest>,
) -> Json<ApiResponse<CreateCodesResponse>> {
    if req.count < 1 || req.count > MAX_REDEEM_BATCH {
        return Json(err(
            400,
            &format!("生成数量必须在 1 ~ {} 之间", MAX_REDEEM_BATCH),
        ));
    }
    // Same validator as users.balance, so a code's face value can always be
    // added to a balance without producing a non-canonical string.
    let amount = match money::parse_balance(&req.amount) {
        Ok(a) => a,
        Err(e) => return Json(err(400, &format!("面额无效：{e}"))),
    };
    if money::balance_to_cents(&amount).is_none_or(|c| c == 0) {
        return Json(err(400, "面额必须大于 0"));
    }
    if let Some(exp) = req.expires_at.as_deref() {
        // Reject a malformed expiry rather than storing a string that would
        // compare wrong: expiry is a TEXT comparison, so a different format
        // (e.g. RFC3339 with a 'T') would sort incorrectly against now_utc().
        if chrono::NaiveDateTime::parse_from_str(exp, "%Y-%m-%d %H:%M:%S").is_err() {
            return Json(err(400, "过期时间格式应为 YYYY-MM-DD HH:MM:SS (UTC)"));
        }
    }

    let batch_id = format!("B{}", chrono::Utc::now().format("%Y%m%d%H%M%S"));
    let display: Vec<String> = (0..req.count).map(|_| redeem::generate_code()).collect();
    let rows: Vec<NewRedeemCode> = display
        .iter()
        .map(|d| NewRedeemCode {
            code: redeem::to_stored(d),
            amount: amount.clone(),
            expires_at: req.expires_at.clone(),
            batch_id: batch_id.clone(),
            remark: req.remark.clone(),
        })
        .collect();

    match state.db.create_redeem_codes(&rows).await {
        Ok(created) => {
            tracing::info!(
                action = "create_redeem_codes",
                batch_id = %batch_id,
                requested = req.count,
                created,
                amount = %amount,
                "redeem codes generated"
            );
            Json(ApiResponse::success(CreateCodesResponse {
                batch_id,
                created,
                codes: display,
            }))
        }
        Err(e) => {
            tracing::error!("create_redeem_codes failed: {}", e);
            Json(err(500, "数据库错误"))
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct ListCodesQuery {
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub batch_id: Option<String>,
    #[serde(default)]
    pub limit: Option<i64>,
    #[serde(default)]
    pub offset: Option<i64>,
}

#[derive(Debug, Serialize)]
pub struct CodeRow {
    pub id: i64,
    /// Display (dashed) form — what an admin copies and a user types.
    pub code: String,
    pub amount: String,
    pub status: String,
    pub used_by: Option<i64>,
    pub used_at: Option<String>,
    pub expires_at: Option<String>,
    pub batch_id: String,
    pub remark: String,
    pub created_at: String,
}

impl From<RedeemCode> for CodeRow {
    fn from(c: RedeemCode) -> Self {
        Self {
            id: c.id,
            code: redeem::to_display(&c.code),
            amount: c.amount,
            status: c.status,
            used_by: c.used_by,
            used_at: c.used_at,
            expires_at: c.expires_at,
            batch_id: c.batch_id,
            remark: c.remark,
            created_at: c.created_at,
        }
    }
}

#[derive(Debug, Serialize)]
pub struct ListCodesResponse {
    pub items: Vec<CodeRow>,
    pub total: i64,
}

/// GET /api/v1/admin/redeem-codes — list with filters + pagination.
pub async fn list_codes(
    _admin: AdminOnly,
    State(state): State<AppState>,
    Query(q): Query<ListCodesQuery>,
) -> Json<ApiResponse<ListCodesResponse>> {
    // Clamp so a hand-written query can't ask for the whole table.
    let limit = q.limit.unwrap_or(50).clamp(1, 500);
    let offset = q.offset.unwrap_or(0).max(0);
    let status = match q.status.as_deref() {
        None | Some("") | Some("all") => None,
        Some(s @ ("unused" | "used" | "void")) => Some(s.to_string()),
        Some(_) => return Json(err(400, "status 只能是 unused / used / void")),
    };
    let filter = RedeemCodeFilter {
        status,
        batch_id: q.batch_id.filter(|b| !b.is_empty()),
        limit,
        offset,
    };

    let items = match state.db.list_redeem_codes(&filter).await {
        Ok(v) => v.into_iter().map(CodeRow::from).collect(),
        Err(e) => {
            tracing::error!("list_redeem_codes failed: {}", e);
            return Json(err(500, "数据库错误"));
        }
    };
    let total = match state.db.count_redeem_codes(&filter).await {
        Ok(n) => n,
        Err(e) => {
            tracing::error!("count_redeem_codes failed: {}", e);
            return Json(err(500, "数据库错误"));
        }
    };
    Json(ApiResponse::success(ListCodesResponse { items, total }))
}

/// POST /api/v1/admin/redeem-codes/{id}/void — burn an unused code.
pub async fn void_code(
    _admin: AdminOnly,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Json<ApiResponse<()>> {
    match state.db.void_redeem_code(id).await {
        Ok(1) => {
            tracing::info!(action = "void_redeem_code", code_id = id, "code voided");
            Json(ApiResponse::success(()))
        }
        // 0 rows = already used or already void. A used code is deliberately
        // NOT voidable: the money moved, and rewriting that row would falsify
        // the audit trail.
        Ok(_) => Json(err(400, "该卡密已被使用或已作废，无法作废")),
        Err(e) => {
            tracing::error!("void_redeem_code {} failed: {}", id, e);
            Json(err(500, "数据库错误"))
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct DeleteCodesRequest {
    pub ids: Vec<i64>,
}

/// DELETE /api/v1/admin/redeem-codes — delete unused/voided codes in bulk.
pub async fn delete_codes(
    _admin: AdminOnly,
    State(state): State<AppState>,
    Json(req): Json<DeleteCodesRequest>,
) -> Json<ApiResponse<u64>> {
    if req.ids.is_empty() {
        return Json(err(400, "未选择卡密"));
    }
    match state.db.delete_unused_redeem_codes(&req.ids).await {
        Ok(n) => {
            tracing::info!(
                action = "delete_redeem_codes",
                requested = req.ids.len(),
                deleted = n,
                "codes deleted (used codes are never deleted)"
            );
            Json(ApiResponse::success(n))
        }
        Err(e) => {
            tracing::error!("delete_unused_redeem_codes failed: {}", e);
            Json(err(500, "数据库错误"))
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct RedeemRequest {
    pub code: String,
}

#[derive(Debug, Serialize)]
pub struct RedeemResponse {
    /// Face value credited.
    pub amount: String,
    /// The user's balance AFTER crediting, so the UI doesn't need a refetch.
    pub balance: String,
}

/// POST /api/v1/user/redeem — redeem a code onto the caller's own balance.
///
/// Deliberately scoped to `user.user_id` from the token: there is no user_id in
/// the request body, so this endpoint cannot be used to credit someone else.
pub async fn redeem_code(
    user: AuthUser,
    State(state): State<AppState>,
    Json(req): Json<RedeemRequest>,
) -> Json<ApiResponse<RedeemResponse>> {
    let Some(code) = redeem::normalize_code(&req.code) else {
        return Json(err(400, "请输入卡密"));
    };

    match state.db.redeem_code(&code, user.user_id, &now_utc()).await {
        Ok((amount, balance)) => {
            tracing::info!(
                action = "redeem_code",
                actor_id = user.user_id,
                amount = %amount,
                "code redeemed"
            );
            Json(ApiResponse::success(RedeemResponse { amount, balance }))
        }
        // ONE message for "no such code" and "already used". Distinguishing
        // them turns the endpoint into an oracle: a stranger could brute-force
        // codes and learn which guesses were real from the different error.
        Err(RedeemCodeError::NotRedeemable) => {
            tracing::warn!(
                action = "redeem_code",
                actor_id = user.user_id,
                "rejected: unknown/used/void code"
            );
            Json(err(400, "卡密无效或已被使用"))
        }
        Err(RedeemCodeError::Expired) => Json(err(400, "卡密已过期")),
        Err(RedeemCodeError::BalanceOverflow) => Json(err(400, "充值后余额将超出上限")),
        Err(RedeemCodeError::Database(e)) => {
            tracing::error!("redeem_code failed for user {}: {}", user.user_id, e);
            Json(err(500, "数据库错误"))
        }
    }
}
