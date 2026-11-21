//! v1.2.21: latency monitor, admin side — each device group's target list, one
//! node's series, and the group's node-by-target comparison. Everything here is
//! `AdminOnly`: the targets and the figures are the operator's own view of
//! their machines' networks.

use super::err;
use crate::api::middleware::AdminOnly;
use crate::api::AppState;
use crate::db::repo::{LatencyGroupCount, LatencyTarget, ResourceScope};
use crate::service::latency::{self, ConfigError, LatencyConfigView, PutConfigRequest};
use axum::extract::{Path, Query, State};
use axum::Json;
use relay_shared::protocol::{node_supports_latency, ApiResponse};
use serde::{Deserialize, Serialize};

/// The group's name, or `None` when no such group exists.
async fn group_name(state: &AppState, group_id: i64) -> Result<Option<String>, ()> {
    state
        .db
        .find_name_by_id(group_id, &ResourceScope::All)
        .await
        .map_err(|e| tracing::error!("latency: group lookup failed: {}", e))
}

pub async fn get_group_latency_config(
    _admin: AdminOnly,
    State(state): State<AppState>,
    Path(group_id): Path<i64>,
) -> Json<ApiResponse<LatencyConfigView>> {
    match group_name(&state, group_id).await {
        Ok(Some(_)) => {}
        Ok(None) => return Json(err(404, "分组不存在")),
        Err(()) => return Json(err(500, "数据库错误")),
    }
    match latency::get_config(state.db.as_ref(), group_id).await {
        Ok(view) => Json(ApiResponse::success(view)),
        Err(e) => {
            tracing::error!("get_group_latency_config: db error: {}", e);
            Json(err(500, "数据库错误"))
        }
    }
}

/// Replace one group's whole configuration (see `latency::save_config`). Only
/// that group's nodes are told to re-fetch, and only when what they probe
/// changed.
pub async fn put_group_latency_config(
    admin: AdminOnly,
    State(state): State<AppState>,
    Path(group_id): Path<i64>,
    Json(req): Json<PutConfigRequest>,
) -> Json<ApiResponse<LatencyConfigView>> {
    match group_name(&state, group_id).await {
        Ok(Some(_)) => {}
        Ok(None) => return Json(err(404, "分组不存在")),
        Err(()) => return Json(err(500, "数据库错误")),
    }
    let now = chrono::Utc::now().format("%Y-%m-%d %H:%M:%S").to_string();
    match latency::save_config(state.db.as_ref(), group_id, &now, &req).await {
        Ok(saved) => {
            if saved.plan_changed {
                state
                    .node_connections
                    .send_group(group_id, r#"{"type":"config_changed"}"#)
                    .await;
            }
            // Counts, not hosts: the trail records that the monitor was
            // reconfigured, and the list itself is one click away.
            let label = crate::service::audit::group_label(&state, group_id).await;
            crate::service::audit::record(
                &state,
                Some(admin.user_id),
                "update_latency_config",
                "group",
                group_id,
                &format!(
                    "分组 {} / 探测间隔 {} 秒 / 目标 {} 个（新增 {}，删除 {}）",
                    label,
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
            tracing::error!("put_group_latency_config: db error: {}", e);
            Json(err(500, "数据库错误"))
        }
    }
}

/// Every group that has targets, with how many: what the node-status page
/// needs to decide where to show the monitor button and icon.
pub async fn get_latency_groups(
    _admin: AdminOnly,
    State(state): State<AppState>,
) -> Json<ApiResponse<Vec<LatencyGroupCount>>> {
    match state.db.count_latency_targets_by_group().await {
        Ok(rows) => Json(ApiResponse::success(rows)),
        Err(e) => {
            tracing::error!("get_latency_groups: db error: {}", e);
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
    /// The group the node's status record sits under, whose targets these are.
    pub group_id: i64,
    pub group_name: Option<String>,
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
    /// The admin switched this node's latency monitor off: the panel no longer
    /// records it, and the series above are what was stored before.
    pub monitoring_off: bool,
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

    let (group_id, status) = match latency::find_node_status(db, &node_id).await {
        Ok(Some(found)) => found,
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
        // The group's own targets: a node only ever probes (and stores) those.
        let targets = db.list_latency_targets(group_id).await?;
        let interval_secs = latency::get_interval(db, group_id).await?;
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
    let group_name = group_name(&state, group_id).await.ok().flatten();

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

    let monitoring_off = !latency::node_enabled(db, group_id, &node_id).await;
    let (series, summary) = latency::build_series(&targets, &buckets, &latest);
    Json(ApiResponse::success(NodeLatencyResponse {
        node_id,
        group_id,
        group_name,
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
        monitoring_off,
    }))
}

#[derive(Debug, Deserialize)]
pub struct GroupLatencyQuery {
    #[serde(default = "default_range")]
    pub range: String,
    #[serde(default = "default_granularity")]
    pub granularity: String,
    /// Which target the chart shows; the group's first when absent.
    #[serde(default)]
    pub target_id: Option<i64>,
}

#[derive(Debug, Serialize)]
pub struct GroupLatencyResponse {
    pub group_id: i64,
    pub group_name: String,
    pub interval_secs: u32,
    pub range: String,
    /// The chart's granularity: "1m" | "5m" | "1h".
    pub granularity: String,
    pub bucket_secs: i64,
    /// Unix seconds, the window the chart covers.
    pub since: i64,
    pub until: i64,
    /// The group's targets, in display order.
    pub targets: Vec<LatencyTarget>,
    /// The group's nodes with a status record and the monitor on, by node id.
    pub nodes: Vec<latency::GroupNode>,
    /// Per node and target, the mean round trip and loss over the whole
    /// `range` (minute rows for 1h / 6h / 24h, hourly rows for 7d / 30d). A
    /// pair with no data has no cell.
    pub matrix: Vec<latency::MatrixCell>,
    /// The target `series` is for; null when the group has none.
    pub target_id: Option<i64>,
    /// One line per node for `target_id`.
    pub series: Vec<latency::NodeSeries>,
}

pub async fn get_group_latency(
    _admin: AdminOnly,
    State(state): State<AppState>,
    Path(group_id): Path<i64>,
    Query(q): Query<GroupLatencyQuery>,
) -> Json<ApiResponse<GroupLatencyResponse>> {
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
    let name = match group_name(&state, group_id).await {
        Ok(Some(n)) => n,
        Ok(None) => return Json(err(404, "分组不存在")),
        Err(()) => return Json(err(500, "数据库错误")),
    };
    let db = state.db.as_ref();

    let load = async {
        let targets = db.list_latency_targets(group_id).await?;
        let interval_secs = latency::get_interval(db, group_id).await?;
        let nodes = latency::list_group_nodes(db, group_id, &targets).await?;
        Ok::<_, crate::db::error::DbError>((targets, interval_secs, nodes))
    };
    let (targets, interval_secs, nodes) = match load.await {
        Ok(v) => v,
        Err(e) => {
            tracing::error!("get_group_latency: db error: {}", e);
            return Json(err(500, "数据库错误"));
        }
    };

    // The chart's target: the one asked for (it must be this group's) or the
    // first.
    let selected = match q.target_id {
        Some(id) if targets.iter().any(|t| t.id == id) => Some(id),
        Some(_) => return Json(err(400, "目标不属于该分组")),
        None => targets.first().map(|t| t.id),
    };

    let now_ts = chrono::Utc::now().timestamp();
    let bucket_secs = granularity.bucket_secs();
    let since = latency::align_down(now_ts - range_secs, bucket_secs);
    let until = now_ts + 1;
    // The table reads off the range alone, so the same window gives the same
    // averages whichever chart granularity is picked: whole hours from the
    // hourly rows for 7d / 30d, minutes otherwise.
    let hourly = matches!(q.range.as_str(), "7d" | "30d");
    let matrix_since = latency::align_down(now_ts - range_secs, if hourly { 3600 } else { 60 });

    // Nodes that cannot run the monitor have nothing stored worth asking for.
    let node_ids: Vec<String> = nodes
        .iter()
        .filter(|n| n.supported)
        .map(|n| n.node_id.clone())
        .collect();
    let target_ids: Vec<i64> = targets.iter().map(|t| t.id).collect();
    let query = async {
        let totals = db
            .query_latency_totals(&node_ids, &target_ids, hourly, matrix_since, until)
            .await?;
        let buckets = match selected {
            Some(t) => {
                db.query_latency_target_series(&node_ids, t, granularity, since, until)
                    .await?
            }
            None => Vec::new(),
        };
        Ok::<_, crate::db::error::DbError>((totals, buckets))
    };
    let (totals, buckets) = match query.await {
        Ok(v) => v,
        Err(e) => {
            tracing::error!("get_group_latency: query failed: {}", e);
            return Json(err(500, "数据库错误"));
        }
    };

    Json(ApiResponse::success(GroupLatencyResponse {
        group_id,
        group_name: name,
        interval_secs,
        range: q.range,
        granularity: latency::granularity_name(granularity).to_string(),
        bucket_secs,
        since,
        until,
        matrix: latency::build_matrix(&totals),
        series: latency::build_group_series(&nodes, &buckets),
        targets,
        nodes,
        target_id: selected,
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

    /// A device group the latency config can be attached to.
    async fn seed_group(pool: &SqlitePool, id: i64, group_type: &str) {
        sqlx::query(
            "INSERT INTO device_groups (id, name, group_type, token, uid) VALUES (?, ?, ?, ?, 1)",
        )
        .bind(id)
        .bind(format!("group-{id}"))
        .bind(group_type)
        .bind(format!("tok-{id}"))
        .execute(pool)
        .await
        .unwrap();
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

    /// Save a group's configuration through the handler.
    async fn save_in(
        state: &AppState,
        group: i64,
        interval: i64,
        targets: Vec<TargetRequest>,
    ) -> ApiResponse<LatencyConfigView> {
        let Json(r) = put_group_latency_config(
            admin(),
            State(state.clone()),
            Path(group),
            put(interval, targets),
        )
        .await;
        r
    }

    /// Save and return the stored target ids.
    async fn save_ids(state: &AppState, group: i64, targets: Vec<TargetRequest>) -> Vec<i64> {
        let r = save_in(state, group, 10, targets).await;
        assert_eq!(r.code, 0, "{}", r.message);
        r.data.unwrap().targets.iter().map(|t| t.id).collect()
    }

    /// Re-submit what the server holds, as the settings drawer does.
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

    /// Count the config_changed pushes a call produces for one group, with a
    /// live connection of that group registered to receive them (same approach
    /// as the rule/group tests).
    async fn pushes_to(
        state: &AppState,
        group: i64,
        f: impl std::future::Future<Output = ()>,
    ) -> usize {
        let (_id, mut rx) = state.node_connections.register(group, None).await;
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
    /// which would log them out), and lets an admin in. The old global config
    /// route is gone.
    #[tokio::test]
    async fn latency_routes_are_admin_only() {
        let (state, pool) = test_state().await;
        seed_group(&pool, 5, "in").await;
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
            ("GET", "/admin/groups/5/latency/config"),
            ("PUT", "/admin/groups/5/latency/config"),
            ("GET", "/admin/groups/5/latency"),
            ("GET", "/admin/latency/groups"),
            ("GET", "/admin/nodes/n1/latency"),
        ] {
            assert_eq!(
                status_of(&state, method, uri, &bob, body).await,
                StatusCode::FORBIDDEN,
                "{method} {uri}"
            );
        }
        let root = sign_token(1);
        for (method, uri, b) in [
            ("GET", "/admin/groups/5/latency/config", ""),
            ("PUT", "/admin/groups/5/latency/config", body),
            ("GET", "/admin/groups/5/latency", ""),
            ("GET", "/admin/latency/groups", ""),
        ] {
            assert_eq!(
                status_of(&state, method, uri, &root, b).await,
                StatusCode::OK,
                "{method} {uri}"
            );
        }
        // The global configuration of the first version no longer exists.
        for method in ["GET", "PUT"] {
            assert_eq!(
                status_of(&state, method, "/admin/latency/config", &root, body).await,
                StatusCode::NOT_FOUND,
                "{method} /admin/latency/config"
            );
        }
        // A bad token is a 401, as everywhere else.
        assert_eq!(
            status_of(&state, "GET", "/admin/groups/5/latency/config", "junk", "").await,
            StatusCode::UNAUTHORIZED
        );
    }

    /// The per-node switch route is admin-only too (403 for a signed-in
    /// non-admin, never 401), and it flips the key for an admin.
    #[tokio::test]
    async fn the_node_switch_route_is_admin_only() {
        let (state, pool) = test_state().await;
        seed_group(&pool, 5, "in").await;
        sqlx::query(
            "INSERT INTO users (id, username, password, admin, all_device_groups, balance, max_rules, traffic_used, traffic_limit, banned) \
             VALUES (2, 'bob', 'x', 0, 1, '0', 5, 0, 0, 0)",
        )
        .execute(&pool)
        .await
        .unwrap();
        add_status(&state, 5, "n1", "1.2.12", &[]).await;
        let body = r#"{"enabled":false}"#;
        let uri = "/nodes/5/latency/n1";
        assert_eq!(
            status_of(&state, "PUT", uri, &sign_token(2), body).await,
            StatusCode::FORBIDDEN
        );
        assert!(
            state.db.get("latency_off:5:n1").await.unwrap().is_none(),
            "a refused request changes nothing"
        );
        assert_eq!(
            status_of(&state, "PUT", uri, &sign_token(1), body).await,
            StatusCode::OK
        );
        assert_eq!(
            state.db.get("latency_off:5:n1").await.unwrap().as_deref(),
            Some("1")
        );
        assert_eq!(
            status_of(&state, "PUT", uri, "junk", body).await,
            StatusCode::UNAUTHORIZED
        );
    }

    #[tokio::test]
    async fn an_empty_group_has_no_targets_and_the_default_interval() {
        let (state, pool) = test_state().await;
        seed_group(&pool, 5, "in").await;
        let Json(resp) = get_group_latency_config(admin(), State(state.clone()), Path(5)).await;
        let cfg = resp.data.unwrap();
        assert!(cfg.targets.is_empty());
        assert_eq!(cfg.interval_secs, 10);
        // and no node is handed a plan
        let plan = crate::service::latency::current_probe_plan(state.db.as_ref(), 5)
            .await
            .unwrap();
        assert!(plan.is_none());
        // a group that does not exist is a 404, for reading and for saving
        let Json(r) = get_group_latency_config(admin(), State(state.clone()), Path(99)).await;
        assert_eq!(r.code, 404);
        assert_eq!(save_in(&state, 99, 10, vec![]).await.code, 404);
        let Json(r) = get_group_latency(
            admin(),
            State(state.clone()),
            Path(99),
            Query(group_query("1h", None)),
        )
        .await;
        assert_eq!(r.code, 404);
    }

    /// The server validates what the page also checks; a refused save changes
    /// nothing and tells nobody.
    #[tokio::test]
    async fn an_invalid_save_is_refused_without_a_push_or_a_write() {
        let (state, pool) = test_state().await;
        seed_group(&pool, 5, "in").await;
        let n = pushes_to(&state, 5, async {
            let r = save_in(&state, 5, 10, vec![target("bad", "not a host", Some(80))]).await;
            assert_eq!(r.code, 400, "{}", r.message);
            let r = save_in(&state, 5, 3, vec![target("ok", "192.0.2.1", Some(80))]).await;
            assert_eq!(r.code, 400);
            let ghost = TargetRequest {
                id: Some(77),
                ..target("ghost", "192.0.2.1", Some(80))
            };
            let r = save_in(&state, 5, 10, vec![ghost]).await;
            assert_eq!(r.code, 409);
        })
        .await;
        assert_eq!(n, 0);
        assert!(state.db.list_latency_targets(5).await.unwrap().is_empty());
    }

    /// A group's nodes are told to re-fetch exactly when what they probe
    /// changes — not for a rename or a re-save of the same list — and every
    /// save is audited with the group named.
    #[tokio::test]
    async fn saving_pushes_only_when_the_probe_plan_changes() {
        let (state, pool) = test_state().await;
        seed_group(&pool, 5, "in").await;
        let save = |interval: i64, targets: Vec<TargetRequest>| {
            let state = state.clone();
            async move {
                let r = save_in(&state, 5, interval, targets).await;
                assert_eq!(r.code, 0, "{}", r.message);
                r.data.unwrap()
            }
        };

        // first save: two new targets
        let mut view = None;
        let n = pushes_to(&state, 5, async {
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
        assert_eq!(view.targets[0].group_id, 5);

        // the same list again: nothing to tell the nodes
        let n = pushes_to(&state, 5, async {
            save(10, as_requests(&view)).await;
        })
        .await;
        assert_eq!(n, 0, "an unchanged save must not push");

        // renaming, re-carrier-ing and re-ordering change nothing a node probes
        let mut edited = as_requests(&view);
        edited[0].name = "电信-北京".into();
        edited[1].carrier = "unicom".into();
        edited.reverse();
        let n = pushes_to(&state, 5, async {
            save(10, edited.clone()).await;
        })
        .await;
        assert_eq!(n, 0, "rename / carrier / order are panel-side only");
        let held = state.db.list_latency_targets(5).await.unwrap();
        assert_eq!(held[0].name, "联通", "the new order is stored");

        // a different interval does
        let n = pushes_to(&state, 5, async {
            save(30, edited.clone()).await;
        })
        .await;
        assert_eq!(n, 1);

        // so does a different host
        edited[0].host = "192.0.2.99".into();
        edited[0].kind = "icmp".into();
        edited[0].port = None;
        let n = pushes_to(&state, 5, async {
            save(30, edited.clone()).await;
        })
        .await;
        assert_eq!(n, 1);

        // emptying the list tells the nodes to stop
        let n = pushes_to(&state, 5, async {
            let view = save(30, vec![]).await;
            assert!(view.targets.is_empty());
        })
        .await;
        assert_eq!(n, 1);
        assert!(
            crate::service::latency::current_probe_plan(state.db.as_ref(), 5)
                .await
                .unwrap()
                .is_none()
        );

        // every successful save is on the audit trail, and says which group
        let audited = state
            .db
            .count_audit_log(Some("update_latency_config"))
            .await
            .unwrap();
        assert_eq!(audited, 6);
        let (target_type, detail): (String, String) = sqlx::query_as(
            "SELECT target_type, detail FROM audit_log WHERE action = 'update_latency_config' ORDER BY id LIMIT 1",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(target_type, "group");
        assert!(detail.contains("group-5"), "{detail}");
    }

    /// Groups are independent: saving one leaves the other's targets and
    /// interval alone and tells only its own nodes, a target id of another group
    /// is refused, and the node-status page's list counts per group.
    #[tokio::test]
    async fn groups_configure_their_targets_independently() {
        let (state, pool) = test_state().await;
        seed_group(&pool, 5, "in").await;
        seed_group(&pool, 6, "out").await;

        let a = save_ids(&state, 5, vec![target("A", "192.0.2.1", Some(443))]).await;
        let mut other_pushes = 0;
        let n = pushes_to(&state, 5, async {
            other_pushes = pushes_to(&state, 6, async {
                let r = save_in(
                    &state,
                    6,
                    30,
                    vec![
                        target("B1", "192.0.2.2", Some(443)),
                        target("B2", "192.0.2.3", None),
                    ],
                )
                .await;
                assert_eq!(r.code, 0);
            })
            .await;
        })
        .await;
        assert_eq!(n, 0, "group 5's nodes are not told about group 6");
        assert_eq!(other_pushes, 1);

        // each group sees only its own, with its own interval
        let c5 = get_group_latency_config(admin(), State(state.clone()), Path(5))
            .await
            .0
            .data
            .unwrap();
        let c6 = get_group_latency_config(admin(), State(state.clone()), Path(6))
            .await
            .0
            .data
            .unwrap();
        assert_eq!((c5.targets.len(), c5.interval_secs), (1, 10));
        assert_eq!((c6.targets.len(), c6.interval_secs), (2, 30));
        // the plan a node of each group is handed is that group's alone
        let plan5 = crate::service::latency::current_probe_plan(state.db.as_ref(), 5)
            .await
            .unwrap()
            .unwrap();
        let plan6 = crate::service::latency::current_probe_plan(state.db.as_ref(), 6)
            .await
            .unwrap()
            .unwrap();
        assert_eq!((plan5.interval_secs, plan5.targets.len()), (10, 1));
        assert_eq!((plan6.interval_secs, plan6.targets.len()), (30, 2));
        assert_eq!(plan5.targets[0].id, a[0]);

        // group 6 cannot adopt (or delete) group 5's target by naming its id
        let mut reqs = as_requests(&c6);
        reqs.push(TargetRequest {
            id: Some(a[0]),
            ..target("steal", "192.0.2.9", Some(80))
        });
        assert_eq!(save_in(&state, 6, 30, reqs).await.code, 409);
        assert_eq!(state.db.list_latency_targets(5).await.unwrap().len(), 1);
        assert_eq!(state.db.list_latency_targets(6).await.unwrap().len(), 2);

        // the node-status page asks which groups have targets, and how many
        let Json(r) = get_latency_groups(admin(), State(state.clone())).await;
        let counts: Vec<(i64, i64)> = r
            .data
            .unwrap()
            .iter()
            .map(|c| (c.group_id, c.targets))
            .collect();
        assert_eq!(counts, vec![(5, 1), (6, 2)]);
        // a group emptied drops out of that list
        save_in(&state, 5, 10, vec![]).await;
        let Json(r) = get_latency_groups(admin(), State(state.clone())).await;
        let counts: Vec<(i64, i64)> = r
            .data
            .unwrap()
            .iter()
            .map(|c| (c.group_id, c.targets))
            .collect();
        assert_eq!(counts, vec![(6, 2)]);
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
            "public_ipv4": "192.0.2.10",
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
            retrans: None,
            retrans_sum_ms: None,
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
    async fn a_node_series_has_one_line_per_target_of_its_group_and_a_summary() {
        let (state, pool) = test_state().await;
        seed_group(&pool, 5, "in").await;
        seed_group(&pool, 6, "in").await;
        let ids = save_ids(
            &state,
            5,
            vec![
                target("电信", "192.0.2.1", Some(443)),
                target("联通", "192.0.2.2", Some(443)),
                target("移动", "192.0.2.3", None),
            ],
        )
        .await;
        // a target of another group must not show up on this node's page
        save_ids(&state, 6, vec![target("其他", "192.0.2.50", Some(443))]).await;
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
        assert_eq!((d.group_id, d.group_name.as_deref()), (5, Some("group-5")));
        assert_eq!(d.granularity, "1m");
        assert_eq!(
            d.series.len(),
            3,
            "one series per target of the node's group"
        );
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
        let (state, pool) = test_state().await;
        seed_group(&pool, 5, "in").await;
        save_ids(&state, 5, vec![target("电信", "192.0.2.1", Some(443))]).await;
        add_status(&state, 5, "old", "1.2.11", &[]).await;
        let d = query(&state, "old", "1h", "auto").await.data.unwrap();
        assert!(!d.supported);
        assert_eq!(d.node_version.as_deref(), Some("1.2.11"));
        assert!(d.series[0].points.is_empty());
    }

    /// A node's status report: its samples are screened and stored, a re-sent
    /// minute does not count twice, unsupported targets reach the status, a
    /// node with no id has no series to put them in, and a target of another
    /// group is dropped like an unknown one.
    #[tokio::test]
    async fn a_status_report_stores_screened_samples_of_its_own_group_once() {
        use relay_shared::protocol::StatusReport;
        let (state, pool) = test_state().await;
        seed_group(&pool, 5, "in").await;
        seed_group(&pool, 6, "in").await;
        let ids = save_ids(
            &state,
            5,
            vec![
                target("电信", "192.0.2.1", Some(443)),
                target("联通", "192.0.2.2", None),
            ],
        )
        .await;
        let foreign = save_ids(&state, 6, vec![target("别组", "192.0.2.60", Some(443))]).await;

        let sample = |target_id: i64, minute_ts: i64, lost: u32| LatencySample {
            target_id,
            minute_ts,
            sent: 6,
            lost,
            rtt_sum_ms: 60.0 - 10.0 * f64::from(lost),
            rtt_min_ms: 9.0,
            rtt_max_ms: 11.0,
            retrans: None,
            retrans_sum_ms: None,
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
            // a real target, but of group 6: not this node's to measure
            sample(foreign[0], m1, 0),
            LatencySample {
                unsupported: true,
                sent: 0,
                lost: 0,
                rtt_sum_ms: 0.0,
                rtt_min_ms: 0.0,
                rtt_max_ms: 0.0,
                ..sample(foreign[0], 0, 0)
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
        assert!(
            stored.iter().all(|b| b.target_id == ids[0]),
            "nothing stored for another group's target"
        );
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

        // the unsupported marker is in the status the page reads, for this
        // group's target only
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

    fn bare_report(
        node_id: &str,
        latency: Vec<LatencySample>,
    ) -> relay_shared::protocol::StatusReport {
        relay_shared::protocol::StatusReport {
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
            node_id: Some(node_id.to_string()),
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
        }
    }

    async fn switch(state: &AppState, group: i64, node: &str, enabled: bool) {
        let Json(r) = crate::api::stats::set_node_latency(
            admin(),
            State(state.clone()),
            Path((group, node.to_string())),
            Json(crate::api::stats::SetLatencyRequest { enabled }),
        )
        .await;
        assert_eq!(r.code, 0, "{}", r.message);
    }

    /// A node switched off is ignored for latency from its next report: its
    /// samples are dropped and its "unsupported" list is not written, while
    /// the rest of the report is handled and a node that is on is unaffected.
    #[tokio::test]
    async fn a_report_from_a_switched_off_node_stores_no_latency() {
        let (state, pool) = test_state().await;
        seed_group(&pool, 5, "in").await;
        let ids = save_ids(
            &state,
            5,
            vec![
                target("电信", "192.0.2.1", Some(443)),
                target("联通", "192.0.2.2", None),
            ],
        )
        .await;
        add_status(&state, 5, "n1", "1.2.12", &[]).await;
        add_status(&state, 5, "n2", "1.2.12", &[]).await;
        let mut headers = axum::http::HeaderMap::new();
        headers.insert("Authorization", "Bearer tok-5".parse().unwrap());
        let send = |r: relay_shared::protocol::StatusReport| {
            let (state, headers) = (state.clone(), headers.clone());
            async move {
                let Json(resp) =
                    crate::api::node::report_status(State(state), headers, Json(r)).await;
                assert_eq!(resp.code, 0);
            }
        };
        let m = minute_now(2);
        let samples = || {
            vec![
                LatencySample {
                    target_id: ids[0],
                    minute_ts: m,
                    sent: 6,
                    lost: 0,
                    rtt_sum_ms: 60.0,
                    rtt_min_ms: 9.0,
                    rtt_max_ms: 11.0,
                    retrans: None,
                    retrans_sum_ms: None,
                    unsupported: false,
                },
                LatencySample {
                    target_id: ids[1],
                    minute_ts: 0,
                    sent: 0,
                    lost: 0,
                    rtt_sum_ms: 0.0,
                    rtt_min_ms: 0.0,
                    rtt_max_ms: 0.0,
                    retrans: None,
                    retrans_sum_ms: None,
                    unsupported: true,
                },
            ]
        };
        let stored = |node: &'static str| {
            let state = state.clone();
            async move {
                state
                    .db
                    .query_latency_buckets(node, LatencyGranularity::Minute1, 0, i64::MAX)
                    .await
                    .unwrap()
                    .len()
            }
        };

        // on by default: both nodes record, and the unsupported marker lands
        send(bare_report("n1", samples())).await;
        send(bare_report("n2", samples())).await;
        assert_eq!((stored("n1").await, stored("n2").await), (1, 1));
        let d = query(&state, "n1", "1h", "auto").await.data.unwrap();
        assert_eq!(d.unsupported_targets, vec![ids[1]]);
        assert!(!d.monitoring_off);

        // n1 off: nothing new is stored for it and its marker clears (as if the
        // report carried no latency); n2 is untouched
        switch(&state, 5, "n1", false).await;
        let m2 = minute_now(1);
        let mut s = samples();
        s[0].minute_ts = m2;
        send(bare_report("n1", s.clone())).await;
        send(bare_report("n2", s)).await;
        assert_eq!(stored("n1").await, 1, "n1's new minute was dropped");
        assert_eq!(stored("n2").await, 2, "n2 still records");
        let d = query(&state, "n1", "1h", "auto").await.data.unwrap();
        assert!(d.unsupported_targets.is_empty());
        // the rest of the report was handled: the status row is fresh
        let status: serde_json::Value =
            serde_json::from_str(&state.db.get("node_status:5:n1").await.unwrap().unwrap())
                .unwrap();
        assert_eq!(status["cpu"], 1.0);

        // back on: recording resumes from the next report
        switch(&state, 5, "n1", true).await;
        assert!(state.db.get("latency_off:5:n1").await.unwrap().is_none());
        let mut s = samples();
        s[0].minute_ts = m2;
        send(bare_report("n1", s)).await;
        assert_eq!(stored("n1").await, 2);
    }

    /// A node the panel has no status record for (under that group) cannot be
    /// switched, and nothing is stored.
    #[tokio::test]
    async fn the_switch_is_refused_for_an_unknown_node_and_leaves_no_key() {
        let (state, pool) = test_state().await;
        seed_group(&pool, 5, "in").await;
        add_status(&state, 5, "n1", "1.2.12", &[]).await;
        let Json(r) = crate::api::stats::set_node_latency(
            admin(),
            State(state.clone()),
            Path((5, "ghost".to_string())),
            Json(crate::api::stats::SetLatencyRequest { enabled: false }),
        )
        .await;
        assert_eq!(r.code, 404);
        let Json(r) = crate::api::stats::set_node_latency(
            admin(),
            State(state.clone()),
            Path((6, "n1".to_string())),
            Json(crate::api::stats::SetLatencyRequest { enabled: false }),
        )
        .await;
        assert_eq!(r.code, 404, "n1 is a node of group 5, not 6");
        assert!(state
            .db
            .scan_prefix("latency_off:")
            .await
            .unwrap()
            .is_empty());
    }

    /// The plan reaches every node of a group that has targets — an outbound
    /// group's as well as an inbound one's — is that group's own, and does not
    /// exist for a group without targets.
    #[tokio::test]
    async fn only_a_group_with_targets_gets_its_own_probe_plan() {
        let (state, pool) = test_state().await;
        for (id, ty) in [(5, "in"), (6, "out"), (7, "in")] {
            seed_group(&pool, id, ty).await;
        }
        for g in [5, 6, 7] {
            let cfg = crate::service::node_config::build_node_config(state.db.as_ref(), g)
                .await
                .unwrap();
            assert!(cfg.latency.is_none(), "no targets, no plan");
        }
        let a = save_ids(&state, 5, vec![target("电信", "192.0.2.1", Some(443))]).await;
        assert_eq!(
            save_in(&state, 6, 15, vec![target("联通", "192.0.2.2", None)])
                .await
                .code,
            0
        );
        for (g, interval) in [(5, 10), (6, 15)] {
            let cfg = crate::service::node_config::build_node_config(state.db.as_ref(), g)
                .await
                .unwrap();
            let plan = cfg.latency.expect("plan");
            assert_eq!(plan.interval_secs, interval);
            assert_eq!(plan.targets.len(), 1);
        }
        let cfg5 = crate::service::node_config::build_node_config(state.db.as_ref(), 5)
            .await
            .unwrap();
        assert_eq!(cfg5.latency.unwrap().targets[0].id, a[0]);
        let cfg7 = crate::service::node_config::build_node_config(state.db.as_ref(), 7)
            .await
            .unwrap();
        assert!(cfg7.latency.is_none(), "group 7 has no targets of its own");
    }

    /// Deleting a device group takes its targets, their minute and hourly
    /// history and its interval with it; another group's are untouched.
    #[tokio::test]
    async fn deleting_a_group_removes_its_latency_data() {
        let (state, pool) = test_state().await;
        seed_group(&pool, 5, "in").await;
        seed_group(&pool, 6, "in").await;
        let a = save_ids(&state, 5, vec![target("A", "192.0.2.1", Some(443))]).await;
        let b = save_ids(&state, 6, vec![target("B", "192.0.2.2", Some(443))]).await;
        state
            .db
            .record_latency_minutes(
                "n1",
                &[
                    row(a[0], minute_now(2), 0, 10.0),
                    row(b[0], minute_now(2), 0, 20.0),
                ],
            )
            .await
            .unwrap();
        assert!(state
            .db
            .get(&crate::service::latency::interval_key(5))
            .await
            .unwrap()
            .is_some());

        let Json(r) = crate::api::admin::delete_group(admin(), State(state.clone()), Path(5)).await;
        assert_eq!(r.code, 0, "{}", r.message);

        assert!(state.db.list_latency_targets(5).await.unwrap().is_empty());
        assert_eq!(state.db.list_latency_targets(6).await.unwrap().len(), 1);
        for g in [LatencyGranularity::Minute1, LatencyGranularity::Hour1] {
            let left = state
                .db
                .query_latency_buckets("n1", g, 0, i64::MAX)
                .await
                .unwrap();
            assert_eq!(
                left.iter().map(|x| x.target_id).collect::<Vec<_>>(),
                vec![b[0]],
                "only the other group's history is left"
            );
        }
        assert!(state
            .db
            .get(&crate::service::latency::interval_key(5))
            .await
            .unwrap()
            .is_none());
        assert!(state
            .db
            .get(&crate::service::latency::interval_key(6))
            .await
            .unwrap()
            .is_some());
    }

    fn group_query(range: &str, target_id: Option<i64>) -> GroupLatencyQuery {
        GroupLatencyQuery {
            range: range.into(),
            granularity: "auto".into(),
            target_id,
        }
    }

    async fn group_view(
        state: &AppState,
        group: i64,
        range: &str,
        target_id: Option<i64>,
    ) -> ApiResponse<GroupLatencyResponse> {
        let Json(r) = get_group_latency(
            admin(),
            State(state.clone()),
            Path(group),
            Query(group_query(range, target_id)),
        )
        .await;
        r
    }

    /// The comparison: every node of the group against every target, averaged
    /// over the range, with one chart line per node for the chosen target.
    #[tokio::test]
    async fn a_group_view_compares_its_nodes_across_its_targets() {
        let (state, pool) = test_state().await;
        seed_group(&pool, 5, "in").await;
        seed_group(&pool, 6, "in").await;
        let ids = save_ids(
            &state,
            5,
            vec![
                target("电信", "192.0.2.1", Some(443)),
                target("联通", "192.0.2.2", Some(443)),
            ],
        )
        .await;
        let other = save_ids(&state, 6, vec![target("别组", "192.0.2.60", Some(443))]).await;
        add_status(&state, 5, "n1", "1.2.12", &[]).await;
        add_status(&state, 5, "n2", "1.2.12", &[ids[1]]).await;
        add_status(&state, 5, "old", "1.2.11", &[]).await;
        add_status(&state, 6, "elsewhere", "1.2.12", &[]).await;
        state.db.set("node_label:5:n1", "香港-1").await.unwrap();
        state
            .db
            .record_latency_minutes(
                "n1",
                &[
                    row(ids[0], minute_now(3), 0, 10.0),
                    row(ids[0], minute_now(2), 0, 20.0),
                    row(ids[0], minute_now(1), 6, 0.0),
                ],
            )
            .await
            .unwrap();
        state
            .db
            .record_latency_minutes("n2", &[row(ids[0], minute_now(2), 3, 30.0)])
            .await
            .unwrap();
        // the other group's node has data too, which must not leak in
        state
            .db
            .record_latency_minutes("elsewhere", &[row(other[0], minute_now(2), 0, 99.0)])
            .await
            .unwrap();

        let r = group_view(&state, 5, "1h", None).await;
        assert_eq!(r.code, 0, "{}", r.message);
        let d = r.data.unwrap();
        assert_eq!((d.group_id, d.group_name.as_str()), (5, "group-5"));
        assert_eq!(d.granularity, "1m");
        assert_eq!(d.targets.len(), 2);
        assert_eq!(d.target_id, Some(ids[0]), "defaults to the first target");

        let node_ids: Vec<&str> = d.nodes.iter().map(|n| n.node_id.as_str()).collect();
        assert_eq!(node_ids, vec!["n1", "n2", "old"]);
        assert_eq!(d.nodes[0].label.as_deref(), Some("香港-1"));
        assert_eq!(d.nodes[0].public_ip.as_deref(), Some("192.0.2.10"));
        assert!(d.nodes[0].supported);
        assert!(!d.nodes[2].supported, "1.2.11 predates the monitor");
        assert_eq!(d.nodes[1].unsupported_targets, vec![ids[1]]);

        // n1 x target0: 18 answered of 18 sent minus one all-lost minute
        let cell = |node: &str, t: i64| {
            d.matrix
                .iter()
                .find(|c| c.node_id == node && c.target_id == t)
        };
        let c = cell("n1", ids[0]).expect("n1 measured target 0");
        assert_eq!(c.avg, Some(15.0), "mean of the answered probes only");
        assert_eq!(c.sent, 18);
        assert_eq!((c.loss * 10.0).round(), 333.0, "6 of 18 lost");
        let c = cell("n2", ids[0]).expect("n2 measured target 0");
        assert_eq!(c.avg, Some(30.0));
        assert_eq!(c.loss, 50.0);
        assert!(cell("n1", ids[1]).is_none(), "no data, no cell");
        assert!(cell("old", ids[0]).is_none());
        assert!(
            d.matrix.iter().all(|c| c.node_id != "elsewhere"),
            "another group's node is not in the table"
        );
        assert!(d.matrix.iter().all(|c| c.target_id != other[0]));

        // the chart: one line per node, for the first target
        assert_eq!(d.series.len(), 3);
        assert_eq!(d.series[0].node_id, "n1");
        assert_eq!(d.series[0].points.len(), 3);
        assert_eq!(d.series[0].points[2].avg, None);
        assert_eq!(d.series[0].points[2].loss, 100.0);
        assert_eq!(d.series[1].points.len(), 1);
        assert!(d.series[2].points.is_empty());

        // another target for the chart, another group's target refused
        let d = group_view(&state, 5, "1h", Some(ids[1]))
            .await
            .data
            .unwrap();
        assert_eq!(d.target_id, Some(ids[1]));
        assert!(d.series.iter().all(|s| s.points.is_empty()));
        assert_eq!(group_view(&state, 5, "1h", Some(other[0])).await.code, 400);

        // 30 days reads the hourly rows and averages the same
        let d = group_view(&state, 5, "30d", None).await.data.unwrap();
        assert_eq!(d.granularity, "1h");
        let c = d
            .matrix
            .iter()
            .find(|c| c.node_id == "n1" && c.target_id == ids[0])
            .unwrap();
        assert_eq!(c.avg, Some(15.0));
        assert_eq!(c.sent, 18);
        assert!(!d.series[0].points.is_empty());

        // a bad range is refused
        assert_eq!(group_view(&state, 5, "2h", None).await.code, 400);
    }

    /// A node switched off is left out of the comparison entirely (rows,
    /// chart lines, node list) but its history is kept, and the node page
    /// still answers for it with the flag set.
    #[tokio::test]
    async fn a_switched_off_node_leaves_the_group_view_but_keeps_its_history() {
        let (state, pool) = test_state().await;
        seed_group(&pool, 5, "in").await;
        let ids = save_ids(&state, 5, vec![target("电信", "192.0.2.1", Some(443))]).await;
        add_status(&state, 5, "n1", "1.2.12", &[]).await;
        add_status(&state, 5, "n2", "1.2.12", &[]).await;
        for node in ["n1", "n2"] {
            state
                .db
                .record_latency_minutes(node, &[row(ids[0], minute_now(2), 0, 10.0)])
                .await
                .unwrap();
        }
        let d = group_view(&state, 5, "1h", None).await.data.unwrap();
        assert_eq!(d.nodes.len(), 2);

        switch(&state, 5, "n2", false).await;
        let d = group_view(&state, 5, "1h", None).await.data.unwrap();
        let node_ids: Vec<&str> = d.nodes.iter().map(|n| n.node_id.as_str()).collect();
        assert_eq!(node_ids, vec!["n1"]);
        assert!(d.matrix.iter().all(|c| c.node_id == "n1"));
        assert_eq!(d.series.len(), 1);
        assert_eq!(d.series[0].node_id, "n1");

        // the node page still has the stored history, with the flag
        let n2 = query(&state, "n2", "1h", "auto").await.data.unwrap();
        assert!(n2.monitoring_off);
        assert_eq!(n2.series[0].points.len(), 1, "history is kept");
        let n1 = query(&state, "n1", "1h", "auto").await.data.unwrap();
        assert!(!n1.monitoring_off);

        // switching back on shows it again
        switch(&state, 5, "n2", true).await;
        let d = group_view(&state, 5, "1h", None).await.data.unwrap();
        assert_eq!(d.nodes.len(), 2);
        assert!(
            !query(&state, "n2", "1h", "auto")
                .await
                .data
                .unwrap()
                .monitoring_off
        );
    }

    /// Retransmit figures reach every response next to the loss: the average
    /// leaves the retransmits out, the rate is over the answered probes of the
    /// reporting rows, and a node that never reports them gets null, not 0.
    #[tokio::test]
    async fn responses_carry_the_retransmit_rate_beside_the_loss() {
        let (state, pool) = test_state().await;
        seed_group(&pool, 5, "in").await;
        let ids = save_ids(&state, 5, vec![target("电信", "192.0.2.1", Some(443))]).await;
        add_status(&state, 5, "n1", "1.2.12", &[]).await;
        add_status(&state, 5, "n2", "1.2.12", &[]).await;
        // n1: a minute with one retransmit of 1200 ms among six answers
        // (five at 20 ms), then an old-style minute of 40 ms answers
        let with_retrans = LatencyMinuteRow {
            target_id: ids[0],
            minute_ts: minute_now(2),
            sent: 6,
            lost: 0,
            rtt_sum_ms: 5.0 * 20.0 + 1200.0,
            rtt_min_ms: Some(20.0),
            rtt_max_ms: Some(1200.0),
            retrans: Some(1),
            retrans_sum_ms: Some(1200.0),
        };
        state
            .db
            .record_latency_minutes("n1", &[with_retrans, row(ids[0], minute_now(1), 0, 40.0)])
            .await
            .unwrap();
        state
            .db
            .record_latency_minutes("n2", &[row(ids[0], minute_now(1), 0, 40.0)])
            .await
            .unwrap();

        let d = group_view(&state, 5, "1h", None).await.data.unwrap();
        let cell = |node: &str| d.matrix.iter().find(|c| c.node_id == node).unwrap();
        // (1300 + 240 - 1200) / (12 - 1); the rate counts the reporting minute only
        let c = cell("n1");
        assert!((c.avg.unwrap() - 340.0 / 11.0).abs() < 1e-9);
        assert!((c.retrans_rate.unwrap() - 100.0 / 6.0).abs() < 1e-9);

        let c = cell("n2");
        assert_eq!((c.avg, c.retrans_rate), (Some(40.0), None));
        let p = &d.series[0].points;
        assert_eq!(p[0].avg, Some(20.0));
        assert!((p[0].retrans_rate.unwrap() - 100.0 / 6.0).abs() < 1e-9);
        assert_eq!(p[1].retrans_rate, None);
        // and on the wire
        let json = serde_json::to_value(&d.matrix[0]).unwrap();
        assert!(json.get("retrans_rate").is_some());

        // the node page: points, the latest minute and the range summary
        let n = query(&state, "n1", "1h", "auto").await.data.unwrap();
        assert!((n.series[0].points[0].retrans_rate.unwrap() - 100.0 / 6.0).abs() < 1e-9);
        assert_eq!(
            n.summary[0].latest_retrans_rate, None,
            "newest minute is old-style"
        );
        assert!((n.summary[0].retrans_rate.unwrap() - 100.0 / 6.0).abs() < 1e-9);
        assert!((n.summary[0].avg.unwrap() - 340.0 / 11.0).abs() < 1e-9);
        assert_eq!(n.summary[0].loss, Some(0.0));
        let n2 = query(&state, "n2", "1h", "auto").await.data.unwrap();
        assert_eq!(n2.summary[0].retrans_rate, None);
    }

    /// A group without targets still answers (the page shows its empty state):
    /// nodes listed, nothing to compare, no chart target.
    #[tokio::test]
    async fn a_group_without_targets_has_nodes_and_no_comparison() {
        let (state, pool) = test_state().await;
        seed_group(&pool, 5, "in").await;
        add_status(&state, 5, "n1", "1.2.12", &[]).await;
        let d = group_view(&state, 5, "1h", None).await.data.unwrap();
        assert!(d.targets.is_empty());
        assert!(d.matrix.is_empty());
        assert_eq!(d.target_id, None);
        assert_eq!(d.nodes.len(), 1);
        assert_eq!(d.series.len(), 1);
        assert!(d.series[0].points.is_empty());
    }
}
