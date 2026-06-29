mod config;
mod diagnose;
mod forwarder;
mod poller;
mod reporter;
mod ws_client;

use config::NodeConfig;
use forwarder::ForwarderManager;
use reporter::{ConnectionTracker, TrafficCounter};
use std::process::ExitCode;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;

/// Built-in version string. Single source of truth: the Cargo package version
/// (`env!("CARGO_PKG_VERSION")`), which is kept in sync with
/// `scripts/relay-node-install.sh` and the GitHub release tag.
const VERSION: &str = env!("CARGO_PKG_VERSION");

fn main() -> ExitCode {
    // Handle CLI flags BEFORE initialising tokio / tracing so that
    // `--version` / `-V` / `--help` print and exit immediately, without ever
    // touching the network or spawning the service loop. This matches the
    // conventional CLI contract: version/help must not start the service.
    let args: Vec<String> = std::env::args().skip(1).collect();
    for arg in &args {
        match arg.as_str() {
            "-V" | "--version" => {
                println!("relay-node {}", VERSION);
                return ExitCode::SUCCESS;
            }
            "-h" | "--help" => {
                print_help();
                return ExitCode::SUCCESS;
            }
            // Unknown flags are ignored for forward-compat; the node is
            // configured via env vars (PANEL_URL / NODE_TOKEN / POLL_INTERVAL).
            _ => {}
        }
    }

    // v0.4.16: install the process-level rustls CryptoProvider BEFORE building
    // the tokio runtime or any TLS config (reqwest HTTPS, tokio-tungstenite
    // WSS, tls_simple ingress). rustls 0.23 refuses to pick a provider
    // automatically when more than one is compiled in, panicking with
    // "Could not automatically determine the process-level CryptoProvider" the
    // first time any TLS client/server config is built. Our feature graph pulls
    // in BOTH ring (via our explicit rustls "ring" feature + reqwest's
    // `__rustls-ring`) and aws-lc-rs (via tokio-rustls's default features), so
    // the auto-selection is genuinely ambiguous. Installing ring explicitly
    // once, up front, resolves it for every consumer and makes the provider
    // deterministic. A process-wide default can only be installed once; a
    // second install returns Err, which we treat as fatal so we never ship a
    // half-broken node (forwarding up, WSS control channel dead).
    if let Err(e) = install_crypto_provider() {
        eprintln!("fatal: failed to install rustls CryptoProvider: {:?}", e);
        eprintln!("       the node cannot start without a TLS provider; refusing to run");
        return ExitCode::FAILURE;
    }

    // No early-exit flag present — start the real runtime.
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("failed to start tokio runtime: {}", e);
            return ExitCode::FAILURE;
        }
    };
    runtime.block_on(run());
    ExitCode::SUCCESS
}

/// v0.4.16: install the process-level rustls CryptoProvider (ring) exactly
/// once. Called from `main()` before the tokio runtime exists, so every TLS
/// consumer in the node (reqwest HTTPS, tokio-tungstenite WSS, tls_simple
/// ingress ServerConfig) shares this single provider and none panics on the
/// ambiguous "which provider?" auto-selection.
///
/// `install_default` is idempotent across providers only by returning `Err` on
/// the second call — it never overwrites (the `Err` carries the already-
/// installed provider). We surface that as a hard error so a misconfigured
/// build fails loudly at startup instead of running a node whose control
/// channel can't establish TLS.
fn install_crypto_provider() -> Result<(), Arc<rustls::crypto::CryptoProvider>> {
    rustls::crypto::ring::default_provider().install_default()
}

fn print_help() {
    println!("relay-node {}", VERSION);
    println!();
    println!("A forwarding node for RelayPanel. Runs as a long-lived service.");
    println!();
    println!("Usage: relay-node [OPTIONS]");
    println!();
    println!("Options:");
    println!("  -V, --version     Print version and exit");
    println!("  -h, --help        Print this help and exit");
    println!();
    println!("Configuration (environment variables):");
    println!("  PANEL_URL         Panel base URL (default: http://127.0.0.1:18888)");
    println!("  NODE_TOKEN        Node auth token (REQUIRED — a real inbound-group");
    println!("                    token from the panel UI. The node refuses to start");
    println!("                    if this is unset or still \"default-token\".)");
    println!("  POLL_INTERVAL     HTTP poll interval in seconds (default: 10)");
    println!("  RUST_LOG          Log level, e.g. info (default: warn)");
}

/// The real service entry point. Only reached when no --version/--help flag
/// was passed on the command line.
async fn run() {
    tracing_subscriber::fmt::init();
    let config = NodeConfig::load();
    let start_time = Instant::now();

    use reporter::{spawn_public_ip_refresher, NodeMetrics};

    let counter = Arc::new(TrafficCounter::new());
    let connections = Arc::new(ConnectionTracker::new());
    let mut manager_inner = ForwarderManager::new(counter.clone(), connections.clone());
    // v1.0.5: configure dual-stack listen and outbound source IP. A
    // misconfigured outbound (invalid IP, missing interface, non-local IP) is
    // a fatal startup error — we refuse to boot rather than silently routing
    // traffic out the wrong NIC.
    if let Err(e) = manager_inner.set_network_config(&config) {
        eprintln!(
            "FATAL: invalid outbound configuration: {}\n  \
             Check OUTBOUND_BIND_IPV4 / OUTBOUND_INTERFACE.",
            e
        );
        std::process::exit(1);
    }

    // v0.4.1: load TLS certificate + start hot-reloader for tls_simple listeners.
    // CertReloader ALWAYS starts the poll task, even if the initial load fails
    // (missing file, bad PEM, etc). The poll task retries every 5s and picks up
    // the cert once the file is fixed — no restart needed. Raw/WS always run.
    let tls_listener_errors = manager_inner.listener_errors_arc();
    match (&config.tls_cert_path, &config.tls_key_path) {
        (Some(cert), Some(key)) => {
            let (reloader, shared) = forwarder::cert_reloader::CertReloader::new(cert, key);
            manager_inner.set_tls_acceptor(Some(shared));
            reloader.spawn_poll_task(tls_listener_errors.clone());
        }
        _ => {
            tracing::info!(
                "No TLS_CERT_PATH/TLS_KEY_PATH set — tls_simple disabled (Raw/WS unaffected)"
            );
        }
    }

    let manager = Arc::new(Mutex::new(manager_inner));

    // Unified sampler: CPU/mem + disk + network rate + cumulative traffic.
    let metrics = Arc::new(NodeMetrics::new(&config.network_interface));
    // Seed CPU + network baselines (fast, no sleep) and fire-and-forget the
    // CPU second-sample warm-up so the first real report is sane.
    metrics.seed_baselines().await;
    metrics.spawn_warmup();
    // Detect public egress IP now + every 30 min (independent task, never
    // blocks startup or the poll loop).
    spawn_public_ip_refresher(metrics.clone());

    tracing::info!("RelayNode {} starting, panel={}", VERSION, config.panel_url);

    // --- Offline resilience: load cached config on startup ---
    // If the panel is down when the node starts, load the last known config
    // from disk and start forwarding immediately. The poller will sync
    // when the panel comes back online.
    if let Some(cached) = poller::load_cache() {
        tracing::info!(
            "Loaded cached config ({} listeners) - starting forwarding immediately",
            cached.listeners.len()
        );
        let mut mgr = manager.lock().await;
        mgr.apply_config(&cached).await;
    } else {
        tracing::info!("No cached config found - will start forwarding after first panel sync");
    }

    // v0.3.0: stable per-node identity. Generated once and persisted to a
    // node-id file, so the panel can tell multiple nodes sharing one group
    // token apart (otherwise their status entries overwrite each other).
    let node_id = poller::get_or_create_node_id();

    // --- Fork 1: WebSocket control channel (real-time config push) ---
    {
        let config_ws = config.clone();
        let manager_ws = manager.clone();
        let node_id_ws = node_id.clone();
        tokio::spawn(async move {
            ws_client::run_ws_loop(&config_ws, &manager_ws, &node_id_ws).await;
        });
    }

    // --- Fork 2: HTTP poll loop (fallback + traffic/status reporting) ---
    // This loop NEVER exits. Even if the panel is down, it keeps retrying
    // and reporting (report calls fail silently). Existing forwarding rules
    // continue working from the last successful config.
    //
    // v0.4.0: when the panel reports a permanent config-protocol mismatch
    // (426), we switch to a long poll interval (5 min) — polling fast is
    // pointless because the only fix is upgrading relay-node. Transient
    // failures keep the normal interval.
    let mut interval = tokio::time::interval(Duration::from_secs(config.poll_interval));
    const MISMATCH_BACKOFF_SECS: u64 = 300; // 5 minutes
    let mut in_mismatch_backoff = false;
    loop {
        interval.tick().await;

        match poller::fetch_config(&config).await {
            poller::FetchResult::Ok(resp) => {
                let mut mgr = manager.lock().await;
                mgr.apply_config(&resp).await;
                // Recovered from a mismatch: restore the normal poll interval.
                if in_mismatch_backoff {
                    tracing::info!("config protocol OK again; restoring normal poll interval");
                    interval = tokio::time::interval(Duration::from_secs(config.poll_interval));
                    in_mismatch_backoff = false;
                }
            }
            poller::FetchResult::ProtocolMismatch => {
                // Permanent: upgrade required. Switch to a long interval if we
                // haven't already (avoids re-logging every tick).
                if !in_mismatch_backoff {
                    tracing::warn!(
                        "switching poll interval to {}s due to config protocol mismatch",
                        MISMATCH_BACKOFF_SECS
                    );
                    interval = tokio::time::interval(Duration::from_secs(MISMATCH_BACKOFF_SECS));
                    in_mismatch_backoff = true;
                }
            }
            poller::FetchResult::Transient => {
                if in_mismatch_backoff {
                    // Was in mismatch backoff but now it's a different error —
                    // could mean the panel was upgraded. Restore normal interval.
                    interval = tokio::time::interval(Duration::from_secs(config.poll_interval));
                    in_mismatch_backoff = false;
                }
            }
        }

        // Report traffic + status (failures are logged but don't crash).
        // Note: connection counting does NOT depend on the WebSocket control
        // channel — these values reflect real active TCP/UDP forwarding state
        // and are reported over plain HTTP, so they keep working even if WS
        // is down.
        reporter::report_traffic(&config, &counter).await;
        // Drain any listener bind/runtime errors captured since the last cycle
        // and forward them to the panel so an operator can see WHY a rule isn't
        // forwarding (port in use, permission denied, etc.).
        let listener_errors = {
            let mgr = manager.lock().await;
            mgr.take_listener_errors().await
        };
        reporter::report_status(
            &config,
            &metrics,
            &connections,
            start_time,
            &node_id,
            listener_errors,
        )
        .await;
    }
}

#[cfg(test)]
mod tests {
    use super::install_crypto_provider;

    /// v0.4.16: `install_crypto_provider()` must succeed and install the ring
    /// provider process-wide. Before this fix, rustls 0.23 panicked with
    /// "Could not automatically determine the process-level CryptoProvider"
    /// because both ring and aws-lc-rs were compiled in. After the fix ring is
    /// the only provider, but we STILL call install_default() (main() does this
    /// at startup) so this test guards against a future feature merge silently
    /// reintroducing ambiguity. `install_default` is idempotent — a second call
    /// returns Err with the already-installed provider, so across the test
    /// binary's life only the FIRST assertion sees Ok; later tests may see the
    /// idempotent Err, which is also acceptable. We assert the provider is
    /// installed by checking that building a TLS client config does NOT panic.
    #[test]
    fn install_crypto_provider_installs_ring() {
        // First call installs; subsequent calls return Err (idempotent). Either
        // way the provider is now installed process-wide, which is what we want.
        let _ = install_crypto_provider();

        // Prove the process-level provider exists by asking rustls for a
        // ClientConfig builder. With no provider installed this returns Err /
        // panics ("Could not automatically determine the process-level
        // CryptoProvider"); with ring installed it succeeds.
        let provider = rustls::crypto::CryptoProvider::get_default();
        assert!(
            provider.is_some(),
            "CryptoProvider::get_default() returned None — ring was not installed process-wide"
        );
    }

    /// A self-signed cert + matching rustls server acceptor and a client config
    /// that trusts ONLY that cert. Used by the real TLS/WSS tests below so they
    /// exercise an actual rustls handshake (not just a plaintext connect). The
    /// cert is minted with `rcgen` (test-only dev-dependency; default features
    /// use ring — already a dependency — so no aws-lc-rs sneaks back in).
    struct SelfSignedTls {
        acceptor: tokio_rustls::TlsAcceptor,
        client_config: std::sync::Arc<rustls::ClientConfig>,
    }

    fn make_self_signed_tls() -> SelfSignedTls {
        use rcgen::{CertificateParams, DistinguishedName, DnType, KeyPair};
        use std::sync::Arc;

        // SAN must be an IP the client connects to (127.0.0.1) so rustls's
        // hostname verification passes.
        let mut params =
            CertificateParams::new(vec!["127.0.0.1".to_string(), "localhost".to_string()])
                .expect("cert params");
        params.distinguished_name = DistinguishedName::new();
        params
            .distinguished_name
            .push(DnType::CommonName, "relay-node-test");
        let key_pair = KeyPair::generate().unwrap();
        let cert = params.self_signed(&key_pair).unwrap();

        // Serialize to DER the way rustls consumes them.
        let cert_der = cert.der().clone();
        let key_der = key_pair.serialize_der();

        // Server: a rustls ServerConfig carrying this cert + key.
        let server_config = rustls::ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(
                vec![cert_der],
                rustls::pki_types::PrivateKeyDer::try_from(key_der).expect("private key der"),
            )
            .expect("server config");
        let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(server_config));

        // Client: trust ONLY this cert (no system roots). This makes the test
        // hermetic — no reliance on the host trust store — and forces a real
        // certificate verification during the TLS handshake.
        let mut root_store = rustls::RootCertStore::empty();
        root_store.add(cert.der().clone()).unwrap();
        let client_config = Arc::new(
            rustls::ClientConfig::builder()
                .with_root_certificates(root_store)
                .with_no_client_auth(),
        );

        SelfSignedTls {
            acceptor,
            client_config,
        }
    }

    /// v0.4.16 regression: building a reqwest client must not panic. reqwest
    /// (used by the poller/reporter for HTTPS panel URLs) builds a rustls
    /// ClientConfig internally; with a missing/ambiguous provider that panicked.
    /// We can't point reqwest at our self-signed cert hermetically without a
    /// different feature set, so this test covers the CLIENT BUILD path (which
    /// is where the historical panic lived) and asserts no panic. The actual
    /// end-to-end TLS handshake is covered by `wss_handshake_and_round_trip_*`.
    #[tokio::test]
    async fn reqwest_client_build_does_not_panic() {
        let _ = install_crypto_provider();
        // Building the client triggers reqwest's rustls ClientConfig
        // construction — the exact call that panicked pre-fix.
        let _client = reqwest::Client::builder()
            .build()
            .expect("reqwest client built (rustls provider must be installed)");
    }

    /// v0.4.16 regression (the real one): a `wss://` connection that performs a
    /// genuine rustls TLS handshake end-to-end must succeed and exchange a
    /// frame. This is the EXACT path the node's control channel uses
    /// (`connect_async(wss://...)` in ws_client.rs). Before the fix it panicked
    /// inside `connect_async`. We mint a self-signed cert, run a TLS WS server,
    /// and connect a `wss://` client whose rustls config trusts that cert — so
    /// both ServerConfig build (server) and ClientConfig build + handshake
    /// (client) are exercised with the ring provider installed.
    #[tokio::test]
    async fn wss_handshake_and_round_trip_succeeds() {
        use futures_util::{SinkExt, StreamExt};
        use tokio::net::TcpListener;
        use tokio_tungstenite::tungstenite::Message;

        let _ = install_crypto_provider();
        let tls = make_self_signed_tls();

        // TLS WebSocket server: accept TCP -> wrap in TLS -> run WS handshake ->
        // echo one frame.
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let acceptor = tls.acceptor.clone();
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let tls_stream = acceptor.accept(stream).await.expect("server TLS accept");
            let mut ws = tokio_tungstenite::accept_async(tls_stream)
                .await
                .expect("server WS handshake over TLS");
            if let Some(Ok(msg)) = ws.next().await {
                let payload = match msg {
                    Message::Binary(b) => b.as_ref().to_vec(),
                    Message::Text(t) => t.as_bytes().to_vec(),
                    _ => return,
                };
                let _ = ws.send(Message::Binary(payload.into())).await;
            }
        });

        // wss:// client with a rustls config trusting only our self-signed cert.
        let connector = tokio_tungstenite::Connector::Rustls(tls.client_config);
        let (mut client, _resp) = tokio_tungstenite::connect_async_tls_with_config(
            format!("wss://127.0.0.1:{}/", port),
            None,
            false,
            Some(connector),
        )
        .await
        .expect("wss connect (TLS handshake) must succeed with ring provider");
        client
            .send(Message::Binary(b"hello-wss".to_vec().into()))
            .await
            .expect("send over wss");

        let echoed = tokio::time::timeout(std::time::Duration::from_secs(5), client.next())
            .await
            .expect("timeout waiting for wss echo")
            .expect("stream ended")
            .expect("wss stream error");
        match echoed {
            Message::Binary(b) => assert_eq!(b.as_ref(), b"hello-wss"),
            other => panic!("expected binary echo over wss, got {:?}", other),
        }
        let _ = server.await;
    }
}
