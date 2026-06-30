//! v0.4.12 PR1: Shared infrastructure endpoints.
//!
//! Lets regular users discover and attach rules to inbound groups owned by an
//! admin ("shared infrastructure"). The actual rule binding is done via the
//! existing POST /rules endpoint; this module provides the discovery APIs:
//!   - GET /groups/shared — admin-owned `group_type='in'` groups (selectable
//!     line list), independent of whether the user already has rules.
//!   - GET /nodes/shared  — per-NODE availability + load metrics, built by
//!     scanning the `node_status:*` kvs keys (there is NO node_status table).
//!
//! Both wrap their payload in `ApiResponse` so a DB failure returns a non-zero
//! `code` (the frontend shows "load failed") instead of an empty `data` array
//! that would be indistinguishable from a legitimate "no lines available".

use crate::api::middleware::AuthUser;
use crate::api::stats::{parse_status_key, status_is_online, status_last_seen};
use crate::api::AppState;
use crate::dto::{SharedGroupSummary, SharedNodeSummary};
use axum::{extract::State, Json};
use relay_shared::protocol::ApiResponse;
use std::collections::{HashMap, HashSet};

/// Build a typed 500 error envelope (`ApiResponse::error` is fixed to
/// `ApiResponse<()>`, so we construct the typed variant by hand).
fn db_error<T>() -> Json<ApiResponse<T>>
where
    T: serde::Serialize,
{
    Json(ApiResponse {
        code: 500,
        message: "database error".into(),
        data: None,
    })
}

/// v1.0.4: filtered by the user's permission group. If the user's group
/// has allow_all_groups, all admin-owned inbound groups are returned
/// (existing behavior). Otherwise, only the groups explicitly assigned
/// to the user's permission group are returned.
pub async fn list_shared_groups(
    user: AuthUser,
    State(state): State<AppState>,
) -> Json<ApiResponse<Vec<SharedGroupSummary>>> {
    // Admins get empty list (they manage groups directly).
    if user.admin {
        return Json(ApiResponse::success(Vec::new()));
    }

    let all_groups = match state.db.list_shared_groups(user.user_id, false).await {
        Ok(groups) => groups,
        Err(e) => {
            tracing::error!("list_shared_groups: db error: {}", e);
            return db_error();
        }
    };

    // v1.0.7: filter to the groups the user is authorized for. The authorized
    // set already expands all_device_groups (→ every inbound group); an
    // unassigned non-admin gets an empty set and sees nothing.
    let authorized = state
        .db
        .authorized_device_group_ids(user.user_id)
        .await
        .unwrap_or_default();
    let filtered: Vec<_> = all_groups
        .into_iter()
        .filter(|g| authorized.contains(&g.id))
        .collect();
    Json(ApiResponse::success(filtered))
}

/// GET /nodes/shared — per-NODE availability + load metrics for the shared
/// inbound groups a regular user can use.
///
/// One row PER NODE. A shared group with no reporting node still yields one
/// placeholder row (node_id empty, online=false, metrics None) so the line
/// never disappears. The aggregation scans the `node_status:*` kvs keys — there
/// is no node_status table — using the backend's single online-window source of
/// truth. Exposes load metrics (cpu/mem/disk/traffic) but NOT secrets: no
/// token, config, listener_errors, or internal IP. `node_id` is included only
/// as the row identity.
///
/// Admin users get an empty list (they use GET /nodes for the full detail view).
/// A DB error returns code 500 (not an empty success).
pub async fn list_shared_node_summary(
    user: AuthUser,
    State(state): State<AppState>,
) -> Json<ApiResponse<Vec<SharedNodeSummary>>> {
    // The shared groups define which groups (and which safe metadata) to
    // return. For an admin this is empty → empty summary list.
    let groups: Vec<SharedGroupSummary> =
        match state.db.list_shared_groups(user.user_id, user.admin).await {
            Ok(g) => g,
            Err(e) => {
                tracing::error!("list_shared_node_summary: list_shared_groups failed: {}", e);
                return db_error();
            }
        };

    // v1.0.7: filter by per-user device-group authorization (same logic as
    // list_shared_groups), AND drop admin-hidden groups — this is the ONLY path
    // that honors `hidden` (the node-status page). The rule dropdown / shop
    // (list_shared_groups) intentionally keep listing hidden groups so existing
    // and new rules work normally. Admins are handled above (empty list).
    let groups = if user.admin {
        groups
    } else {
        let authorized = state
            .db
            .authorized_device_group_ids(user.user_id)
            .await
            .unwrap_or_default();
        groups
            .into_iter()
            .filter(|g| !g.hidden && authorized.contains(&g.id))
            .collect()
    };

    if groups.is_empty() {
        return Json(ApiResponse::success(vec![]));
    }

    // v0.4.12 PR1: prune stale status rows BEFORE scanning, mirroring the admin
    // node-status read path. Without this, a group whose every node stopped
    // reporting could linger as a ghost "0/1" until some other code path swept
    // it. Best-effort: a sweep error is logged but does not fail the request
    // (the aggregation's online check still gives a correct online count).
    if let Err(e) = crate::service::traffic::sweep_stale_status(state.db.as_ref()).await {
        tracing::warn!("list_shared_node_summary: sweep_stale_status failed: {}", e);
    }

    // Scan all node_status rows once and aggregate by group. A DB error here is
    // surfaced (not swallowed) so the frontend shows a load failure.
    let rows: Vec<(String, String)> = match state.db.scan_prefix("node_status:").await {
        Ok(r) => r,
        Err(e) => {
            tracing::error!("list_shared_node_summary: scan_prefix failed: {}", e);
            return db_error();
        }
    };

    let mut summaries = aggregate_shared_node_summaries(groups, &rows, chrono::Utc::now());

    // v0.4.15: enrich each node row with GeoIP country from the KVS cache
    // (geoip:{ip}). This is a READ of cached results — no third-party call
    // here (lookups are triggered asynchronously at report_status time). If
    // GEOIP is disabled or the cache is empty, country stays None ("未知").
    for sm in &mut summaries {
        if let Some(ref ip) = sm.public_ipv4 {
            if let Some(entry) = crate::api::geoip::read_cache(state.db.as_ref(), ip).await {
                sm.ipv4_country_code = entry.country_code;
                sm.ipv4_country_name = entry.country_name;
            }
        }
        if let Some(ref ip) = sm.public_ipv6 {
            if let Some(entry) = crate::api::geoip::read_cache(state.db.as_ref(), ip).await {
                sm.ipv6_country_code = entry.country_code;
                sm.ipv6_country_name = entry.country_name;
            }
        }
    }

    Json(ApiResponse::success(summaries))
}

/// Pure transform of node_status kvs rows into per-NODE [`SharedNodeSummary`]
/// rows for the given shared groups. Extracted so it can be unit-tested without
/// a DB.
///
/// - One row per node that has a parseable status record with a valid last_seen
///   (corrupt / last_seen-less rows are skipped — no ghost rows).
/// - A shared group with NO node rows still yields ONE placeholder row
///   (node_id empty, online=false, metrics None) so the line never disappears.
/// - Only rows whose group_id is in `groups` are included. Online-ness uses the
///   backend's single source of truth ([`status_is_online`]).
fn aggregate_shared_node_summaries(
    groups: Vec<SharedGroupSummary>,
    rows: &[(String, String)],
    now: chrono::DateTime<chrono::Utc>,
) -> Vec<SharedNodeSummary> {
    let shared_ids: HashSet<i64> = groups.iter().map(|g| g.id).collect();

    // group_id -> Vec of (node_id, parsed status JSON, online, last_seen)
    let mut nodes_by_group: HashMap<i64, Vec<(String, serde_json::Value, bool, String)>> =
        HashMap::new();
    for (key, value) in rows {
        let (group_id, node_id) = match parse_status_key(key) {
            Some(parsed) => parsed,
            None => continue,
        };
        if !shared_ids.contains(&group_id) {
            continue;
        }
        // Only a parseable record with a valid last_seen counts (stale rows were
        // swept before the scan; corrupt JSON must never become a ghost node).
        let last_seen = match status_last_seen(value) {
            Some(ls) => ls,
            None => continue,
        };
        let json: serde_json::Value = match serde_json::from_str(value) {
            Ok(v) => v,
            Err(_) => continue,
        };
        let node_key = node_id.unwrap_or("legacy").to_string();
        let online = status_is_online(value, now);
        nodes_by_group.entry(group_id).or_default().push((
            node_key,
            json,
            online,
            last_seen.to_rfc3339(),
        ));
    }

    // Build per-node rows in group-list order. Group metadata repeats per node.
    let mut out: Vec<SharedNodeSummary> = Vec::new();
    for g in groups {
        let base = |node_id: String,
                    online: bool,
                    json: Option<&serde_json::Value>,
                    last_seen: Option<String>| {
            let f = |k: &str| json.and_then(|j| j.get(k)).and_then(|v| v.as_f64());
            let i = |k: &str| json.and_then(|j| j.get(k)).and_then(|v| v.as_i64());
            let s = |k: &str| {
                json.and_then(|j| j.get(k))
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string())
            };
            SharedNodeSummary {
                group_id: g.id,
                group_name: g.name.clone(),
                connect_host: g.connect_host.clone(),
                capabilities: g.capabilities.clone(),
                region: g.region.clone(),
                line_type: g.line_type.clone(),
                node_id,
                online,
                public_ip: s("public_ip"),
                // v0.4.15: dual-stack. public_ipv4 falls back to the legacy
                // public_ip for older nodes that haven't upgraded yet.
                public_ipv4: s("public_ipv4").or_else(|| s("public_ip")),
                public_ipv6: s("public_ipv6"),
                // GeoIP country fields are filled by the handler AFTER
                // aggregation (they come from the geoip:{ip} KVS cache, not
                // from the node's status row).
                ipv4_country_code: None,
                ipv4_country_name: None,
                ipv6_country_code: None,
                ipv6_country_name: None,
                node_version: s("node_version"),
                config_protocol_version: i("config_protocol_version"),
                // connections defaults to 0 (not None) — a placeholder/old node
                // simply has no active connections.
                connections: i("connections").unwrap_or(0),
                uptime: i("uptime"),
                process_uptime: i("process_uptime"),
                network_interface: s("network_interface"),
                cpu: f("cpu"),
                mem: f("mem"),
                disk_mount: s("disk_mount"),
                disk_usage_percent: f("disk_usage_percent"),
                disk_used: i("disk_used"),
                disk_total: i("disk_total"),
                upload_bps: i("upload_bps"),
                download_bps: i("download_bps"),
                boot_upload_bytes: i("boot_upload_bytes"),
                boot_download_bytes: i("boot_download_bytes"),
                last_seen,
            }
        };
        match nodes_by_group.remove(&g.id) {
            Some(mut nodes) if !nodes.is_empty() => {
                // Stable order: by node_id so the table doesn't reshuffle.
                nodes.sort_by(|a, b| a.0.cmp(&b.0));
                for (node_id, json, online, last_seen) in nodes {
                    out.push(base(node_id, online, Some(&json), Some(last_seen)));
                }
            }
            // No node reported for this shared group → one placeholder row.
            _ => out.push(base(String::new(), false, None, None)),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn group(id: i64, name: &str) -> SharedGroupSummary {
        SharedGroupSummary {
            id,
            name: name.into(),
            group_type: "in".into(),
            connect_host: "1.2.3.4".into(),
            capabilities: "[]".into(),
            region: None,
            line_type: None,
            hidden: false,
        }
    }

    /// A status row JSON with the given last_seen offset (seconds ago).
    fn row(key: &str, secs_ago: i64, now: chrono::DateTime<chrono::Utc>) -> (String, String) {
        let ts = (now - chrono::Duration::seconds(secs_ago)).to_rfc3339();
        (
            key.to_string(),
            serde_json::json!({ "last_seen": ts }).to_string(),
        )
    }

    /// A status row carrying load metrics + last_seen.
    fn row_full(key: &str, secs_ago: i64, now: chrono::DateTime<chrono::Utc>) -> (String, String) {
        let ts = (now - chrono::Duration::seconds(secs_ago)).to_rfc3339();
        (
            key.to_string(),
            serde_json::json!({
                "last_seen": ts,
                "node_id": "n1",
                "public_ip": "1.2.3.4",
                "node_version": "0.4.13",
                "config_protocol_version": 4,
                "connections": 10,
                "uptime": 1_555_200,
                "process_uptime": 864_000,
                "network_interface": "eth0",
                "cpu": 27.5,
                "mem": 60.0,
                "disk_mount": "/",
                "disk_usage_percent": 41.2,
                "disk_used": 1024,
                "disk_total": 4096,
                "upload_bps": 100,
                "download_bps": 200,
                "boot_upload_bytes": 5000,
                "boot_download_bytes": 9000,
            })
            .to_string(),
        )
    }

    /// v0.4.13 PR2: one row PER NODE; online flag reflects each node's last_seen.
    #[test]
    fn one_row_per_node_with_online_flag() {
        let now = chrono::Utc::now();
        let groups = vec![group(5, "g5")];
        let rows = vec![
            row("node_status:5:a", 5, now),   // online
            row("node_status:5:b", 10, now),  // online
            row("node_status:5:c", 600, now), // offline (stale but not swept here)
        ];
        let out = aggregate_shared_node_summaries(groups, &rows, now);
        assert_eq!(out.len(), 3, "one row per node");
        assert!(out.iter().all(|s| s.group_id == 5));
        assert_eq!(out.iter().filter(|s| s.online).count(), 2);
        assert_eq!(out.iter().filter(|s| !s.online).count(), 1);
        // Stable order by node_id.
        assert_eq!(out[0].node_id, "a");
        assert_eq!(out[1].node_id, "b");
        assert_eq!(out[2].node_id, "c");
    }

    /// v0.4.13 PR2: a shared group with NO status rows yields ONE placeholder
    /// row (node_id empty, offline, metrics None) so the line never disappears.
    #[test]
    fn group_with_no_status_returns_placeholder_row() {
        let now = chrono::Utc::now();
        let groups = vec![group(7, "g7")];
        let out = aggregate_shared_node_summaries(groups, &[], now);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].group_id, 7);
        assert_eq!(out[0].node_id, "");
        assert!(!out[0].online);
        assert!(out[0].cpu.is_none());
        assert!(out[0].last_seen.is_none());
    }

    /// Rows for groups NOT in the shared set are ignored; each group present
    /// gets at least its placeholder row.
    #[test]
    fn ignores_non_shared_groups() {
        let now = chrono::Utc::now();
        let groups = vec![group(1, "g1"), group(2, "g2")];
        let rows = vec![
            row("node_status:1:x", 5, now),
            row("node_status:9:z", 5, now), // group 9 not shared → ignored
        ];
        let out = aggregate_shared_node_summaries(groups, &rows, now);
        // g1 → its one node row; g2 → one placeholder row. No g9 anywhere.
        assert_eq!(out.len(), 2);
        assert!(out.iter().all(|s| s.group_id == 1 || s.group_id == 2));
        let g1 = out.iter().find(|s| s.group_id == 1).unwrap();
        assert_eq!(g1.node_id, "x");
        let g2 = out.iter().find(|s| s.group_id == 2).unwrap();
        assert_eq!(g2.node_id, "", "g2 has no node → placeholder");
    }

    /// A legacy per-group key (no node_id) becomes one node row keyed "legacy".
    #[test]
    fn legacy_per_group_key_is_one_node_row() {
        let now = chrono::Utc::now();
        let groups = vec![group(3, "g3")];
        let rows = vec![row("node_status:3", 5, now)];
        let out = aggregate_shared_node_summaries(groups, &rows, now);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].node_id, "legacy");
        assert!(out[0].online);
    }

    /// v0.4.13 PR2: load metrics (cpu/mem/disk/traffic) are extracted onto the
    /// node row; secrets are never present in the DTO (compile-time guarantee).
    #[test]
    fn extracts_load_metrics() {
        let now = chrono::Utc::now();
        let groups = vec![group(8, "g8")];
        let rows = vec![row_full("node_status:8:n1", 3, now)];
        let out = aggregate_shared_node_summaries(groups, &rows, now);
        assert_eq!(out.len(), 1);
        let s = &out[0];
        assert_eq!(s.cpu, Some(27.5));
        assert_eq!(s.mem, Some(60.0));
        assert_eq!(s.disk_usage_percent, Some(41.2));
        assert_eq!(s.disk_used, Some(1024));
        assert_eq!(s.disk_total, Some(4096));
        assert_eq!(s.upload_bps, Some(100));
        assert_eq!(s.download_bps, Some(200));
        assert_eq!(s.boot_upload_bytes, Some(5000));
        assert_eq!(s.boot_download_bytes, Some(9000));
        // v0.4.14: extended fields exposed to regular users.
        assert_eq!(s.public_ip.as_deref(), Some("1.2.3.4"));
        assert_eq!(s.node_version.as_deref(), Some("0.4.13"));
        assert_eq!(s.config_protocol_version, Some(4));
        assert_eq!(s.connections, 10);
        assert_eq!(s.uptime, Some(1_555_200));
        assert_eq!(s.process_uptime, Some(864_000));
        assert_eq!(s.network_interface.as_deref(), Some("eth0"));
        assert_eq!(s.disk_mount.as_deref(), Some("/"));
    }

    /// v0.4.14: a placeholder row (no node) has connections 0 and all extended
    /// fields None — never a misleading value.
    #[test]
    fn placeholder_row_has_zero_connections_and_none_metrics() {
        let now = chrono::Utc::now();
        let out = aggregate_shared_node_summaries(vec![group(9, "g9")], &[], now);
        assert_eq!(out.len(), 1);
        let s = &out[0];
        assert_eq!(s.connections, 0);
        assert!(s.public_ip.is_none());
        assert!(s.node_version.is_none());
        assert!(s.uptime.is_none());
        assert!(s.network_interface.is_none());
        assert!(s.disk_mount.is_none());
    }

    /// A corrupt-JSON or last_seen-less status row must NOT become a ghost node
    /// row. Only parseable rows with a valid last_seen produce a node row; if a
    /// group ends up with none, it still gets its placeholder.
    #[test]
    fn corrupt_or_missing_last_seen_rows_are_skipped() {
        let now = chrono::Utc::now();
        let groups = vec![group(4, "g4")];
        let rows = vec![
            ("node_status:4:bad".to_string(), "not-json{".to_string()),
            (
                "node_status:4:no-ls".to_string(),
                serde_json::json!({ "cpu": 1.0 }).to_string(),
            ),
            row("node_status:4:good", 5, now), // the only valid row
        ];
        let out = aggregate_shared_node_summaries(groups, &rows, now);
        assert_eq!(out.len(), 1, "only the parseable row produces a node row");
        assert_eq!(out[0].node_id, "good");
    }
}
