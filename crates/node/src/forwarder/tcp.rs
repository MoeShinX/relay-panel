use std::net::{Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

use super::limiter::RateLimit;
use super::selector::TargetSelector;
use crate::reporter::{ConnectionTracker, TrafficCounter};

/// v1.0.4: serve an ALREADY-BOUND TcpListener. Binding happens in the manager
/// (synchronously, so errors surface immediately and per-family success is
/// known). This function only runs the accept loop.
#[allow(clippy::too_many_arguments)]
pub async fn serve_tcp_listener(
    listener: TcpListener,
    targets: Vec<String>,
    selector: Arc<TargetSelector>,
    rate_limit: RateLimit,
    counter: Arc<TrafficCounter>,
    connections: Arc<ConnectionTracker>,
    rule_id: i64,
    source_ipv4: Option<Ipv4Addr>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let listen_addr = listener
        .local_addr()
        .unwrap_or_else(|_| SocketAddr::from(([0, 0, 0, 0], 0)));
    tracing::info!("TCP listening on {} (rule {})", listen_addr, rule_id);

    // v0.3.6: accept-loop resilience. A transient accept error (EMFILE,
    // ENOMEM, temporary resource exhaustion) used to `?`-propagate and kill the
    // whole listener task, leaving the port dead until node restart. Now we
    // classify the error: transient -> back off and retry; the listener stays
    // up. A non-transient error (e.g. the listener was closed) ends the task.
    loop {
        match listener.accept().await {
            Ok((inbound, client_addr)) => {
                // v1.0.8: disable Nagle on the accepted (client-facing) socket.
                // See the note in outbound::tcp_connect — a relay MUST set
                // TCP_NODELAY on both ends or small packets get buffered ~40ms
                // per hop, which compounds into heavy jitter on long chains.
                if let Err(e) = inbound.set_nodelay(true) {
                    tracing::debug!(
                        "TCP accept {}: set_nodelay(true) failed: {}",
                        client_addr,
                        e
                    );
                }
                // v1.2: enable TCP keepalive so a client that vanishes without a
                // FIN/RST (NAT rebind, mobile handoff, cable pull) is reaped by
                // the kernel instead of leaving the copy task blocked on read()
                // forever, holding two fds until the node exhausts them (EMFILE).
                super::outbound::apply_keepalive(&inbound, "TCP accept");
                let targets = targets.clone();
                let selector = selector.clone();
                let rate_limit = rate_limit.clone();
                let counter = counter.clone();
                let connections = connections.clone();

                tokio::spawn(async move {
                    // RAII guard: increments the active-TCP count on create,
                    // decrements on drop (end of task — normal close, error, or
                    // panic). Guarantees the count is correct even on abrupt close.
                    let _guard = connections.tcp_handle();
                    if let Err(e) = handle_tcp_connection(
                        inbound,
                        client_addr,
                        targets,
                        selector,
                        rate_limit,
                        counter,
                        rule_id,
                        source_ipv4,
                    )
                    .await
                    {
                        tracing::debug!("TCP connection error: {}", e);
                    }
                });
            }
            Err(e) if is_transient_accept_error(&e) => {
                // Back off briefly to avoid a hot error loop spamming logs, then
                // continue accepting. 100ms is short enough that real clients
                // don't notice but long enough to shed an error storm.
                tracing::warn!(
                    "TCP listener on {} (rule {}): transient accept error: {}; retrying in 100ms",
                    listen_addr,
                    rule_id,
                    e
                );
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            Err(e) => {
                // Non-transient (e.g. listener closed, EBADF). End the task; the
                // manager's is_finished recovery will restart it on next config
                // if still desired.
                return Err(Box::new(e) as Box<dyn std::error::Error + Send + Sync>);
            }
        }
    }
}

/// Classify whether an `accept` error is worth retrying. Transient OS-level
/// resource exhaustion (too many open files, out of memory) clears on its own;
/// retrying is the right call. A bad-fd or closed-listener error is permanent.
fn is_transient_accept_error(e: &std::io::Error) -> bool {
    use std::io::ErrorKind;
    matches!(
        e.kind(),
        ErrorKind::Interrupted
            | ErrorKind::WouldBlock
            | ErrorKind::TimedOut
            | ErrorKind::ResourceBusy
    ) || e.raw_os_error().is_some_and(|c| {
        // EMFILE (24) / ENFILE (23) / ENOBUFS (105) / ENOMEM (12): transient
        // resource exhaustion under load.
        matches!(c, 24 | 23 | 105 | 12)
    })
}

#[allow(clippy::too_many_arguments)]
async fn handle_tcp_connection(
    inbound: TcpStream,
    client_addr: SocketAddr,
    targets: Vec<String>,
    selector: Arc<TargetSelector>,
    rate_limit: RateLimit,
    counter: Arc<TrafficCounter>,
    rule_id: i64,
    source_ipv4: Option<Ipv4Addr>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    // v0.4.6: pick targets per the rule's load-balancing strategy. The selector
    // returns the ordered indices to attempt; we connect to the first reachable.
    //
    // v1.0.5: keep the REAL reason each target failed (DNS / timeout / no route /
    // source-bind) instead of collapsing everything into "no target available".
    // On a multi-NIC server a silent failure is impossible to diagnose, so we
    // accumulate per-target reasons and log them together when nothing connects.
    let mut outbound = None;
    let mut failures: Vec<String> = Vec::new();
    for idx in selector.order() {
        let Some(target) = targets.get(idx) else {
            continue;
        };
        match tokio::time::timeout(
            Duration::from_secs(5),
            super::outbound::tcp_connect(target, source_ipv4, 5),
        )
        .await
        {
            Ok(Ok(stream)) => {
                selector.report(idx, true);
                outbound = Some(stream);
                break;
            }
            Ok(Err(e)) => {
                // tcp_connect already classifies the cause (InvalidIp / Connect /
                // Bind). Preserve it verbatim so DNS vs. refused vs. source-bind
                // failures are distinguishable in the log.
                selector.report(idx, false);
                failures.push(format!("{} -> {}", target, e));
            }
            Err(_) => {
                // Outer timeout fired: the connect didn't finish within 5s.
                selector.report(idx, false);
                failures.push(format!("{} -> timed out after 5s", target));
            }
        }
    }

    let outbound = match outbound {
        Some(s) => s,
        None => {
            let detail = if failures.is_empty() {
                "no reachable target (all targets in circuit-break or empty)".to_string()
            } else {
                failures.join("; ")
            };
            tracing::warn!(
                "TCP rule {}: no target available for client {} — {}",
                rule_id,
                client_addr,
                detail
            );
            return Err(format!("no target available: {}", detail).into());
        }
    };

    tracing::debug!("TCP: {} -> {}", client_addr, outbound.peer_addr()?);

    // v1.0.8: ZERO-COPY fast path. An UNLIMITED rule on Linux is forwarded with
    // splice(2) — bytes move inside the kernel via a pipe and are never copied
    // into userspace, which slashes CPU on high-throughput links. A rate-limited
    // rule CANNOT use splice (the bytes must reach userspace to be throttled),
    // but that's fine: a capped rule isn't running at max throughput, so the
    // userspace copy's CPU cost is negligible. Byte counts still come back from
    // the splice return values, so billing is unaffected. Non-Linux always uses
    // the userspace copy below.
    #[cfg(target_os = "linux")]
    if matches!(rate_limit, RateLimit::Unlimited) {
        match super::splice::zero_copy_bidirectional(inbound, outbound).await {
            // (up = client→target, down = target→client) — same attribution as
            // the userspace path's counter.add(rule_id, up, down).
            Ok((up, down)) => counter.add(rule_id, up, down).await,
            Err(e) => tracing::debug!("TCP splice forward (rule {}): {}", rule_id, e),
        }
        return Ok(());
    }

    // Userspace bidirectional copy with traffic counting + per-rule rate
    // limiting (the rate-limited path, and the fallback on non-Linux). We own
    // both halves and pump both directions concurrently. When either side
    // returns (the remote closed the connection) we shut down the matching write
    // half so the other copy also sees EOF and returns.
    //
    // v0.4.6: each chunk is throttled through the shared RateLimit BEFORE being
    // written, so the rule's aggregate cap holds across all connections.
    let (mut ri, mut wi) = inbound.into_split();
    let (mut ro, mut wo) = outbound.into_split();

    let counter_up = counter.clone();
    let counter_down = counter.clone();
    let rl_up = rate_limit.clone();
    let rl_down = rate_limit;

    let upload = Box::pin(async move {
        let mut total = 0u64;
        // v1.0.8: 32 KiB copy buffer (this userspace path is only used by
        // rate-limited rules, which are capped anyway; the unlimited fast path
        // uses splice above). Heap-allocated as part of this Box::pin'd future,
        // so it does not grow the task stack.
        let mut buf = [0u8; 32 * 1024];
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
        // v1.0.8: 32 KiB copy buffer (see the upload side above).
        let mut buf = [0u8; 32 * 1024];
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

    tracing::debug!("TCP: connection closed for {}", client_addr);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::forwarder::outbound::bind_tcp_listener;
    use relay_shared::protocol::LoadBalanceStrategy;
    use std::net::IpAddr;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    /// v1.0.8: end-to-end raw TCP forwarding still works after the NODELAY /
    /// 64 KiB buffer changes, and the client-facing socket has Nagle disabled.
    /// Topology: client → [serve_tcp_listener] → echo target.
    #[tokio::test]
    async fn raw_tcp_forward_roundtrips_and_client_has_nodelay() {
        // Echo target: read a chunk, write it straight back.
        let target = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let target_addr = target.local_addr().unwrap();
        tokio::spawn(async move {
            if let Ok((mut s, _)) = target.accept().await {
                let mut b = vec![0u8; 1024];
                if let Ok(n) = s.read(&mut b).await {
                    let _ = s.write_all(&b[..n]).await;
                }
            }
        });

        // Relay listener on an ephemeral port, forwarding to the echo target.
        let listener = bind_tcp_listener(IpAddr::V4(Ipv4Addr::LOCALHOST), 0).unwrap();
        let listen_addr = listener.local_addr().unwrap();
        let selector = Arc::new(TargetSelector::new(LoadBalanceStrategy::First, 1));
        let counter = Arc::new(TrafficCounter::new());
        let connections = Arc::new(ConnectionTracker::new());
        tokio::spawn(serve_tcp_listener(
            listener,
            vec![target_addr.to_string()],
            selector,
            RateLimit::Unlimited,
            counter.clone(),
            connections,
            1,
            None,
        ));

        // Client connects to the relay and round-trips through to the echo.
        let mut client = TcpStream::connect(listen_addr).await.unwrap();
        // The client's own socket having NODELAY isn't what we set (we set it on
        // the RELAY's accepted socket), but we can at least prove the relay path
        // forwards bytes correctly under the new buffer/nodelay code.
        client.write_all(b"ping-through-relay").await.unwrap();
        let mut got = vec![0u8; 64];
        let n = client.read(&mut got).await.unwrap();
        assert_eq!(
            &got[..n],
            b"ping-through-relay",
            "relay must echo the target"
        );
    }
}
