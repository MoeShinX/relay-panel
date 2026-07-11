//! v1.0.4: IPv4 outbound source-address binding for multi-NIC servers.
//!
//! Servers with separate NICs for IPv6 ingress and IPv4 egress need the
//! outbound TCP/UDP connection to originate from a specific IPv4 address.
//! This module provides a clean, testable abstraction over that choice.
//!
//! Configuration priority:
//! 1. OUTBOUND_BIND_IPV4 → use that exact IPv4 (fail on parse/not-local).
//! 2. OUTBOUND_INTERFACE → resolve the NIC's IPv4 address.
//! 3. Neither / OUTBOUND_INTERFACE=auto → system auto-route (no bind).

use socket2::{Domain, Protocol as S2Protocol, Socket, Type};
use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, SocketAddrV4};
use std::sync::OnceLock;
use std::time::{Duration, Instant};
use tokio::net::{TcpListener, TcpSocket, TcpStream, UdpSocket};
use tokio::sync::Mutex as AsyncMutex;

// ── errors ──

#[allow(dead_code)]
#[derive(Debug)]
pub enum OutboundError {
    InvalidIp(String),
    IpNotLocal(String, String),
    InterfaceNotFound(String),
    InterfaceNoIpv4(String),
    Bind(std::io::Error),
    Connect(std::io::Error),
}

impl std::fmt::Display for OutboundError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidIp(s) => write!(f, "invalid IPv4 address: {}", s),
            Self::IpNotLocal(ip, iface) => write!(f, "IPv4 {} is not on interface {}", ip, iface),
            Self::InterfaceNotFound(name) => write!(f, "interface not found: {}", name),
            Self::InterfaceNoIpv4(name) => {
                write!(f, "interface {} has no IPv4 address", name)
            }
            Self::Bind(e) => write!(f, "bind failed: {}", e),
            Self::Connect(e) => write!(f, "connect failed: {}", e),
        }
    }
}

impl std::error::Error for OutboundError {}

// ── config ──

#[derive(Debug, Clone)]
pub struct OutboundConfig {
    /// Exact IPv4 source address, e.g. "10.0.2.61".
    pub bind_ipv4: Option<String>,
    /// NIC name to resolve IPv4 from, e.g. "ens18". "auto" means no bind.
    pub interface: String,
}

impl Default for OutboundConfig {
    fn default() -> Self {
        Self {
            bind_ipv4: None,
            interface: "auto".to_string(),
        }
    }
}

/// Resolve the source IPv4 address from the outbound configuration.
/// Returns None when the system should auto-route (no bind).
fn resolve_bind_ipv4(config: &OutboundConfig) -> Result<Option<Ipv4Addr>, OutboundError> {
    // Priority 1: explicit OUTBOUND_BIND_IPV4.
    if let Some(ref raw) = config.bind_ipv4 {
        let ip: Ipv4Addr = raw
            .parse()
            .map_err(|_| OutboundError::InvalidIp(raw.clone()))?;
        // Verify the IP is actually local.
        if !is_local_ipv4(ip) {
            return Err(OutboundError::IpNotLocal(
                raw.clone(),
                config.interface.clone(),
            ));
        }
        tracing::info!("outbound: source IPv4 = {} (explicit)", ip);
        return Ok(Some(ip));
    }

    // Priority 2: OUTBOUND_INTERFACE (if not "auto").
    if config.interface != "auto" {
        let ip = interface_ipv4(&config.interface)?;
        tracing::info!(
            "outbound: source IPv4 = {} (from interface {})",
            ip,
            config.interface
        );
        return Ok(Some(ip));
    }

    // Priority 3: auto-route.
    tracing::info!("outbound: system auto-route (no source bind)");
    Ok(None)
}

// ── platform helpers ──

/// v1.0.4: verify an IPv4 address actually belongs to this host by trying to
/// bind a UDP socket to it. A non-local address fails with EADDRNOTAVAIL.
/// This is cross-platform and needs no external commands — the kernel is the
/// authority on which addresses are local. Used at startup so a typo'd or
/// foreign OUTBOUND_BIND_IPV4 aborts the node instead of silently sending
/// traffic from the wrong (or no) source.
fn is_local_ipv4(ip: Ipv4Addr) -> bool {
    std::net::UdpSocket::bind(SocketAddrV4::new(ip, 0)).is_ok()
}

fn interface_ipv4(name: &str) -> Result<Ipv4Addr, OutboundError> {
    // Use the OS to find the interface's IPv4 address.
    // Cross-platform approach: iterate network interfaces.
    #[cfg(target_os = "linux")]
    {
        // On Linux, we can use `ip -4 addr show dev {name}` via std::process.
        // For a no-dependency approach, try creating a UDP socket bound to the
        // interface via SO_BINDTODEVICE and reading the local address.
        // Fallback: try `getifaddrs` or parse `/proc/net/fib_trie`.
        let output = std::process::Command::new("ip")
            .args(["-4", "addr", "show", "dev", name, "scope", "global"])
            .output()
            .map_err(|_| OutboundError::InterfaceNotFound(name.to_string()))?;

        if !output.status.success() {
            return Err(OutboundError::InterfaceNotFound(name.to_string()));
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        for line in stdout.lines() {
            let trimmed = line.trim();
            if trimmed.starts_with("inet ") {
                // "inet 10.0.2.61/23 brd 10.0.2.255 scope global ens18"
                let parts: Vec<&str> = trimmed.split_whitespace().collect();
                if parts.len() >= 2 {
                    let cidr = parts[1];
                    if let Some(ip_str) = cidr.split('/').next() {
                        if let Ok(ip) = ip_str.parse::<Ipv4Addr>() {
                            return Ok(ip);
                        }
                    }
                }
            }
        }
        Err(OutboundError::InterfaceNoIpv4(name.to_string()))
    }

    #[cfg(not(target_os = "linux"))]
    {
        let _ = name;
        Err(OutboundError::InterfaceNotFound(
            "interface resolution only supported on Linux".into(),
        ))
    }
}

// ── TCP ──

/// Create a TCP stream connected to `target`, optionally binding to
/// `source_ipv4:0` first.
///
/// v1.0.8: TCP_NODELAY is set on the outbound stream. This node is a relay —
/// disabling Nagle's algorithm is essential. With Nagle ON, small writes are
/// buffered until an ACK returns, and combined with the peer's delayed-ACK
/// (~40ms) this adds up to ~40ms of variable latency PER HOP. On a long relay
/// chain that compounds into severe jitter and makes interactive traffic
/// (SSH/RDP/gaming/request-response) unusable. Every serious proxy sets
/// TCP_NODELAY on both ends; the inbound (accept) side is set in tcp.rs.
pub async fn tcp_connect(
    target: &str,
    source_ipv4: Option<Ipv4Addr>,
    _timeout_secs: u64,
) -> Result<TcpStream, OutboundError> {
    // v1.0.9: resolve through the DNS cache instead of re-resolving on every
    // connection. The caller (tcp.rs) still wraps this in a per-attempt timeout.
    let addrs = resolve_cached(target)
        .await
        .map_err(OutboundError::Connect)?;
    if addrs.is_empty() {
        return Err(OutboundError::Connect(std::io::Error::new(
            std::io::ErrorKind::AddrNotAvailable,
            "could not resolve target",
        )));
    }
    let stream = match source_ipv4 {
        None => connect_first(&addrs).await?,
        Some(src) => tcp_connect_bound(&addrs, src).await?,
    };
    // Non-fatal: a socket that just connected virtually never rejects this.
    if let Err(e) = stream.set_nodelay(true) {
        tracing::debug!("outbound: set_nodelay(true) failed for {}: {}", target, e);
    }
    apply_keepalive(&stream, "outbound");
    Ok(stream)
}

/// Enable TCP keepalive on a forwarded socket — applied to BOTH the accepted
/// client socket (see `tcp::serve_tcp_listener`) and the dialed target socket
/// above, so a dead peer on either side is detected.
///
/// Why this matters: the bidirectional copy blocks on `read()` until the peer
/// sends FIN/RST. A peer that vanishes SILENTLY (NAT rebind, mobile handoff,
/// cable pull, a firewall that drops instead of resets) never sends one, so the
/// copy task hangs forever holding two fds. Under connection churn these dead
/// half-open connections pile up until the process hits `EMFILE` ("Too many
/// open files", os error 24) — exhausting even a systemd node's
/// `LimitNOFILE=65536`. Keepalive makes the kernel probe an idle peer and RST
/// it when dead, so `read()` returns and the task releases its fds.
///
/// idle 60s, then a probe every 15s up to 4 times → a dead peer is reaped
/// within ~120s. A live connection resets the idle timer on every byte, so busy
/// links never emit a probe.
pub(super) fn apply_keepalive(stream: &TcpStream, ctx: &str) {
    #[cfg(unix)]
    {
        let ka = socket2::TcpKeepalive::new()
            .with_time(Duration::from_secs(60))
            .with_interval(Duration::from_secs(15))
            .with_retries(4);
        if let Err(e) = socket2::SockRef::from(stream).set_tcp_keepalive(&ka) {
            tracing::debug!("{}: set_tcp_keepalive failed: {}", ctx, e);
        }
    }
    #[cfg(not(unix))]
    {
        // Keepalive tuning is a unix concern; the node only runs on Linux.
        let _ = (stream, ctx);
    }
}

/// Try each resolved address in order until one connects — mirrors
/// `TcpStream::connect(host:port)`'s multi-address behavior over our cached
/// resolution.
async fn connect_first(addrs: &[SocketAddr]) -> Result<TcpStream, OutboundError> {
    let mut last_err = None;
    for addr in addrs {
        match TcpStream::connect(addr).await {
            Ok(s) => return Ok(s),
            Err(e) => last_err = Some(e),
        }
    }
    Err(OutboundError::Connect(last_err.unwrap_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::AddrNotAvailable, "no address")
    })))
}

async fn tcp_connect_bound(
    addrs: &[SocketAddr],
    src: Ipv4Addr,
) -> Result<TcpStream, OutboundError> {
    // Use the first resolved address (matches the previous behavior). An IPv4
    // target uses the source bind; an IPv6 target can't (the source is IPv4),
    // so it falls back to a plain connect.
    let Some(&addr) = addrs.first() else {
        return Err(OutboundError::Connect(std::io::Error::new(
            std::io::ErrorKind::AddrNotAvailable,
            "could not resolve target",
        )));
    };
    match addr {
        SocketAddr::V4(v4) => {
            let sock = TcpSocket::new_v4().map_err(OutboundError::Bind)?;
            sock.bind(SocketAddrV4::new(src, 0).into())
                .map_err(OutboundError::Bind)?;
            sock.connect(SocketAddr::from(v4))
                .await
                .map_err(OutboundError::Connect)
        }
        SocketAddr::V6(_) => TcpStream::connect(addr)
            .await
            .map_err(OutboundError::Connect),
    }
}

// ── DNS cache ──

/// How long a resolved target is reused before re-resolving. Short enough to
/// follow DNS changes within a minute, long enough to spare a lookup on every
/// new connection to a domain target.
const DNS_CACHE_TTL: Duration = Duration::from_secs(30);

struct CachedDns {
    addrs: Vec<SocketAddr>,
    at: Instant,
}

fn dns_cache() -> &'static AsyncMutex<HashMap<String, CachedDns>> {
    static CACHE: OnceLock<AsyncMutex<HashMap<String, CachedDns>>> = OnceLock::new();
    CACHE.get_or_init(|| AsyncMutex::new(HashMap::new()))
}

/// Resolve `target` ("host:port") to socket addresses, caching for
/// `DNS_CACHE_TTL`. On a resolver error a stale cached entry (if any) is reused
/// rather than failing the connection outright — DNS blips shouldn't drop links.
///
/// `pub(super)` so the UDP forwarder can re-resolve session targets through the
/// same cache (following DDNS changes) instead of pinning a boot-time IP.
pub(super) async fn resolve_cached(target: &str) -> std::io::Result<Vec<SocketAddr>> {
    {
        let cache = dns_cache().lock().await;
        if let Some(c) = cache.get(target) {
            if c.at.elapsed() < DNS_CACHE_TTL {
                return Ok(c.addrs.clone());
            }
        }
    }
    match tokio::net::lookup_host(target).await {
        Ok(it) => {
            let addrs: Vec<SocketAddr> = it.collect();
            if !addrs.is_empty() {
                dns_cache().lock().await.insert(
                    target.to_string(),
                    CachedDns {
                        addrs: addrs.clone(),
                        at: Instant::now(),
                    },
                );
            }
            Ok(addrs)
        }
        Err(e) => {
            if let Some(c) = dns_cache().lock().await.get(target) {
                tracing::debug!(
                    "outbound: DNS for {} failed ({}); using stale cached addrs",
                    target,
                    e
                );
                return Ok(c.addrs.clone());
            }
            Err(e)
        }
    }
}

// ── UDP ──

/// v1.0.9: requested UDP socket buffer size (bytes) for both send and receive.
/// Absorbs bursts / high packet rates so the kernel drops fewer datagrams. The
/// OS clamps this to net.core.{r,w}mem_max, so requesting more than the cap is
/// harmless (best-effort — a failure to set never fails the bind).
const UDP_SOCKET_BUFFER_BYTES: usize = 4 * 1024 * 1024;

/// Bind a UDP socket for outbound traffic. When `source_ipv4` is set,
/// binds to `{src}:0`; otherwise binds to `0.0.0.0:0` (system auto).
pub async fn udp_outbound_socket(
    source_ipv4: Option<Ipv4Addr>,
) -> Result<UdpSocket, OutboundError> {
    let bind_addr: SocketAddr = match source_ipv4 {
        Some(src) => SocketAddr::V4(SocketAddrV4::new(src, 0)),
        None => "0.0.0.0:0".parse().unwrap(),
    };
    // v1.0.9: build via socket2 so we can enlarge the send/recv buffers before
    // the socket goes live. (IPv4 outbound only — unchanged from the previous
    // 0.0.0.0/src bind.)
    let sock = Socket::new(Domain::IPV4, Type::DGRAM, Some(S2Protocol::UDP))
        .map_err(OutboundError::Bind)?;
    sock.set_nonblocking(true).map_err(OutboundError::Bind)?;
    let _ = sock.set_recv_buffer_size(UDP_SOCKET_BUFFER_BYTES);
    let _ = sock.set_send_buffer_size(UDP_SOCKET_BUFFER_BYTES);
    sock.bind(&bind_addr.into()).map_err(OutboundError::Bind)?;
    let std_sock: std::net::UdpSocket = sock.into();
    UdpSocket::from_std(std_sock).map_err(OutboundError::Bind)
}

// ── dual-stack listener binding ──

/// Build a TCP listener bound to the given IP + port. For IPv6 addresses,
/// sets IPV6_V6ONLY so the IPv6 socket does NOT also claim the IPv4 wildcard
/// (which would make a separate 0.0.0.0 bind fail with EADDRINUSE on Linux,
/// where bindv6only defaults to 0).
///
/// The address is constructed via SocketAddr::new — NEVER string-formatted —
/// so "::" + port can never produce the broken ":::port" form.
pub fn bind_tcp_listener(ip: IpAddr, port: u16) -> Result<TcpListener, OutboundError> {
    let addr = SocketAddr::new(ip, port);
    let domain = match ip {
        IpAddr::V4(_) => Domain::IPV4,
        IpAddr::V6(_) => Domain::IPV6,
    };
    let sock =
        Socket::new(domain, Type::STREAM, Some(S2Protocol::TCP)).map_err(OutboundError::Bind)?;
    if ip.is_ipv6() {
        sock.set_only_v6(true).map_err(OutboundError::Bind)?;
    }
    sock.set_reuse_address(true).map_err(OutboundError::Bind)?;
    sock.set_nonblocking(true).map_err(OutboundError::Bind)?;
    sock.bind(&addr.into()).map_err(OutboundError::Bind)?;
    sock.listen(1024).map_err(OutboundError::Bind)?;
    let std_listener: std::net::TcpListener = sock.into();
    TcpListener::from_std(std_listener).map_err(OutboundError::Bind)
}

/// Build a UDP socket bound to the given IP + port (for inbound listeners).
/// IPv6 sockets get IPV6_V6ONLY for the same reason as TCP.
pub fn bind_udp_socket(ip: IpAddr, port: u16) -> Result<UdpSocket, OutboundError> {
    let addr = SocketAddr::new(ip, port);
    let domain = match ip {
        IpAddr::V4(_) => Domain::IPV4,
        IpAddr::V6(_) => Domain::IPV6,
    };
    let sock =
        Socket::new(domain, Type::DGRAM, Some(S2Protocol::UDP)).map_err(OutboundError::Bind)?;
    if ip.is_ipv6() {
        sock.set_only_v6(true).map_err(OutboundError::Bind)?;
    }
    sock.set_reuse_address(true).map_err(OutboundError::Bind)?;
    sock.set_nonblocking(true).map_err(OutboundError::Bind)?;
    // v1.0.9: enlarge inbound UDP buffers to absorb bursts (best-effort).
    let _ = sock.set_recv_buffer_size(UDP_SOCKET_BUFFER_BYTES);
    let _ = sock.set_send_buffer_size(UDP_SOCKET_BUFFER_BYTES);
    sock.bind(&addr.into()).map_err(OutboundError::Bind)?;
    let std_sock: std::net::UdpSocket = sock.into();
    UdpSocket::from_std(std_sock).map_err(OutboundError::Bind)
}

/// Parse a listen address string ("0.0.0.0", "::", or empty) into an IpAddr.
/// Returns None when the string is empty (family disabled).
pub fn parse_listen_ip(s: &str) -> Option<IpAddr> {
    let t = s.trim();
    if t.is_empty() {
        return None;
    }
    t.parse::<IpAddr>().ok()
}

// ── init ──

/// Called once at startup. Validates the outbound config and returns
/// the resolved source IPv4 (or None for auto-route).
///
/// v1.0.4 fix: a MISCONFIGURED outbound (invalid IP, missing interface,
/// non-local IP) returns Err — the caller decides whether to abort. It does
/// NOT silently fall back to auto-route, which could send traffic out the
/// wrong NIC without the operator noticing.
pub fn init_outbound(config: &OutboundConfig) -> Result<Option<Ipv4Addr>, OutboundError> {
    resolve_bind_ipv4(config)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// v1.0.9: resolve_cached returns an IP target verbatim (lookup_host on an
    /// IP:port is local, no network) and a second call is served identically —
    /// exercising the cache-hit path.
    #[tokio::test]
    async fn dns_cache_resolves_and_reuses_ip_target() {
        let want: SocketAddr = "127.0.0.1:9".parse().unwrap();
        let a = resolve_cached("127.0.0.1:9").await.expect("resolve ok");
        assert_eq!(a, vec![want]);
        let b = resolve_cached("127.0.0.1:9").await.expect("cached ok");
        assert_eq!(a, b);
    }

    /// v1.2: apply_keepalive must not error on a live connected socket, and on
    /// unix must actually turn SO_KEEPALIVE on. Exercises the real setsockopt
    /// path on Linux (CI + the node's real target); a no-op elsewhere.
    /// Regression guard for the fd-exhaustion fix.
    #[tokio::test]
    async fn apply_keepalive_enables_so_keepalive() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let accept = tokio::spawn(async move { listener.accept().await.unwrap().0 });
        let client = tokio::net::TcpStream::connect(addr).await.unwrap();
        let server = accept.await.unwrap();
        // The two call sites in production: the dialed (outbound) and accepted
        // (inbound) sockets.
        apply_keepalive(&client, "test-client");
        apply_keepalive(&server, "test-server");
        #[cfg(unix)]
        {
            assert!(
                socket2::SockRef::from(&client).keepalive().unwrap(),
                "SO_KEEPALIVE must be on after apply_keepalive (client)"
            );
            assert!(
                socket2::SockRef::from(&server).keepalive().unwrap(),
                "SO_KEEPALIVE must be on after apply_keepalive (server)"
            );
        }
    }

    #[test]
    fn auto_mode_no_bind() {
        let cfg = OutboundConfig::default();
        let ip = init_outbound(&cfg).expect("auto must succeed");
        assert!(ip.is_none(), "auto should not bind");
    }

    #[test]
    fn invalid_ip_rejected() {
        let cfg = OutboundConfig {
            bind_ipv4: Some("not-an-ip".into()),
            interface: "auto".into(),
        };
        // v1.0.4: invalid IP must ERROR, not silently fall back.
        assert!(init_outbound(&cfg).is_err(), "invalid IP must be an error");
    }

    #[test]
    fn explicit_ip_accepted_if_valid() {
        // 127.0.0.1 is always local, so it should be accepted.
        let cfg = OutboundConfig {
            bind_ipv4: Some("127.0.0.1".into()),
            interface: "auto".into(),
        };
        let ip = init_outbound(&cfg).expect("valid local IP must succeed");
        assert_eq!(ip, Some(Ipv4Addr::new(127, 0, 0, 1)));
    }

    #[test]
    fn non_local_ip_rejected_at_startup() {
        // 192.0.2.1 is TEST-NET-1 (RFC 5737), never assigned to a real host,
        // so binding to it fails → init_outbound must Err, not silently accept.
        let cfg = OutboundConfig {
            bind_ipv4: Some("192.0.2.1".into()),
            interface: "auto".into(),
        };
        let result = init_outbound(&cfg);
        assert!(
            matches!(result, Err(OutboundError::IpNotLocal(_, _))),
            "non-local IP must be rejected at startup, got {:?}",
            result
        );
    }

    #[test]
    fn outbound_config_default_is_safe() {
        let cfg = OutboundConfig::default();
        assert!(cfg.bind_ipv4.is_none());
        assert_eq!(cfg.interface, "auto");
    }

    // ── v1.0.4: dual-stack listen tests ──

    #[test]
    fn parse_listen_ip_handles_v4_v6_empty() {
        assert_eq!(
            parse_listen_ip("0.0.0.0"),
            Some(IpAddr::V4(Ipv4Addr::UNSPECIFIED))
        );
        assert!(parse_listen_ip("::").is_some(), ":: must parse");
        assert!(parse_listen_ip("::").unwrap().is_ipv6());
        assert_eq!(parse_listen_ip(""), None, "empty = disabled");
        assert_eq!(parse_listen_ip("  "), None, "whitespace = disabled");
    }

    #[tokio::test]
    async fn tcp_binds_both_v4_and_v6_same_port() {
        // Pick an ephemeral port by binding v4 to :0 first, then reuse the
        // resolved port for both families on loopback.
        let v4 = bind_tcp_listener(IpAddr::V4(Ipv4Addr::LOCALHOST), 0).unwrap();
        let port = v4.local_addr().unwrap().port();
        drop(v4);
        // Now bind BOTH families to the same explicit port.
        let v4 = bind_tcp_listener(IpAddr::V4(Ipv4Addr::UNSPECIFIED), port);
        let v6 = bind_tcp_listener(IpAddr::V6(std::net::Ipv6Addr::UNSPECIFIED), port);
        assert!(v4.is_ok(), "IPv4 bind must succeed: {:?}", v4.err());
        assert!(
            v6.is_ok(),
            "IPv6 bind must succeed alongside IPv4 (V6ONLY): {:?}",
            v6.err()
        );
    }

    #[tokio::test]
    async fn udp_binds_both_v4_and_v6_same_port() {
        let v4 = bind_udp_socket(IpAddr::V4(Ipv4Addr::LOCALHOST), 0).unwrap();
        let port = v4.local_addr().unwrap().port();
        drop(v4);
        let v4 = bind_udp_socket(IpAddr::V4(Ipv4Addr::UNSPECIFIED), port);
        let v6 = bind_udp_socket(IpAddr::V6(std::net::Ipv6Addr::UNSPECIFIED), port);
        assert!(v4.is_ok(), "UDP IPv4 bind must succeed: {:?}", v4.err());
        assert!(
            v6.is_ok(),
            "UDP IPv6 bind must succeed alongside IPv4: {:?}",
            v6.err()
        );
    }

    #[tokio::test]
    async fn ipv6_address_never_produces_triple_colon() {
        // Regression: SocketAddr::new must never stringify to ":::port".
        let addr = SocketAddr::new(IpAddr::V6(std::net::Ipv6Addr::UNSPECIFIED), 33418);
        let s = addr.to_string();
        assert!(!s.contains(":::"), "got broken addr string: {}", s);
        assert_eq!(s, "[::]:33418");
    }

    // ── v1.0.5: outbound source-bind tests ──

    #[tokio::test]
    async fn tcp_connect_with_explicit_source_binds_and_connects() {
        // #6: a TCP connection with an explicit source IPv4 must bind that
        // source, then reach the target. Use loopback for both so the test is
        // hermetic (127.0.0.1 is always local).
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let target = format!("127.0.0.1:{}", port);

        let accept = tokio::spawn(async move { listener.accept().await.map(|(s, _)| s) });

        let stream = tcp_connect(&target, Some(Ipv4Addr::LOCALHOST), 5)
            .await
            .expect("connect with source bind must succeed");
        // The connection's local (source) address must be the bound 127.0.0.1.
        assert_eq!(
            stream.local_addr().unwrap().ip(),
            IpAddr::V4(Ipv4Addr::LOCALHOST)
        );
        assert!(accept.await.unwrap().is_ok(), "server must accept it");
    }

    #[tokio::test]
    async fn tcp_connect_without_source_uses_auto_route() {
        // #8: no source configured → system auto-route still connects.
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let target = format!("127.0.0.1:{}", port);
        let accept = tokio::spawn(async move { listener.accept().await.map(|(s, _)| s) });

        let stream = tcp_connect(&target, None, 5)
            .await
            .expect("auto-route connect must succeed");
        assert!(stream.peer_addr().unwrap().port() == port);
        assert!(accept.await.unwrap().is_ok());
    }

    #[tokio::test]
    async fn tcp_connect_sets_nodelay() {
        // v1.0.8: the outbound stream MUST have TCP_NODELAY enabled (Nagle off)
        // — the core of the long-chain jitter fix. Verify via the getter for
        // both the auto-route and the source-bound paths.
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let target = format!("127.0.0.1:{}", port);
        let accept = tokio::spawn(async move {
            // Accept twice (auto + bound connects below).
            let a = listener.accept().await.map(|(s, _)| s);
            let b = listener.accept().await.map(|(s, _)| s);
            (a, b)
        });

        let auto = tcp_connect(&target, None, 5).await.expect("auto connect");
        assert!(
            auto.nodelay().unwrap(),
            "auto-route stream must have NODELAY"
        );

        let bound = tcp_connect(&target, Some(Ipv4Addr::LOCALHOST), 5)
            .await
            .expect("bound connect");
        assert!(
            bound.nodelay().unwrap(),
            "source-bound stream must have NODELAY"
        );

        let (ra, rb) = accept.await.unwrap();
        assert!(ra.is_ok() && rb.is_ok());
    }

    #[tokio::test]
    async fn udp_outbound_socket_binds_explicit_source() {
        // #7: an explicit source IPv4 must produce a socket bound to {src}:0.
        let sock = udp_outbound_socket(Some(Ipv4Addr::LOCALHOST))
            .await
            .expect("udp source bind must succeed");
        let local = sock.local_addr().unwrap();
        assert_eq!(local.ip(), IpAddr::V4(Ipv4Addr::LOCALHOST));
        assert_ne!(local.port(), 0, "kernel must assign an ephemeral port");
    }

    #[tokio::test]
    async fn udp_outbound_socket_auto_binds_wildcard() {
        // #8: no source → bind 0.0.0.0:0 (system auto), backward compatible.
        let sock = udp_outbound_socket(None)
            .await
            .expect("udp auto bind must succeed");
        assert_eq!(
            sock.local_addr().unwrap().ip(),
            IpAddr::V4(Ipv4Addr::UNSPECIFIED)
        );
    }

    #[test]
    fn nonexistent_interface_is_an_error() {
        // #11: a typo'd / missing NIC must surface a clear error, never silently
        // fall back to auto-route out the wrong interface.
        let cfg = OutboundConfig {
            bind_ipv4: None,
            interface: "rp-no-such-nic-xyz".into(),
        };
        let result = init_outbound(&cfg);
        assert!(
            result.is_err(),
            "missing interface must error, got {:?}",
            result
        );
    }
}
