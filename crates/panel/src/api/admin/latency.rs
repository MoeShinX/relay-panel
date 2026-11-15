//! v1.2.21: latency monitor, admin side — the target list and one node's
//! series. Everything here is `AdminOnly`: the targets and the figures are the
//! operator's own view of their machines' networks.

use super::err;
use crate::api::middleware::AdminOnly;
use crate::api::AppState;
use crate::service::latency::{self, ConfigError, LatencyConfigView, PutConfigRequest};
use axum::extract::{Path, Query, State};
use axum::Json;
use relay_shared::protocol::{node_supports_latency, ApiResponse};
use serde::{Deserialize, Serialize};

pub async fn get_latency_config(
    _admin: AdminOnly,
    State(state): State<AppState>,
) -> Json<ApiResponse<LatencyConfigView>> {
    match latency::get_config(state.db.as_ref()).await {
        Ok(view) => Json(ApiResponse::success(view)),
        Err(e) => {
            tracing::error!("get_latency_config: db error: {}", e);
            Json(err(500, "数据库错误"))
        }
    }
}

/// Replace the whole configuration (see `latency::save_config`). The nodes are
/// told to re-fetch only when what they probe changed.
pub async fn put_latency_config(
    admin: AdminOnly,
    State(state): State<AppState>,
    Json(req): Json<PutConfigRequest>,
) -> Json<ApiResponse<LatencyConfigView>> {
    let now = chrono::Utc::now().format("%Y-%m-%d %H:%M:%S").to_string();
    match latency::save_config(state.db.as_ref(), &now, &req).await {
        Ok(saved) => {
            if saved.plan_changed {
                state
                    .node_connections
                    .broadcast_all(r#"{"type":"config_changed"}"#)
                    .await;
            }
            // Counts, not hosts: the trail records that the monitor was
            // reconfigured, and the list itself is one click away.
            crate::service::audit::record(
                &state,
                Some(admin.user_id),
                "update_latency_config",
                "settings",
                "latency",
                &format!(
                    "探测间隔 {} 秒 / 目标 {} 个（新增 {}，删除 {}）",
                    saved.view.interval_secs,
                    saved.view.targets.len(),
                    saved.added,
                    saved.removed
                ),
            )
            .await;
            Json(ApiResponse::success(saved.view))
        }
        Err(ConfigError::Invalid(msg)) => Json(err(400, msg)),
        Err(ConfigError::UnknownTarget) => Json(err(409, "某个目标已被删除，请刷新页面后重新编辑")),
        Err(ConfigError::Db(e)) => {
            tracing::error!("put_latency_config: db error: {}", e);
            Json(err(500, "数据库错误"))
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct NodeLatencyQuery {
    #[serde(default = "default_range")]
    pub range: String,
    #[serde(default = "default_granularity")]
    pub granularity: String,
}

fn default_range() -> String {
    "1h".into()
}

fn default_granularity() -> String {
    "auto".into()
}

#[derive(Debug, Serialize)]
pub struct NodeLatencyResponse {
    pub node_id: String,
    pub node_version: Option<String>,
    /// Whether the node's version runs the monitor at all (node-v1.2.12+).
    pub supported: bool,
    pub interval_secs: u32,
    pub range: String,
    /// The granularity actually used: "1m" | "5m" | "1h".
    pub granularity: String,
    pub bucket_secs: i64,
    /// Unix seconds, the window the series cover.
    pub since: i64,
    pub until: i64,
    pub series: Vec<latency::TargetSeries>,
    pub summary: Vec<latency::TargetSummary>,
    /// Configured targets the node reported it cannot probe.
    pub unsupported_targets: Vec<i64>,
}

pub async fn get_node_latency(
    _admin: AdminOnly,
    State(state): State<AppState>,
    Path(node_id): Path<String>,
    Query(q): Query<NodeLatencyQuery>,
) -> Json<ApiResponse<NodeLatencyResponse>> {
    let Some(range_secs) = latency::range_secs(&q.range) else {
        return Json(err(
            400,
            format!("range 只能是 1h / 6h / 24h / 7d / 30d，收到: {}", q.range),
        ));
    };
    let granularity = match latency::resolve_granularity(&q.range, &q.granularity) {
        Ok(g) => g,
        Err(msg) => return Json(err(400, msg)),
    };
    let db = state.db.as_ref();

    let status = match latency::find_node_status(db, &node_id).await {
        Ok(Some(s)) => s,
        Ok(None) => return Json(err(404, "节点不存在")),
        Err(e) => {
            tracing::error!("get_node_latency: status lookup failed: {}", e);
            return Json(err(500, "数据库错误"));
        }
    };
    let node_version = status
        .get("node_version")
        .and_then(|v| v.as_str())
        .map(str::to_string);
    let supported = node_supports_latency(node_version.as_deref());

    let now_ts = chrono::Utc::now().timestamp();
    let bucket_secs = granularity.bucket_secs();
    let since = latency::align_down(now_ts - range_secs, bucket_secs);
    let until = now_ts + 1;

    let load = async {
        let targets = db.list_latency_targets().await?;
        let interval_secs = latency::get_interval(db).await?;
        // A node that cannot run the monitor has nothing stored worth asking
        // for; the page tells the operator to upgrade instead.
        let (buckets, latest) = if supported && !targets.is_empty() {
            (
                db.query_latency_buckets(&node_id, granularity, since, until)
                    .await?,
                db.query_latest_latency(&node_id, now_ts - latency::ACCEPT_WINDOW_SECS)
                    .await?,
            )
        } else {
            (Vec::new(), Vec::new())
        };
        Ok::<_, crate::db::error::DbError>((targets, interval_secs, buckets, latest))
    };
    let (targets, interval_secs, buckets, latest) = match load.await {
        Ok(v) => v,
        Err(e) => {
            tracing::error!("get_node_latency: db error: {}", e);
            return Json(err(500, "数据库错误"));
        }
    };

    let unsupported_targets: Vec<i64> = status
        .get("latency_unsupported")
        .and_then(|v| v.as_array())
        .map(|ids| {
            ids.iter()
                .filter_map(|v| v.as_i64())
                .filter(|id| targets.iter().any(|t| t.id == *id))
                .collect()
        })
        .unwrap_or_default();

    let (series, summary) = latency::build_series(&targets, &buckets, &latest);
    Json(ApiResponse::success(NodeLatencyResponse {
        node_id,
        node_version,
        supported,
        interval_secs,
        range: q.range,
        granularity: latency::granularity_name(granularity).to_string(),
        bucket_secs,
        since,
        until,
        series,
        summary,
        unsupported_targets,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::middleware::{AdminOnly, Claims};
    use crate::api::system::ReleaseCache;
    use crate::api::ws::NodeConnections;
    use crate::config::Config;
    use crate::db::repo::{LatencyGranularity, LatencyMinuteRow};
    use crate::db::schema::SCHEMA_SQL;
    use crate::db::sqlite_repo::SqliteRepository;
    use crate::service::latency::{PutConfigRequest, TargetRequest};
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use relay_shared::protocol::LatencySample;
    use sqlx::sqlite::SqlitePoolOptions;
    use sqlx::SqlitePool;
    use std::sync::Arc;
    use tower::ServiceExt;

    async fn test_state() -> (AppState, SqlitePool) {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("connect memory db");
        sqlx::query(SCHEMA_SQL)
            .execute(&pool)
            .await
            .expect("create schema");
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
            geoip_in_flight: Arc::new(tokio::sync::Mutex::new(std::collections::HashSet::new())),
        };
        (state, pool)
    }

    fn admin() -> AdminOnly {
        AdminOnly { user_id: 1 }
    }

    fn target(name: &str, host: &str, port: Option<i64>) -> TargetRequest {
        TargetRequest {
            id: None,
            name: name.into(),
            carrier: "telecom".into(),
            kind: if port.is_some() { "tcp" } else { "icmp" }.into(),
            host: host.into(),
            port,
        }
    }

    fn put(interval: i64, targets: Vec<TargetRequest>) -> Json<PutConfigRequest> {
        Json(PutConfigRequest {
            interval_secs: interval,
            targets,
        })
    }

    /// Re-submit what the server holds, as the settings page does.
    fn as_requests(view: &LatencyConfigView) -> Vec<TargetRequest> {
        view.targets
            .iter()
            .map(|t| TargetRequest {
                id: Some(t.id),
                name: t.name.clone(),
                carrier: t.carrier.clone(),
                kind: t.kind.clone(),
                host: t.host.clone(),
                port: t.port.map(i64::from),
            })
            .collect()
    }

    /// Count the config_changed pushes a call produces, with a live connection
    /// registered to receive them (same approach as the rule/group tests).
    async fn broadcasts_during(
        state: &AppState,
        f: impl std::future::Future<Output = ()>,
    ) -> usize {
        let (_id, mut rx) = state.node_connections.register(99, None).await;
        f.await;
        let mut n = 0;
        for _ in 0..20 {
            while rx.try_recv().is_ok() {
                n += 1;
            }
            tokio::task::yield_now().await;
        }
        n
    }

    fn sign_token(sub: i64) -> String {
        jsonwebtoken::encode(
            &jsonwebtoken::Header::default(),
            &Claims {
                sub,
                admin: true, // ignored by the server: the database decides
                token_version: 0,
                exp: chrono::Utc::now().timestamp() as usize + 3600,
            },
            &jsonwebtoken::EncodingKey::from_secret(b"test-secret"),
        )
        .unwrap()
    }

    async fn status_of(
        state: &AppState,
        method: &str,
        uri: &str,
        token: &str,
        body: &str,
    ) -> StatusCode {
        let req = Request::builder()
            .method(method)
            .uri(uri)
            .header("Authorization", format!("Bearer {token}"))
            .header("Content-Type", "application/json")
            .body(Body::from(body.to_string()))
            .unwrap();
        crate::api::routes()
            .with_state(state.clone())
            .oneshot(req)
            .await
            .unwrap()
            .status()
    }

    /// Every latency route refuses a signed-in non-admin with 403 (not 401,
    /// which would log them out), and lets an admin in.
    #[tokio::test]
    async fn latency_routes_are_admin_only() {
        let (state, pool) = test_state().await;
        sqlx::query(
            "INSERT INTO users (id, username, password, admin, all_device_groups, balance, max_rules, traffic_used, traffic_limit, banned) \
             VALUES (2, 'bob', 'x', 0, 1, '0', 5, 0, 0, 0)",
        )
        .execute(&pool)
        .await
        .unwrap();
        let body = r#"{"interval_secs":10,"targets":[]}"#;
        let bob = sign_token(2);
        for (method, uri) in [
            ("GET", "/admin/latency/config"),
            ("PUT", "/admin/latency/config"),
            ("GET", "/admin/nodes/n1/latency"),
        ] {
            assert_eq!(
                status_of(&state, method, uri, &bob, body).await,
                StatusCode::FORBIDDEN,
                "{method} {uri}"
            );
        }
        let root = sign_token(1);
        assert_eq!(
            status_of(&state, "GET", "/admin/latency/config", &root, "").await,
            StatusCode::OK
        );
        assert_eq!(
            status_of(&state, "PUT", "/admin/latency/config", &root, body).await,
            StatusCode::OK
        );
        // A bad token is a 401, as everywhere else.
        assert_eq!(
            status_of(&state, "GET", "/admin/latency/config", "junk", "").await,
            StatusCode::UNAUTHORIZED
        );
    }

    #[tokio::test]
    async fn an_empty_panel_has_no_targets_and_the_default_interval() {
        let (state, _pool) = test_state().await;
        let Json(resp) = get_latency_config(admin(), State(state.clone())).await;
        let cfg = resp.data.unwrap();
        assert!(cfg.targets.is_empty());
        assert_eq!(cfg.interval_secs, 10);
        // and no node is handed a plan
        let plan = crate::service::latency::current_probe_plan(state.db.as_ref())
            .await
            .unwrap();
        assert!(plan.is_none());
    }

    /// The server validates what the page also checks; a refused save changes
    /// nothing and tells nobody.
    #[tokio::test]
    async fn an_invalid_save_is_refused_without_a_broadcast_or_a_write() {
        let (state, _pool) = test_state().await;
        let n = broadcasts_during(&state, async {
            let Json(r) = put_latency_config(
                admin(),
                State(state.clone()),
                put(10, vec![target("bad", "not a host", Some(80))]),
            )
            .await;
            assert_eq!(r.code, 400, "{}", r.message);
            let Json(r) = put_latency_config(
                admin(),
                State(state.clone()),
                put(3, vec![target("ok", "192.0.2.1", Some(80))]),
            )
            .await;
            assert_eq!(r.code, 400);
            let ghost = TargetRequest {
                id: Some(77),
                ..target("ghost", "192.0.2.1", Some(80))
            };
            let Json(r) =
                put_latency_config(admin(), State(state.clone()), put(10, vec![ghost])).await;
            assert_eq!(r.code, 409);
        })
        .await;
        assert_eq!(n, 0);
        assert!(state.db.list_latency_targets().await.unwrap().is_empty());
    }

    /// Nodes are told to re-fetch exactly when what they probe changes — not
    /// for a rename or a re-save of the same list — and every save is audited.
    #[tokio::test]
    async fn saving_broadcasts_only_when_the_probe_plan_changes() {
        let (state, _pool) = test_state().await;
        let save = |interval: i64, targets: Vec<TargetRequest>| {
            let state = state.clone();
            async move {
                let Json(r) =
                    put_latency_config(admin(), State(state), put(interval, targets)).await;
                assert_eq!(r.code, 0, "{}", r.message);
                r.data.unwrap()
            }
        };

        // first save: two new targets
        let mut view = None;
        let n = broadcasts_during(&state, async {
            view = Some(
                save(
                    10,
                    vec![
                        target("电信", "192.0.2.1", Some(443)),
                        target("联通", "probe.example.com", None),
                    ],
                )
                .await,
            );
        })
        .await;
        assert_eq!(n, 1);
        let view = view.unwrap();
        assert_eq!(view.targets.len(), 2);
        assert_eq!(view.targets[0].sort_order, 0);

        // the same list again: nothing to tell the nodes
        let n = broadcasts_during(&state, async {
            save(10, as_requests(&view)).await;
        })
        .await;
        assert_eq!(n, 0, "an unchanged save must not broadcast");

        // renaming, re-carrier-ing and re-ordering change nothing a node probes
        let mut edited = as_requests(&view);
        edited[0].name = "电信-北京".into();
        edited[1].carrier = "unicom".into();
        edited.reverse();
        let n = broadcasts_during(&state, async {
            save(10, edited.clone()).await;
        })
        .await;
        assert_eq!(n, 0, "rename / carrier / order are panel-side only");
        let held = state.db.list_latency_targets().await.unwrap();
        assert_eq!(held[0].name, "联通", "the new order is stored");

        // a different interval does
        let n = broadcasts_during(&state, async {
            save(30, edited.clone()).await;
        })
        .await;
        assert_eq!(n, 1);

        // so does a different host
        edited[0].host = "192.0.2.99".into();
        edited[0].kind = "icmp".into();
        edited[0].port = None;
        let n = broadcasts_during(&state, async {
            save(30, edited.clone()).await;
        })
        .await;
        assert_eq!(n, 1);

        // emptying the list tells the nodes to stop
        let n = broadcasts_during(&state, async {
            let view = save(30, vec![]).await;
            assert!(view.targets.is_empty());
        })
        .await;
        assert_eq!(n, 1);
        assert!(
            crate::service::latency::current_probe_plan(state.db.as_ref())
                .await
                .unwrap()
                .is_none()
        );

        // every successful save is on the audit trail
        let audited = state
            .db
            .count_audit_log(Some("update_latency_config"))
            .await
            .unwrap();
        assert_eq!(audited, 6);
    }

    async fn add_status(
        state: &AppState,
        group: i64,
        node: &str,
        version: &str,
        unsupported: &[i64],
    ) {
        let status = serde_json::json!({
            "node_id": node,
            "node_version": version,
            "last_seen": chrono::Utc::now().to_rfc3339(),
            "latency_unsupported": unsupported,
        });
        state
            .db
            .set(&format!("node_status:{group}:{node}"), &status.to_string())
            .await
            .unwrap();
    }

    fn minute_now(back_minutes: i64) -> i64 {
        let now = chrono::Utc::now().timestamp();
        now - now % 60 - back_minutes * 60
    }

    fn row(target_id: i64, minute_ts: i64, lost: i64, avg: f64) -> LatencyMinuteRow {
        let answered = 6 - lost;
        LatencyMinuteRow {
            target_id,
            minute_ts,
            sent: 6,
            lost,
            rtt_sum_ms: avg * answered as f64,
            rtt_min_ms: (answered > 0).then_some(avg - 1.0),
            rtt_max_ms: (answered > 0).then_some(avg + 5.0),
        }
    }

    async fn query(
        state: &AppState,
        node: &str,
        range: &str,
        granularity: &str,
    ) -> ApiResponse<NodeLatencyResponse> {
        let Json(r) = get_node_latency(
            admin(),
            State(state.clone()),
            Path(node.to_string()),
            Query(NodeLatencyQuery {
                range: range.into(),
                granularity: granularity.into(),
            }),
        )
        .await;
        r
    }

    #[tokio::test]
    async fn a_node_series_has_one_line_per_target_and_a_summary() {
        let (state, _pool) = test_state().await;
        let Json(saved) = put_latency_config(
            admin(),
            State(state.clone()),
            put(
                10,
                vec![
                    target("电信", "192.0.2.1", Some(443)),
                    target("联通", "192.0.2.2", Some(443)),
                    target("移动", "192.0.2.3", None),
                ],
            ),
        )
        .await;
        let ids: Vec<i64> = saved.data.unwrap().targets.iter().map(|t| t.id).collect();
        add_status(&state, 5, "n1", "1.2.12", &[ids[2]]).await;
        state
            .db
            .record_latency_minutes(
                "n1",
                &[
                    row(ids[0], minute_now(3), 0, 10.0),
                    row(ids[0], minute_now(2), 0, 20.0),
                    row(ids[0], minute_now(1), 6, 0.0),
                    row(ids[1], minute_now(2), 3, 30.0),
                ],
            )
            .await
            .unwrap();

        let r = query(&state, "n1", "1h", "auto").await;
        assert_eq!(r.code, 0, "{}", r.message);
        let d = r.data.unwrap();
        assert!(d.supported);
        assert_eq!(d.granularity, "1m");
        assert_eq!(d.series.len(), 3, "one series per configured target");
        assert_eq!(d.series[0].name, "电信");
        assert_eq!(d.series[0].points.len(), 3);
        assert_eq!(
            d.series[0].points[2].avg, None,
            "an all-lost minute has no mean"
        );
        assert_eq!(d.series[0].points[2].loss, 100.0);
        assert_eq!(d.summary[0].avg, Some(15.0));
        assert_eq!(d.summary[0].loss.map(|l| (l * 10.0).round()), Some(333.0));
        assert_eq!(
            d.summary[0].latest, None,
            "the newest minute lost everything"
        );
        assert_eq!(d.summary[1].loss, Some(50.0));
        assert!(d.series[2].points.is_empty());
        assert_eq!(d.unsupported_targets, vec![ids[2]]);

        // 24h reads the 5-minute view
        let r = query(&state, "n1", "24h", "auto").await;
        assert_eq!(r.data.as_ref().unwrap().granularity, "5m");
        // 30d reads the hourly table
        let r = query(&state, "n1", "30d", "auto").await;
        let d = r.data.unwrap();
        assert_eq!(d.granularity, "1h");
        assert!(!d.series[0].points.is_empty());
    }

    #[tokio::test]
    async fn bad_ranges_granularities_and_nodes_are_refused() {
        let (state, _pool) = test_state().await;
        add_status(&state, 5, "n1", "1.2.12", &[]).await;
        assert_eq!(query(&state, "n1", "2h", "auto").await.code, 400);
        assert_eq!(query(&state, "n1", "1h", "10s").await.code, 400);
        // minute rows only go back 48 hours
        assert_eq!(query(&state, "n1", "7d", "1m").await.code, 400);
        assert_eq!(query(&state, "n1", "30d", "5m").await.code, 400);
        assert_eq!(query(&state, "n1", "7d", "1h").await.code, 0);
        assert_eq!(query(&state, "ghost", "1h", "auto").await.code, 404);
    }

    #[tokio::test]
    async fn an_old_node_is_reported_unsupported_without_a_query() {
        let (state, _pool) = test_state().await;
        let _ = put_latency_config(
            admin(),
            State(state.clone()),
            put(10, vec![target("电信", "192.0.2.1", Some(443))]),
        )
        .await;
        add_status(&state, 5, "old", "1.2.11", &[]).await;
        let d = query(&state, "old", "1h", "auto").await.data.unwrap();
        assert!(!d.supported);
        assert_eq!(d.node_version.as_deref(), Some("1.2.11"));
        assert!(d.series[0].points.is_empty());
    }

    /// A node's status report: its samples are screened and stored, a re-sent
    /// minute does not count twice, unsupported targets reach the status, and a
    /// node with no id has no series to put them in.
    #[tokio::test]
    async fn a_status_report_stores_screened_samples_once() {
        use relay_shared::protocol::StatusReport;
        let (state, pool) = test_state().await;
        sqlx::query(
            "INSERT INTO device_groups (id, name, group_type, token, uid) VALUES (5, 'g', 'in', 'tok-5', 1)",
        )
        .execute(&pool)
        .await
        .unwrap();
        let Json(saved) = put_latency_config(
            admin(),
            State(state.clone()),
            put(
                10,
                vec![
                    target("电信", "192.0.2.1", Some(443)),
                    target("联通", "192.0.2.2", None),
                ],
            ),
        )
        .await;
        let ids: Vec<i64> = saved.data.unwrap().targets.iter().map(|t| t.id).collect();

        let sample = |target_id: i64, minute_ts: i64, lost: u32| LatencySample {
            target_id,
            minute_ts,
            sent: 6,
            lost,
            rtt_sum_ms: 60.0 - 10.0 * f64::from(lost),
            rtt_min_ms: 9.0,
            rtt_max_ms: 11.0,
            unsupported: false,
        };
        let report = |node_id: Option<&str>, latency: Vec<LatencySample>| StatusReport {
            cpu_usage: 1.0,
            mem_usage: 1.0,
            active_connections: 0,
            uptime_secs: 1,
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
            node_id: node_id.map(str::to_string),
            process_uptime_secs: None,
            node_version: Some("1.2.12".into()),
            config_protocol_version: None,
            listener_errors: None,
            install_method: None,
            cpu_model: None,
            cpu_cores: None,
            mem_total: None,
            mem_used: None,
            disks: vec![],
            latency,
        };
        let mut headers = axum::http::HeaderMap::new();
        headers.insert("Authorization", "Bearer tok-5".parse().unwrap());
        let send = |r: StatusReport| {
            let (state, headers) = (state.clone(), headers.clone());
            async move {
                let Json(resp) =
                    crate::api::node::report_status(State(state), headers, Json(r)).await;
                assert_eq!(resp.code, 0);
            }
        };

        let (m1, m2) = (minute_now(2), minute_now(1));
        let good = vec![
            sample(ids[0], m1, 0),
            sample(ids[0], m2, 1),
            // each of these must be dropped on its own
            sample(999, m1, 0),
            sample(ids[0], m1 - 7200, 0),
            sample(ids[0], m1 + 1, 0),
            LatencySample {
                lost: 9,
                ..sample(ids[0], m2, 0)
            },
            LatencySample {
                unsupported: true,
                sent: 0,
                lost: 0,
                rtt_sum_ms: 0.0,
                rtt_min_ms: 0.0,
                rtt_max_ms: 0.0,
                ..sample(ids[1], 0, 0)
            },
        ];
        send(report(Some("n1"), good.clone())).await;
        // the node did not hear back and sends the same minutes again, twice
        send(report(Some("n1"), good.clone())).await;
        send(report(Some("n1"), good)).await;

        let stored = state
            .db
            .query_latency_buckets("n1", LatencyGranularity::Minute1, 0, i64::MAX)
            .await
            .unwrap();
        assert_eq!(stored.len(), 2, "two good minutes, however often sent");
        let hour = state
            .db
            .query_latency_buckets("n1", LatencyGranularity::Hour1, 0, i64::MAX)
            .await
            .unwrap();
        assert_eq!(
            hour.iter().map(|h| h.sent).sum::<i64>(),
            12,
            "the hour counts each minute once"
        );

        // the unsupported marker is in the status the page reads
        let d = query(&state, "n1", "1h", "auto").await.data.unwrap();
        assert_eq!(d.unsupported_targets, vec![ids[1]]);

        // a later report without the marker clears it (the node can probe now)
        send(report(Some("n1"), vec![])).await;
        let d = query(&state, "n1", "1h", "auto").await.data.unwrap();
        assert!(d.unsupported_targets.is_empty());

        // no node_id: nothing is stored
        send(report(None, vec![sample(ids[0], m1, 0)])).await;
        let all = state
            .db
            .query_latency_buckets("", LatencyGranularity::Minute1, 0, i64::MAX)
            .await
            .unwrap();
        assert!(all.is_empty());
    }

    /// The plan reaches every node — a node of an outbound group as well as an
    /// inbound one — and only exists while there are targets.
    #[tokio::test]
    async fn every_group_type_gets_the_probe_plan() {
        let (state, pool) = test_state().await;
        for (id, ty) in [(5, "in"), (6, "out")] {
            sqlx::query(
                "INSERT INTO device_groups (id, name, group_type, token, uid) VALUES (?, 'g', ?, ?, 1)",
            )
            .bind(id)
            .bind(ty)
            .bind(format!("tok-{id}"))
            .execute(&pool)
            .await
            .unwrap();
        }
        for g in [5, 6] {
            let cfg = crate::service::node_config::build_node_config(state.db.as_ref(), g)
                .await
                .unwrap();
            assert!(cfg.latency.is_none(), "no targets, no plan");
        }
        let _ = put_latency_config(
            admin(),
            State(state.clone()),
            put(15, vec![target("电信", "192.0.2.1", Some(443))]),
        )
        .await;
        for g in [5, 6] {
            let cfg = crate::service::node_config::build_node_config(state.db.as_ref(), g)
                .await
                .unwrap();
            let plan = cfg.latency.expect("plan");
            assert_eq!(plan.interval_secs, 15);
            assert_eq!(plan.targets.len(), 1);
        }
    }
}
