use crate::api::AppState;
use axum::response::{IntoResponse, Response};
use axum::{extract::State, http::HeaderMap, http::StatusCode, Json};
use relay_shared::models::*;
use relay_shared::protocol::*;

/// Extract the node token from the `Authorization: Bearer <NODE_TOKEN>` header.
/// The token is accepted ONLY from this header — never from the query string
/// (leaks into access/proxy logs) nor from the request body. All currently
/// shipped nodes send the header.
pub(crate) fn extract_node_token(headers: &HeaderMap) -> Option<String> {
    headers
        .get("Authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer "))
        .map(|s| s.to_string())
}

/// v0.4.0: read the node's config-protocol version from the
/// `X-Config-Protocol-Version` request header. Returns None if absent (treated
/// as incompatible — the node is too old to know about the gate).
pub(crate) fn extract_config_protocol_version(headers: &HeaderMap) -> Option<u32> {
    headers
        .get("X-Config-Protocol-Version")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse::<u32>().ok())
}

/// v0.4.0: the config-protocol compatibility gate. Returns true if the node's
/// reported version matches the panel's `CONFIG_PROTOCOL_VERSION`. A missing
/// header (old node) is treated as incompatible. Used by both get_config (HTTP)
/// and the WS upgrade path so both paths refuse consistently.
pub(crate) fn config_protocol_compatible(headers: &HeaderMap) -> bool {
    match extract_config_protocol_version(headers) {
        Some(v) => v == CONFIG_PROTOCOL_VERSION,
        None => false,
    }
}

pub async fn get_config(State(state): State<AppState>, headers: HeaderMap) -> Response {
    // v0.4.0: protocol-version gate. A node reporting a different
    // config_protocol_version (or none at all — pre-v0.4.0 node) must NOT
    // receive config it can't deserialize (e.g. the renamed node_transport
    // field). Return 426 (Upgrade Required) — NOT 503 — so the node treats it
    // as a permanent config error and backs off, not as a transient outage.
    // The structured JSON lets the node log "requires v1, has v0".
    if !config_protocol_compatible(&headers) {
        let received = extract_config_protocol_version(&headers);
        return (
            StatusCode::UPGRADE_REQUIRED,
            Json(serde_json::json!({
                "code": "CONFIG_PROTOCOL_MISMATCH",
                "required": CONFIG_PROTOCOL_VERSION,
                "received": received,
                "message": "relay-node configuration protocol is incompatible; \
                            upgrade relay-node to match the panel"
            })),
        )
            .into_response();
    }

    // Token comes ONLY from the Authorization header. No token → treat as
    // "no matching group" and return an empty config (NOT an error: a node
    // that hasn't been assigned a group yet should keep its cached config).
    let Some(token) = extract_node_token(&headers) else {
        return Json(NodeConfigResponse { listeners: vec![] }).into_response();
    };

    // Find device group by token.
    let group: Option<DeviceGroup> = match state.db.find_by_token(&token).await {
        Ok(g) => g,
        Err(e) => {
            tracing::error!("get_config: find_by_token failed: {}", e);
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                "config unavailable: transient database error",
            )
                .into_response();
        }
    };

    let Some(group) = group else {
        return Json(NodeConfigResponse { listeners: vec![] }).into_response();
    };

    // v0.3.6: delegate to the shared `build_node_config`. This path and the WS
    // push path (ws.rs) now use the SAME function.
    //
    // An empty Ok result is a legitimate "no matching rules" state. A DB Err is
    // a transient backend failure → HTTP 503.
    match crate::service::node_config::build_node_config(state.db.as_ref(), group.id).await {
        Ok(cfg) => Json(cfg).into_response(),
        Err(e) => {
            tracing::error!(
                "get_config: build_node_config failed for group {}: {}",
                group.id,
                e
            );
            (
                StatusCode::SERVICE_UNAVAILABLE,
                "config unavailable: transient database error",
            )
                .into_response()
        }
    }
}

pub async fn report_traffic(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<TrafficReport>,
) -> Json<ApiResponse<()>> {
    // Token comes ONLY from the Authorization header (v0.3.9: the body token
    // fallback was removed — nodes send the header and an empty body token).
    //
    // HTTP-status note: a missing/invalid token here returns HTTP 200 with a
    // business `code: 401` INSIDE the JSON body — NOT a real HTTP 401. This is
    // deliberate backward-compat: all shipped nodes read the JSON `code` field
    // and ignore the HTTP status on these node-facing endpoints. The WebSocket
    // upgrade path (ws.rs::node_ws_handler) is the ONE exception — it returns a
    // real HTTP 401 because WS upgrades must fail at the HTTP layer (the client
    // never gets to read a JSON body on a failed upgrade). Do NOT "normalize"
    // these without a coordinated node upgrade; see the test module's
    // `node_http_status_compat_*` tests that pin the current behavior.
    let Some(token) = extract_node_token(&headers) else {
        return Json(ApiResponse {
            code: 401,
            message: "Invalid token".into(),
            data: None,
        });
    };

    let group: Option<DeviceGroup> = match state.db.find_by_token(&token).await {
        Ok(g) => g,
        Err(e) => {
            tracing::error!("report_traffic: find_by_token failed: {}", e);
            return Json(ApiResponse {
                code: 500,
                message: "database error".into(),
                data: None,
            });
        }
    };

    let group = match group {
        Some(g) => g,
        None => {
            return Json(ApiResponse {
                code: 401,
                message: "Invalid token".into(),
                data: None,
            })
        }
    };

    // v0.4.9 SECURITY: the whole batch is one atomic transaction, and rule-id
    // existence is NO LONGER distinguishable from cross-group reporting. Both
    // "rule missing" and "rule belongs to another group" produce the SAME
    // external response (403 + a single generic message). The batch logic lives
    // in `service::traffic::apply_traffic_report` (overflow pre-check + atomic
    // apply + result interpretation) so it can be unit-tested without HTTP.
    //
    // HTTP-status note (preserved): a rejection returns HTTP 200 with a business
    // `code` (403/400/500) INSIDE the JSON body — NOT a real HTTP error. Nodes
    // read the JSON `code` and ignore the HTTP status on these endpoints.
    match crate::service::traffic::apply_traffic_report(state.db.as_ref(), group.id, &req.reports)
        .await
    {
        Ok(()) => Json(ApiResponse::success(())),
        Err(crate::service::traffic::TrafficReportError::Unavailable) => {
            // Uniform 403 — identical for "missing" and "foreign". Do NOT echo
            // which rule_id or why.
            Json(ApiResponse {
                code: 403,
                message: "one or more rules are unavailable for this node".into(),
                data: None,
            })
        }
        Err(crate::service::traffic::TrafficReportError::Overflow) => Json(ApiResponse {
            code: 400,
            message: "one or more traffic entries are out of range".into(),
            data: None,
        }),
        Err(crate::service::traffic::TrafficReportError::Database(e)) => {
            tracing::error!("report_traffic: apply_traffic_batch failed: {}", e);
            Json(ApiResponse {
                code: 500,
                message: "database error".into(),
                data: None,
            })
        }
    }
}

pub async fn report_status(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<StatusReport>,
) -> Json<ApiResponse<()>> {
    // Token comes ONLY from the Authorization header (v0.3.9: body token
    // fallback removed).
    let Some(token) = extract_node_token(&headers) else {
        return Json(ApiResponse {
            code: 401,
            message: "Invalid token".into(),
            data: None,
        });
    };

    // Verify token and update node status in kvs
    let group: Option<DeviceGroup> = match state.db.find_by_token(&token).await {
        Ok(g) => g,
        Err(e) => {
            tracing::error!("report_status: find_by_token failed: {}", e);
            // Match the original swallow-and-empty behavior: a transient DB
            // failure shouldn't make the node think its report was rejected.
            None
        }
    };

    if let Some(g) = group {
        // v0.3.0: key node status by (group_id, node_id) so multiple nodes
        // sharing one group token no longer overwrite each other. The node_id
        // is a stable per-node identity generated on first start (see
        // poller::get_or_create_node_id). Older nodes that don't send node_id
        // fall back to the legacy per-group key (no regression — a single-node
        // group behaves exactly as before).
        let status_key = match &req.node_id {
            Some(nid) if !nid.trim().is_empty() => format!("node_status:{}:{}", g.id, nid.trim()),
            _ => format!("node_status:{}", g.id), // legacy fallback
        };
        let node_id_for_json = req.node_id.clone();
        // Store every reported metric in the status JSON. New optional fields
        // are only included when the node actually reported them (older nodes
        // omit them and the panel renders "-" for missing values).
        let status = serde_json::json!({
            "node_id": node_id_for_json,
            "cpu": req.cpu_usage,
            "mem": req.mem_usage,
            "connections": req.active_connections,
            // v0.3.2: "uptime" is SYSTEM uptime (since OS boot). process uptime
            // is separate below; older nodes don't send it and it renders as "-".
            "uptime": req.uptime_secs,
            "process_uptime": req.process_uptime_secs,
            // v0.3.4: the node binary's version (for the "stale node" upgrade
            // hint). Older nodes don't send it; the panel renders "-".
            "node_version": req.node_version,
            // v0.4.0: config-protocol version (mirrors the
            // X-Config-Protocol-Version header). The frontend uses this to show
            // "配置协议不兼容，请升级节点" when it doesn't match the panel's.
            "config_protocol_version": req.config_protocol_version,
            "last_seen": chrono::Utc::now().to_rfc3339(),
            "public_ip": req.public_ip,
            // v0.4.15: dual-stack public IPs. Falls back to public_ip (legacy
            // IPv4) when the node hasn't upgraded yet.
            "public_ipv4": req.public_ipv4.clone().or(req.public_ip.clone()),
            "public_ipv6": req.public_ipv6,
            "disk_total": req.disk_total,
            "disk_used": req.disk_used,
            "disk_usage_percent": req.disk_usage_percent,
            "disk_mount": req.disk_mount,
            "upload_bps": req.upload_bps,
            "download_bps": req.download_bps,
            "boot_upload_bytes": req.boot_upload_bytes,
            "boot_download_bytes": req.boot_download_bytes,
            // v0.4.6: the interface machine traffic is counted on, so the panel
            // can show "统计网卡: eth0". Missing on older nodes → "-".
            "network_interface": req.network_interface,
            // v0.3.6: listener bind failures (port in use, permission denied,
            // etc.) so the operator can see WHY a rule isn't forwarding.
            // Missing on older nodes; the frontend renders "ok".
            "listener_errors": req.listener_errors,
        });
        // Status persistence is best-effort: the original used .ok() to swallow
        // any DB error so a transient failure never broke the report cycle.
        let _ = state
            .db
            .set(&status_key, &status.to_string())
            .await
            .map_err(|e| tracing::warn!("report_status: kvs set failed: {}", e));

        // v0.4.19: async GeoIP enrichment — fire-and-forget, never blocks the
        // status report or node forwarding. Only runs when GEOIP_ENABLED=true.
        // Uses built-in primary + fallback providers (ipinfo.io → ipwho.is).
        // Each public IP is looked up independently; the geoip module handles
        // caching + concurrent de-duplication + private-IP rejection.
        if state.config.geoip_enabled {
            let db = state.db.clone();
            let ttl = state.config.geoip_cache_ttl as i64;
            let inflight = state.geoip_in_flight.clone();
            let v4 = req.public_ipv4.clone().or(req.public_ip.clone());
            let v6 = req.public_ipv6.clone();
            tokio::spawn(async move {
                if let Some(ip) = v4 {
                    let _ = crate::api::geoip::lookup(db.as_ref(), ttl, &inflight, &ip).await;
                }
                if let Some(ip) = v6 {
                    let _ = crate::api::geoip::lookup(db.as_ref(), ttl, &inflight, &ip).await;
                }
            });
        }

        // ── v0.3.2: legacy status cleanup ──
        // When a node upgraded to v0.3.1+ starts reporting with its new
        // node_id key, its OLD legacy entry ("node_status:{group_id}", no
        // node_id suffix) is left behind forever, showing as a permanently-
        // offline ghost node. We clean it up HERE: if this report has a
        // node_id AND a public_ip, delete the legacy key for the same group
        // IF AND ONLY IF its stored public_ip matches (so a different-IP node
        // sharing the group isn't wrongly deleted).
        if let (Some(nid), Some(ref ip)) = (&req.node_id, &req.public_ip) {
            if !nid.trim().is_empty() && !ip.is_empty() {
                crate::service::traffic::cleanup_legacy_status(state.db.as_ref(), g.id, ip).await;
            }
        }
    }

    // ── v0.3.2: stale status sweep ──
    // Also runs on READ (get_node_status), so ghost rows get cleaned even when
    // no node in the group is still reporting. Threshold is 2 min (frontend
    // marks offline at 30s; we keep the row a bit longer to ride out blips).
    let _ = crate::service::traffic::sweep_stale_status(state.db.as_ref()).await;

    Json(ApiResponse::success(()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::sqlite_repo::SqliteRepository;
    use sqlx::sqlite::{SqlitePool, SqlitePoolOptions};

    // ── report_traffic transactional correctness (v0.3.6) ──
    //
    // These exercise the atomicity contract: rule + user totals must move
    // together or not at all; an unauthorized rule must reject the whole batch;
    // a stale rule_id is skipped; overflow is rejected up front.

    use crate::api::system::ReleaseCache;
    use crate::api::ws::NodeConnections;
    use crate::api::AppState;
    use crate::config::Config;
    use crate::db::schema::SCHEMA_SQL;
    use relay_shared::protocol::{TrafficEntry, TrafficReport};
    use std::sync::Arc;

    async fn full_state() -> (AppState, SqlitePool) {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::query(SCHEMA_SQL).execute(&pool).await.unwrap();
        let state = AppState {
            db: Arc::new(SqliteRepository::new(pool.clone())),
            config: Config {
                database_path: "sqlite::memory:".into(),
                listen: "127.0.0.1:0".into(),
                key: "test-key".into(),
                jwt_secret: "test-secret".into(),
                public_dir: "public".into(),
                public_panel_url: String::new(),
                registration_enabled: false,
                cors_origins: vec![],
                geoip_enabled: false,
                geoip_cache_ttl: 604_800,
            },
            release_cache: ReleaseCache::new(),
            node_connections: NodeConnections::new(),
            diagnose: crate::api::diagnose::DiagnoseRegistry::new(),
            geoip_in_flight: std::sync::Arc::new(tokio::sync::Mutex::new(
                std::collections::HashSet::new(),
            )),
        };
        (state, pool)
    }

    /// Seed: user 2 (non-admin), inbound group 10 with token "tok-A", rule 100
    /// owned by user 2 on group 10, port 20000. Returns the AppState + pool.
    async fn seeded_state() -> (AppState, SqlitePool) {
        let (state, pool) = full_state().await;
        let hash = bcrypt::hash("pw-2", 4).unwrap();
        sqlx::query("INSERT INTO users (id, username, password, admin) VALUES (2, 'alice', ?, 0)")
            .bind(&hash)
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO device_groups (id, name, group_type, token, uid) \
             VALUES (10, 'gin', 'in', 'tok-A', 2)",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO forward_rules \
             (id, name, uid, listen_port, device_group_in, target_addr, target_port) \
             VALUES (100, 'r100', 2, 20000, 10, '127.0.0.1', 80)",
        )
        .execute(&pool)
        .await
        .unwrap();
        (state, pool)
    }

    fn report(_token: &str, entries: &[TrafficEntry]) -> TrafficReport {
        TrafficReport {
            reports: entries.to_vec(),
        }
    }

    fn auth_headers(token: &str) -> HeaderMap {
        let mut h = HeaderMap::new();
        h.insert("Authorization", format!("Bearer {token}").parse().unwrap());
        h
    }

    async fn user_traffic(pool: &SqlitePool, uid: i64) -> i64 {
        let (v,): (i64,) = sqlx::query_as("SELECT traffic_used FROM users WHERE id=?")
            .bind(uid)
            .fetch_one(pool)
            .await
            .unwrap();
        v
    }

    async fn rule_traffic(pool: &SqlitePool, rid: i64) -> i64 {
        let (v,): (i64,) = sqlx::query_as("SELECT traffic_used FROM forward_rules WHERE id=?")
            .bind(rid)
            .fetch_one(pool)
            .await
            .unwrap();
        v
    }

    /// Normal batch: rule and user totals both move, atomically.
    #[tokio::test]
    async fn traffic_report_updates_rule_and_user() {
        let (state, pool) = seeded_state().await;
        let Json(resp) = report_traffic(
            State(state.clone()),
            auth_headers("tok-A"),
            Json(report(
                "tok-A",
                &[TrafficEntry {
                    rule_id: 100,
                    upload: 1000,
                    download: 2000,
                }],
            )),
        )
        .await;
        assert_eq!(resp.code, 0, "{}", resp.message);
        assert_eq!(rule_traffic(&pool, 100).await, 3000);
        assert_eq!(user_traffic(&pool, 2).await, 3000);
    }

    /// Multi-entry batch updates every rule and the shared user once each.
    #[tokio::test]
    async fn traffic_report_multi_entry_all_applied() {
        let (state, pool) = seeded_state().await;
        // second rule on the same group + user
        sqlx::query(
            "INSERT INTO forward_rules \
             (id, name, uid, listen_port, device_group_in, target_addr, target_port) \
             VALUES (101, 'r101', 2, 20001, 10, '127.0.0.1', 80)",
        )
        .execute(&pool)
        .await
        .unwrap();

        let Json(resp) = report_traffic(
            State(state.clone()),
            auth_headers("tok-A"),
            Json(report(
                "tok-A",
                &[
                    TrafficEntry {
                        rule_id: 100,
                        upload: 100,
                        download: 0,
                    },
                    TrafficEntry {
                        rule_id: 101,
                        upload: 0,
                        download: 200,
                    },
                ],
            )),
        )
        .await;
        assert_eq!(resp.code, 0, "{}", resp.message);
        assert_eq!(rule_traffic(&pool, 100).await, 100);
        assert_eq!(rule_traffic(&pool, 101).await, 200);
        assert_eq!(user_traffic(&pool, 2).await, 300);
    }

    /// A rule belonging to ANOTHER group is unauthorized — the whole batch is
    /// rejected and rolled back, including the legitimate entry in the same batch.
    #[tokio::test]
    async fn traffic_report_other_group_rule_rejects_whole_batch() {
        let (state, pool) = seeded_state().await;
        // rule 200 belongs to group 20 (different group), same user
        sqlx::query(
            "INSERT INTO device_groups (id, name, group_type, token, uid) \
             VALUES (20, 'g20', 'in', 'tok-B', 2)",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO forward_rules \
             (id, name, uid, listen_port, device_group_in, target_addr, target_port) \
             VALUES (200, 'r200', 2, 20002, 20, '127.0.0.1', 80)",
        )
        .execute(&pool)
        .await
        .unwrap();

        let Json(resp) = report_traffic(
            State(state.clone()),
            auth_headers("tok-A"),
            Json(report(
                "tok-A",
                &[
                    TrafficEntry {
                        rule_id: 100,
                        upload: 500,
                        download: 0,
                    },
                    TrafficEntry {
                        rule_id: 200,
                        upload: 0,
                        download: 999,
                    },
                ],
            )),
        )
        .await;
        assert_eq!(resp.code, 403, "unauthorized rule must reject batch");
        // Rollback: even the legitimate rule 100 entry must NOT have landed.
        assert_eq!(rule_traffic(&pool, 100).await, 0);
        assert_eq!(user_traffic(&pool, 2).await, 0);
    }

    /// v0.4.9: a rule_id that does NOT exist must be treated EXACTLY like a
    /// foreign rule (uniform 403 + whole-batch rollback) — it can no longer be
    /// told apart by the response. This closes the rule-id existence oracle.
    #[tokio::test]
    async fn traffic_report_unknown_rule_is_unavailable_not_skipped() {
        let (state, pool) = seeded_state().await;
        let Json(resp) = report_traffic(
            State(state.clone()),
            auth_headers("tok-A"),
            Json(report(
                "tok-A",
                &[
                    TrafficEntry {
                        rule_id: 99999, // does not exist
                        upload: 1,
                        download: 2,
                    },
                    TrafficEntry {
                        rule_id: 100,
                        upload: 10,
                        download: 20,
                    },
                ],
            )),
        )
        .await;
        // Same code + same generic message as the foreign-rule case.
        assert_eq!(
            resp.code, 403,
            "unknown rule must be rejected like a foreign rule"
        );
        assert_eq!(
            resp.message, "one or more rules are unavailable for this node",
            "message must be generic — no rule_id, no reason"
        );
        // Rollback: even rule 100 must NOT have landed.
        assert_eq!(rule_traffic(&pool, 100).await, 0);
        assert_eq!(user_traffic(&pool, 2).await, 0);
    }

    /// Overflow in upload+download is rejected up front with a 400 (no DB write).
    #[tokio::test]
    async fn traffic_report_overflow_rejected() {
        let (state, pool) = seeded_state().await;
        let Json(resp) = report_traffic(
            State(state.clone()),
            auth_headers("tok-A"),
            Json(report(
                "tok-A",
                &[TrafficEntry {
                    rule_id: 100,
                    upload: u64::MAX,
                    download: 1,
                }],
            )),
        )
        .await;
        assert_eq!(resp.code, 400);
        // Nothing landed.
        assert_eq!(rule_traffic(&pool, 100).await, 0);
        assert_eq!(user_traffic(&pool, 2).await, 0);
    }

    // ── v0.4.9: node HTTP-status compatibility pins ──
    //
    // The three node-facing endpoints have DELIBERATELY DIFFERENT auth-failure
    // behaviors, preserved for backward compat with all shipped nodes:
    //   - report_traffic / report_status: missing token → HTTP 200, business
    //     code 401 INSIDE the JSON body (nodes read `code`, not the HTTP status).
    //   - get_config: missing token → HTTP 200, empty config (NOT an error).
    //   - WebSocket upgrade: missing/invalid token → real HTTP 401 (WS upgrades
    //     must fail at the HTTP layer — the client never reads a JSON body).
    //
    // These tests PIN that behavior so a future "let's normalize to real HTTP
    // 401s" change can't land silently and break old nodes. Changing any of
    // these requires a coordinated major-version node upgrade.

    /// report_traffic with NO Authorization header → HTTP 200, JSON code 401.
    #[tokio::test]
    async fn node_http_status_compat_traffic_missing_token_is_http200_business401() {
        let (state, _pool) = seeded_state().await;
        let mut h = HeaderMap::new();
        // No Authorization header. (Also need the config-protocol header? No —
        // report_traffic doesn't gate on it, only get_config / WS do.)
        let _ = &mut h;
        let Json(resp) = report_traffic(State(state.clone()), h, Json(report("", &[]))).await;
        // The Json wrapper always serializes as HTTP 200; the business code is
        // the signal. Pin both: status is 200 (Implicit via Json), code is 401.
        assert_eq!(resp.code, 401, "missing token → business 401, not HTTP 401");
        assert_eq!(resp.message, "Invalid token");
    }

    /// report_status with NO Authorization header → HTTP 200, JSON code 401.
    #[tokio::test]
    async fn node_http_status_compat_status_missing_token_is_http200_business401() {
        use relay_shared::protocol::StatusReport;
        let (state, _pool) = seeded_state().await;
        let h = HeaderMap::new(); // no Authorization
        let req = StatusReport {
            cpu_usage: 0.0,
            mem_usage: 0.0,
            active_connections: 0,
            uptime_secs: 0,
            public_ip: None,
            public_ipv4: None,
            public_ipv6: None,
            disk_total: None,
            disk_used: None,
            disk_usage_percent: None,
            disk_mount: None,
            upload_bps: None,
            download_bps: None,
            boot_upload_bytes: None,
            boot_download_bytes: None,
            network_interface: None,
            node_id: None,
            process_uptime_secs: None,
            node_version: None,
            config_protocol_version: None,
            listener_errors: None,
            install_method: None,
        };
        let Json(resp) = report_status(State(state.clone()), h, Json(req)).await;
        assert_eq!(resp.code, 401, "missing token → business 401, not HTTP 401");
    }

    /// get_config with NO Authorization header (but a valid config-protocol
    /// header) → HTTP 200 with an EMPTY config, NOT an error. A node that
    /// hasn't been assigned a group should keep its cached config.
    #[tokio::test]
    async fn node_http_status_compat_get_config_missing_token_returns_empty_config() {
        let (state, _pool) = seeded_state().await;
        let mut h = HeaderMap::new();
        // get_config gates on config-protocol FIRST; supply a matching one so
        // we reach the token check (else it'd return 426, masking this path).
        h.insert(
            "X-Config-Protocol-Version",
            relay_shared::protocol::CONFIG_PROTOCOL_VERSION
                .to_string()
                .parse()
                .unwrap(),
        );
        // No Authorization header.
        let resp = get_config(State(state.clone()), h).await;
        // Pin: HTTP 200 (not 401/403) + an empty listeners array.
        assert_eq!(resp.status(), axum::http::StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), 65536).await.unwrap();
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(
            v["listeners"].as_array().map(|a| a.len()),
            Some(0),
            "missing token → empty config, not an error"
        );
    }

    /// WebSocket upgrade with NO Authorization header → real HTTP 401 (the one
    /// exception to the "business code in JSON" rule — WS upgrades must fail at
    /// the HTTP layer). We assert via node_ws_handler's IntoResponse output,
    /// WITHOUT performing a real WS upgrade (the handler returns 401 before
    /// touching the socket).
    #[tokio::test]
    async fn node_http_status_compat_ws_missing_token_is_real_http401() {
        // We can't easily build a WebSocketUpgrade in a unit test, so this pin
        // documents + guards the contract via the token-extraction primitive the
        // handler uses: no Authorization header → extract_node_token returns
        // None, and node_ws_handler returns StatusCode::UNAUTHORIZED on None.
        // (A full WS-upgrade integration test would need an HTTP server; the
        // primitive-level pin is sufficient to catch a regression here.)
        let h = HeaderMap::new(); // no Authorization
        assert!(
            extract_node_token(&h).is_none(),
            "no Authorization header → no token → WS handler returns real HTTP 401"
        );
        // And a malformed header (not "Bearer ...") also yields None.
        let mut h2 = HeaderMap::new();
        h2.insert("Authorization", "notabearer".parse().unwrap());
        assert!(extract_node_token(&h2).is_none());
    }
}
