//! v1.2.0: manual rule restart — drop a rule's live connections and rebuild its
//! listeners on every node of its inbound group.
//!
//! ## Why this is not pause + resume
//!
//! The obvious implementation is two writes: `paused = true`, then
//! `paused = false`. It needs no new protocol, but it is wrong for a button
//! whose entire job is "get me unstuck":
//!
//! - If the resume half fails (node offline, authorization revoked between the
//!   two calls, panel restarted mid-way), the rule is left PAUSED. The user
//!   clicked "restart" and got an outage. Batch restart makes this worse: a
//!   partial failure strands an arbitrary subset of rules off.
//! - The port is released while paused, so auto-assignment can hand it to
//!   another rule in the gap.
//! - It writes `paused`, which resets `auto_paused` (v1.0.8) and so corrupts
//!   the system-paused vs. human-paused distinction.
//!
//! A restart carries no state instead: it either happens or it doesn't, and the
//! rule's stored fields are never touched.
//!
//! ## Old nodes
//!
//! A node below `node_supports_restart_rule` silently ignores the unknown WS
//! message. Reporting those as restarted would be a lie the user can't detect,
//! so they are surfaced explicitly as "upgrade this node" — the Node Status page
//! already offers one-click upgrade.

use axum::extract::{Path, State};
use axum::Json;
use relay_shared::protocol::{node_supports_restart_rule, ApiResponse, RestartRuleMessage};
use serde::Serialize;

use crate::api::diagnose::group_node_statuses;
use crate::api::middleware::AuthUser;
use crate::api::AppState;
use crate::db::repo::ResourceScope;

/// One node's outcome for a restart request.
#[derive(Debug, Serialize, PartialEq)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum NodeRestartStatus {
    /// The restart command reached this node's live control channel.
    Restarted {
        node_id: String,
        group_name: String,
        public_ip: Option<String>,
    },
    /// The node is too old to understand `restart_rule`. It would ignore the
    /// message, so we do NOT send it and tell the operator to upgrade instead.
    Unsupported {
        node_id: String,
        group_name: String,
        public_ip: Option<String>,
        node_version: Option<String>,
    },
    /// No live WS connection (or it dropped between the check and the send).
    ControlChannelOffline {
        node_id: String,
        group_name: String,
        public_ip: Option<String>,
    },
}

#[derive(Debug, Serialize)]
pub struct RestartResponse {
    pub rule_id: i64,
    /// How many nodes actually received the command. The frontend keys its
    /// success/warning message off this rather than off the HTTP status: a
    /// request can succeed while restarting nothing (every node old or offline).
    pub restarted: usize,
    pub nodes: Vec<NodeRestartStatus>,
}

/// Send `restart_rule` for `rule_id` to every eligible node of `group_id`.
///
/// Shared by the HTTP handler and the auto-restart scheduler so both apply the
/// same version gate and the same online check — a scheduled restart must not
/// quietly do something a manual one wouldn't.
///
/// `request_id` only correlates panel logs with node logs; nothing is sent back
/// over the wire. A restart is fire-and-forget by design: the node's work is
/// local and bounded (cancel connections, rebind), and making the HTTP call wait
/// for confirmation from every node would hold a request open on the slowest
/// node for no decision the caller can act on.
pub(crate) async fn dispatch_restart(
    state: &AppState,
    rule_id: i64,
    group_id: i64,
    request_id: &str,
) -> Result<Vec<NodeRestartStatus>, crate::db::error::DbError> {
    // ResourceScope::All: the group is shared infrastructure and the caller's
    // ownership of the RULE was already checked. Scoping this lookup to the
    // caller would make a regular user's restart fail on an admin-owned group.
    let group_name = crate::db::repo::GroupRepository::find_by_id(
        state.db.as_ref(),
        group_id,
        &ResourceScope::All,
    )
    .await
    .ok()
    .flatten()
    .map(|g| g.name)
    .unwrap_or_else(|| format!("#{group_id}"));

    let nodes = group_node_statuses(state, group_id, group_name).await?;
    let online = state.node_connections.online_node_ids(group_id).await;

    let msg_for = |node_id: &str| {
        serde_json::to_string(&RestartRuleMessage::new(
            node_id.to_string(),
            rule_id,
            request_id.to_string(),
        ))
        .unwrap_or_default()
    };

    let mut out = Vec::with_capacity(nodes.len());
    for n in &nodes {
        // Version FIRST, then liveness — mirrors the diagnose classifier. An old
        // node with a healthy socket must read as "upgrade me", not "offline":
        // the socket is fine, the node just can't do this.
        if !node_supports_restart_rule(n.node_version.as_deref()) {
            out.push(NodeRestartStatus::Unsupported {
                node_id: n.node_id.clone(),
                group_name: n.group_name.clone(),
                public_ip: n.public_ip.clone(),
                node_version: n.node_version.clone(),
            });
            continue;
        }
        if !online.contains(&n.node_id) {
            out.push(NodeRestartStatus::ControlChannelOffline {
                node_id: n.node_id.clone(),
                group_name: n.group_name.clone(),
                public_ip: n.public_ip.clone(),
            });
            continue;
        }
        // send_node returns the number of live connections it reached; 0 means
        // the WS dropped between the online check above and here.
        if state
            .node_connections
            .send_node(group_id, &n.node_id, &msg_for(&n.node_id))
            .await
            > 0
        {
            out.push(NodeRestartStatus::Restarted {
                node_id: n.node_id.clone(),
                group_name: n.group_name.clone(),
                public_ip: n.public_ip.clone(),
            });
        } else {
            out.push(NodeRestartStatus::ControlChannelOffline {
                node_id: n.node_id.clone(),
                group_name: n.group_name.clone(),
                public_ip: n.public_ip.clone(),
            });
        }
    }
    Ok(out)
}

/// POST /api/v1/rules/{id}/restart — drop the rule's connections and rebuild its
/// listeners.
///
/// Owner-scoped: a regular user may restart only their own rules (the scope
/// folds `uid = ?` into the lookup, so someone else's rule_id is a uniform 404).
/// Admins are unscoped.
pub async fn restart_rule(
    user: AuthUser,
    State(state): State<AppState>,
    Path(rule_id): Path<i64>,
) -> Json<ApiResponse<RestartResponse>> {
    let scope = user.resource_scope();
    tracing::info!(
        action = "restart_rule",
        rule_id = rule_id,
        actor_id = user.user_id,
        actor_admin = user.admin,
        "rule restart requested"
    );

    let rule = match state.db.find_rule_by_id(rule_id, &scope).await {
        Ok(Some(r)) => r,
        Ok(None) => {
            return Json(ApiResponse {
                code: 404,
                message: "Rule not found".into(),
                data: None,
            })
        }
        Err(e) => {
            tracing::error!("restart_rule {}: find_rule_by_id failed: {}", rule_id, e);
            return Json(ApiResponse {
                code: 500,
                message: "database error".into(),
                data: None,
            });
        }
    };

    // A paused rule has no listener on any node, so a restart is a guaranteed
    // no-op. Reject rather than report a hollow success — the user's actual
    // intent for a paused rule is "resume", which is a different button.
    if rule.paused {
        return Json(ApiResponse {
            code: 400,
            message: "规则已暂停，无需重启（启用后才有连接）".into(),
            data: None,
        });
    }

    let request_id = uuid_like_id();
    let nodes = match dispatch_restart(&state, rule_id, rule.device_group_in, &request_id).await {
        Ok(n) => n,
        Err(e) => {
            tracing::error!("restart_rule {}: dispatch failed: {}", rule_id, e);
            return Json(ApiResponse {
                code: 500,
                message: "database error".into(),
                data: None,
            });
        }
    };

    let restarted = nodes
        .iter()
        .filter(|n| matches!(n, NodeRestartStatus::Restarted { .. }))
        .count();
    tracing::info!(
        "restart_rule: actor_id={} rule_id={} request_id={} reached {}/{} node(s)",
        user.user_id,
        rule_id,
        request_id,
        restarted,
        nodes.len()
    );

    Json(ApiResponse::success(RestartResponse {
        rule_id,
        restarted,
        nodes,
    }))
}

/// Correlation id for one restart run. Not a real UUID and not security-
/// relevant — it only ties a panel log line to a node log line, so process id +
/// nanosecond timestamp is enough to be unique in practice.
fn uuid_like_id() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("rst-{}-{}", std::process::id(), nanos)
}
