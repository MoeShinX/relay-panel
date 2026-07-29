use super::err;
use super::PlanWithGroups;
use crate::api::middleware::AuthUser;
use crate::api::AppState;
use crate::db::repo::BuyPlanError;
use axum::{extract::State, Json};
use relay_shared::models::*;
use relay_shared::protocol::*;

// === Shop (v1.0.8) ===
//
// Self-service plan purchase. GET /plans lists ONLY non-hidden plans. POST
// /user/buy-plan atomically charges the user's balance, stacks the plan's
// traffic onto their quota, sets max_rules / plan_id, computes a stacking
// expiry, and records an order row — all in one transaction (防双花).

/// GET /plans — public list of purchasable plans (hidden excluded). v1.0.9:
/// each plan carries its `device_group_ids` so the shop can show "包含线路".
pub async fn list_public_plans(
    _user: AuthUser,
    State(state): State<AppState>,
) -> Json<ApiResponse<Vec<PlanWithGroups>>> {
    let plans: Vec<Plan> = match state.db.list_visible_plans().await {
        Ok(p) => p,
        Err(e) => {
            // v1.0.9: a DB failure returns 500, not an empty "success" that the
            // shop would render as "no plans available".
            tracing::error!("list_public_plans: db error: {}", e);
            return Json(err(500, "数据库错误"));
        }
    };
    let mut out = Vec::with_capacity(plans.len());
    for plan in plans {
        // grant_all_groups plans grant everything — the explicit list is moot,
        // so skip the lookup and return an empty list (the card shows "全部线路").
        let device_group_ids = if plan.grant_all_groups {
            Vec::new()
        } else {
            state
                .db
                .list_plan_device_groups(plan.id)
                .await
                .unwrap_or_else(|e| {
                    tracing::error!(
                        "list_public_plans: list_plan_device_groups({}) failed: {}",
                        plan.id,
                        e
                    );
                    Vec::new()
                })
        };
        let device_group_names = state
            .db
            .list_group_names_by_ids(&device_group_ids)
            .await
            .unwrap_or_default();
        out.push(PlanWithGroups {
            plan,
            device_group_ids,
            device_group_names,
        });
    }
    Json(ApiResponse::success(out))
}

/// POST /user/buy-plan — purchase a plan. Refuses hidden plans, out-of-range
/// duration on time plans, and insufficient balance (atomic; no partial state).
pub async fn buy_plan(
    user: AuthUser,
    State(state): State<AppState>,
    Json(req): Json<BuyPlanRequest>,
) -> Json<ApiResponse<()>> {
    // Resolve the plan row first (outside the buy tx — read-only lookup).
    // Hidden plans are not self-purchasable even if the caller knows the id.
    let plan = match state.db.find_plan_by_id(req.plan_id).await {
        Ok(Some(p)) => p,
        Ok(None) => return Json(err(404, "套餐不存在")),
        Err(e) => {
            tracing::error!("buy_plan {}: plan lookup failed: {}", req.plan_id, e);
            return Json(err(500, "数据库错误"));
        }
    };
    if plan.hidden {
        // Don't reveal existence — same 404 as a missing plan.
        return Json(err(404, "套餐不存在"));
    }
    // A time plan must have a positive duration (the CRUD layer enforces this
    // too, but a pre-existing bad row shouldn't crash the purchase).
    if plan.plan_type == "time" && plan.duration_days <= 0 {
        return Json(err(400, "该套餐无有效时长"));
    }

    // Decimal money: compare + deduct in integer cents (no floats). price is
    // stored canonical, so balance_to_cents succeeds; a None here is a data
    // integrity fault — refuse rather than mis-bill.
    let price_cents = match relay_shared::money::balance_to_cents(&plan.price) {
        Some(c) => c,
        None => {
            tracing::error!(
                "buy_plan: plan {} has non-canonical price {:?}",
                plan.id,
                plan.price
            );
            return Json(err(500, "数据库错误"));
        }
    };

    // duration_days drives the expiry. Non-time plans (or duration_days=0)
    // → no expiry (NULL), per the spec's "不限时=NULL".
    let duration_days = if plan.plan_type == "time" {
        plan.duration_days
    } else {
        0
    };

    // v1.0.9: resolve the plan's device-group grant set. Only used when
    // grant_all_groups=false (buy_plan ignores the list otherwise), but fetch
    // it unconditionally so the same call shape covers both modes.
    let device_group_ids = if plan.grant_all_groups {
        Vec::new()
    } else {
        match state.db.list_plan_device_groups(plan.id).await {
            Ok(ids) => ids,
            Err(e) => {
                tracing::error!(
                    "buy_plan {}: list_plan_device_groups failed: {}",
                    plan.id,
                    e
                );
                return Json(err(500, "数据库错误"));
            }
        }
    };

    // v1.0.8: compute the new authorized set that will drive pause_rules_outside
    // _groups inside buy_plan. grant_all_groups → all inbound groups (nothing
    // paused), else the plan's own groups.
    let new_authorized_group_ids: Vec<i64> = if plan.grant_all_groups {
        match state.db.list_all_inbound_group_ids().await {
            Ok(ids) => ids,
            Err(e) => {
                tracing::error!(
                    "buy_plan {}: list_all_inbound_group_ids failed: {}",
                    plan.id,
                    e
                );
                return Json(err(500, "数据库错误"));
            }
        }
    } else {
        device_group_ids.clone()
    };

    match state
        .db
        .buy_plan(
            user.user_id,
            plan.id,
            &plan.name,
            price_cents,
            plan.traffic,
            plan.max_rules,
            duration_days,
            plan.reset_traffic,
            plan.grant_all_groups,
            &device_group_ids,
            &new_authorized_group_ids,
        )
        .await
    {
        Ok(()) => {
            // The purchase can change what the user's nodes forward (max_rules
            // / traffic_limit / expiry all feed list_active_for_config), so
            // refresh node configs.
            state
                .node_connections
                .broadcast_all(r#"{"type":"config_changed"}"#)
                .await;
            Json(ApiResponse::success(()))
        }
        Err(BuyPlanError::InsufficientBalance) => Json(err(400, "余额不足")),
        Err(BuyPlanError::Database(e)) => {
            tracing::error!("buy_plan {}: db error: {}", plan.id, e);
            Json(err(500, "数据库错误"))
        }
    }
}

/// GET /user/orders — the calling user's purchase history, newest first.
/// v1.3.0: one order with the buyer's name resolved for display.
#[derive(Debug, serde::Serialize)]
pub struct AdminOrderRow {
    pub id: i64,
    pub user_id: i64,
    /// None when the account was deleted — the order row survives as the
    /// money-in record, so the id is kept and only the name goes missing.
    pub username: Option<String>,
    pub plan_name: String,
    pub price: String,
    pub created_at: String,
}

#[derive(Debug, serde::Deserialize)]
pub struct AdminOrdersQuery {
    #[serde(default)]
    pub limit: Option<i64>,
    #[serde(default)]
    pub offset: Option<i64>,
}

#[derive(Debug, serde::Serialize)]
pub struct AdminOrdersResponse {
    pub items: Vec<AdminOrderRow>,
    pub total: i64,
}

/// GET /api/v1/admin/orders — every user's purchases.
///
/// The shop page shows the caller their own; this is the operator's view of
/// all of them, which is what answers "who bought what this week".
pub async fn list_all_orders(
    _admin: crate::api::middleware::AdminOnly,
    State(state): State<AppState>,
    axum::extract::Query(q): axum::extract::Query<AdminOrdersQuery>,
) -> Json<ApiResponse<AdminOrdersResponse>> {
    let limit = q.limit.unwrap_or(20).clamp(1, 200);
    let offset = q.offset.unwrap_or(0).max(0);

    let total = match state.db.count_all_orders().await {
        Ok(n) => n,
        Err(e) => {
            tracing::error!("list_all_orders: count failed: {}", e);
            return Json(err(500, "数据库错误"));
        }
    };
    let orders = match state.db.list_all_orders(limit, offset).await {
        Ok(o) => o,
        Err(e) => {
            tracing::error!("list_all_orders: query failed: {}", e);
            return Json(err(500, "数据库错误"));
        }
    };

    // One user fetch for the page, not one per row — the same shape the redeem
    // list uses. An id that no longer resolves stays None rather than blocking
    // the page: the order outlives the account on purpose.
    let names: std::collections::HashMap<i64, String> = match state.db.list_users_public().await {
        Ok(users) => users.into_iter().map(|u| (u.id, u.username)).collect(),
        Err(e) => {
            tracing::error!("list_all_orders: user lookup failed: {}", e);
            return Json(err(500, "数据库错误"));
        }
    };

    let items = orders
        .into_iter()
        .map(|o| AdminOrderRow {
            id: o.id,
            user_id: o.user_id,
            username: names.get(&o.user_id).cloned(),
            plan_name: o.plan_name,
            price: o.price,
            created_at: o.created_at,
        })
        .collect();
    Json(ApiResponse::success(AdminOrdersResponse { items, total }))
}

pub async fn list_my_orders(
    user: AuthUser,
    State(state): State<AppState>,
) -> Json<ApiResponse<Vec<Order>>> {
    let orders: Vec<Order> = match state.db.list_orders_by_user(user.user_id).await {
        Ok(o) => o,
        Err(e) => {
            tracing::error!("list_my_orders {}: db error: {}", user.user_id, e);
            return Json(err(500, "数据库错误"));
        }
    };
    Json(ApiResponse::success(orders))
}
