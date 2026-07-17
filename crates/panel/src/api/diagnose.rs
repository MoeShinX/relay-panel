// v0.4.8: in-memory rule-diagnosis task registry.
//
// A diagnosis run is started by POST /rules/{id}/diagnose. The panel:
//   1. generates a request_id (uuid)
//   2. records the rule_id + the set of node_ids it expects results from
//   3. sends DiagnoseRuleMessage to the rule's inbound group over WS
//   4. waits up to DIAGNOSE_TIMEOUT for results to arrive via POST
//      /api/v1/node/diagnose_result (correlated by request_id + node_id)
//
// Results are collected here as they arrive. The HTTP handler that started a
// run polls the registry and returns once all expected nodes replied or the
// deadline elapses (whichever first).
//
// Everything is in memory: a panel restart loses in-flight runs, which the
// frontend surfaces as "诊断已中断". This is acceptable — diagnosis is an
// on-demand read-only probe, not persistent state.

use relay_shared::protocol::{
    node_supports_directed_diagnose, DiagnoseResult, DiagnoseRuleMessage,
};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;

use crate::api::middleware::AuthUser;
use crate::api::node::extract_node_token;
use crate::api::AppState;
use crate::db::repo::ResourceScope;
use axum::extract::{Path, State};
use axum::http::HeaderMap;
use axum::Json;
use relay_shared::protocol::ApiResponse;

/// How long a diagnosis run waits for node results before giving up.
pub const DIAGNOSE_TIMEOUT: Duration = Duration::from_secs(8);

/// One in-flight (or recently-finished) diagnosis run.
#[derive(Debug)]
pub struct DiagnoseRun {
    pub rule_id: i64,
    pub started_at: Instant,
    /// node_id → result. A node_id that was sent the probe but hasn't replied
    /// yet is absent from this map; the caller treats that as "timeout".
    pub results: HashMap<String, DiagnoseResult>,
    /// The set of node_ids we expect results from (populated from node_status).
    pub expected_node_ids: Vec<String>,
    /// v0.4.9: the opaque per-run challenge the panel sent in
    /// DiagnoseRuleMessage. A recorded result MUST echo this verbatim or it's
    /// rejected — defeats a forged result that guesses request_id+node_id.
    pub challenge: String,
}

/// In-memory registry of active diagnosis runs, keyed by request_id.
/// Entries are removed lazily when fetched past their deadline, or by a
/// periodic sweep (callers don't need to drive the sweep — reads prune too).
#[derive(Clone, Default)]
pub struct DiagnoseRegistry {
    inner: Arc<Mutex<HashMap<String, DiagnoseRun>>>,
}

impl DiagnoseRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a new run. Returns the request_id and the per-run challenge
    /// (the caller puts the challenge into the outgoing DiagnoseRuleMessage).
    pub async fn start(&self, rule_id: i64, expected_node_ids: Vec<String>) -> (String, String) {
        let request_id = uuid_v4_str();
        let challenge = uuid_v4_str();
        let run = DiagnoseRun {
            rule_id,
            started_at: Instant::now(),
            results: HashMap::new(),
            expected_node_ids,
            challenge: challenge.clone(),
        };
        self.inner.lock().await.insert(request_id.clone(), run);
        (request_id, challenge)
    }

    /// Record a result arriving from a node. Returns false (caller rejects the
    /// POST) if:
    ///   - the request_id is unknown/expired;
    ///   - the node_id wasn't in the expected set (forged node);
    ///   - v0.4.9: the echoed challenge is empty or doesn't byte-for-byte match
    ///     the one the panel sent. This rejects a v0.4.8 node (which omits the
    ///     field) as well as any forged result that didn't actually receive the
    ///     probe. The panel only dispatches to >=0.4.9 nodes, so a legitimately
    ///     accepted result MUST carry the exact challenge.
    ///
    /// Stale/duplicate results for an already-replied node_id overwrite
    /// (last-write-wins is fine for a probe).
    pub async fn record(&self, request_id: &str, result: DiagnoseResult) -> bool {
        let mut map = self.inner.lock().await;
        let Some(run) = map.get_mut(request_id) else {
            return false;
        };
        // Accept only expected node_ids to prevent a node from injecting results
        // for a run it wasn't part of. node_id may be empty on old nodes; in
        // that case accept by group-scoped fallback (the caller passes a
        // synthesized id).
        if !run.expected_node_ids.iter().any(|n| n == &result.node_id) {
            return false;
        }
        // v0.4.9: challenge must be non-empty and match exactly. An empty echo
        // (pre-0.4.9 node, or a forged body) is rejected outright.
        if result.challenge.is_empty() || result.challenge != run.challenge {
            return false;
        }
        run.results.insert(result.node_id.clone(), result);
        true
    }

    /// v0.4.14: prune a run's expected set to ONLY the nodes the probe was
    /// actually delivered to. Called after dispatch: if the WS dropped between
    /// the online check and `send_node` (returning 0), that node must NOT keep
    /// the run waiting for a reply it will never send — otherwise we'd wait the
    /// full deadline and falsely report Timeout. Returns the pruned expected
    /// count.
    pub async fn retain_expected(
        &self,
        request_id: &str,
        keep: &std::collections::HashSet<String>,
    ) -> usize {
        let mut map = self.inner.lock().await;
        let Some(run) = map.get_mut(request_id) else {
            return 0;
        };
        run.expected_node_ids.retain(|n| keep.contains(n));
        run.expected_node_ids.len()
    }

    /// Whether all expected nodes have replied.
    pub async fn all_received(&self, request_id: &str) -> bool {
        let map = self.inner.lock().await;
        let Some(run) = map.get(request_id) else {
            return true; // gone → stop waiting
        };
        run.results.len() >= run.expected_node_ids.len()
    }

    /// Collect the results so far for a run (does NOT remove it).
    pub async fn snapshot(&self, request_id: &str) -> Option<(i64, Vec<DiagnoseResult>)> {
        let map = self.inner.lock().await;
        let run = map.get(request_id)?;
        Some((
            run.rule_id,
            run.expected_node_ids
                .iter()
                .filter_map(|nid| run.results.get(nid).cloned())
                .collect(),
        ))
    }

    /// Remove a finished/abandoned run.
    pub async fn remove(&self, request_id: &str) {
        self.inner.lock().await.remove(request_id);
    }

    /// Drop runs past their deadline. Called opportunistically; not required
    /// for correctness, just bounds memory.
    pub async fn sweep(&self) {
        let now = Instant::now();
        let mut map = self.inner.lock().await;
        map.retain(|_, run| now.duration_since(run.started_at) < DIAGNOSE_TIMEOUT * 2);
    }
}

fn uuid_v4_str() -> String {
    // uuid crate is already a dependency (used by the node for session ids);
    // use a fresh v4 here. Falls back to a timestamp+random string if the uuid
    // crate isn't available at this call site for any reason.
    uuid::Uuid::new_v4().to_string()
}

// === HTTP handlers ===

/// A single node's diagnosis view, returned to the frontend. Wraps the node's
/// DiagnoseResult with a status the frontend renders directly.
///
/// Serialized with `tag = "status"` (internally tagged) + snake_case, so the
/// frontend sees `{"status":"result", ...result-fields...}` etc. — a flat
/// discriminated object rather than the default externally-tagged form.
#[derive(Debug, serde::Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum NodeDiagnoseStatus {
    /// The node replied with a DiagnoseResult.
    ///
    /// v0.4.15: group_name + public_ip are PANEL-supplied display fields (from
    /// kvs/the group), NOT part of the node's diagnose wire message — the
    /// DiagnoseResult protocol is unchanged. Flattened so the result fields stay
    /// inline on the wire (frontend reads them flat).
    Result {
        group_name: String,
        public_ip: Option<String>,
        #[serde(flatten)]
        result: DiagnoseResult,
    },
    /// Node version < 0.4.14 — has no X-Node-ID, so it can't be targeted by
    /// directed diagnosis even with a healthy socket. Surfaced as
    /// "节点版本过旧，请升级". v0.4.15: carries group_name/public_ip for display.
    Unsupported {
        node_id: String,
        node_version: String,
        group_name: String,
        public_ip: Option<String>,
    },
    /// Node is recent enough but its WS control channel is offline. v0.4.15:
    /// carries group_name/public_ip for display.
    ControlChannelOffline {
        node_id: String,
        group_name: String,
        public_ip: Option<String>,
    },
    /// Probe was sent but no reply within the deadline. v0.4.15: carries
    /// group_name/public_ip for display.
    Timeout {
        node_id: String,
        group_name: String,
        public_ip: Option<String>,
    },
}

#[derive(Debug, serde::Serialize)]
pub struct DiagnoseResponse {
    pub request_id: String,
    pub rule_id: i64,
    pub nodes: Vec<NodeDiagnoseStatus>,
}

/// A node's status row from kvs, parsed for diagnosis scheduling. v0.4.15:
/// carries group_name + public_ip so each NodeDiagnoseStatus can show
/// "分组名 · 公网IP" without exposing the raw node_id as the label.
/// v1.2.0: `pub(crate)` so `api::restart` can reuse the same node enumeration.
/// Restart and diagnose both need "the nodes of this rule's inbound group, with
/// their versions", and duplicating that logic would let the two drift apart on
/// which nodes are considered live.
pub(crate) struct NodeStatusRow {
    pub(crate) node_id: String,
    pub(crate) node_version: Option<String>,
    pub(crate) public_ip: Option<String>,
    pub(crate) group_name: String,
}

/// POST /api/v1/rules/{id}/diagnose — start a diagnosis run for a rule.
///
/// Resolves the rule's inbound group, enumerates nodes that reported status in
/// the last 120s, sends a DiagnoseRuleMessage to the group over WS, and waits
/// up to DIAGNOSE_TIMEOUT for results (correlated by request_id). Nodes that
/// are too old, offline, or silent are surfaced explicitly so the frontend
/// doesn't confuse "no reply" with "network timeout".
///
/// v0.4.9: diagnosis is TCP-only. A pure-UDP rule is rejected (400) — UDP
/// can't be probed cheaply. A tcp_udp rule probes its TCP listener. In
/// parallel, the panel runs its OWN "panel → ingress listen" TCP probe
/// (panel → group.connect_host:listen_port) so the total wait doesn't grow;
/// that probe only confirms the ingress port is reachable from the PANEL's
/// network, not that forwarding end-to-end works.
pub async fn diagnose_rule(
    user: AuthUser,
    State(state): State<AppState>,
    Path(rule_id): Path<i64>,
) -> Json<ApiResponse<DiagnoseResponse>> {
    // SECURITY (v0.4.9 SSRF boundary; v0.4.10 scoped): the probe targets are
    // read DIRECTLY from the database (the rule's stored targets + the inbound
    // group's connect_host) — the request body carries NO host/port override,
    // so a caller cannot redirect the probe at an arbitrary address. Private-
    // network targets are NOT blocked: RelayPanel legitimately forwards to
    // internal hosts, so a private target is a valid config, not an attack.
    //
    // v0.4.10: this is now usable by regular users, but ONLY for THEIR OWN
    // rules — the resource scope folds `uid = ?` into the rule + group lookups,
    // so a non-admin diagnosing someone else's (or a non-existent) rule_id gets
    // a uniform 404. An admin keeps unscoped access. Do NOT accept caller-
    // supplied probe addresses without a full threat review — the only inputs
    // are a rule_id the caller already owns.
    let scope = user.resource_scope();
    tracing::info!(
        action = "diagnose_rule",
        rule_id = rule_id,
        actor_id = user.user_id,
        actor_admin = user.admin,
        "rule diagnosis requested"
    );

    // 1. Load the rule to find its inbound group.
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
            tracing::error!("diagnose_rule {}: find_rule_by_id failed: {}", rule_id, e);
            return Json(ApiResponse {
                code: 500,
                message: "database error".into(),
                data: None,
            });
        }
    };

    // v0.4.9: diagnosis is TCP-only. Reject a pure-UDP rule before doing any
    // work — there's no TCP listener to probe and UDP liveness can't be
    // verified cheaply. (tcp_udp rules are fine: they have a TCP listener.)
    if rule.protocol == "udp" {
        return Json(ApiResponse {
            code: 400,
            message: "UDP 暂不支持诊断".into(),
            data: None,
        });
    }

    let group_id = rule.device_group_in;

    // v0.4.15: load the inbound group ONCE — its name is attached to every
    // node row for display ("分组名 · 公网IP"), avoiding a per-node lookup. If
    // the group is gone, fall back to the id string; the rule's FK still points
    // at it but the row may have been pruned concurrently.
    //
    // Use ResourceScope::All (NOT the user's scope): a regular user diagnoses
    // a rule bound to an ADMIN-owned shared group, which an owner scope lookup
    // wouldn't find → the label would wrongly fall back to "#group_id". The
    // rule's owner was already validated above; the group name is display-only.
    let group_name = match crate::db::repo::GroupRepository::find_by_id(
        state.db.as_ref(),
        group_id,
        &ResourceScope::All,
    )
    .await
    {
        Ok(Some(g)) => g.name,
        Ok(None) => format!("#{group_id}"),
        Err(e) => {
            tracing::error!("diagnose_rule {}: group find_by_id failed: {}", rule_id, e);
            return Json(ApiResponse {
                code: 500,
                message: "database error".into(),
                data: None,
            });
        }
    };

    // 2. Enumerate the group's nodes from kvs (for node_version + node_id +
    //    public_ip; group_name is passed in for display).
    let nodes = match group_node_statuses(&state, group_id, group_name.clone()).await {
        Ok(n) => n,
        Err(e) => {
            tracing::error!(
                "diagnose_rule {}: group_node_statuses failed: {}",
                rule_id,
                e
            );
            return Json(ApiResponse {
                code: 500,
                message: "database error".into(),
                data: None,
            });
        }
    };

    // 3. Classify each node. v0.4.14 ordering — VERSION first, THEN WS liveness:
    //    - version < 0.4.14: the node has no X-Node-ID, so it can't be targeted
    //      by directed diagnosis even with a healthy socket → Unsupported
    //      ("please upgrade"), NOT a misleading "control channel offline".
    //    - recent enough but no live WS connection → ControlChannelOffline.
    //    - recent + online → a dispatch candidate.
    let online = state.node_connections.online_node_ids(group_id).await;
    let mut candidates: Vec<String> = Vec::new();
    let mut unsupported: Vec<NodeDiagnoseStatus> = Vec::new();
    let mut offline: Vec<NodeDiagnoseStatus> = Vec::new();
    for n in &nodes {
        // v0.4.15 取证 DEBUG + 纯函数分类（四场景可单测）。
        let directed_ok = node_supports_directed_diagnose(n.node_version.as_deref());
        let ws_online = online.contains(&n.node_id);
        tracing::debug!(
            rule_id,
            node_id = %n.node_id,
            raw_node_version = ?n.node_version,
            directed_ok,
            ws_online,
            "diagnose classification"
        );
        match classify_node(n, &online) {
            ClassifyOutcome::Candidate(nid) => candidates.push(nid),
            ClassifyOutcome::Status(s) => match s {
                NodeDiagnoseStatus::Unsupported { .. } => unsupported.push(s),
                _ => offline.push(s),
            },
        }
    }

    // 4. No dispatch candidate → return IMMEDIATELY (never wait the 8s deadline
    //    when there's no node to hear from).
    if candidates.is_empty() {
        let mut statuses = unsupported;
        statuses.extend(offline);
        tracing::info!(
            "diagnose_rule: actor_id={} actor_admin={} rule_id={} no dispatchable node (immediate)",
            user.user_id,
            user.admin,
            rule_id
        );
        return Json(ApiResponse::success(DiagnoseResponse {
            request_id: String::new(),
            rule_id,
            nodes: statuses,
        }));
    }

    // 5. Register the run + dispatch a DIRECTED probe to each candidate
    //    (send_node, not send_group). Track which sends actually reached a live
    //    connection: the WS can drop between the online check above and
    //    send_node here (returns 0). Such a node is reclassified
    //    ControlChannelOffline and PRUNED from the run's expected set, so it
    //    never holds the wait open for a reply that will never come (the
    //    disconnect-race that previously caused a false 8s "timeout").
    let (request_id, challenge) = state.diagnose.start(rule_id, candidates.clone()).await;
    let msg = serde_json::to_string(&DiagnoseRuleMessage::new(
        request_id.clone(),
        rule_id,
        challenge,
    ))
    .unwrap_or_default();
    let mut expected: Vec<String> = Vec::new();
    // Index nodes by node_id so the send loop can carry group_name/public_ip
    // onto any reclassified ControlChannelOffline (the candidates vec only
    // holds node_id strings).
    let node_by_id: std::collections::HashMap<&str, &NodeStatusRow> =
        nodes.iter().map(|n| (n.node_id.as_str(), n)).collect();
    for nid in &candidates {
        let row = node_by_id.get(nid.as_str()).copied();
        if state.node_connections.send_node(group_id, nid, &msg).await > 0 {
            expected.push(nid.clone());
        } else {
            // Dropped between the online check and the send — treat as offline.
            offline.push(NodeDiagnoseStatus::ControlChannelOffline {
                node_id: nid.clone(),
                group_name: row.map(|r| r.group_name.clone()).unwrap_or_default(),
                public_ip: row.and_then(|r| r.public_ip.clone()),
            });
        }
    }
    // Prune the run's expected set to ONLY the nodes we actually reached, so
    // all_received() doesn't wait on a node the probe never got to.
    let keep: std::collections::HashSet<String> = expected.iter().cloned().collect();
    state.diagnose.retain_expected(&request_id, &keep).await;

    // If every send failed (all WS dropped after the online check), return now
    // instead of waiting the full deadline for replies that can't arrive.
    if expected.is_empty() {
        state.diagnose.remove(&request_id).await;
        let mut statuses = unsupported;
        statuses.extend(offline);
        tracing::info!(
            "diagnose_rule: actor_id={} actor_admin={} rule_id={} all sends failed (immediate offline)",
            user.user_id,
            user.admin,
            rule_id
        );
        return Json(ApiResponse::success(DiagnoseResponse {
            request_id: String::new(),
            rule_id,
            nodes: statuses,
        }));
    }

    // 6. Wait for results up to the deadline, polling the registry.
    let deadline = Instant::now() + DIAGNOSE_TIMEOUT;
    while Instant::now() < deadline {
        if state.diagnose.all_received(&request_id).await {
            break;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }

    // Collect results; mark expected nodes that were dispatched but didn't
    // reply as Timeout.
    let (_, received) = state
        .diagnose
        .snapshot(&request_id)
        .await
        .unwrap_or((rule_id, Vec::new()));
    let mut statuses: Vec<NodeDiagnoseStatus> = Vec::new();
    let mut replied: std::collections::HashSet<&str> = std::collections::HashSet::new();
    for r in &received {
        replied.insert(r.node_id.as_str());
        let row = node_by_id.get(r.node_id.as_str()).copied();
        statuses.push(NodeDiagnoseStatus::Result {
            group_name: row.map(|x| x.group_name.clone()).unwrap_or_default(),
            public_ip: row.and_then(|x| x.public_ip.clone()),
            result: r.clone(),
        });
    }
    for nid in &expected {
        if !replied.contains(nid.as_str()) {
            let row = node_by_id.get(nid.as_str()).copied();
            statuses.push(NodeDiagnoseStatus::Timeout {
                node_id: nid.clone(),
                group_name: row.map(|x| x.group_name.clone()).unwrap_or_default(),
                public_ip: row.and_then(|x| x.public_ip.clone()),
            });
        }
    }
    statuses.extend(unsupported);
    statuses.extend(offline);

    // Clean up the run (it's done either way) + opportunistic sweep.
    state.diagnose.remove(&request_id).await;
    state.diagnose.sweep().await;

    // v0.4.9 audit log: record who ran a diagnose, on which rule, and how many
    // nodes actually replied. This is an SSRF-adjacent capability (directs
    // outbound probes), so every run is attributed.
    let replied_count = statuses
        .iter()
        .filter(|s| matches!(s, NodeDiagnoseStatus::Result { .. }))
        .count();
    tracing::info!(
        "diagnose_rule: actor_id={} actor_admin={} rule_id={} dispatched={} replied={}",
        user.user_id,
        user.admin,
        rule_id,
        expected.len(),
        replied_count
    );

    Json(ApiResponse::success(DiagnoseResponse {
        request_id,
        rule_id,
        nodes: statuses,
    }))
}

/// POST /api/v1/node/diagnose_result — a node reports its probe results back.
/// Authenticated by NODE_TOKEN; the panel verifies the rule belongs to the
/// token's inbound group before recording (defense against a node reporting
/// for a rule it isn't part of).
pub async fn receive_diagnose_result(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<DiagnoseResult>,
) -> Json<ApiResponse<()>> {
    let Some(token) = extract_node_token(&headers) else {
        return Json(ApiResponse {
            code: 401,
            message: "Invalid token".into(),
            data: None,
        });
    };
    // Resolve the group this token belongs to.
    let group = match state.db.find_by_token(&token).await {
        Ok(Some(g)) => g,
        Ok(None) => {
            return Json(ApiResponse {
                code: 401,
                message: "Invalid token".into(),
                data: None,
            })
        }
        Err(e) => {
            tracing::error!("diagnose_result: find_by_token failed: {}", e);
            return Json(ApiResponse {
                code: 500,
                message: "database error".into(),
                data: None,
            });
        }
    };
    // The rule must belong to THIS group's inbound set. This is the node-auth
    // path (group token verified above), not a user request — use an unscoped
    // lookup; the device_group_in check below is the real authorization.
    let rule = match state
        .db
        .find_rule_by_id(req.rule_id, &ResourceScope::All)
        .await
    {
        Ok(Some(r)) => r,
        Ok(None) => {
            return Json(ApiResponse {
                code: 404,
                message: "Rule not found".into(),
                data: None,
            })
        }
        Err(e) => {
            tracing::error!("diagnose_result: find_rule_by_id failed: {}", e);
            return Json(ApiResponse {
                code: 500,
                message: "database error".into(),
                data: None,
            });
        }
    };
    if rule.device_group_in != group.id {
        return Json(ApiResponse {
            code: 403,
            message: "rule does not belong to this node's group".into(),
            data: None,
        });
    }
    // Record (rejects unknown request_id / unexpected node_id).
    let request_id = req.request_id.clone();
    let accepted = state.diagnose.record(&request_id, req).await;
    if !accepted {
        return Json(ApiResponse {
            code: 409,
            message: "diagnose task unknown, expired, or unexpected node".into(),
            data: None,
        });
    }
    Json(ApiResponse::success(()))
}

/// Parse the kvs node_status rows for one group into structured rows. v0.4.15:
/// `group_name` is passed in by the caller (which has already loaded the group)
/// and attached to each row; `public_ip` is read from the status JSON for
/// display. No extra DB lookup here.
pub(crate) async fn group_node_statuses(
    state: &AppState,
    group_id: i64,
    group_name: String,
) -> Result<Vec<NodeStatusRow>, crate::db::error::DbError> {
    let rows = state.db.scan_prefix("node_status:").await?;
    let prefix = format!("node_status:{}:", group_id);
    let legacy_key = format!("node_status:{}", group_id);
    let mut out: Vec<NodeStatusRow> = Vec::new();
    for (key, value) in &rows {
        // Match either the per-node key (node_status:{gid}:{nid}) or the legacy
        // per-group key (node_status:{gid}). Skip other groups entirely.
        let node_id = if key == &legacy_key {
            // Legacy single-node-per-group row. Use a synthetic id so it doesn't
            // collide with real per-node ids; the frontend shows it once.
            "__legacy__".to_string()
        } else if let Some(rest) = key.strip_prefix(&prefix) {
            rest.to_string()
        } else {
            continue;
        };
        let v: serde_json::Value = match serde_json::from_str(value) {
            Ok(v) => v,
            Err(_) => continue,
        };
        let node_version = v
            .get("node_version")
            .and_then(|x| x.as_str())
            .map(|s| s.to_string());
        // Prefer the JSON node_id field if present (canonical), else the key.
        let nid = v
            .get("node_id")
            .and_then(|x| x.as_str())
            .map(|s| s.to_string())
            .unwrap_or(node_id);
        // v0.4.15: public_ip for display ("分组名 · 公网IP"). Read defensively —
        // older/edge nodes may omit it.
        let public_ip = v
            .get("public_ip")
            .and_then(|x| x.as_str())
            .map(|s| s.to_string());
        out.push(NodeStatusRow {
            node_id: nid,
            node_version,
            public_ip,
            group_name: group_name.clone(),
        });
    }
    Ok(out)
}

/// v0.4.15: pure classification decision for one node, extracted so the four
/// scenarios (recent+online, old version, recent+WS-down, multi-node) can be
/// unit-tested WITHOUT AppState/HTTP. Version-first: <0.4.14 → Unsupported
/// ("please upgrade"); recent but no live WS → ControlChannelOffline; recent +
/// online → Candidate (dispatchable).
#[derive(Debug)]
#[allow(clippy::large_enum_variant)] // test helper; Status holds NodeDiagnoseStatus
enum ClassifyOutcome {
    Candidate(String),
    Status(NodeDiagnoseStatus),
}

fn classify_node(n: &NodeStatusRow, online: &std::collections::HashSet<String>) -> ClassifyOutcome {
    let nid = n.node_id.clone();
    if !node_supports_directed_diagnose(n.node_version.as_deref()) {
        return ClassifyOutcome::Status(NodeDiagnoseStatus::Unsupported {
            node_id: nid,
            node_version: n.node_version.clone().unwrap_or_default(),
            group_name: n.group_name.clone(),
            public_ip: n.public_ip.clone(),
        });
    }
    if !online.contains(&nid) {
        return ClassifyOutcome::Status(NodeDiagnoseStatus::ControlChannelOffline {
            node_id: nid,
            group_name: n.group_name.clone(),
            public_ip: n.public_ip.clone(),
        });
    }
    ClassifyOutcome::Candidate(nid)
}

#[cfg(test)]
mod tests {
    use super::*;
    use relay_shared::protocol::{DiagnoseResult, TargetProbeOutcome};

    fn mk_result(req: &str, nid: &str) -> DiagnoseResult {
        DiagnoseResult {
            msg_type: "diagnose_result".into(),
            request_id: req.into(),
            rule_id: 1,
            node_id: nid.into(),
            // Default empty challenge; tests that exercise the challenge check
            // override this with `with_challenge`.
            challenge: String::new(),
            listener_running: true,
            listen_port: 10000,
            protocol: "tcp".into(),
            transport: "raw".into(),
            results: vec![],
        }
    }

    /// Stamp a result with the run's challenge so it passes the record() check.
    fn with_challenge(mut r: DiagnoseResult, challenge: &str) -> DiagnoseResult {
        r.challenge = challenge.into();
        r
    }

    #[tokio::test]
    async fn record_accepts_expected_rejects_unknown_node() {
        let reg = DiagnoseRegistry::new();
        let (rid, ch) = reg.start(1, vec!["n1".into(), "n2".into()]).await;
        assert!(
            reg.record(&rid, with_challenge(mk_result(&rid, "n1"), &ch))
                .await
        );
        assert!(
            reg.record(&rid, with_challenge(mk_result(&rid, "n2"), &ch))
                .await
        );
        assert!(reg.all_received(&rid).await, "both expected nodes replied");
        // forged node id
        assert!(
            !reg.record(&rid, with_challenge(mk_result(&rid, "evil"), &ch))
                .await,
            "result from a node not in the expected set must be rejected"
        );
    }

    /// v0.4.14 disconnect-race: a node that was online at the check but whose
    /// send failed is pruned from the expected set, so all_received() no longer
    /// waits on it (previously caused a false 8s timeout).
    #[tokio::test]
    async fn retain_expected_prunes_unreached_nodes() {
        let reg = DiagnoseRegistry::new();
        let (rid, ch) = reg
            .start(1, vec!["n1".into(), "n2".into(), "n3".into()])
            .await;
        // Only n1 + n2 were actually reached by send_node; n3 dropped.
        let keep: std::collections::HashSet<String> =
            ["n1".to_string(), "n2".to_string()].into_iter().collect();
        let remaining = reg.retain_expected(&rid, &keep).await;
        assert_eq!(remaining, 2, "n3 pruned from expected");

        // n1 + n2 reply → all_received true WITHOUT n3 (no false wait/timeout).
        reg.record(&rid, with_challenge(mk_result(&rid, "n1"), &ch))
            .await;
        reg.record(&rid, with_challenge(mk_result(&rid, "n2"), &ch))
            .await;
        assert!(
            reg.all_received(&rid).await,
            "run completes once reached nodes reply; pruned node doesn't block"
        );
    }

    #[tokio::test]
    async fn record_rejects_unknown_request_id() {
        let reg = DiagnoseRegistry::new();
        assert!(!reg.record("does-not-exist", mk_result("x", "n1")).await);
    }

    #[tokio::test]
    async fn record_rejects_empty_or_mismatched_challenge() {
        // v0.4.9: a result whose challenge is empty (pre-0.4.9 node / forged
        // body) or doesn't match the run's challenge is rejected, even when
        // request_id + node_id are otherwise valid.
        let reg = DiagnoseRegistry::new();
        let (rid, ch) = reg.start(1, vec!["n1".into()]).await;
        // Empty challenge → reject (this is what a v0.4.8 node would POST).
        assert!(
            !reg.record(&rid, mk_result(&rid, "n1")).await,
            "empty challenge must be rejected"
        );
        // Wrong challenge → reject.
        assert!(
            !reg.record(&rid, with_challenge(mk_result(&rid, "n1"), "wrong"))
                .await,
            "mismatched challenge must be rejected"
        );
        // Exact match → accept.
        assert!(
            reg.record(&rid, with_challenge(mk_result(&rid, "n1"), &ch))
                .await
        );
    }

    #[tokio::test]
    async fn snapshot_preserves_expected_order_and_skips_missing() {
        let reg = DiagnoseRegistry::new();
        let (rid, ch) = reg
            .start(1, vec!["n1".into(), "n2".into(), "n3".into()])
            .await;
        reg.record(&rid, with_challenge(mk_result(&rid, "n2"), &ch))
            .await;
        let (rule_id, got) = reg.snapshot(&rid).await.unwrap();
        assert_eq!(rule_id, 1);
        assert_eq!(got.len(), 1, "only n2 has replied");
        assert_eq!(got[0].node_id, "n2");
    }

    #[tokio::test]
    async fn snapshot_missing_outcome_keeps_enum_honest() {
        // Sanity: a result with no targets serializes cleanly (no outcome blob
        // leaks). v0.4.9 dropped RouteOnly; this just keeps the enum honest.
        let r = mk_result("r", "n");
        assert!(r.results.is_empty());
        let _ = TargetProbeOutcome::Timeout; // variant exists
    }

    /// v0.4.15: every NodeDiagnoseStatus variant carries group_name + public_ip
    /// (panel-supplied display fields) on the wire, so the frontend can always
    /// render "分组名 · 公网IP" — node_id is NEVER the visible label.
    #[test]
    fn all_variants_carry_group_name_and_public_ip() {
        // Result variant (result fields stay flattened; group_name/public_ip inline).
        let res = NodeDiagnoseStatus::Result {
            group_name: "g1".into(),
            public_ip: Some("1.2.3.4".into()),
            result: mk_result("r", "n"),
        };
        let j = serde_json::to_string(&res).unwrap();
        assert!(j.contains("\"status\":\"result\""), "{j}");
        assert!(
            j.contains("\"group_name\":\"g1\""),
            "group_name missing: {j}"
        );
        assert!(
            j.contains("\"public_ip\":\"1.2.3.4\""),
            "public_ip missing: {j}"
        );
        // Result's own fields must still be present (flatten intact).
        assert!(
            j.contains("\"node_id\":\"n\""),
            "flattened result field lost: {j}"
        );

        let cases = [
            (
                NodeDiagnoseStatus::Unsupported {
                    node_id: "n".into(),
                    node_version: "0.4.13".into(),
                    group_name: "g1".into(),
                    public_ip: Some("1.2.3.4".into()),
                },
                "unsupported",
            ),
            (
                NodeDiagnoseStatus::ControlChannelOffline {
                    node_id: "n".into(),
                    group_name: "g1".into(),
                    public_ip: None,
                },
                "control_channel_offline",
            ),
            (
                NodeDiagnoseStatus::Timeout {
                    node_id: "n".into(),
                    group_name: "g1".into(),
                    public_ip: Some("9.9.9.9".into()),
                },
                "timeout",
            ),
        ];
        for (s, tag) in &cases {
            let j = serde_json::to_string(s).unwrap();
            assert!(j.contains(&format!("\"status\":\"{tag}\"")), "{tag}: {j}");
            assert!(j.contains("\"group_name\":\"g1\""), "{tag} group_name: {j}");
            // node_id stays present on the wire (for the tooltip), just not as
            // the visible label — that's a frontend concern.
            assert!(j.contains("\"node_id\":\"n\""), "{tag} node_id: {j}");
        }
        // public_ip serializes as null when None (control_channel_offline above).
        let off = serde_json::to_string(&cases[1].0).unwrap();
        assert!(
            off.contains("\"public_ip\":null"),
            "None public_ip must be explicit null, not omitted: {off}"
        );
    }

    /// v0.4.15: the four classification scenarios via the pure classify_node.
    fn row(node_id: &str, ver: Option<&str>, ip: Option<&str>) -> NodeStatusRow {
        NodeStatusRow {
            node_id: node_id.into(),
            node_version: ver.map(String::from),
            public_ip: ip.map(String::from),
            group_name: "g1".into(),
        }
    }

    /// Scenario 1: v0.4.14+ and WS online → Candidate (dispatchable).
    #[test]
    fn classify_recent_online_is_candidate() {
        let online: std::collections::HashSet<String> = ["n1".into()].into_iter().collect();
        assert!(matches!(
            classify_node(&row("n1", Some("0.4.14"), Some("1.1.1.1")), &online),
            ClassifyOutcome::Candidate(n) if n == "n1"
        ));
    }

    /// Scenario 2: old version (<0.4.14) → Unsupported, even if WS is online.
    #[test]
    fn classify_old_version_is_unsupported() {
        let online: std::collections::HashSet<String> = ["n2".into()].into_iter().collect();
        match classify_node(&row("n2", Some("0.4.13"), Some("2.2.2.2")), &online) {
            ClassifyOutcome::Status(NodeDiagnoseStatus::Unsupported { node_version, .. }) => {
                assert_eq!(node_version, "0.4.13")
            }
            other => panic!("expected Unsupported for 0.4.13, got {other:?}"),
        }
    }

    /// Scenario 3: recent version but WS offline → ControlChannelOffline.
    #[test]
    fn classify_recent_ws_offline() {
        let online: std::collections::HashSet<String> = [].into_iter().collect();
        match classify_node(&row("n3", Some("0.4.14"), Some("3.3.3.3")), &online) {
            ClassifyOutcome::Status(NodeDiagnoseStatus::ControlChannelOffline { .. }) => {}
            other => panic!("expected ControlChannelOffline, got {other:?}"),
        }
    }

    /// Scenario 4: same group, multiple nodes classified independently.
    #[test]
    fn classify_multi_node_group() {
        let online: std::collections::HashSet<String> = ["a".into()].into_iter().collect(); // only node 'a' is online
        let nodes = vec![
            row("a", Some("0.4.14"), Some("1.1.1.1")), // online → candidate
            row("b", Some("0.4.13"), None),            // old → unsupported
            row("c", Some("0.4.14"), Some("3.3.3.3")), // recent but offline
        ];
        let mut candidates = 0;
        let mut unsupported = 0;
        let mut offline = 0;
        for n in &nodes {
            match classify_node(n, &online) {
                ClassifyOutcome::Candidate(_) => candidates += 1,
                ClassifyOutcome::Status(NodeDiagnoseStatus::Unsupported { .. }) => unsupported += 1,
                ClassifyOutcome::Status(NodeDiagnoseStatus::ControlChannelOffline { .. }) => {
                    offline += 1
                }
                _ => {}
            }
        }
        assert_eq!(candidates, 1, "node a is the only candidate");
        assert_eq!(unsupported, 1, "node b is old");
        assert_eq!(offline, 1, "node c is WS-offline");
    }

    /// v0.4.15 #1: group_name is filled on every status (for the "分组名·公网IP"
    /// label). A regular user binding an admin-shared group must see the real
    /// group name, not #group_id.
    #[test]
    fn classify_carries_group_name_and_ip() {
        let online: std::collections::HashSet<String> = [].into_iter().collect();
        match classify_node(&row("n4", Some("0.4.14"), Some("9.9.9.9")), &online) {
            ClassifyOutcome::Status(NodeDiagnoseStatus::ControlChannelOffline {
                group_name,
                public_ip,
                ..
            }) => {
                assert_eq!(group_name, "g1");
                assert_eq!(public_ip.as_deref(), Some("9.9.9.9"));
            }
            other => panic!("got {other:?}"),
        }
    }
}
