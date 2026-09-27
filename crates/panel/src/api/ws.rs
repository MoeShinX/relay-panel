use crate::api::node::extract_node_token;
use crate::api::AppState;
use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        State,
    },
    response::IntoResponse,
};
use futures_util::{SinkExt, StreamExt};
use relay_shared::protocol::{NodeConfigResponse, NodeLiveRate, NodeRateMessage};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{mpsc, RwLock};

/// One live connection's sender + its optional per-node identity (v0.4.14
/// X-Node-ID). `node_id` is None for an older node that didn't send the header;
/// such a connection still receives config_changed broadcasts but cannot be
/// targeted by directed diagnosis.
struct ConnEntry {
    tx: mpsc::UnboundedSender<String>,
    node_id: Option<String>,
    /// v1.2.11: the node's latest pushed NIC rate (upload, download, received
    /// at). Lives on the connection on purpose: when the socket goes away the
    /// entry goes with it, so a disconnected node can never keep showing its
    /// last live figure.
    rate: Option<(u64, u64, Instant)>,
}

/// v1.2.11: a pushed rate older than this is ignored and the page falls back
/// to the status report's figure. Three push intervals: one late frame is not
/// enough to drop a node back to the slow number, a stalled feed is.
const LIVE_RATE_MAX_AGE: Duration =
    Duration::from_secs(3 * relay_shared::protocol::LIVE_RATE_INTERVAL_SECS);
/// Per-group map of live connection senders.
type GroupConns = HashMap<u64, ConnEntry>;
/// Shared registry: group_id -> that group's live connections.
type ConnMap = Arc<RwLock<HashMap<i64, GroupConns>>>;

/// Tracks live WebSocket connections per group_id so the panel can push
/// `config_changed` notifications when an admin mutates rules or groups.
///
/// Each connection registers an mpsc sender; on disconnect it unregisters.
/// `broadcast` fans a message out to every live connection (we broadcast to
/// ALL groups on any admin mutation — correct and simple for small fleets).
#[derive(Clone, Default)]
pub struct NodeConnections {
    next_id: Arc<AtomicU64>,
    inner: ConnMap,
}

impl NodeConnections {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a new connection. Returns (conn_id, receiver) — the caller
    /// owns the receiver and forwards anything it receives to the socket.
    /// `node_id` is the v0.4.14 X-Node-ID (None for older nodes).
    pub async fn register(
        &self,
        group_id: i64,
        node_id: Option<String>,
    ) -> (u64, mpsc::UnboundedReceiver<String>) {
        let conn_id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = mpsc::unbounded_channel();
        self.inner
            .write()
            .await
            .entry(group_id)
            .or_default()
            .insert(
                conn_id,
                ConnEntry {
                    tx,
                    node_id,
                    rate: None,
                },
            );
        (conn_id, rx)
    }

    /// Remove a connection. Called when the socket task exits.
    pub async fn unregister(&self, group_id: i64, conn_id: u64) {
        let mut map = self.inner.write().await;
        if let Some(conns) = map.get_mut(&group_id) {
            conns.remove(&conn_id);
            if conns.is_empty() {
                map.remove(&group_id);
            }
        }
    }

    /// Fan a message out to every live connection across every group.
    /// Dead senders (receiver dropped) are pruned opportunistically.
    pub async fn broadcast_all(&self, msg: &str) {
        let mut map = self.inner.write().await;
        for conns in map.values_mut() {
            conns.retain(|_, e| e.tx.send(msg.to_string()).is_ok());
        }
    }

    /// Send a message to every live connection in ONE group only (not all
    /// groups like broadcast_all). Returns the number of connections the message
    /// was handed to (dead senders pruned). Does NOT close the connections.
    ///
    /// v0.4.14: directed diagnosis now uses `send_node` instead; this group-wide
    /// send is retained as general infrastructure (no current caller).
    #[allow(dead_code)]
    pub async fn send_group(&self, group_id: i64, msg: &str) -> usize {
        let mut map = self.inner.write().await;
        let Some(conns) = map.get_mut(&group_id) else {
            return 0;
        };
        let mut sent = 0usize;
        conns.retain(|_, e| {
            if e.tx.send(msg.to_string()).is_ok() {
                sent += 1;
                true
            } else {
                false
            }
        });
        if conns.is_empty() {
            map.remove(&group_id);
        }
        sent
    }

    /// v1.2.11: store a node's pushed rate on its connection. A connection
    /// without an X-Node-ID is skipped — its rate could not be matched to a row
    /// on the page anyway, and pre-0.4.14 nodes never push one.
    pub async fn record_rate(
        &self,
        group_id: i64,
        conn_id: u64,
        upload_bps: u64,
        download_bps: u64,
    ) {
        let mut map = self.inner.write().await;
        if let Some(entry) = map.get_mut(&group_id).and_then(|c| c.get_mut(&conn_id)) {
            if entry.node_id.is_some() {
                entry.rate = Some((upload_bps, download_bps, Instant::now()));
            }
        }
    }

    /// v1.2.11: every node's current live rate. Stale entries are left out so
    /// the caller falls back to the status report instead of freezing on an old
    /// number. When one node briefly has two connections (a reconnect racing the
    /// old socket's teardown) the fresher reading wins.
    pub async fn live_rates(&self) -> Vec<NodeLiveRate> {
        self.live_rates_at(Instant::now()).await
    }

    async fn live_rates_at(&self, now: Instant) -> Vec<NodeLiveRate> {
        let map = self.inner.read().await;
        let mut best: HashMap<(i64, String), (u64, u64, Instant)> = HashMap::new();
        for (&group_id, conns) in map.iter() {
            for e in conns.values() {
                let (Some(node_id), Some(rate)) = (&e.node_id, e.rate) else {
                    continue;
                };
                if now.saturating_duration_since(rate.2) > LIVE_RATE_MAX_AGE {
                    continue;
                }
                let key = (group_id, node_id.clone());
                if best.get(&key).is_none_or(|cur| rate.2 > cur.2) {
                    best.insert(key, rate);
                }
            }
        }
        let mut out: Vec<NodeLiveRate> = best
            .into_iter()
            .map(|((group_id, node_id), (up, down, _))| NodeLiveRate {
                group_id,
                node_id,
                upload_bps: up,
                download_bps: down,
            })
            .collect();
        out.sort_by(|a, b| (a.group_id, &a.node_id).cmp(&(b.group_id, &b.node_id)));
        out
    }

    /// v0.4.14: send a message ONLY to the connection(s) in a group whose
    /// X-Node-ID matches `node_id`. Used by directed diagnosis to target a
    /// specific node instead of the whole group. Returns how many connections
    /// received it (0 = that node has no live WS connection right now). Dead
    /// senders are pruned.
    pub async fn send_node(&self, group_id: i64, node_id: &str, msg: &str) -> usize {
        let mut map = self.inner.write().await;
        let Some(conns) = map.get_mut(&group_id) else {
            return 0;
        };
        let mut sent = 0usize;
        conns.retain(|_, e| {
            if e.node_id.as_deref() != Some(node_id) {
                return true; // not the target node — leave untouched
            }
            if e.tx.send(msg.to_string()).is_ok() {
                sent += 1;
                true
            } else {
                false // target but dead — prune
            }
        });
        if conns.is_empty() {
            map.remove(&group_id);
        }
        sent
    }

    /// v0.4.14: the set of node_ids in a group that currently have a live WS
    /// connection AND advertised an X-Node-ID. This is the source of truth for
    /// "is this node's control channel online", replacing the stale kvs
    /// last_seen heuristic for diagnosis. Older nodes (no X-Node-ID) are NOT
    /// included — they can't be targeted by directed diagnosis.
    pub async fn online_node_ids(&self, group_id: i64) -> std::collections::HashSet<String> {
        self.inner
            .read()
            .await
            .get(&group_id)
            .map(|conns| conns.values().filter_map(|e| e.node_id.clone()).collect())
            .unwrap_or_default()
    }

    /// Number of live connections currently registered for a group. Used by
    /// diagnosis to decide "WS online" vs "control channel offline".
    #[allow(dead_code)]
    pub async fn group_conn_count(&self, group_id: i64) -> usize {
        self.inner
            .read()
            .await
            .get(&group_id)
            .map(|c| c.len())
            .unwrap_or(0)
    }

    /// Force-close every live connection for ONE group. Used by token rotation:
    /// the old token is invalid immediately, so the old WS connection (which
    /// authenticated with it at upgrade time) must be torn down — otherwise the
    /// node keeps an authenticated socket open with a revoked credential until
    /// its next reconnect.
    ///
    /// Drops the group's senders; each connection's `push_rx.recv()` returns
    /// None and the socket task exits (handle_node_ws → unregister, a no-op
    /// since close_group already removed the entry). The node then reconnects
    /// and re-authenticates with the new token.
    pub async fn close_group(&self, group_id: i64) -> usize {
        let mut map = self.inner.write().await;
        map.remove(&group_id).map(|conns| conns.len()).unwrap_or(0)
    }
}

/// WebSocket endpoint for node control channel.
/// Node authenticates via Authorization: Bearer <NODE_TOKEN>.
/// The token is intentionally NOT accepted from `?token=` because query
/// parameters leak into access/proxy logs (Nginx/Caddy/CDN).
///
/// Protocol:
///   - On connect: server sends config_snapshot (NodeConfigResponse JSON)
///   - ping/pong: heartbeat
///   - config_changed: server pushes `{"type":"config_changed"}` to all
///     connections whenever an admin mutates rules/groups; the node then
///     re-fetches /node/config over HTTP.
pub async fn node_ws_handler(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
) -> impl IntoResponse {
    let token = match extract_node_token(&headers) {
        Some(t) => t,
        None => return axum::http::StatusCode::UNAUTHORIZED.into_response(),
    };

    // v0.4.0: config-protocol gate. A node whose X-Config-Protocol-Version
    // header is absent or doesn't match the panel's is refused at upgrade time
    // (426 Upgrade Required) with a structured JSON body so the node can log
    // the exact mismatch. NOT a transient error — the node must back off.
    if !crate::api::node::config_protocol_compatible(&headers) {
        let received = crate::api::node::extract_config_protocol_version(&headers);
        return (
            axum::http::StatusCode::UPGRADE_REQUIRED,
            axum::Json(serde_json::json!({
                "code": "CONFIG_PROTOCOL_MISMATCH",
                "required": relay_shared::protocol::CONFIG_PROTOCOL_VERSION,
                "received": received,
                "message": "relay-node configuration protocol is incompatible; \
                            upgrade relay-node to match the panel"
            })),
        )
            .into_response();
    }

    use relay_shared::models::DeviceGroup;
    let group: Option<DeviceGroup> = match state.db.find_by_token(&token).await {
        Ok(g) => g,
        Err(e) => {
            tracing::error!("node_ws_handler: find_by_token failed: {}", e);
            None
        }
    };

    let group = match group {
        Some(g) => g,
        None => return axum::http::StatusCode::UNAUTHORIZED.into_response(),
    };

    let group_id = group.id;
    // v0.4.14: optional per-node identity. None for an older node that didn't
    // send X-Node-ID — it still connects and gets config_changed, it just can't
    // be targeted by directed diagnosis.
    let node_id = headers
        .get("X-Node-ID")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    // Clone the Arc<dyn Repository> so the WS task can keep using it after the
    // upgrade handler returns. The pool snapshot is shared read-only.
    let db = state.db.clone();

    ws.on_upgrade(move |socket| {
        handle_node_ws(socket, group_id, node_id, db, state.node_connections)
    })
}

async fn handle_node_ws(
    socket: WebSocket,
    group_id: i64,
    node_id: Option<String>,
    db: std::sync::Arc<dyn crate::db::Repository>,
    node_connections: NodeConnections,
) {
    tracing::info!(
        "websocket connected: group_id={} node_id={:?}",
        group_id,
        node_id
    );

    // Split so we can concurrently read ping/close from the socket AND write
    // broadcast pushes from the channel. Both halves borrow independent state.
    let (mut sender, mut receiver) = socket.split();
    let (conn_id, mut push_rx) = node_connections.register(group_id, node_id).await;

    // Send initial config snapshot so a freshly-connected node has its config
    // immediately, without waiting for the first HTTP poll. None (DB error) →
    // skip the push; the node will get its config on the next HTTP poll.
    if let Some(config) = build_config_snapshot(db.as_ref(), group_id).await {
        if let Ok(config_json) = serde_json::to_string(&config) {
            let _ = sender.send(Message::Text(config_json.into())).await;
        }
    }

    use tokio::time::{timeout, Duration};

    // The read loop idles when the node neither pings nor sends data. We
    // cap idle at 120s so a silently-dropped connection (NAT timeout,
    // half-open TCP) is eventually cleaned up. The node's heartbeat is
    // expected well within this window.
    const READ_TIMEOUT: Duration = Duration::from_secs(120);

    // Drive both halves. `receiver.recv()` (wrapped in a timeout) and
    // `push_rx.recv()` borrow different variables, so select! can hold both
    // pending at once; the branch bodies both use `sender` but only one
    // runs at a time.
    loop {
        tokio::select! {
            msg = timeout(READ_TIMEOUT, receiver.next()) => match msg {
                Err(_) => {
                    tracing::warn!(
                        "websocket idle timeout ({}s): group_id={}",
                        READ_TIMEOUT.as_secs(),
                        group_id
                    );
                    break;
                }
                Ok(Some(Ok(Message::Ping(data)))) => {
                    let _ = sender.send(Message::Pong(data)).await;
                }
                Ok(Some(Ok(Message::Pong(_)))) => {
                    // keepalive acknowledged
                }
                Ok(Some(Ok(Message::Close(_)))) | Ok(None) | Ok(Some(Err(_))) => {
                    tracing::info!("websocket disconnected: group_id={}", group_id);
                    break;
                }
                Ok(Some(Ok(Message::Text(text)))) => {
                    // v1.2.11: the only node -> panel text frame is the live
                    // rate. Anything else is ignored as before, which is also
                    // what keeps a newer node safe against this panel.
                    if let Ok(m) = serde_json::from_str::<NodeRateMessage>(text.as_str()) {
                        if m.msg_type == NodeRateMessage::TYPE {
                            node_connections
                                .record_rate(group_id, conn_id, m.upload_bps, m.download_bps)
                                .await;
                        }
                    }
                }
                Ok(Some(Ok(_))) => {
                    // ignore other message types
                }
            },
            pushed = push_rx.recv() => match pushed {
                Some(text) => {
                    if sender.send(Message::Text(text.into())).await.is_err() {
                        tracing::warn!(
                            "websocket send failed: group_id={}, closing",
                            group_id
                        );
                        break;
                    }
                }
                None => break, // all senders dropped — shouldn't happen here
            },
        }
    }

    node_connections.unregister(group_id, conn_id).await;
}

async fn build_config_snapshot(
    db: &dyn crate::db::Repository,
    group_id: i64,
) -> Option<NodeConfigResponse> {
    // v0.3.6: delegate to the shared `build_node_config` (same function
    // `get_config` uses). This fixes the v0.3.5 drift where the WS path queried
    // forward_rules WITHOUT joining users, so a reconnecting node could be
    // handed a banned / over-quota user's rules until the next HTTP poll. Now
    // both paths apply the identical filter (paused / banned / quota) and the
    // identical target resolution + listener assembly.
    //
    // Returns None on DB error so the caller skips the snapshot push (rather
    // than pushing an empty config that would incorrectly tear down the node's
    // listeners). An empty Ok is a legitimate "no rules" snapshot.
    match crate::service::node_config::build_node_config(db, group_id).await {
        Ok(cfg) => Some(cfg),
        Err(e) => {
            tracing::error!(
                "build_config_snapshot: build_node_config failed for group {}: {}",
                group_id,
                e
            );
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// register() must hand back a receiver that actually receives what
    /// broadcast_all sends. This is the contract every admin mutation
    /// relies on for the config_changed push.
    #[tokio::test]
    async fn register_then_broadcast_delivers() {
        let conns = NodeConnections::new();
        let (_id, mut rx) = conns.register(7, None).await;

        conns.broadcast_all(r#"{"type":"config_changed"}"#).await;

        let msg = rx.recv().await;
        assert_eq!(msg.as_deref(), Some(r#"{"type":"config_changed"}"#));
    }

    /// broadcast_all must fan out to EVERY registered connection, not just
    /// the first one — otherwise only one node per group would get pushes.
    #[tokio::test]
    async fn broadcast_fans_out_to_multiple_connections_same_group() {
        let conns = NodeConnections::new();
        let (_, mut rx_a) = conns.register(1, None).await;
        let (_, mut rx_b) = conns.register(1, None).await;
        // Different group should also receive (broadcast_all hits all groups).
        let (_, mut rx_c) = conns.register(99, None).await;

        conns.broadcast_all("hi").await;

        assert_eq!(rx_a.recv().await.as_deref(), Some("hi"));
        assert_eq!(rx_b.recv().await.as_deref(), Some("hi"));
        assert_eq!(rx_c.recv().await.as_deref(), Some("hi"));
    }

    /// After unregister, a connection must no longer receive broadcasts.
    /// This is what prevents memory growth as nodes reconnect.
    #[tokio::test]
    async fn unregister_stops_delivery() {
        let conns = NodeConnections::new();
        let (id, mut rx) = conns.register(3, None).await;

        conns.unregister(3, id).await;
        conns.broadcast_all("late").await;

        // recv on an unregistered sender's receiver: either the sender was
        // removed (so nothing arrives) — either way, no "late" message.
        let leaked = rx.try_recv().ok();
        assert_ne!(leaked.as_deref(), Some("late"));
    }

    /// If a connection's receiver is dropped (node disconnected without
    /// cleanly unregistering), broadcast_all must prune the dead sender
    /// instead of leaking it forever. Verified by checking that the next
    /// broadcast doesn't panic and the live connection still gets the msg.
    #[tokio::test]
    async fn broadcast_prunes_dead_senders() {
        let conns = NodeConnections::new();

        // Register and immediately drop the receiver — simulates a node
        // whose socket died before unregister ran.
        let (_, rx_dead) = conns.register(5, None).await;
        drop(rx_dead);

        // Live connection on the same group.
        let (_, mut rx_live) = conns.register(5, None).await;

        // First broadcast hits the dead sender (send fails) and prunes it;
        // the live sender must still receive.
        conns.broadcast_all("after-death").await;
        assert_eq!(rx_live.recv().await.as_deref(), Some("after-death"));

        // Second broadcast must not error on the pruned entry.
        conns.broadcast_all("again").await;
        assert_eq!(rx_live.recv().await.as_deref(), Some("again"));
    }

    /// close_group must disconnect every connection of the targeted group by
    /// dropping their senders (receiver returns None). This is the token-
    /// rotation contract: the old token is invalid, so every socket that
    /// authenticated with it must be torn down.
    #[tokio::test]
    async fn close_group_disconnects_all_connections_in_group() {
        let conns = NodeConnections::new();
        let (_, mut rx_a) = conns.register(3, None).await;
        let (_, mut rx_b) = conns.register(3, None).await;
        // A different group must be UNAFFECTED.
        let (_, mut rx_other) = conns.register(7, None).await;

        let closed = conns.close_group(3).await;

        // Both connections in group 3 see their receiver return None (sender
        // dropped) — the handle_node_ws loop breaks on this and the socket
        // closes, forcing the node to reconnect and re-auth with the new token.
        assert_eq!(closed, 2, "close_group must report the count closed");
        assert!(rx_a.recv().await.is_none(), "group-3 conn A must be closed");
        assert!(rx_b.recv().await.is_none(), "group-3 conn B must be closed");
        // The other group keeps working.
        conns.broadcast_all("still-here").await;
        assert_eq!(rx_other.recv().await.as_deref(), Some("still-here"));
    }

    /// close_group on a group with no connections returns 0 and is a no-op.
    #[tokio::test]
    async fn close_group_unknown_group_is_noop() {
        let conns = NodeConnections::new();
        let (_, mut rx) = conns.register(3, None).await;

        let closed = conns.close_group(999).await;

        assert_eq!(closed, 0);
        // The real group is untouched.
        conns.broadcast_all("ok").await;
        assert_eq!(rx.recv().await.as_deref(), Some("ok"));
    }

    /// v0.4.14: send_node delivers ONLY to the connection whose X-Node-ID
    /// matches; other nodes in the same group are untouched.
    #[tokio::test]
    async fn send_node_targets_only_matching_node() {
        let conns = NodeConnections::new();
        let (_, mut rx_a) = conns.register(1, Some("node-a".into())).await;
        let (_, mut rx_b) = conns.register(1, Some("node-b".into())).await;

        let sent = conns.send_node(1, "node-a", "probe").await;
        assert_eq!(sent, 1, "exactly one connection matched node-a");
        assert_eq!(rx_a.recv().await.as_deref(), Some("probe"));
        // node-b must NOT have received it.
        assert!(
            rx_b.try_recv().is_err(),
            "node-b must not receive node-a's probe"
        );
    }

    /// send_node to a node that has no live connection returns 0 (control
    /// channel offline) — the diagnose path turns this into an immediate
    /// "offline" instead of waiting for a timeout.
    #[tokio::test]
    async fn send_node_unknown_node_returns_zero() {
        let conns = NodeConnections::new();
        let (_, _rx) = conns.register(1, Some("node-a".into())).await;
        assert_eq!(conns.send_node(1, "ghost", "probe").await, 0);
        assert_eq!(conns.send_node(999, "node-a", "probe").await, 0);
    }

    /// online_node_ids returns the node_ids with a live connection; older nodes
    /// (no X-Node-ID) are excluded so they don't get targeted.
    #[tokio::test]
    async fn online_node_ids_excludes_nodeless_connections() {
        let conns = NodeConnections::new();
        let (_, _a) = conns.register(1, Some("node-a".into())).await;
        let (_, _b) = conns.register(1, Some("node-b".into())).await;
        let (_, _legacy) = conns.register(1, None).await; // older node, no X-Node-ID

        let ids = conns.online_node_ids(1).await;
        assert_eq!(ids.len(), 2);
        assert!(ids.contains("node-a"));
        assert!(ids.contains("node-b"));
        // An empty group → empty set.
        assert!(conns.online_node_ids(42).await.is_empty());
    }

    fn rate(r: &NodeLiveRate) -> (i64, &str, u64, u64) {
        (r.group_id, r.node_id.as_str(), r.upload_bps, r.download_bps)
    }

    #[tokio::test]
    async fn a_recorded_rate_is_reported_for_its_node() {
        let conns = NodeConnections::new();
        let (id, _rx) = conns.register(4, Some("gz-1".into())).await;
        conns.record_rate(4, id, 3_650_000, 128_930).await;

        let rates = conns.live_rates().await;
        assert_eq!(
            rates.iter().map(rate).collect::<Vec<_>>(),
            vec![(4, "gz-1", 3_650_000, 128_930)]
        );
    }

    /// The whole point of the feed is to be current. A reading the node stopped
    /// refreshing must drop out so the page falls back to the status report,
    /// not freeze on the last live number.
    #[tokio::test]
    async fn a_stale_rate_is_left_out() {
        let conns = NodeConnections::new();
        let (id, _rx) = conns.register(4, Some("gz-1".into())).await;
        conns.record_rate(4, id, 1, 2).await;

        let fresh = Instant::now();
        assert_eq!(conns.live_rates_at(fresh).await.len(), 1);
        let later = fresh + LIVE_RATE_MAX_AGE + Duration::from_secs(1);
        assert!(conns.live_rates_at(later).await.is_empty());
    }

    /// Living on the connection means a disconnect takes the rate with it.
    #[tokio::test]
    async fn disconnecting_removes_the_rate() {
        let conns = NodeConnections::new();
        let (id, _rx) = conns.register(4, Some("gz-1".into())).await;
        conns.record_rate(4, id, 1, 2).await;
        conns.unregister(4, id).await;
        assert!(conns.live_rates().await.is_empty());
    }

    /// Without an X-Node-ID the reading can't be matched to a row on the page.
    #[tokio::test]
    async fn a_connection_without_a_node_id_records_nothing() {
        let conns = NodeConnections::new();
        let (id, _rx) = conns.register(4, None).await;
        conns.record_rate(4, id, 1, 2).await;
        assert!(conns.live_rates().await.is_empty());
    }

    /// A reconnect can briefly leave the old socket registered beside the new
    /// one. The page must show one number per node — the newest.
    #[tokio::test]
    async fn two_connections_for_one_node_report_the_fresher_rate() {
        let conns = NodeConnections::new();
        let (old, _rx1) = conns.register(4, Some("gz-1".into())).await;
        let (new, _rx2) = conns.register(4, Some("gz-1".into())).await;
        conns.record_rate(4, old, 111, 111).await;
        tokio::time::sleep(Duration::from_millis(5)).await;
        conns.record_rate(4, new, 222, 222).await;

        let rates = conns.live_rates().await;
        assert_eq!(
            rates.iter().map(rate).collect::<Vec<_>>(),
            vec![(4, "gz-1", 222, 222)]
        );
    }
}
