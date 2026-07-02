use crate::config::NodeConfig;
use crate::forwarder::ForwarderManager;
use crate::poller;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;
use tokio::time::{interval, Instant};
use tokio_tungstenite::tungstenite::Message;

/// Send a Ping every this many seconds. Must be comfortably shorter than the
/// panel's READ_TIMEOUT (120s) and any reverse-proxy/CDN idle timeout (often
/// 60s), so the connection is never seen as idle and dropped.
const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(25);
/// If no Pong arrives within this many seconds after a Ping, assume the
/// connection is dead and force a reconnect (rather than waiting for the
/// panel's 120s timeout to notice).
const PONG_TIMEOUT: Duration = Duration::from_secs(10);

/// Derive the WebSocket URL from PANEL_URL.
/// http://ip:port -> ws://ip:port/api/v1/node/ws
/// https://domain -> wss://domain/api/v1/node/ws
fn derive_ws_url(panel_url: &str) -> String {
    let url = panel_url.trim_end_matches('/');
    if let Some(rest) = url.strip_prefix("https://") {
        format!("wss://{}/api/v1/node/ws", rest)
    } else if let Some(rest) = url.strip_prefix("http://") {
        format!("ws://{}/api/v1/node/ws", rest)
    } else {
        // No scheme — assume ws://
        format!("ws://{}/api/v1/node/ws", url)
    }
}

/// Run the WebSocket control channel with automatic reconnection.
/// Exponential backoff: 1s initial, 30s max for transient errors.
/// Permanent errors (426 protocol mismatch, 401/403 auth) use a 5-minute
/// backoff — polling fast is pointless because the only fix is an upgrade or
/// reconfiguration.
///
/// This runs in a separate tokio task alongside the HTTP poller.
/// If WS fails (panel down, bad reverse proxy, CDN blocking), the node
/// continues forwarding with the last known config.
pub async fn run_ws_loop(
    config: &NodeConfig,
    manager: &Arc<Mutex<ForwarderManager>>,
    node_id: &str,
) {
    let ws_url = derive_ws_url(&config.panel_url);
    let mut backoff = 1u64;
    const PERMANENT_BACKOFF_SECS: u64 = 300; // 5 minutes
                                             // Log dedup: avoid re-logging the SAME permanent error every backoff cycle.
                                             // The message is stored; if the next exit is the same, we skip the log.
    let mut last_permanent_msg: Option<String> = None;

    loop {
        tracing::info!("websocket connecting to {} ...", ws_url);

        let exit = connect_and_run(&ws_url, &config.token, config, manager, node_id).await;
        match exit {
            WsExit::ConfigChanged => {
                tracing::info!("websocket: config_changed received, reconnecting immediately");
                backoff = 1;
                last_permanent_msg = None;
            }
            WsExit::Disconnected => {
                tracing::warn!(
                    "websocket disconnected, reconnecting in {} seconds",
                    backoff
                );
                tokio::time::sleep(Duration::from_secs(backoff)).await;
                backoff = (backoff * 2).min(30);
                last_permanent_msg = None;
            }
            WsExit::PermanentError(msg) => {
                // 426 / 401 / 403: configuration or version problem that won't
                // fix itself. Back off 5 minutes. Dedup the log so it doesn't
                // repeat every cycle.
                if last_permanent_msg.as_deref() != Some(msg.as_str()) {
                    tracing::warn!(
                        "websocket permanent error: {} — backing off {}s (upgrade or reconfigure to fix)",
                        msg,
                        PERMANENT_BACKOFF_SECS
                    );
                    last_permanent_msg = Some(msg);
                }
                tokio::time::sleep(Duration::from_secs(PERMANENT_BACKOFF_SECS)).await;
                // Don't touch `backoff` — it's for transient errors only.
            }
            WsExit::Error(e) => {
                tracing::warn!(
                    "websocket error: {}, reconnecting in {} seconds",
                    e,
                    backoff
                );
                tokio::time::sleep(Duration::from_secs(backoff)).await;
                backoff = (backoff * 2).min(30);
                last_permanent_msg = None;
            }
        }
    }
}

enum WsExit {
    ConfigChanged,
    Disconnected,
    /// A permanent error (426 protocol mismatch, 401/403 auth). The node backs
    /// off 5 minutes — the only fix is an upgrade or reconfiguration.
    PermanentError(String),
    /// A transient error (network, 5xx, proxy hiccup). Standard exponential
    /// backoff.
    Error(String),
}

/// Classify a tungstenite connect error into PermanentError (426/401/403) or
/// transient Error. 426 = config protocol mismatch; 401/403 = auth. These
/// won't fix themselves on retry, so the caller backs off 5 minutes. 5xx and
/// network errors are transient (standard exponential backoff).
fn classify_ws_connect_error(e: tokio_tungstenite::tungstenite::Error) -> WsExit {
    use tokio_tungstenite::tungstenite::http::StatusCode;
    use tokio_tungstenite::tungstenite::Error;

    if let Error::Http(resp) = &e {
        let status = resp.status();
        match status {
            StatusCode::UPGRADE_REQUIRED => {
                // Try to parse the structured body for a better message.
                let required = resp
                    .body()
                    .as_ref()
                    .and_then(|b| serde_json::from_slice::<serde_json::Value>(b).ok())
                    .as_ref()
                    .and_then(|d| d.get("required"))
                    .and_then(|v| v.as_u64());
                WsExit::PermanentError(format!(
                    "config protocol mismatch (panel requires v{:?}, node has v{}) — upgrade relay-node",
                    required,
                    relay_shared::protocol::CONFIG_PROTOCOL_VERSION
                ))
            }
            StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => WsExit::PermanentError(format!(
                "authentication rejected (HTTP {}): invalid or revoked token",
                status.as_u16()
            )),
            _ => WsExit::Error(format!("connect: HTTP {} from panel", status.as_u16())),
        }
    } else {
        WsExit::Error(format!("connect: {}", e))
    }
}

async fn connect_and_run(
    ws_url: &str,
    token: &str,
    config: &NodeConfig,
    manager: &Arc<Mutex<ForwarderManager>>,
    node_id: &str,
) -> WsExit {
    use futures_util::{SinkExt, StreamExt};
    use tokio_tungstenite::connect_async;
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;

    // Build the WebSocket handshake request from the URL. `IntoClientRequest`
    // (implemented for &str/String) generates ALL the standard handshake
    // headers tungstenite requires: Host, Upgrade, Connection,
    // Sec-WebSocket-Key, Sec-WebSocket-Version. We must NOT set any of those
    // manually — overriding them (especially the per-request random
    // Sec-WebSocket-Key) is exactly what produced
    //   "WebSocket protocol error: Missing, duplicated or incorrect header
    //    sec-websocket-key"
    // against the OpenResty reverse proxy. Only add our app-level headers.
    let mut request = match ws_url.into_client_request() {
        Ok(req) => req,
        Err(e) => return WsExit::Error(format!("request build: {}", e)),
    };

    // Authorization: Bearer <token> is REQUIRED by the panel's
    // node_ws_handler; without it the upgrade returns 401.
    if let Ok(v) = format!("Bearer {}", token).parse() {
        request.headers_mut().insert("Authorization", v);
    }
    // v0.4.0: config-protocol version gate. The panel refuses the upgrade
    // (426) if this is absent or mismatches, so an old node keeps its cached
    // config instead of receiving fields it can't deserialize.
    if let Ok(v) = relay_shared::protocol::CONFIG_PROTOCOL_VERSION
        .to_string()
        .parse()
    {
        request.headers_mut().insert("X-Config-Protocol-Version", v);
    }
    if let Ok(v) = "relay-node-ws".parse() {
        request.headers_mut().insert("User-Agent", v);
    }
    // v0.4.14: optional per-node identity so the panel can target diagnosis at a
    // SPECIFIC node (not the whole group). This is an OPTIONAL extension — it
    // does NOT change the config structure, so CONFIG_PROTOCOL_VERSION is
    // unchanged. An older node that omits this still connects fine; it just
    // can't be targeted by directed diagnosis (the panel surfaces "upgrade").
    if !node_id.is_empty() {
        if let Ok(v) = node_id.parse() {
            request.headers_mut().insert("X-Node-ID", v);
        }
    }

    let ws_result = connect_async(request).await;

    let (mut ws_stream, _response) = match ws_result {
        Ok(c) => {
            tracing::info!("websocket connected");
            c
        }
        Err(e) => {
            // v0.4.0: distinguish permanent HTTP errors (426/401/403) from
            // transient ones. tungstenite gives us the HTTP response for non-101
            // upgrades via Error::Http(response).
            return classify_ws_connect_error(e);
        }
    };

    // ── Heartbeat state ──
    // All timestamps are millis relative to `session_start` (a monotonic
    // Instant captured once per connection). We avoid SystemTime (can jump
    // backwards on NTP sync) and Instant::now() confusion. AtomicU64 so the
    // heartbeat arm and message arm read/write without a lock.
    let session_start = Instant::now();
    let now_ms = || session_start.elapsed().as_millis() as u64;
    let last_pong = AtomicU64::new(now_ms()); // last time we got a Pong
    let last_ping = AtomicU64::new(0u64); // last time we sent a Ping; 0 = none outstanding
    let mut heartbeat = interval(HEARTBEAT_INTERVAL);
    // Don't fire immediately on the first tick (we just connected).
    heartbeat.reset();

    loop {
        tokio::select! {
            // ── Incoming messages ──
            msg_result = ws_stream.next() => {
                let Some(msg_result) = msg_result else {
                    // Stream ended (server closed without a Close frame).
                    return WsExit::Disconnected;
                };
                match msg_result {
                    Ok(Message::Text(text)) => {
                        if let Ok(resp) =
                            serde_json::from_str::<relay_shared::protocol::NodeConfigResponse>(&text)
                        {
                            tracing::info!(
                                "websocket: received config ({} listeners), applying",
                                resp.listeners.len()
                            );
                            let mut mgr = manager.lock().await;
                            mgr.apply_config(&resp).await;
                            tracing::info!("websocket: config applied");
                        } else if text.contains("config_changed") {
                            tracing::info!("websocket: config_changed received, re-fetching");
                            match poller::fetch_config(config).await {
                                poller::FetchResult::Ok(resp) => {
                                    let mut mgr = manager.lock().await;
                                    mgr.apply_config(&resp).await;
                                    tracing::info!("websocket: config applied after config_changed");
                                }
                                poller::FetchResult::ProtocolMismatch => {
                                    tracing::warn!("websocket: config fetch returned protocol mismatch; keeping cached config");
                                }
                                poller::FetchResult::Transient => {
                                    tracing::warn!("websocket: config fetch failed transiently; keeping cached config");
                                }
                            }
                            return WsExit::ConfigChanged;
                        } else if let Ok(dm) =
                            serde_json::from_str::<relay_shared::protocol::DiagnoseRuleMessage>(&text)
                        {
                            // v0.4.8: rule diagnosis request from the panel.
                            // Run the probe on a detached task so the WS loop
                            // keeps draining messages; the result is POSTed back
                            // over HTTP by diagnose::run_and_report.
                            // v0.4.9: the message carries a per-run `challenge`
                            // the node MUST echo back verbatim; the panel rejects
                            // a result without an exact match.
                            tracing::info!(
                                "websocket: diagnose_rule request_id={} rule_id={}",
                                dm.request_id,
                                dm.rule_id
                            );
                            let cfg = config.clone();
                            let mgr = manager.clone();
                            let nid = node_id.to_string();
                            let req_id = dm.request_id.clone();
                            let rid = dm.rule_id;
                            let challenge = dm.challenge.clone();
                            tokio::spawn(async move {
                                crate::diagnose::run_and_report(
                                    &mgr, &cfg, &nid, req_id, rid, challenge,
                                )
                                .await;
                            });
                        } else if let Ok(um) = serde_json::from_str::<
                            relay_shared::protocol::UpgradeNodeMessage,
                        >(&text)
                        {
                            // v1.0.10: directed self-upgrade command. send_node
                            // already routed this to only this node; re-check the
                            // node_id as defence in depth, then run the upgrade on
                            // a detached task so the WS loop keeps draining. On
                            // success the task exits the process → systemd restarts
                            // into the new binary; on failure the node keeps running.
                            if um.msg_type == "upgrade_node" && um.node_id != node_id {
                                tracing::warn!(
                                    "websocket: ignoring upgrade for {} (I am {})",
                                    um.node_id,
                                    node_id
                                );
                            } else if um.msg_type == "upgrade_node" {
                                tracing::warn!("websocket: self-upgrade requested; starting");
                                tokio::spawn(async move {
                                    match crate::updater::self_upgrade().await {
                                        Ok(()) => {
                                            tracing::warn!(
                                                "self-upgrade done; exiting to restart into new binary"
                                            );
                                            std::process::exit(0);
                                        }
                                        Err(e) => tracing::error!(
                                            "self-upgrade failed: {} (keeping current binary)",
                                            e
                                        ),
                                    }
                                });
                            }
                        } else {
                            tracing::debug!("websocket: received text: {}", &text[..text.len().min(100)]);
                        }
                    }
                    Ok(Message::Pong(_)) => {
                        // Server replied to our Ping — connection is alive.
                        last_pong.store(now_ms(), Ordering::Relaxed);
                        last_ping.store(0, Ordering::Relaxed);
                        tracing::debug!("websocket: pong received");
                    }
                    Ok(Message::Ping(_)) => {
                        // tungstenite auto-responds to server pings with a pong.
                    }
                    Ok(Message::Close(_)) => {
                        return WsExit::Disconnected;
                    }
                    Ok(_) => {}
                    Err(e) => {
                        return WsExit::Error(format!("stream: {}", e));
                    }
                }
            }

            // ── Heartbeat tick (every HEARTBEAT_INTERVAL) ──
            _ = heartbeat.tick() => {
                // Before sending this ping, check if the PREVIOUS ping is still
                // unanswered past PONG_TIMEOUT. We capture the old last_ping
                // value BEFORE overwriting it with the new one.
                let prev_ping = last_ping.load(Ordering::Relaxed);
                let last_pong_val = last_pong.load(Ordering::Relaxed);
                let now = now_ms();

                if prev_ping != 0
                    && last_pong_val < prev_ping
                    && now.saturating_sub(prev_ping) > PONG_TIMEOUT.as_millis() as u64
                {
                    // The previous ping (sent HEARTBEAT_INTERVAL ago) never got
                    // a Pong within PONG_TIMEOUT — the connection is dead.
                    tracing::warn!(
                        "websocket: heartbeat timeout (no pong within {}s), reconnecting",
                        PONG_TIMEOUT.as_secs()
                    );
                    return WsExit::Disconnected;
                }

                // Send a fresh Ping and record when we sent it.
                if let Err(e) = ws_stream.send(Message::Ping(Vec::new().into())).await {
                    return WsExit::Error(format!("ping send: {}", e));
                }
                last_ping.store(now, Ordering::Relaxed);
                tracing::debug!("websocket: ping sent");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::derive_ws_url;

    /// v0.4.16: the WSS control-channel URL is derived from PANEL_URL. The
    /// https→wss mapping is the half that crosses the TLS provider, so pin it
    /// — a regression here would make every WSS node fail to connect even with
    /// the provider fixed.
    #[test]
    fn derive_ws_url_maps_schemes() {
        assert_eq!(
            derive_ws_url("https://panel.example.com"),
            "wss://panel.example.com/api/v1/node/ws"
        );
        assert_eq!(
            derive_ws_url("https://panel.example.com/"),
            "wss://panel.example.com/api/v1/node/ws"
        );
        assert_eq!(
            derive_ws_url("http://127.0.0.1:18888"),
            "ws://127.0.0.1:18888/api/v1/node/ws"
        );
        // No scheme → assume ws:// (matches the node's default PANEL_URL).
        assert_eq!(
            derive_ws_url("127.0.0.1:18888"),
            "ws://127.0.0.1:18888/api/v1/node/ws"
        );
    }
}
