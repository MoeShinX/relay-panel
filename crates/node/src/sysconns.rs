//! System-wide active TCP and UDP connection counts, read from Linux procfs.
//!
//! Pure parsers are compiled and tested on every platform; only the `/proc`
//! file reads are Linux-gated (matching `cpuinfo.rs` and `disks.rs`).
//!
//! Why `/proc/net/snmp` + `/proc/net/sockstat{,6}` instead of scanning
//! `/proc/net/tcp{,6}` line-by-line:
//! - Both files are a few hundred bytes and read in O(1), so a busy relay with
//!   tens of thousands of connections never pays a multi-megabyte allocation or
//!   kernel socket-table lock on every status report.
//! - `Tcp: CurrEstab` in `/proc/net/snmp` is the kernel's own counter of
//!   ESTABLISHED + CLOSE_WAIT TCP connections across BOTH IPv4 and IPv6
//!   (covering SSH, HTTPS, proxies, and forwarded TCP connections alike).
//! - `UDP: inuse` (`/proc/net/sockstat`) + `UDP6: inuse` (`/proc/net/sockstat6`)
//!   counts open IPv4 + IPv6 UDP sockets across the machine (NTP, DNS,
//!   WireGuard, active UDP relay sessions, etc.), matching `ss -s`.

/// Read `(system_tcp, system_udp)` for the current network namespace.
///
/// Returns `(None, None)` off Linux or if procfs is unavailable, so the panel
/// can fall back cleanly to the relay's own `active_connections`.
pub fn read_system_connections() -> (Option<u32>, Option<u32>) {
    #[cfg(target_os = "linux")]
    {
        let tcp = std::fs::read_to_string("/proc/net/snmp")
            .ok()
            .and_then(|t| parse_proc_net_snmp_curr_estab(&t));
        let udp4 = std::fs::read_to_string("/proc/net/sockstat")
            .ok()
            .and_then(|t| parse_sockstat_inuse(&t, "UDP:"));
        // sockstat6 is absent when IPv6 is disabled in the kernel; treat that
        // as 0 rather than failing the whole UDP reading when IPv4 succeeded.
        let udp6 = std::fs::read_to_string("/proc/net/sockstat6")
            .ok()
            .and_then(|t| parse_sockstat_inuse(&t, "UDP6:"));
        let udp = match (udp4, udp6) {
            (Some(a), Some(b)) => Some(a.saturating_add(b)),
            (Some(a), None) => Some(a),
            (None, Some(b)) => Some(b),
            (None, None) => None,
        };
        (tcp, udp)
    }
    #[cfg(not(target_os = "linux"))]
    {
        (None, None)
    }
}

/// Parse `Tcp: CurrEstab` from `/proc/net/snmp`.
///
/// `/proc/net/snmp` writes paired lines for each protocol: a header line
/// naming the columns, immediately followed by a value line with the same
/// prefix:
/// ```text
/// Tcp: RtoAlgorithm RtoMin ... CurrEstab InSegs ...
/// Tcp: 1 200 ... 48 12345 ...
/// ```
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub fn parse_proc_net_snmp_curr_estab(text: &str) -> Option<u32> {
    let mut lines = text.lines();
    while let Some(header) = lines.next() {
        let mut hcols = header.split_whitespace();
        if hcols.next() != Some("Tcp:") {
            continue;
        }
        let Some(idx) = hcols.position(|col| col == "CurrEstab") else {
            continue;
        };
        let values = lines.next()?;
        let mut vcols = values.split_whitespace();
        if vcols.next() != Some("Tcp:") {
            return None;
        }
        return vcols.nth(idx)?.parse::<u32>().ok();
    }
    None
}

/// Parse `<prefix> inuse <N>` from `/proc/net/sockstat` or `/proc/net/sockstat6`.
///
/// Example lines:
/// - `UDP: inuse 5 mem 1`
/// - `UDP6: inuse 3`
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub fn parse_sockstat_inuse(text: &str, prefix: &str) -> Option<u32> {
    for line in text.lines() {
        let mut cols = line.split_whitespace();
        if cols.next() != Some(prefix) {
            continue;
        }
        while let Some(key) = cols.next() {
            let val = cols.next()?;
            if key == "inuse" {
                return val.parse::<u32>().ok();
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE_SNMP: &str = "\
Ip: Forwarding DefaultTTL InReceives
Ip: 1 64 1000
Tcp: RtoAlgorithm RtoMin RtoMax MaxConn ActiveOpens PassiveOpens AttemptFails EstabResets CurrEstab InSegs OutSegs RetransSegs InErrs OutRsts InCsumErrors
Tcp: 1 200 120000 -1 420 185 3 12 48 99812 88210 14 0 9 0
Udp: InDatagrams NoPorts InErrors OutDatagrams
Udp: 500 0 0 510
";

    const SAMPLE_SOCKSTAT: &str = "\
sockets: used 214
TCP: inuse 54 orphan 0 tw 7 alloc 60 mem 12
UDP: inuse 11 mem 3
UDPLITE: inuse 0
RAW: inuse 0
FRAG: inuse 0 memory 0
";

    const SAMPLE_SOCKSTAT6: &str = "\
TCP6: inuse 18
UDP6: inuse 4
UDPLITE6: inuse 0
RAW6: inuse 1
FRAG6: inuse 0 memory 0
";

    #[test]
    fn parses_curr_estab_from_proc_net_snmp() {
        assert_eq!(parse_proc_net_snmp_curr_estab(SAMPLE_SNMP), Some(48));
    }

    #[test]
    fn snmp_parser_handles_missing_or_malformed_sections() {
        assert_eq!(parse_proc_net_snmp_curr_estab(""), None);
        assert_eq!(
            parse_proc_net_snmp_curr_estab("Tcp: CurrEstab\nUdp: 1\n"),
            None
        );
        assert_eq!(
            parse_proc_net_snmp_curr_estab("Tcp: CurrEstab\nTcp: -1\n"),
            None
        );
    }

    #[test]
    fn parses_udp_inuse_from_sockstat_and_sockstat6() {
        assert_eq!(parse_sockstat_inuse(SAMPLE_SOCKSTAT, "UDP:"), Some(11));
        assert_eq!(parse_sockstat_inuse(SAMPLE_SOCKSTAT6, "UDP6:"), Some(4));
        assert_eq!(parse_sockstat_inuse(SAMPLE_SOCKSTAT, "UDP6:"), None);
    }

    #[test]
    fn sockstat_parser_handles_malformed_lines() {
        assert_eq!(parse_sockstat_inuse("", "UDP:"), None);
        assert_eq!(
            parse_sockstat_inuse("UDP: mem 3 inuse 7\n", "UDP:"),
            Some(7)
        );
        assert_eq!(parse_sockstat_inuse("UDP: inuse\n", "UDP:"), None);
    }
}
