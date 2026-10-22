//! node-v1.2.9: PROXY protocol (HAProxy's v1 text / v2 binary header).
//!
//! A relay hides the client: the target sees every connection coming from the
//! node. With PROXY protocol the node writes one short header to the target
//! before any data — "this connection is really from client:port to dst:port"
//! — and a target set up for it (nginx `proxy_protocol`, Caddy, Xray, HAProxy)
//! logs, limits and bans by the real client.
//!
//! Two independent per-rule switches:
//! - **send**: write a v1 or v2 header to the target once connected.
//! - **accept**: the previous hop is another relay node sending a header;
//!   read it from the start of the inbound connection, take the real client
//!   from it, and do not forward it. A middle node of a chain does both, so
//!   the address reaching the last target is the original client's.
//!
//! TCP only: a UDP datagram has nowhere to carry a per-connection header that
//! targets would read, and practically no UDP service accepts one.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};

use relay_shared::protocol::ProxyProtocolVersion;
use tokio::io::{AsyncRead, AsyncReadExt};

/// The v2 signature every binary header starts with.
const V2_SIG: [u8; 12] = *b"\r\n\r\n\0\r\nQUIT\n";
/// v1: "PROXY " + the longest line (TCP6, two full IPv6 addresses, two ports)
/// is 107 bytes including CRLF, per the spec.
const V1_MAX: usize = 107;
/// v2: a header this node would send carries no TLVs (16 + 36 bytes). Other
/// senders may append TLVs; anything past this is refused rather than buffered
/// without bound on behalf of whoever connected.
const V2_MAX_PAYLOAD: usize = 1024;

/// The addresses a received header carried. Both are None for a LOCAL (v2) or
/// UNKNOWN (v1) header — a health check, not a proxied client — in which case
/// the connection's own addresses stand.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Received {
    pub src: Option<SocketAddr>,
    pub dst: Option<SocketAddr>,
    /// Bytes read past the header (the client's first data, when it arrived
    /// in the same packet). They must be forwarded before anything else.
    pub leftover: Vec<u8>,
}

/// Build the header for `version` announcing `src` → `dst`. Off yields nothing.
/// Mixed families (an IPv4 client reaching an IPv6 listener, or the reverse)
/// are written as IPv6 with the IPv4 side as `::ffff:a.b.c.d`, since both
/// addresses of a header must share a family.
pub fn encode(version: ProxyProtocolVersion, src: SocketAddr, dst: SocketAddr) -> Vec<u8> {
    let (src, dst) = same_family(src, dst);
    match version {
        ProxyProtocolVersion::Off => Vec::new(),
        ProxyProtocolVersion::V1 => {
            let fam = if src.is_ipv4() { "TCP4" } else { "TCP6" };
            format!(
                "PROXY {} {} {} {} {}\r\n",
                fam,
                src.ip(),
                dst.ip(),
                src.port(),
                dst.port()
            )
            .into_bytes()
        }
        ProxyProtocolVersion::V2 => {
            let mut out = Vec::with_capacity(16 + 36);
            out.extend_from_slice(&V2_SIG);
            out.push(0x21); // version 2, command PROXY
            match (src.ip(), dst.ip()) {
                (IpAddr::V4(s), IpAddr::V4(d)) => {
                    out.push(0x11); // AF_INET, STREAM
                    out.extend_from_slice(&12u16.to_be_bytes());
                    out.extend_from_slice(&s.octets());
                    out.extend_from_slice(&d.octets());
                }
                (s, d) => {
                    out.push(0x21); // AF_INET6, STREAM
                    out.extend_from_slice(&36u16.to_be_bytes());
                    out.extend_from_slice(&to_v6(s).octets());
                    out.extend_from_slice(&to_v6(d).octets());
                }
            }
            out.extend_from_slice(&src.port().to_be_bytes());
            out.extend_from_slice(&dst.port().to_be_bytes());
            out
        }
    }
}

fn to_v6(ip: IpAddr) -> Ipv6Addr {
    match ip {
        IpAddr::V4(v4) => v4.to_ipv6_mapped(),
        IpAddr::V6(v6) => v6,
    }
}

fn same_family(src: SocketAddr, dst: SocketAddr) -> (SocketAddr, SocketAddr) {
    if src.is_ipv4() == dst.is_ipv4() {
        return (src, dst);
    }
    (
        SocketAddr::new(IpAddr::V6(to_v6(src.ip())), src.port()),
        SocketAddr::new(IpAddr::V6(to_v6(dst.ip())), dst.port()),
    )
}

/// Read a v1 or v2 header from the start of `stream`. The caller bounds the
/// wait (a peer that never sends one must not hold the connection open).
/// Anything that is not a well-formed header is an error: a rule that accepts
/// PROXY protocol has its port facing relay nodes, and a client reaching it
/// directly must not get through with its own address unannounced.
pub async fn read_header<R: AsyncRead + Unpin>(stream: &mut R) -> std::io::Result<Received> {
    let mut buf: Vec<u8> = Vec::with_capacity(256);
    let mut chunk = [0u8; 512];
    loop {
        if let Some(parsed) = try_parse(&buf)? {
            let (len, src, dst) = parsed;
            return Ok(Received {
                src,
                dst,
                leftover: buf[len..].to_vec(),
            });
        }
        let n = stream.read(&mut chunk).await?;
        if n == 0 {
            return Err(bad("connection closed before a complete PROXY header"));
        }
        buf.extend_from_slice(&chunk[..n]);
    }
}

type Parsed = (usize, Option<SocketAddr>, Option<SocketAddr>);

/// Ok(None): need more bytes. Ok(Some((header_len, src, dst))): complete.
fn try_parse(buf: &[u8]) -> std::io::Result<Option<Parsed>> {
    // v2: decide on the signature as soon as there are enough bytes to rule
    // it in or out.
    let sig_len = buf.len().min(V2_SIG.len());
    if buf[..sig_len] == V2_SIG[..sig_len] && !buf.is_empty() {
        if buf.len() < 16 {
            return Ok(None);
        }
        return parse_v2(buf);
    }
    let v1_prefix = b"PROXY ";
    let pre_len = buf.len().min(v1_prefix.len());
    if buf[..pre_len] != v1_prefix[..pre_len] {
        return Err(bad("connection did not start with a PROXY header"));
    }
    match buf.windows(2).position(|w| w == b"\r\n") {
        Some(end) if end + 2 <= V1_MAX => parse_v1(&buf[..end]).map(|(s, d)| Some((end + 2, s, d))),
        Some(_) => Err(bad("PROXY v1 header too long")),
        None if buf.len() >= V1_MAX => Err(bad("PROXY v1 header too long")),
        None => Ok(None),
    }
}

fn parse_v1(line: &[u8]) -> std::io::Result<(Option<SocketAddr>, Option<SocketAddr>)> {
    let line = std::str::from_utf8(line).map_err(|_| bad("PROXY v1 header is not text"))?;
    let parts: Vec<&str> = line.split(' ').collect();
    match parts.as_slice() {
        ["PROXY", "UNKNOWN", ..] => Ok((None, None)),
        ["PROXY", fam @ ("TCP4" | "TCP6"), src_ip, dst_ip, src_port, dst_port] => {
            let ip = |s: &str| -> std::io::Result<IpAddr> {
                let ip: IpAddr = s.parse().map_err(|_| bad("PROXY v1: bad address"))?;
                if (*fam == "TCP4") != ip.is_ipv4() {
                    return Err(bad("PROXY v1: address does not match its family"));
                }
                Ok(ip)
            };
            let port = |s: &str| -> std::io::Result<u16> {
                s.parse().map_err(|_| bad("PROXY v1: bad port"))
            };
            Ok((
                Some(SocketAddr::new(ip(src_ip)?, port(src_port)?)),
                Some(SocketAddr::new(ip(dst_ip)?, port(dst_port)?)),
            ))
        }
        _ => Err(bad("PROXY v1: malformed header")),
    }
}

fn parse_v2(buf: &[u8]) -> std::io::Result<Option<Parsed>> {
    let ver_cmd = buf[12];
    if ver_cmd >> 4 != 2 {
        return Err(bad("PROXY v2: unsupported version"));
    }
    let len = u16::from_be_bytes([buf[14], buf[15]]) as usize;
    if len > V2_MAX_PAYLOAD {
        return Err(bad("PROXY v2 header too long"));
    }
    let total = 16 + len;
    if buf.len() < total {
        return Ok(None);
    }
    let body = &buf[16..total];
    match ver_cmd & 0x0f {
        0x0 => return Ok(Some((total, None, None))), // LOCAL: no proxied client
        0x1 => {}
        _ => return Err(bad("PROXY v2: unknown command")),
    }
    let (src, dst) = match buf[13] {
        // AF_INET over STREAM / DGRAM
        0x11 | 0x12 if body.len() >= 12 => {
            let s = Ipv4Addr::new(body[0], body[1], body[2], body[3]);
            let d = Ipv4Addr::new(body[4], body[5], body[6], body[7]);
            let sp = u16::from_be_bytes([body[8], body[9]]);
            let dp = u16::from_be_bytes([body[10], body[11]]);
            (SocketAddr::new(s.into(), sp), SocketAddr::new(d.into(), dp))
        }
        // AF_INET6 over STREAM / DGRAM
        0x21 | 0x22 if body.len() >= 36 => {
            let mut s = [0u8; 16];
            let mut d = [0u8; 16];
            s.copy_from_slice(&body[0..16]);
            d.copy_from_slice(&body[16..32]);
            let sp = u16::from_be_bytes([body[32], body[33]]);
            let dp = u16::from_be_bytes([body[34], body[35]]);
            (
                SocketAddr::new(unmap(Ipv6Addr::from(s)), sp),
                SocketAddr::new(unmap(Ipv6Addr::from(d)), dp),
            )
        }
        // AF_UNSPEC: like LOCAL, no usable address.
        0x00 => return Ok(Some((total, None, None))),
        _ => return Err(bad("PROXY v2: unsupported address family")),
    };
    Ok(Some((total, Some(src), Some(dst))))
}

/// `::ffff:a.b.c.d` back to the IPv4 address it stands for.
fn unmap(v6: Ipv6Addr) -> IpAddr {
    match v6.to_ipv4_mapped() {
        Some(v4) => IpAddr::V4(v4),
        None => IpAddr::V6(v6),
    }
}

fn bad(msg: &str) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::InvalidData, msg.to_string())
}

/// Whether `peer` may send a header: true when `trusted` is empty, else when
/// it matches one of the IPs or CIDRs in it. Entries that do not parse match
/// nothing (the panel validates them on save).
pub fn peer_trusted(peer: IpAddr, trusted: &[String]) -> bool {
    if trusted.is_empty() {
        return true;
    }
    let peer = match peer {
        IpAddr::V6(v6) => unmap(v6),
        v4 => v4,
    };
    trusted.iter().any(|entry| matches_entry(peer, entry))
}

fn matches_entry(peer: IpAddr, entry: &str) -> bool {
    let (net, prefix) = match entry.split_once('/') {
        Some((net, p)) => match p.parse::<u8>() {
            Ok(p) => (net, Some(p)),
            Err(_) => return false,
        },
        None => (entry, None),
    };
    let Ok(net) = net.parse::<IpAddr>() else {
        return false;
    };
    match (peer, net) {
        (IpAddr::V4(p), IpAddr::V4(n)) => {
            let bits = prefix.unwrap_or(32);
            if bits > 32 {
                return false;
            }
            let mask = if bits == 0 {
                0
            } else {
                u32::MAX << (32 - bits)
            };
            u32::from(p) & mask == u32::from(n) & mask
        }
        (IpAddr::V6(p), IpAddr::V6(n)) => {
            let bits = prefix.unwrap_or(128);
            if bits > 128 {
                return false;
            }
            let mask = if bits == 0 {
                0
            } else {
                u128::MAX << (128 - bits)
            };
            u128::from(p) & mask == u128::from(n) & mask
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sa(s: &str) -> SocketAddr {
        s.parse().unwrap()
    }

    #[test]
    fn v1_header_is_the_spec_line() {
        let h = encode(
            ProxyProtocolVersion::V1,
            sa("203.0.113.7:51000"),
            sa("198.51.100.1:443"),
        );
        assert_eq!(h, b"PROXY TCP4 203.0.113.7 198.51.100.1 51000 443\r\n");
        let h = encode(
            ProxyProtocolVersion::V1,
            sa("[2001:db8::7]:51000"),
            sa("[2001:db8::1]:443"),
        );
        assert_eq!(h, b"PROXY TCP6 2001:db8::7 2001:db8::1 51000 443\r\n");
    }

    #[test]
    fn v2_header_is_the_spec_layout() {
        let h = encode(
            ProxyProtocolVersion::V2,
            sa("203.0.113.7:51000"),
            sa("198.51.100.1:443"),
        );
        let mut want = V2_SIG.to_vec();
        want.extend_from_slice(&[0x21, 0x11, 0, 12, 203, 0, 113, 7, 198, 51, 100, 1]);
        want.extend_from_slice(&51000u16.to_be_bytes());
        want.extend_from_slice(&443u16.to_be_bytes());
        assert_eq!(h, want);
        assert!(encode(ProxyProtocolVersion::Off, sa("1.1.1.1:1"), sa("2.2.2.2:2")).is_empty());
    }

    /// Both addresses of a header share a family: an IPv4 client reaching an
    /// IPv6 listener goes out as TCP6 with the client as ::ffff:a.b.c.d, and
    /// a receiving node turns that back into the IPv4 address.
    #[tokio::test]
    async fn mixed_families_go_out_as_ipv6_and_come_back_as_ipv4() {
        let h = encode(
            ProxyProtocolVersion::V1,
            sa("203.0.113.7:5"),
            sa("[2001:db8::1]:443"),
        );
        assert_eq!(h, b"PROXY TCP6 ::ffff:203.0.113.7 2001:db8::1 5 443\r\n");
        let h = encode(
            ProxyProtocolVersion::V2,
            sa("203.0.113.7:5"),
            sa("[2001:db8::1]:443"),
        );
        let r = read_header(&mut &h[..]).await.unwrap();
        assert_eq!(r.src, Some(sa("203.0.113.7:5")));
    }

    /// What one node sends, the next reads back, for both versions — the
    /// chain case — and data that arrived with the header is kept.
    #[tokio::test]
    async fn a_header_round_trips_and_keeps_the_data_behind_it() {
        for v in [ProxyProtocolVersion::V1, ProxyProtocolVersion::V2] {
            for (src, dst) in [
                (sa("203.0.113.7:51000"), sa("198.51.100.1:443")),
                (sa("[2001:db8::7]:51000"), sa("[2001:db8::1]:443")),
            ] {
                let mut wire = encode(v, src, dst);
                wire.extend_from_slice(b"GET / HTTP/1.1\r\n");
                let r = read_header(&mut &wire[..]).await.unwrap();
                assert_eq!((r.src, r.dst), (Some(src), Some(dst)), "{v:?}");
                assert_eq!(r.leftover, b"GET / HTTP/1.1\r\n");
            }
        }
    }

    /// A header split across reads (one byte at a time) still parses, and
    /// nothing past it is consumed from the stream.
    #[tokio::test]
    async fn a_header_arriving_in_pieces_is_assembled() {
        /// Hands out one byte per read, as a slow peer would.
        struct OneByte<'a>(&'a [u8]);
        impl AsyncRead for OneByte<'_> {
            fn poll_read(
                mut self: std::pin::Pin<&mut Self>,
                _: &mut std::task::Context<'_>,
                buf: &mut tokio::io::ReadBuf<'_>,
            ) -> std::task::Poll<std::io::Result<()>> {
                if let Some((first, rest)) = self.0.split_first() {
                    buf.put_slice(&[*first]);
                    self.0 = rest;
                }
                std::task::Poll::Ready(Ok(()))
            }
        }
        for v in [ProxyProtocolVersion::V1, ProxyProtocolVersion::V2] {
            let mut wire = encode(v, sa("203.0.113.7:1"), sa("198.51.100.1:2"));
            wire.extend_from_slice(b"data");
            let mut stream = OneByte(&wire);
            let r = read_header(&mut stream).await.unwrap();
            assert_eq!(r.src, Some(sa("203.0.113.7:1")), "{v:?}");
            assert!(
                r.leftover.is_empty(),
                "read one byte at a time, nothing past the header"
            );
            assert_eq!(stream.0, b"data");
        }
    }

    #[tokio::test]
    async fn local_and_unknown_headers_carry_no_address() {
        let r = read_header(&mut &b"PROXY UNKNOWN\r\nabc"[..])
            .await
            .unwrap();
        assert_eq!((r.src, r.dst, r.leftover), (None, None, b"abc".to_vec()));
        let mut local = V2_SIG.to_vec();
        local.extend_from_slice(&[0x20, 0x00, 0, 0]);
        let r = read_header(&mut &local[..]).await.unwrap();
        assert_eq!((r.src, r.dst), (None, None));
    }

    /// No header, a broken one, or one too long: refused, never forwarded as
    /// if the client were the relay's own.
    #[tokio::test]
    async fn anything_but_a_header_is_refused() {
        for wire in [
            &b"GET / HTTP/1.1\r\n\r\n"[..],
            &b"PROXY TCP4 not-an-ip 1.2.3.4 1 2\r\n"[..],
            &b"PROXY TCP4 2001:db8::1 1.2.3.4 1 2\r\n"[..],
            &b"PROXY TCP4 1.2.3.4 1.2.3.4 99999 2\r\n"[..],
            &b"PROXY"[..], // closed before complete
        ] {
            assert!(
                read_header(&mut &wire[..]).await.is_err(),
                "{:?}",
                String::from_utf8_lossy(wire)
            );
        }
        let long = format!("PROXY TCP4 {}\r\n", "1".repeat(200));
        assert!(read_header(&mut long.as_bytes()).await.is_err());
        let mut huge = V2_SIG.to_vec();
        huge.extend_from_slice(&[0x21, 0x11, 0xff, 0xff]);
        assert!(read_header(&mut &huge[..]).await.is_err());
    }

    #[test]
    fn trusted_sources_match_ips_and_cidrs() {
        let t = |list: &[&str]| list.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let peer: IpAddr = "203.0.113.7".parse().unwrap();
        assert!(peer_trusted(peer, &[]), "empty list trusts anyone");
        assert!(peer_trusted(peer, &t(&["203.0.113.7"])));
        assert!(peer_trusted(peer, &t(&["10.0.0.1", "203.0.113.0/24"])));
        assert!(!peer_trusted(peer, &t(&["203.0.114.0/24"])));
        assert!(!peer_trusted(peer, &t(&["garbage", "203.0.113.0/40"])));
        // an IPv4 peer seen as ::ffff:a.b.c.d still matches its IPv4 entry
        let mapped: IpAddr = "::ffff:203.0.113.7".parse().unwrap();
        assert!(peer_trusted(mapped, &t(&["203.0.113.0/24"])));
        let v6: IpAddr = "2001:db8::7".parse().unwrap();
        assert!(peer_trusted(v6, &t(&["2001:db8::/32"])));
        assert!(!peer_trusted(v6, &t(&["2001:db9::/32", "203.0.113.0/24"])));
    }
}
