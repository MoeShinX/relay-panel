use super::err;
use crate::api::middleware::AdminOnly;
use crate::api::AppState;
use axum::{
    extract::{Path, State},
    Json,
};
use relay_shared::models::*;
use relay_shared::protocol::*;

// === Plans (v1.0.8) ===
//
// Admin CRUD over the plans table. GET returns ALL plans (including hidden);
// the public shop endpoint (GET /plans) filters hidden=0. Deletion is blocked
// (409) when any user's plan_id still references the plan.

/// v1.0.9: a Plan plus the device groups it grants on purchase. `#[serde(flatten)]`
/// keeps every Plan field at the top level so the JSON shape stays a superset
/// of `Plan` (the frontend reads `device_group_ids` alongside the plan fields).
#[derive(serde::Serialize)]
pub struct PlanWithGroups {
    #[serde(flatten)]
    pub plan: Plan,
    pub device_group_ids: Vec<i64>,
    /// v1.0.9: resolved names for `device_group_ids`. The shop needs these
    /// because a plan grants groups the buyer isn't authorized for yet, so the
    /// shared-group endpoint (visible groups only) can't resolve the ids — it
    /// would fall back to "#<id>". Resolving server-side fixes that. Order is
    /// by name (not id); it's a display-only set.
    pub device_group_names: Vec<String>,
}

/// Validate the invariant fields shared by create + update. Returns the
/// canonicalized price on success, or an error message on failure.
/// `name_required` controls whether an empty name is rejected (create) or
/// allowed through (update, where None means "leave unchanged").
fn validate_plan_fields(
    name: Option<&str>,
    max_rules: Option<i32>,
    traffic: Option<i64>,
    price: Option<&str>,
    plan_type: Option<&str>,
    duration_days: Option<i32>,
    name_required: bool,
) -> Result<Option<String>, String> {
    if let Some(n) = name {
        let trimmed = n.trim();
        if name_required && trimmed.is_empty() {
            return Err("名称不能为空".into());
        }
        if trimmed.len() > 100 {
            return Err("名称不能超过100个字符".into());
        }
    }
    if let Some(mr) = max_rules {
        if !(0..=100_000).contains(&mr) {
            return Err("max_rules 必须在 0 到 100000 之间".into());
        }
    }
    if let Some(t) = traffic {
        if t < 0 {
            return Err("流量必须为非负数".into());
        }
    }
    if let Some(pt) = plan_type {
        if pt != "data" && pt != "time" {
            return Err("套餐类型必须是 data 或 time".into());
        }
    }
    if let Some(dd) = duration_days {
        if dd < 0 {
            return Err("时长天数必须为非负数".into());
        }
        // A time plan with duration_days=0 makes no sense; reject it at write
        // time so the shop never offers an instantly-expiring plan.
        if plan_type == Some("time") && dd == 0 {
            return Err("限时套餐的时长天数必须大于 0".into());
        }
    }
    // price is a decimal string — canonicalize via the balance parser (same
    // rules: non-negative, ≤ 2 fraction digits, ≤ 9999999999.99). None on the
    // update path means "leave unchanged".
    match price {
        None => Ok(None),
        Some(raw) => match relay_shared::money::parse_balance(raw) {
            Ok(c) => Ok(Some(c)),
            Err(reason) => Err(reason.into()),
        },
    }
}

pub async fn list_plans(
    _admin: AdminOnly,
    State(state): State<AppState>,
) -> Json<ApiResponse<Vec<PlanWithGroups>>> {
    let plans: Vec<Plan> = match state.db.list_plans().await {
        Ok(p) => p,
        Err(e) => {
            // v1.0.9: surface DB failures as 500 instead of masquerading as an
            // empty (but "successful") list — the admin UI would otherwise show
            // "no plans" on a transient DB error.
            tracing::error!("list_plans: db error: {}", e);
            return Json(err(500, "数据库错误"));
        }
    };
    // Attach each plan's grant set. N+1 over the (small) plan list — fine for an
    // admin-only page; a JOIN-aggregate could replace it if plan counts grow.
    let mut out = Vec::with_capacity(plans.len());
    for plan in plans {
        let device_group_ids = state
            .db
            .list_plan_device_groups(plan.id)
            .await
            .unwrap_or_else(|e| {
                tracing::error!(
                    "list_plans: list_plan_device_groups({}) failed: {}",
                    plan.id,
                    e
                );
                Vec::new()
            });
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

pub async fn create_plan(
    _admin: AdminOnly,
    State(state): State<AppState>,
    Json(req): Json<CreatePlanRequest>,
) -> Json<ApiResponse<i64>> {
    let canonical_price = match validate_plan_fields(
        Some(&req.name),
        Some(req.max_rules),
        Some(req.traffic),
        Some(&req.price),
        Some(&req.plan_type),
        Some(req.duration_days),
        true,
    ) {
        Ok(p) => p.unwrap_or_default(),
        Err(msg) => return Json(err(400, msg)),
    };

    // v1.0.9: insert the plan AND its device-group grant set atomically, so a
    // failure can't leave a plan row with no grants (was two separate calls).
    let id = match state
        .db
        .create_plan_with_groups(
            &req.name,
            req.max_rules,
            req.traffic,
            &canonical_price,
            &req.plan_type,
            req.duration_days,
            req.hidden,
            req.reset_traffic,
            &req.description,
            req.grant_all_groups,
            &req.device_group_ids,
        )
        .await
    {
        Ok(id) => id,
        Err(e) => {
            tracing::error!("create_plan: db error: {}", e);
            return Json(err(500, "数据库错误"));
        }
    };

    Json(ApiResponse::success(id))
}

pub async fn update_plan(
    _admin: AdminOnly,
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(req): Json<UpdatePlanRequest>,
) -> Json<ApiResponse<()>> {
    if req.name.is_none()
        && req.max_rules.is_none()
        && req.traffic.is_none()
        && req.price.is_none()
        && req.plan_type.is_none()
        && req.duration_days.is_none()
        && req.hidden.is_none()
        && req.reset_traffic.is_none()
        && req.description.is_none()
        && req.grant_all_groups.is_none()
        && req.device_group_ids.is_none()
    {
        return Json(err(400, "无需要更新的字段"));
    }

    // v1.0.8: when plan_type is being changed to 'time' in this same request,
    // the duration_days > 0 rule must be evaluated against the NEW type. If
    // plan_type isn't being changed, we can't know the existing type here
    // cheaply, so we only enforce the cross-field rule when plan_type is
    // present in the request.
    let effective_plan_type = req.plan_type.as_deref();
    let canonical_price = match validate_plan_fields(
        req.name.as_deref(),
        req.max_rules,
        req.traffic,
        req.price.as_deref(),
        effective_plan_type,
        req.duration_days,
        false,
    ) {
        Ok(p) => p,
        Err(msg) => return Json(err(400, msg)),
    };

    // Reject the (plan_type=time, duration_days=0) combination when BOTH are
    // supplied together — validate_plan_fields only checks it when plan_type is
    // present, so cover the case where the caller flips to time but leaves
    // duration_days untouched (None) by reading the existing row.
    if let Some("time") = effective_plan_type {
        if req.duration_days == Some(0) {
            return Json(err(400, "限时套餐的时长天数必须大于 0"));
        }
        if req.duration_days.is_none() {
            // Caller set plan_type=time without duration_days — verify the
            // existing row's duration_days is > 0 before flipping.
            match state.db.find_plan_by_id(id).await {
                Ok(Some(p)) if p.duration_days > 0 => {}
                Ok(Some(_)) => return Json(err(400, "限时套餐的时长天数必须大于 0")),
                Ok(None) => return Json(err(404, "套餐不存在")),
                Err(e) => {
                    tracing::error!("update_plan {}: lookup failed: {}", id, e);
                    return Json(err(500, "数据库错误"));
                }
            }
        }
    } else if req.duration_days == Some(0) && req.plan_type.is_none() {
        // v1.0.9: the reverse gap — setting duration_days=0 WITHOUT touching
        // plan_type. validate_plan_fields only checks the time+0 rule when
        // plan_type is present, so a caller could quietly zero out an existing
        // TIME plan's duration. Read the stored type and reject if it's time.
        match state.db.find_plan_by_id(id).await {
            Ok(Some(p)) if p.plan_type == "time" => {
                return Json(err(400, "限时套餐的时长天数必须大于 0"));
            }
            Ok(Some(_)) => {}
            Ok(None) => return Json(err(404, "套餐不存在")),
            Err(e) => {
                tracing::error!("update_plan {}: lookup failed: {}", id, e);
                return Json(err(500, "数据库错误"));
            }
        }
    }

    // v1.0.9: `device_group_ids` is NOT a plans-table column (it lives in
    // plan_device_groups), so a request carrying ONLY device_group_ids would
    // make update_plan_fields a no-op (0 rows) and be misread as 404. Split the
    // two: apply scalar fields via update_plan_fields ONLY when present, and
    // replace the grant set separately. Track existence so the right error wins.
    let has_scalar_update = req.name.is_some()
        || req.max_rules.is_some()
        || req.traffic.is_some()
        || req.price.is_some()
        || req.plan_type.is_some()
        || req.duration_days.is_some()
        || req.hidden.is_some()
        || req.reset_traffic.is_some()
        || req.description.is_some()
        || req.grant_all_groups.is_some();

    if has_scalar_update {
        match state
            .db
            .update_plan_fields(
                id,
                req.name.as_deref(),
                req.max_rules,
                req.traffic,
                canonical_price.as_deref(),
                req.plan_type.as_deref(),
                req.duration_days,
                req.hidden,
                req.reset_traffic,
                req.description.as_deref(),
                req.grant_all_groups,
            )
            .await
        {
            Ok(0) => return Json(err(404, "套餐不存在")),
            Ok(_) => {}
            Err(e) => {
                tracing::error!("update_plan {}: db error: {}", id, e);
                return Json(err(500, "数据库错误"));
            }
        }
    } else {
        // Only a grant-set change: confirm the plan exists so we still 404.
        match state.db.find_plan_by_id(id).await {
            Ok(Some(_)) => {}
            Ok(None) => return Json(err(404, "套餐不存在")),
            Err(e) => {
                tracing::error!("update_plan {}: existence check failed: {}", id, e);
                return Json(err(500, "数据库错误"));
            }
        }
    }

    // v1.0.9: REPLACE the grant set when device_group_ids is provided. None =
    // leave the existing grants untouched.
    if let Some(ref ids) = req.device_group_ids {
        if let Err(e) = state.db.set_plan_device_groups(id, ids).await {
            tracing::error!("update_plan {}: set_plan_device_groups failed: {}", id, e);
            return Json(err(500, "数据库错误"));
        }
    }

    // A plan change can alter max_rules / the shop list, but does NOT change
    // what nodes forward (gating is per-user, not per-plan). No broadcast.
    // Existing users' authorizations are NOT retroactively changed by editing a
    // plan's grant set — grants apply at purchase time only.
    Json(ApiResponse::success(()))
}

pub async fn delete_plan(
    _admin: AdminOnly,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Json<ApiResponse<()>> {
    // Pre-delete safety check: refuse if any user's plan_id references this
    // plan. Deleting would orphan the FK and leave users on a ghost plan.
    let in_use = match state.db.count_users_on_plan(id).await {
        Ok(n) => n,
        Err(e) => {
            tracing::error!("delete_plan {}: count_users_on_plan failed: {}", id, e);
            return Json(err(500, "数据库错误"));
        }
    };
    if in_use > 0 {
        return Json(err(
            409,
            format!("该套餐仍被 {} 个用户使用，请先迁移用户。", in_use),
        ));
    }

    match state.db.delete_plan(id).await {
        Ok(0) => Json(err(404, "套餐不存在")),
        Ok(_) => Json(ApiResponse::success(())),
        Err(e) => {
            tracing::error!("delete_plan {}: db error: {}", id, e);
            Json(err(500, "数据库错误"))
        }
    }
}
