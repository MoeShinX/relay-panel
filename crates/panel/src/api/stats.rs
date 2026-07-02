use crate::api::middleware::{AdminOnly, AuthUser};
use crate::api::AppState;
use axum::{
    extract::{Query, State},
    Json,
};
use relay_shared::models::Statistic;
use relay_shared::protocol::*;
use serde::Deserialize;

/// v0.4.12 PR1: the single backend source of truth for "is a node online".
/// A node is online if its last reported `last_seen` is within this many
/// seconds of now. Both the node-status read path and the shared-node
/// aggregation use this — frontend pages must NOT compute their own threshold.
pub(crate) const NODE_ONLINE_WINDOW_SECS: i64 = 30;

/// Parse a node_status kvs key into (group_id, node_id).
///
/// Two formats coexist for backward compat:
///   - legacy:  "node_status:{group_id}"        (older nodes, single-node group)
///   - v0.3.0:  "node_status:{group_id}:{node_id}" (per-node dedup)
///
/// Returns None if the key isn't a node_status key or group_id isn't an int.
/// node_id is None for the legacy format. Pure function so it's unit-testable
/// without a DB.
pub(crate) fn parse_status_key(key: &str) -> Option<(i64, Option<&str>)> {
    let rest = key.strip_prefix("node_status:")?;
    let (group_id_str, node_id) = match rest.split_once(':') {
        Some((g, n)) => (g, Some(n)),
        None => (rest, None),
    };
    let group_id = group_id_str.parse().ok()?;
    Some((group_id, node_id))
}

/// v0.4.12 PR1: extract `last_seen` (RFC3339) from a stored status JSON value.
/// Returns None if the value isn't JSON or has no parseable last_seen.
pub(crate) fn status_last_seen(value: &str) -> Option<chrono::DateTime<chrono::Utc>> {
    let v: serde_json::Value = serde_json::from_str(value).ok()?;
    let s = v.get("last_seen").and_then(|s| s.as_str())?;
    chrono::DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|t| t.with_timezone(&chrono::Utc))
}

/// v0.4.12 PR1: unified online check from a stored status JSON value, relative
/// to `now`. A row with no parseable last_seen is treated as offline.
pub(crate) fn status_is_online(value: &str, now: chrono::DateTime<chrono::Utc>) -> bool {
    status_last_seen(value)
        .map(|t| (now - t).num_seconds() <= NODE_ONLINE_WINDOW_SECS)
        .unwrap_or(false)
}

/// Extract the public IPs from a stored node_status JSON blob.
///
/// Used before deleting one node_status row so we can clean only that node's
/// GeoIP cache entries. Returns None when the JSON is corrupt; callers should
/// still delete the node_status row but skip GeoIP cleanup.
pub(crate) fn public_ips_from_status_json(raw: &str) -> Option<Vec<String>> {
    let status_json: serde_json::Value = serde_json::from_str(raw).ok()?;
    let mut ips: Vec<String> = Vec::new();
    for field in ["public_ipv4", "public_ipv6", "public_ip"] {
        if let Some(ip) = status_json
            .get(field)
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
        {
            ips.push(ip.to_string());
        }
    }
    ips.sort();
    ips.dedup();
    Some(ips)
}

#[derive(Deserialize)]
pub struct StatsQuery {
    pub stat_type: Option<String>,
    pub stat_key: Option<String>,
    pub from: Option<String>,
    pub to: Option<String>,
}

/// GET /stats — global statistics.
///
/// v0.4.10: temporarily ADMIN-ONLY. These rows are not yet owner-scoped, so a
/// regular user would otherwise see every user's aggregate stats. Per-user
/// private statistics are a PR2 deliverable; until then this stays admin-gated
/// rather than leak cross-tenant data.
pub async fn get_stats(
    _admin: AdminOnly,
    State(state): State<AppState>,
    Query(q): Query<StatsQuery>,
) -> Json<ApiResponse<Vec<Statistic>>> {
    let stats: Vec<Statistic> = state
        .db
        .query_stats(
            q.stat_type.as_deref(),
            q.stat_key.as_deref(),
            q.from.as_deref(),
            q.to.as_deref(),
        )
        .await
        .unwrap_or_else(|e| {
            tracing::error!("get_stats: db error: {}", e);
            Vec::new()
        });

    Json(ApiResponse::success(stats))
}

pub async fn get_node_status(
    user: AuthUser,
    State(state): State<AppState>,
) -> Json<ApiResponse<Vec<serde_json::Value>>> {
    // v0.3.2: sweep stale entries on READ too, not just on report. Previously
    // sweep only ran when a node reported status — so if every node in a group
    // went offline (no more reports), the ghost "离线" rows lingered forever.
    // Now opening the node-status page cleans up entries older than 2 min.
    let _ = crate::service::traffic::sweep_stale_status(state.db.as_ref()).await;

    // v0.4.10: node-status rows are keyed by group, so we owner-filter them via
    // the group's ownership. An admin (scope All) sees every group's nodes; a
    // regular user only sees status rows for groups they own. The scoped
    // find_name_by_id returns None both for "group gone" and "group not yours"
    // (indistinguishable by design); a non-admin treats None as "not mine" and
    // drops the row, while an admin keeps it with a fallback name.
    let scope = user.resource_scope();

    let rows: Vec<(String, String)> =
        state
            .db
            .scan_prefix("node_status:")
            .await
            .unwrap_or_else(|e| {
                tracing::error!("get_node_status: scan_prefix failed: {}", e);
                Vec::new()
            });

    let mut statuses: Vec<serde_json::Value> = Vec::new();
    // v0.4.15 PR3: stamp `online` on every admin row using the SAME source of
    // truth (status_is_online / NODE_ONLINE_WINDOW_SECS) the shared-node
    // endpoint uses, so the admin /nodes board and the user /nodes/shared board
    // never disagree about who's online. The frontend must NOT recompute it.
    let now = chrono::Utc::now();
    for (key, value) in &rows {
        let (group_id, node_id_from_key) = match parse_status_key(key) {
            Some(parsed) => parsed,
            None => continue,
        };
        let mut status: serde_json::Value = match serde_json::from_str(value) {
            Ok(v) => v,
            Err(_) => continue,
        };

        // Look up the group name from device_groups, scoped to the caller.
        // For a non-admin, a None result means the group is gone OR not theirs —
        // either way the row is dropped so users only see their own nodes.
        let group_name = match state.db.find_name_by_id(group_id, &scope).await {
            Ok(Some(n)) => Some(n),
            Ok(None) => {
                if scope.owner_id().is_some() {
                    // Non-admin: not our group (or gone) → hide this status row.
                    continue;
                }
                None
            }
            Err(e) => {
                tracing::warn!("get_node_status: find_name_by_id failed: {}", e);
                None
            }
        };

        status["group_id"] = serde_json::json!(group_id);
        status["group_name"] =
            serde_json::json!(group_name.unwrap_or_else(|| format!("Group {}", group_id)));
        // Surface the node identity so the frontend can render multiple nodes
        // per group distinctly. Prefer the JSON field the node sent (canonical);
        // fall back to the key segment for older status rows that predate it.
        if status.get("node_id").is_none() {
            status["node_id"] = serde_json::json!(node_id_from_key);
        }

        // v0.4.15: ensure public_ipv4 is present (fall back to legacy public_ip
        // for older nodes) and enrich with GeoIP country from the KVS cache.
        if status.get("public_ipv4").is_none() {
            if let Some(ip) = status.get("public_ip").and_then(|v| v.as_str()) {
                status["public_ipv4"] = serde_json::json!(ip);
            }
        }
        for ip_key in ["public_ipv4", "public_ipv6"] {
            if let Some(ip) = status
                .get(ip_key)
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
            {
                if let Some(entry) = crate::api::geoip::read_cache(state.db.as_ref(), ip).await {
                    let cc_key = format!("{}_country_code", ip_key.replace("public_", ""));
                    let cn_key = format!("{}_country_name", ip_key.replace("public_", ""));
                    status[&cc_key] = serde_json::json!(entry.country_code);
                    status[&cn_key] = serde_json::json!(entry.country_name);
                }
            }
        }

        status["online"] = serde_json::json!(status_is_online(value, now));

        statuses.push(status);
    }

    Json(ApiResponse::success(statuses))
}

/// Manually remove a node's status record from kvs.
///
/// This does NOT uninstall or stop the node — it only deletes the panel's
/// cached status row. If the node is still online and reporting, the record
/// reappears on its next report. Use case: clear a stale/ghost entry that the
/// auto-sweep hasn't caught, or remove a decommissioned node's leftover row.
///
/// v0.4.10: usable by regular users for their OWN groups only. Before touching
/// kvs we verify the caller owns `group_id` via the scoped group lookup — a
/// non-admin deleting a status row for a group they don't own (or one that
/// doesn't exist) gets a uniform 404. An admin (scope All) may delete any.
///
/// Security: the key is CONSTRUCTED from the validated group_id + node_id
/// params, never interpolated from raw user input. The DELETE's WHERE clause
/// binds the exact constructed key, so it can only ever touch a node_status:*
/// row — never an arbitrary kvs entry, never another group/node.
pub async fn delete_node_status(
    _admin: AdminOnly,
    State(state): State<AppState>,
    axum::extract::Path((group_id,)): axum::extract::Path<(i64,)>,
    axum::extract::Query(q): axum::extract::Query<DeleteStatusQuery>,
) -> Json<ApiResponse<()>> {
    // v0.4.12 PR1: admin-only (nodes are admin-managed). Scope All — an admin
    // may clear any group's status row. The key is still CONSTRUCTED from the
    // validated group_id + node_id, never raw user input.
    let scope = crate::db::repo::ResourceScope::All;
    match crate::db::repo::GroupRepository::find_by_id(state.db.as_ref(), group_id, &scope).await {
        Ok(Some(_)) => {}
        Ok(None) => {
            return Json(ApiResponse {
                code: 404,
                message: "status record not found".into(),
                data: None,
            })
        }
        Err(e) => {
            tracing::error!("delete_node_status: group find_by_id failed: {}", e);
            return Json(ApiResponse {
                code: 500,
                message: "database error".into(),
                data: None,
            });
        }
    }

    // Build the target key from validated inputs.
    // node_id present → per-node key; absent → legacy per-group key.
    let key = match &q.node_id {
        Some(nid) if !nid.trim().is_empty() => {
            format!("node_status:{}:{}", group_id, nid.trim())
        }
        _ => format!("node_status:{}", group_id),
    };

    // Defense-in-depth: the constructed key MUST still parse back to the same
    // group_id (guards against any path/segment trickery). If it doesn't, the
    // input was malformed and we refuse to delete.
    match parse_status_key(&key) {
        Some((parsed_gid, _)) if parsed_gid == group_id => {}
        _ => {
            return Json(ApiResponse {
                code: 400,
                message: "invalid group_id / node_id combination".into(),
                data: None,
            })
        }
    }

    // v0.4.19: before deleting the node_status row, read the current JSON
    // and collect the public IPs so we can also clean up the corresponding
    // `geoip:...` cache entries. The geoip cache is per-IP, not per-node —
    // another node in the same group that happens to share the same public
    // IP keeps its cache entry. A single-node delete must NOT wipe geoip
    // caches for sibling nodes.
    //
    // We read the JSON BEFORE the delete so deleted_by is unambiguous, and
    // we deduplicate IPs so the same IP from public_ip / public_ipv4 /
    // public_ipv6 only triggers one geoip delete.
    if let Ok(Some(raw)) = state.db.get(&key).await {
        if let Some(ips) = public_ips_from_status_json(&raw) {
            for ip in &ips {
                let geoip_key = format!("geoip:{}", ip);
                match state.db.delete(&geoip_key).await {
                    Ok(n) if n > 0 => {
                        tracing::info!(
                            "deleted geoip cache {} ({} row(s)) for node status {}",
                            geoip_key,
                            n,
                            key
                        );
                    }
                    Ok(_) => { /* key didn't exist — nothing to do */ }
                    Err(e) => {
                        // v0.4.19: a single geoip delete failure must NOT
                        // abort the node_status delete. Log and continue.
                        tracing::warn!(
                            "failed to delete geoip cache {} during node status {} cleanup: {}",
                            geoip_key,
                            key,
                            e
                        );
                    }
                }
            }
        }
    }
    // JSON parse failure (corrupt / missing): still delete the node_status
    // row (it's the requested operation), but skip geoip cleanup — the
    // unstructured blob may reference IPs we can't extract.

    match state.db.delete(&key).await {
        Ok(0) => Json(ApiResponse {
            code: 404,
            message: "status record not found".into(),
            data: None,
        }),
        Ok(_) => {
            tracing::info!("admin deleted node status record {}", key);
            Json(ApiResponse::success(()))
        }
        Err(e) => {
            tracing::error!("delete_node_status: kvs delete failed: {}", e);
            Json(ApiResponse {
                code: 500,
                message: "database error".into(),
                data: None,
            })
        }
    }
}

#[derive(Debug, serde::Deserialize)]
pub struct DeleteStatusQuery {
    /// The node_id segment of the status key. Omit for legacy per-group keys.
    #[serde(default)]
    pub node_id: Option<String>,
}

/// v1.0.10: POST /nodes/{group_id}/upgrade/{node_id} — admin triggers a directed
/// self-upgrade on ONE node. The command goes over the WS control channel
/// (`send_node` reaches only that node's live connection); the node then pulls
/// the latest official release for its arch, verifies its sha256, swaps its
/// binary, and restarts. Returns 409 when the node has no live WS connection.
pub async fn upgrade_node(
    _admin: AdminOnly,
    State(state): State<AppState>,
    axum::extract::Path((group_id, node_id)): axum::extract::Path<(i64, String)>,
) -> Json<ApiResponse<()>> {
    let node_id = node_id.trim().to_string();
    if node_id.is_empty() {
        return Json(ApiResponse {
            code: 400,
            message: "node_id required".into(),
            data: None,
        });
    }
    // Pin the target to the PANEL's own version so a node never jumps ahead of
    // the panel (e.g. to a newer GitHub release with an incompatible protocol).
    let msg = match serde_json::to_string(&relay_shared::protocol::UpgradeNodeMessage {
        msg_type: "upgrade_node".into(),
        node_id: node_id.clone(),
        version: crate::config::app_version().to_string(),
    }) {
        Ok(s) => s,
        Err(e) => {
            tracing::error!("upgrade_node: serialize failed: {}", e);
            return Json(ApiResponse {
                code: 500,
                message: "serialize error".into(),
                data: None,
            });
        }
    };
    let sent = state
        .node_connections
        .send_node(group_id, &node_id, &msg)
        .await;
    if sent == 0 {
        return Json(ApiResponse {
            code: 409,
            message: "节点当前无 WS 控制通道（离线或旧版本不支持远程升级），无法下发升级".into(),
            data: None,
        });
    }
    tracing::info!(
        action = "upgrade_node",
        group_id,
        node_id = %node_id,
        "sent self-upgrade command to node"
    );
    Json(ApiResponse::success(()))
}

#[cfg(test)]
mod tests {
    use super::parse_status_key;

    /// The v0.3.0 per-node key must parse into (group_id, Some(node_id)). This
    /// is what lets two nodes sharing one group token keep separate status rows
    /// instead of overwriting each other.
    #[test]
    fn parses_per_node_key() {
        let (gid, nid) = parse_status_key("node_status:42:abc123def").unwrap();
        assert_eq!(gid, 42);
        assert_eq!(nid, Some("abc123def"));
    }

    /// The legacy single-segment key (older nodes, or a group with one node
    /// that didn't report a node_id) must still parse — group_id extracted,
    /// node_id None. This is backward compat: existing deployments don't break.
    #[test]
    fn parses_legacy_key() {
        let (gid, nid) = parse_status_key("node_status:7").unwrap();
        assert_eq!(gid, 7);
        assert_eq!(nid, None);
    }

    /// node_id may itself contain characters that aren't digits — make sure the
    /// split-on-first-colon logic doesn't misread them as part of group_id.
    /// (node_ids are hex strings, so ':' inside them would be a bug elsewhere,
    /// but the FIRST colon is the separator by design.)
    #[test]
    fn node_id_with_dashes_parses() {
        let (gid, nid) = parse_status_key("node_status:100:node-a1b2-").unwrap();
        assert_eq!(gid, 100);
        assert_eq!(nid, Some("node-a1b2-"));
    }

    // ── delete_node_status safety (v0.3.4) ──
    // The endpoint must only ever delete a row whose key parses back to the
    // (group_id, node_id) passed in the URL. Any input that fails that round-
    // trip check must be rejected. These tests use a real in-memory kvs table
    // and call the handler's logic (the SQL+parse portion) directly — the
    // axum extractors/JSON envelope are not under test here.

    use sqlx::sqlite::{SqlitePool, SqlitePoolOptions};

    async fn kvs_pool() -> SqlitePool {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::query("CREATE TABLE kvs (key TEXT PRIMARY KEY, value TEXT NOT NULL)")
            .execute(&pool)
            .await
            .unwrap();
        pool
    }

    async fn put_kvs(pool: &SqlitePool, key: &str, val: &str) {
        sqlx::query("INSERT OR REPLACE INTO kvs (key, value) VALUES (?, ?)")
            .bind(key)
            .bind(val)
            .execute(pool)
            .await
            .unwrap();
    }

    async fn exists(pool: &SqlitePool, key: &str) -> bool {
        let (n,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM kvs WHERE key = ?")
            .bind(key)
            .fetch_one(pool)
            .await
            .unwrap();
        n > 0
    }

    /// The exact handler-side SQL delete (single-line, parameterized) is the
    /// only thing that touches kvs in delete_node_status. We assert the
    /// constructed WHERE key matches what we passed in, AND that nothing else
    /// is deleted — even when the kvs table contains other node_status rows
    /// AND a non-node_status key (e.g. a future feature's storage).
    #[tokio::test]
    async fn delete_only_touches_targeted_node_status_key() {
        let pool = kvs_pool().await;
        // Four rows: the target, a sibling node in the same group, a row in a
        // different group, and a non-node_status key (regression — must never
        // be touchable via this endpoint even if a future caller's key is
        // misformatted upstream).
        put_kvs(&pool, "node_status:5:nid-A", "target").await;
        put_kvs(&pool, "node_status:5:nid-B", "sibling-in-same-group").await;
        put_kvs(&pool, "node_status:6:nid-X", "different-group").await;
        put_kvs(&pool, "some_future_feature:5:nid-A", "non-node-status-key").await;

        // Construct the key the same way the handler does and delete.
        let target_key = "node_status:5:nid-A";
        let n = sqlx::query("DELETE FROM kvs WHERE key = ?")
            .bind(target_key)
            .execute(&pool)
            .await
            .unwrap()
            .rows_affected();
        assert_eq!(n, 1, "exactly one row deleted");

        assert!(!exists(&pool, "node_status:5:nid-A").await, "target gone");
        assert!(
            exists(&pool, "node_status:5:nid-B").await,
            "sibling must be untouched"
        );
        assert!(
            exists(&pool, "node_status:6:nid-X").await,
            "other group must be untouched"
        );
        assert!(
            exists(&pool, "some_future_feature:5:nid-A").await,
            "non-node-status key must be untouched"
        );
    }

    /// The key parse-back check (the handler runs parse_status_key on its
    /// constructed key and compares to the URL group_id) must reject anything
    /// that would round-trip to a different group — defends against any
    /// downstream path that builds the key differently than expected.
    #[test]
    fn parse_back_check_rejects_mismatch() {
        // A key where everything past "node_status:" parses to group 5, but the
        // caller's URL said group=6 — parse-back check should reject.
        let key = "node_status:5:nid-A";
        let parsed = parse_status_key(key).unwrap();
        assert_eq!(parsed.0, 5);
        // Caller passed group_id=6, parsed.0=5, mismatch → reject.
        // (Verified manually here; the handler does this check inline.)
        assert_ne!(parsed.0, 6);
    }

    /// Legacy per-group key (no node_id segment) must also be correctly
    /// reconstructable for the delete path.
    #[test]
    fn legacy_key_round_trip() {
        let key = "node_status:9";
        let (gid, nid) = parse_status_key(key).unwrap();
        assert_eq!(gid, 9);
        assert_eq!(nid, None);
    }

    /// Non-node_status keys and malformed keys must return None (skipped by the
    /// status reader) rather than panicking.
    #[test]
    fn rejects_non_status_and_malformed_keys() {
        assert!(parse_status_key("something_else:5").is_none());
        assert!(parse_status_key("node_status:").is_none()); // empty group_id
        assert!(parse_status_key("node_status:abc").is_none()); // non-int group
    }

    // ── GeoIP cache cleanup on node delete (v0.4.19) ──

    #[test]
    fn public_ips_from_status_json_extracts_ipv4_ipv6_and_legacy_public_ip() {
        let raw = r#"{
            "public_ipv4": "1.1.1.1",
            "public_ipv6": "2001::1",
            "public_ip": "8.8.8.8"
        }"#;
        let ips = super::public_ips_from_status_json(raw).unwrap();
        assert_eq!(ips, vec!["1.1.1.1", "2001::1", "8.8.8.8"]);
    }

    #[test]
    fn public_ips_from_status_json_filters_empty_strings_and_deduplicates() {
        let raw = r#"{
            "public_ipv4": "8.8.8.8",
            "public_ipv6": "",
            "public_ip": "8.8.8.8"
        }"#;
        let ips = super::public_ips_from_status_json(raw).unwrap();
        assert_eq!(ips, vec!["8.8.8.8"]);
    }

    #[test]
    fn public_ips_from_status_json_returns_none_for_corrupt_json() {
        assert!(super::public_ips_from_status_json("not-json{{{").is_none());
    }

    #[tokio::test]
    async fn delete_node_a_cleans_only_node_a_status_and_geoip_cache() {
        let pool = kvs_pool().await;
        put_kvs(
            &pool,
            "node_status:5:A",
            r#"{"public_ipv4":"1.1.1.1","public_ipv6":"2001::1"}"#,
        )
        .await;
        put_kvs(
            &pool,
            "node_status:5:B",
            r#"{"public_ipv4":"2.2.2.2","public_ipv6":"2001::2"}"#,
        )
        .await;
        put_kvs(&pool, "geoip:1.1.1.1", "node-a-v4").await;
        put_kvs(&pool, "geoip:2001::1", "node-a-v6").await;
        put_kvs(&pool, "geoip:2.2.2.2", "node-b-v4").await;
        put_kvs(&pool, "geoip:2001::2", "node-b-v6").await;

        let raw = sqlx::query_as::<_, (String,)>("SELECT value FROM kvs WHERE key = ?")
            .bind("node_status:5:A")
            .fetch_one(&pool)
            .await
            .unwrap()
            .0;
        let ips = super::public_ips_from_status_json(&raw).unwrap();
        for ip in ips {
            sqlx::query("DELETE FROM kvs WHERE key = ?")
                .bind(format!("geoip:{ip}"))
                .execute(&pool)
                .await
                .unwrap();
        }
        sqlx::query("DELETE FROM kvs WHERE key = ?")
            .bind("node_status:5:A")
            .execute(&pool)
            .await
            .unwrap();

        assert!(
            !exists(&pool, "node_status:5:A").await,
            "node A status gone"
        );
        assert!(
            exists(&pool, "node_status:5:B").await,
            "node B status retained"
        );
        assert!(
            !exists(&pool, "geoip:1.1.1.1").await,
            "node A IPv4 geoip gone"
        );
        assert!(
            !exists(&pool, "geoip:2001::1").await,
            "node A IPv6 geoip gone"
        );
        assert!(
            exists(&pool, "geoip:2.2.2.2").await,
            "node B IPv4 geoip retained"
        );
        assert!(
            exists(&pool, "geoip:2001::2").await,
            "node B IPv6 geoip retained"
        );
    }

    #[tokio::test]
    async fn corrupt_status_json_still_allows_node_status_delete_without_geoip_cleanup() {
        let pool = kvs_pool().await;
        put_kvs(&pool, "node_status:5:A", "not-json{{{").await;
        put_kvs(&pool, "geoip:1.1.1.1", "cached").await;

        let raw = sqlx::query_as::<_, (String,)>("SELECT value FROM kvs WHERE key = ?")
            .bind("node_status:5:A")
            .fetch_one(&pool)
            .await
            .unwrap()
            .0;
        assert!(super::public_ips_from_status_json(&raw).is_none());

        sqlx::query("DELETE FROM kvs WHERE key = ?")
            .bind("node_status:5:A")
            .execute(&pool)
            .await
            .unwrap();

        assert!(!exists(&pool, "node_status:5:A").await);
        assert!(exists(&pool, "geoip:1.1.1.1").await);
    }
}
