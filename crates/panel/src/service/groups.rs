//! Device group + rule deletion service.
//!
//! Extracted from `api/admin/{groups,rules}.rs`. Houses the group CRUD business
//! rules (admin-owned token generation, no-fields guard, 404-on-zero-rows) and
//! the rule deletion mutation. The handlers keep HTTP concerns + the
//! connection-manager side effects (`close_group` / `broadcast_all`) and audit
//! logging — those depend on `node_connections`, not the `Repository`.

use crate::db::error::DbError;
use crate::db::repo::ResourceScope;
use crate::db::Repository;
use crate::service::rules::group_type_to_str;
use relay_shared::models::DeviceGroup;
use relay_shared::protocol::GroupType;

#[derive(Debug)]
pub enum CreateGroupError {
    /// INSERT succeeded but the follow-up SELECT-by-token found nothing.
    FetchFailed,
    Database(DbError),
}

#[derive(Debug)]
pub enum UpdateGroupError {
    NotFound,
    NoFields,
    Database(DbError),
}

/// v1.0.8: billing rate bounds. `rate` lives on device_groups; users are
/// charged real bytes × rate in `apply_traffic_batch`. 1.0 = bill what you
/// use. The same bounds are enforced inside `apply_traffic_batch` (a stray
/// out-of-range value refuses the whole traffic batch) — this is the
/// write-side guard so bad values never get persisted.
pub const RATE_MIN: f64 = 0.1;
pub const RATE_MAX: f64 = 100.0;
pub const RATE_DEFAULT: f64 = 1.0;

/// Validate a billing rate. Returns the clamped-or-passed value, or `None`
/// when the input is out of `[RATE_MIN, RATE_MAX]` (callers map None → 400).
pub fn validate_rate(rate: f64) -> Option<f64> {
    (RATE_MIN..=RATE_MAX).contains(&rate).then_some(rate)
}

/// Create an admin-owned device group. Generates a fresh token, inserts, then
/// returns the persisted row (INSERT-then-SELECT-by-token; the token is a
/// freshly generated UUID so the SELECT is guaranteed to hit the new row).
///
/// v0.4.12 PR1: device groups are admin-managed shared infrastructure — the
/// caller passes the creating admin's id as `owner_uid` (the handler ignores
/// any client-supplied owner_uid).
pub async fn create_group(
    db: &dyn Repository,
    name: &str,
    group_type: &GroupType,
    owner_uid: i64,
    connect_host: &str,
    port_range: &str,
    rate: f64,
) -> Result<DeviceGroup, CreateGroupError> {
    let token = uuid::Uuid::new_v4().to_string();
    let group_type = group_type_to_str(group_type);
    db.insert_group(
        name,
        group_type,
        &token,
        owner_uid,
        connect_host,
        port_range,
        rate,
    )
    .await
    .map_err(CreateGroupError::Database)?;

    match db.find_by_token_after_insert(&token).await {
        Ok(Some(g)) => Ok(g),
        Ok(None) => Err(CreateGroupError::FetchFailed),
        Err(e) => Err(CreateGroupError::Database(e)),
    }
}

/// Rotate a device group's node token. Generates a fresh UUID and persists it.
/// Returns `Ok(Some(new_token))` when a row changed, `Ok(None)` when the group
/// didn't exist (the handler maps that to 404). The connection teardown
/// (`close_group`) + broadcast stay in the handler.
pub async fn rotate_group_token(db: &dyn Repository, id: i64) -> Result<Option<String>, DbError> {
    // v0.4.12 PR1: admin-only. Scope All — an admin operates on any group.
    let new_token = uuid::Uuid::new_v4().to_string();
    match db
        .update_group_token(id, &ResourceScope::All, &new_token)
        .await?
    {
        0 => Ok(None),
        _ => Ok(Some(new_token)),
    }
}

/// Update an admin-owned device group. Enforces the no-fields guard and
/// 404-on-zero-rows. The token is NOT updatable here (rotation is a separate
/// endpoint).
pub async fn update_group(
    db: &dyn Repository,
    id: i64,
    name: Option<&str>,
    group_type: Option<&GroupType>,
    connect_host: Option<&str>,
    port_range: Option<&str>,
    rate: Option<f64>,
) -> Result<(), UpdateGroupError> {
    if name.is_none()
        && group_type.is_none()
        && connect_host.is_none()
        && port_range.is_none()
        && rate.is_none()
    {
        return Err(UpdateGroupError::NoFields);
    }

    match db
        .update_group_fields(
            id,
            &ResourceScope::All,
            name,
            group_type.map(group_type_to_str),
            connect_host,
            port_range,
            rate,
        )
        .await
        .map_err(UpdateGroupError::Database)?
    {
        0 => Err(UpdateGroupError::NotFound),
        _ => Ok(()),
    }
}

/// v1.0.4: error returned when a group cannot be deleted because rules
/// still reference it.
#[derive(Debug)]
pub struct GroupInUseError {
    pub group_id: i64,
    pub rule_count: i64,
}

impl std::fmt::Display for GroupInUseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "group {} still referenced by {} rule(s)",
            self.group_id, self.rule_count
        )
    }
}

impl std::error::Error for GroupInUseError {}

/// Delete an admin-owned device group. Before deleting, checks that no
/// forward_rules reference this group via device_group_in, device_group_out,
/// or fallback_group. Returns `GroupInUseError` with the rule count if any
/// references exist, so the handler can return a clear 409.
pub async fn delete_group(
    db: &dyn Repository,
    id: i64,
) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
    let count = db.count_rules_by_group(id).await?;
    if count > 0 {
        return Err(Box::new(GroupInUseError {
            group_id: id,
            rule_count: count,
        }));
    }
    Ok(db.delete_group(id, &ResourceScope::All).await? > 0)
}

/// Delete a rule within `scope` (owner-scoped for regular users, All for
/// admins). Returns `Ok(true)` when a row was deleted, `Ok(false)` when nothing
/// matched (the handler maps that to 404 + no broadcast).
pub async fn delete_rule(
    db: &dyn Repository,
    id: i64,
    scope: &ResourceScope,
) -> Result<bool, DbError> {
    Ok(db.delete_rule(id, scope).await? > 0)
}
