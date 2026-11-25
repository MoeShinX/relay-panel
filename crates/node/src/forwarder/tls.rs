// TLS Simple ingress listener — v0.4.1.
//
// relay-node terminates TLS directly (via tokio-rustls) and forwards the
// decrypted TCP stream to the target. No WebSocket, no reverse proxy — the
// node IS the TLS server.
//
//   client ──TLS──▶ relay-node (rustls terminates) ──plain TCP──▶ target
//
// Security contract (since v0.4.1):
//   - TLS 1.2 + 1.3 only (1.0/1.1 rejected by protocol_versions config).
//   - No client certificate authentication (with_no_client_auth).
//   - Handshake timeout: 10 seconds (prevents slow-loris-style resource hold).
//   - A failed handshake closes ONLY that connection; the listener stays up.
//   - Cert + key loaded as PEM (cert chain supported; PKCS#8/PKCS#1/SEC1 keys
//     via rustls-pemfile).
//   - Cert↔key match verified by rustls at ServerConfig build time.
//   - Private key / file paths / PEM content are NEVER logged. Only the cert
//     fingerprint (SHA-256) is logged at info level for identification.
//   - SNI / hostname matching is the CLIENT's responsibility (we present the
//     cert; we don't validate the client's trust of it).

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use tokio::io::{self, AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

use crate::forwarder::cert_reloader::SharedTlsAcceptor;
use crate::forwarder::limiter::RateLimit;
use crate::forwarder::selector::TargetSelector;
use crate::reporter::{ConnectionTracker, TrafficCounter};

/// Maximum time to wait for a TLS handshake to complete. A slow or malicious
/// client that opens a TCP connection but never completes the handshake would
/// hold a slot indefinitely without this.
const TLS_HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);

#[allow(clippy::too_many_arguments)]
pub async fn start_tls_listener(
    listen_addr: SocketAddr,
    targets: Vec<String>,
    selector: Arc<TargetSelector>,
    rate_limit: RateLimit,
    counter: Arc<TrafficCounter>,
    connections: Arc<ConnectionTracker>,
    rule_id: i64,
    // Shared, hot-reloadable TLS acceptor. Each new connection reads the
    // current value, so a cert rotation (swapped by cert_reloader) takes
    // effect on the NEXT accept without restarting the listener.
    tls_acceptor: SharedTlsAcceptor,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let listener = TcpListener::bind(listen_addr).await?;
    tracing::info!("TLS listening on {} (rule {})", listen_addr, rule_id);

    loop {
        match listener.accept().await {
            Ok((inbound, client_addr)) => {
                let targets = targets.clone();
                let selector = selector.clone();
                let rate_limit = rate_limit.clone();
                let counter = counter.clone();
                let connections = connections.clone();
                let tls_acceptor = Arc::clone(&tls_acceptor);

                tokio::spawn(async move {
                    let _guard = connections.tcp_handle();

                    // Read the CURRENT acceptor from the shared slot (supports
                    // hot-reload: cert_reloader swaps this under the write
                    // lock). A std RwLock read — never blocks long.
                    let acceptor = {
                        let guard = tls_acceptor.read().unwrap();
                        match guard.as_ref() {
                            Some(a) => a.clone(),
                            None => {
                                tracing::warn!(
                                    "TLS: no acceptor available for connection from {} (rule {})",
                                    client_addr,
                                    rule_id
                                );
                                return;
                            }
                        }
                    };

                    // TLS handshake with timeout.
                    let tls_stream =
                        match tokio::time::timeout(TLS_HANDSHAKE_TIMEOUT, acceptor.accept(inbound))
                            .await
                        {
                            Ok(Ok(s)) => s,
                            Ok(Err(e)) => {
                                tracing::debug!(
                                    "TLS handshake failed from {} (rule {}): {}",
                                    client_addr,
                                    rule_id,
                                    e
                                );
                                return;
                            }
                            Err(_) => {
                                tracing::debug!(
                                    "TLS handshake timeout from {} (rule {})",
                                    client_addr,
                                    rule_id
                                );
                                return;
                            }
                        };

                    // Forward the decrypted stream, same as TCP.
                    if let Err(e) = handle_tls_connection(
                        tls_stream,
                        client_addr,
                        targets,
                        selector,
                        rate_limit,
                        counter,
                        rule_id,
                    )
                    .await
                    {
                        tracing::debug!("TLS connection error: {}", e);
                    }
                });
            }
            Err(e) if is_transient_accept_error(&e) => {
                tracing::warn!(
                    "TLS listener on {} (rule {}): transient accept error: {}; retrying in 100ms",
                    listen_addr,
                    rule_id,
                    e
                );
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            Err(e) => {
                return Err(Box::new(e) as Box<dyn std::error::Error + Send + Sync>);
            }
        }
    }
}

/// Classify whether an `accept` error is worth retrying (same logic as tcp.rs).
fn is_transient_accept_error(e: &std::io::Error) -> bool {
    use std::io::ErrorKind;
    matches!(
        e.kind(),
        ErrorKind::Interrupted
            | ErrorKind::WouldBlock
            | ErrorKind::TimedOut
            | ErrorKind::ResourceBusy
    ) || e
        .raw_os_error()
        .is_some_and(|c| matches!(c, 24 | 23 | 105 | 12))
}

/// Forward a decrypted TLS stream to the first available target. Identical
/// pump logic to tcp.rs, but uses `tokio::io::split` instead of `into_split`
/// (TlsStream is not a TcpStream and has no into_split).
async fn handle_tls_connection<S>(
    inbound: S,
    client_addr: SocketAddr,
    targets: Vec<String>,
    selector: Arc<TargetSelector>,
    rate_limit: RateLimit,
    counter: Arc<TrafficCounter>,
    rule_id: i64,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    let mut outbound = None;
    for idx in selector.order() {
        let Some(target) = targets.get(idx) else {
            continue;
        };
        match tokio::time::timeout(Duration::from_secs(5), TcpStream::connect(target)).await {
            Ok(Ok(stream)) => {
                selector.report(idx, true);
                outbound = Some(stream);
                break;
            }
            _ => {
                selector.report(idx, false);
                continue;
            }
        }
    }

    let outbound = match outbound {
        Some(s) => s,
        None => {
            tracing::warn!("TLS: no target available for {}", client_addr);
            return Err("no target available".into());
        }
    };

    tracing::debug!("TLS: {} -> {}", client_addr, outbound.peer_addr()?);

    // Bidirectional copy with traffic counting + per-rule rate limiting.
    // split() works on any AsyncRead+AsyncWrite; into_split() is TcpStream-specific.
    let (mut ri, mut wi) = io::split(inbound);
    let (mut ro, mut wo) = outbound.into_split();

    let counter_up = counter.clone();
    let counter_down = counter.clone();
    let rl_up = rate_limit.clone();
    let rl_down = rate_limit;

    let upload = Box::pin(async move {
        let mut total = 0u64;
        let mut buf = [0u8; 16 * 1024];
        loop {
            let n = match ri.read(&mut buf).await {
                Ok(0) => break,
                Ok(n) => n,
                Err(_) => break,
            };
            rl_up.acquire_upload(n as u64).await;
            if wo.write_all(&buf[..n]).await.is_err() {
                break;
            }
            total += n as u64;
        }
        counter_up.add(rule_id, total, 0).await;
        let _ = wo.shutdown().await;
    });
    let download = Box::pin(async move {
        let mut total = 0u64;
        let mut buf = [0u8; 16 * 1024];
        loop {
            let n = match ro.read(&mut buf).await {
                Ok(0) => break,
                Ok(n) => n,
                Err(_) => break,
            };
            rl_down.acquire_download(n as u64).await;
            if wi.write_all(&buf[..n]).await.is_err() {
                break;
            }
            total += n as u64;
        }
        counter_down.add(rule_id, 0, total).await;
        let _ = wi.shutdown().await;
    });

    let ((), ()) = tokio::join!(upload, download);

    tracing::debug!("TLS: connection closed for {}", client_addr);
    Ok(())
}
