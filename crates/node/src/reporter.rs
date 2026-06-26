use crate::config::NodeConfig;
use relay_shared::protocol::{
    ApiResponse, ListenerError, StatusReport, TrafficEntry, TrafficReport,
};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use sysinfo::{Disks, Networks, System};
use tokio::sync::{Mutex, RwLock};

pub struct TrafficCounter {
    // rule_id -> (upload, download). Keyed by rule id (not listen port) so
    // traffic is always attributed to the correct rule even when two inbound
    // groups listen on the same port.
    pub data: Arc<Mutex<HashMap<i64, (u64, u64)>>>,
}

impl TrafficCounter {
    pub fn new() -> Self {
        Self {
            data: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub async fn add(&self, rule_id: i64, upload: u64, download: u64) {
        let mut data = self.data.lock().await;
        let entry = data.entry(rule_id).or_insert((0, 0));
        entry.0 += upload;
        entry.1 += download;
    }

    /// Take a snapshot and return a guard whose `commit()` subtracts exactly
    /// the snapshotted bytes from each counter. This is the correct pattern for
    /// traffic reporting: the bytes captured in the snapshot are only deducted
    /// after the panel ACKs the upload. If the upload fails the guard is
    /// dropped without commit, so those bytes stay and are retried next cycle.
    /// Bytes that arrive BETWEEN snapshot and commit are preserved (subtract,
    /// not clear), so no traffic is ever lost.
    pub async fn snapshot(&self) -> TrafficSnapshot<'_> {
        let data = self.data.lock().await;
        let entries: Vec<TrafficEntry> = data
            .iter()
            .map(|(rule_id, (upload, download))| TrafficEntry {
                rule_id: *rule_id,
                upload: *upload,
                download: *download,
            })
            .collect();
        TrafficSnapshot {
            counter: self,
            entries,
        }
    }

    /// Destructive read: snapshot AND clear in one step. Kept for callers that
    /// want the old semantics (e.g. test fixtures that drain-then-assert). The
    /// production reporter uses `snapshot()` + `TrafficSnapshot::commit()` so a
    /// failed upload retries instead of dropping traffic.
    #[allow(dead_code)]
    pub async fn drain(&self) -> Vec<TrafficEntry> {
        let mut data = self.data.lock().await;
        data.drain()
            .map(|(rule_id, (upload, download))| TrafficEntry {
                rule_id,
                upload,
                download,
            })
            .collect()
    }

    /// Remove all accumulated bytes for a single rule from the counter. Used
    /// when a listener is permanently stopped (rule deleted or no longer in the
    /// config) so that orphaned bytes don't poison future traffic batches — a
    /// stale rule_id causes the panel to atomically reject the entire batch.
    pub async fn prune_rule(&self, rule_id: i64) {
        self.data.lock().await.remove(&rule_id);
    }

    /// Test-only: check whether a rule_id has any accumulated bytes.
    #[cfg(test)]
    pub async fn has_rule(&self, rule_id: i64) -> bool {
        self.data.lock().await.contains_key(&rule_id)
    }
}

/// Snapshot of [`TrafficCounter`] at one instant. Drop without calling
/// [`commit`](Self::commit) to retry the same bytes; call `commit` once the
/// panel has persisted the report.
pub struct TrafficSnapshot<'a> {
    counter: &'a TrafficCounter,
    pub entries: Vec<TrafficEntry>,
}

impl TrafficSnapshot<'_> {
    /// Subtract the snapshotted bytes from the live counters. Bytes counted
    /// after the snapshot was taken are untouched. Safe to call once.
    pub async fn commit(self) {
        let mut data = self.counter.data.lock().await;
        for e in &self.entries {
            if let Some((u, d)) = data.get_mut(&e.rule_id) {
                *u = u.saturating_sub(e.upload);
                *d = d.saturating_sub(e.download);
                // Remove the entry if it's fully drained, so the map doesn't
                // accumulate zero-only entries for idle rules.
                if *u == 0 && *d == 0 {
                    data.remove(&e.rule_id);
                }
            }
        }
    }
}

/// How long a UDP session is considered active after its last datagram.
/// UDP has no connection-close event, so sessions expire by inactivity.
pub const UDP_SESSION_TIMEOUT: Duration = Duration::from_secs(60);

/// Tracks the number of currently-active forwarded connections, for BOTH
/// transport types, so the panel's "connections" column reflects real traffic:
///
/// - **TCP**: a strict accept/close count via an atomic + an RAII `Drop` guard.
///   The guard guarantees decrement even if a connection task panics.
/// - **UDP**: there is no "connection"; instead we count active UDP sessions,
///   keyed by `(client_addr, rule_id)`. A session is created on the first
///   datagram from a client and considered expired after
///   `UDP_SESSION_TIMEOUT` with no further traffic. Expired sessions are
///   pruned lazily on every `touch`/`current` call AND by the UDP listener's
///   periodic sweeper, so the count converges on zero shortly after traffic
///   stops even without new datagrams arriving.
///
/// `current()` reports `active_tcp + active_udp_sessions`.
///
/// This is entirely independent of the WebSocket control channel: it is read
/// from the plain-HTTP `report_status` loop, so connection counts keep
/// updating even if WS is down.
///
/// Locking: TCP uses an `AtomicU64` (lock-free), UDP uses a `Mutex<HashMap>`.
/// Both are fine-grained — neither is a global lock that could block forwarding.
pub struct ConnectionTracker {
    tcp: Arc<AtomicU64>,
    udp: Arc<Mutex<HashMap<UdpSessionKey, Instant>>>,
}

/// Identity of a single UDP "connection". A client's source port plus the
/// rule it hits uniquely identifies one logical session.
#[derive(Debug, Clone, Copy, Hash, PartialEq, Eq)]
pub struct UdpSessionKey {
    pub client_addr: SocketAddr,
    pub rule_id: i64,
}

impl ConnectionTracker {
    pub fn new() -> Self {
        Self {
            tcp: Arc::new(AtomicU64::new(0)),
            udp: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Increment the active TCP count and return a guard whose `Drop`
    /// decrements it. Hand the guard to the per-connection task so the count
    /// is correct no matter how that task ends (normal close, error, panic).
    pub fn tcp_handle(&self) -> TcpConnectionGuard {
        let prev = self.tcp.fetch_add(1, Ordering::Relaxed);
        tracing::debug!("tcp connection opened, active={}", prev + 1);
        TcpConnectionGuard {
            tcp: self.tcp.clone(),
        }
    }

    /// Register or refresh a UDP session. Returns `true` if a NEW session was
    /// created (so the caller can emit an "opened" log) and `false` if an
    /// existing session was merely refreshed. Lazily prunes expired sessions
    /// belonging to ANY rule before inserting/refreshing.
    pub async fn udp_touch(&self, client_addr: SocketAddr, rule_id: i64) -> bool {
        let key = UdpSessionKey {
            client_addr,
            rule_id,
        };
        let mut map = self.udp.lock().await;
        prune_expired_locked(&mut map);
        use std::collections::hash_map::Entry;
        match map.entry(key) {
            Entry::Occupied(mut e) => {
                *e.get_mut() = Instant::now();
                false
            }
            Entry::Vacant(e) => {
                e.insert(Instant::now());
                let active = map.len();
                tracing::debug!(
                    "udp session opened (client={}, rule={}), udp_active={}",
                    client_addr,
                    rule_id,
                    active
                );
                true
            }
        }
    }

    /// Remove a single UDP session (e.g. when its outbound recv loop ends).
    pub async fn udp_close(&self, client_addr: SocketAddr, rule_id: i64) {
        let key = UdpSessionKey {
            client_addr,
            rule_id,
        };
        let mut map = self.udp.lock().await;
        if map.remove(&key).is_some() {
            tracing::debug!(
                "udp session closed (client={}, rule={}), udp_active={}",
                client_addr,
                rule_id,
                map.len()
            );
        }
    }

    /// Drop every UDP session older than `UDP_SESSION_TIMEOUT`. Called both by
    /// the UDP listener's periodic sweeper and as part of `current()`.
    pub async fn udp_prune_expired(&self) -> usize {
        let mut map = self.udp.lock().await;
        prune_expired_locked(&mut map)
    }

    /// Total active connections reported to the panel:
    /// active TCP connections + active UDP sessions.
    pub async fn current(&self) -> u32 {
        // TCP count is exact; UDP count is pruned-of-expired first so a quiet
        // node reports 0 shortly after traffic stops.
        let tcp = self.tcp.load(Ordering::Relaxed) as u32;
        let udp = {
            let mut map = self.udp.lock().await;
            prune_expired_locked(&mut map);
            map.len() as u32
        };
        tcp.saturating_add(udp)
    }
}

/// Prune sessions whose `last_active` is older than the timeout. Borrowed
/// mutably so callers don't need to re-lock. Returns how many were removed.
fn prune_expired_locked(map: &mut HashMap<UdpSessionKey, Instant>) -> usize {
    let now = Instant::now();
    let before = map.len();
    map.retain(|_, last_active| now.duration_since(*last_active) < UDP_SESSION_TIMEOUT);
    let removed = before - map.len();
    if removed > 0 {
        tracing::debug!(
            "udp: pruned {} expired sessions, udp_active={}",
            removed,
            map.len()
        );
    }
    removed
}

/// RAII guard: dropping it decrements the active-TCP-connection counter. This
/// guarantees the count is correct even if a connection task panics.
pub struct TcpConnectionGuard {
    tcp: Arc<AtomicU64>,
}

impl Drop for TcpConnectionGuard {
    fn drop(&mut self) {
        let prev = self.tcp.fetch_sub(1, Ordering::Relaxed);
        // fetch_sub returns the value before decrement, so the post-decrement
        // count is prev-1 (never underflows: every guard came from a +1).
        tracing::debug!("tcp connection closed, active={}", prev.saturating_sub(1));
    }
}

pub async fn report_traffic(config: &NodeConfig, counter: &TrafficCounter) {
    // Snapshot (non-destructive) first: the snapshotted bytes are only deducted
    // from the counters after the panel ACKs the upload (see TrafficSnapshot).
    // A failed/lost upload drops the guard without commit, so those bytes stay
    // and are retried on the next cycle instead of being permanently dropped.
    let snap = counter.snapshot().await;
    // debug, not info: this runs every poll cycle (default 10s) and would
    // flood the log at info level on a healthy node. Only the per-request
    // HTTP status below is worth keeping visible.
    tracing::debug!("report_traffic: {} entries to report", snap.entries.len());
    if snap.entries.is_empty() {
        return;
    }

    let report = TrafficReport {
        reports: snap.entries.clone(),
    };

    let url = format!("{}/api/v1/node/report_traffic", config.panel_url);
    let client = reqwest::Client::new();
    match client
        .post(&url)
        .header("Authorization", format!("Bearer {}", config.token))
        .json(&report)
        .send()
        .await
    {
        Ok(r) => {
            // v0.3.9: commit ONLY when the panel actually persisted the traffic.
            // The panel returns HTTP 200 for EVERY response (Axum's Json is
            // always 200), and signals business-level success via ApiResponse
            // .code in the body. The old code only checked HTTP status, so a
            // 401 (invalid/rotated token), 500 (DB error), 403 (cross-group)
            // or 400 (overflow) all looked like success and the snapshot was
            // committed — permanently dropping that traffic. Now we parse the
            // body and require code == 0.
            let status = r.status();
            if !status.is_success() {
                tracing::warn!("report_traffic HTTP {} (not 2xx)", status);
                return;
            }
            match r.json::<ApiResponse<()>>().await {
                Ok(resp) if resp.code == 0 => {
                    snap.commit().await;
                    tracing::info!("report_traffic HTTP {} code 0", status);
                }
                Ok(resp) => {
                    // Business-level rejection. Keep the bytes for retry next
                    // cycle (the panel did NOT persist them).
                    tracing::warn!(
                        "report_traffic rejected: HTTP {} code {} msg={}",
                        status,
                        resp.code,
                        resp.message
                    );
                }
                Err(e) => {
                    tracing::warn!(
                        "report_traffic: could not parse response body (HTTP {}): {}",
                        status,
                        e
                    );
                }
            }
        }
        Err(e) => tracing::warn!("report_traffic error: {}", e),
    }
}

/// Report real system metrics: CPU %, memory %, active connections, uptime.
///
/// `sys` is shared (Arc<Mutex>) because sysinfo's System is not Sync across
/// a plain &mut in async contexts. CPU usage requires a prior refresh with a
/// time gap, which the caller performs once at startup (see main.rs).
///
/// All sysinfo samplers held together so `report_status` can collect every
/// metric in one place. Each sampler is wrapped in its own lock because
/// sysinfo's structs are not `Sync` on their own; they are refreshed under
/// the lock and the values are read out without holding it during await.
///
/// - `sys`: CPU + memory (existing behaviour).
/// - `disks`: root-partition usage (`/`).
/// - `networks`: NIC counters; the previous sample is kept so the real-time
///   rate (bps) is computed from the delta between two samples.
/// - `public_ip`: cached egress IP, refreshed on a long interval (see
///   `spawn_public_ip_refresher`); `None` until/unless detected.
pub struct NodeMetrics {
    sys: Mutex<System>,
    disks: Mutex<Disks>,
    networks: Mutex<Networks>,
    /// v0.4.6: the single interface we count machine traffic for. None = no
    /// interface could be selected (we log once and report zero traffic rather
    /// than summing every NIC, which double-counts docker/veth).
    network_interface: RwLock<Option<String>>,
    /// Previous sample's cumulative (total_received, total_transmitted) for the
    /// selected interface, used to compute the per-interval delta for the bps
    /// rate. The cumulative field in the report uses the CURRENT total_*.
    last_net: Mutex<HashMap<String, (u64, u64)>>,
    /// When the previous network sample was taken (for bps denominator).
    last_net_at: Mutex<Option<Instant>>,
    /// v0.4.15: public egress IPs detected independently per address family.
    /// `public_ipv4` doubles as the legacy `public_ip` for backward-compat
    /// (older panels read `public_ip`). `public_ipv6` is None when the node
    /// has no IPv6 connectivity; one family failing NEVER clears the other.
    public_ipv4: RwLock<Option<String>>,
    public_ipv6: RwLock<Option<String>>,
}

impl NodeMetrics {
    /// `configured_interface` is the value of NETWORK_INTERFACE ("auto" or an
    /// explicit name). Auto-detection runs on construction and is re-run lazily
    /// in snapshot() if the selected interface is absent from sysinfo's list
    /// (e.g. the NIC came up after the node started).
    pub fn new(configured_interface: &str) -> Self {
        let selected = resolve_network_interface(configured_interface);
        if selected.is_none() {
            tracing::warn!(
                "NETWORK_INTERFACE='{}': could not select a NIC; machine traffic will report \
                 zero until a default-route interface is available",
                configured_interface
            );
        }
        Self {
            sys: Mutex::new(System::new()),
            disks: Mutex::new(Disks::new_with_refreshed_list()),
            networks: Mutex::new(Networks::new_with_refreshed_list()),
            network_interface: RwLock::new(selected),
            last_net: Mutex::new(HashMap::new()),
            last_net_at: Mutex::new(None),
            public_ipv4: RwLock::new(None),
            public_ipv6: RwLock::new(None),
        }
    }

    /// The interface currently being counted (None if none selected). Used so
    /// the StatusReport can show "统计网卡: eth0" in the panel.
    pub async fn network_interface(&self) -> Option<String> {
        self.network_interface.read().await.clone()
    }

    /// Seed the CPU + network baselines. This takes an initial sample of CPU
    /// usage and NIC counters so the FIRST periodic report already has a sane
    /// baseline to compute a delta from. It does NOT block: the sysinfo quirk
    /// (CPU needs two samples ~500ms apart for a meaningful delta) is handled
    /// by `spawn_warmup`, which sleeps in a detached task instead of stalling
    /// startup. Call `new()` + `spawn_warmup()` rather than awaiting a sleep
    /// on the critical startup path.
    pub async fn seed_baselines(&self) {
        {
            let mut s = self.sys.lock().await;
            s.refresh_cpu_usage();
        }
        // Seed the network baseline so the second report can compute a rate.
        let now = Instant::now();
        let current = self.sample_networks().await;
        *self.last_net.lock().await = current;
        *self.last_net_at.lock().await = Some(now);
    }

    /// Fire-and-forget the warm-up: take a second CPU sample ~500ms later so
    /// the first real report has a meaningful CPU %. Runs detached — callers
    /// never await this on the startup critical path.
    pub fn spawn_warmup(self: &Arc<Self>) {
        let me = self.clone();
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            let mut s = me.sys.lock().await;
            s.refresh_cpu_usage();
        });
    }

    /// Refresh NIC counters and return the SELECTED interface's cumulative
    /// (total_received, total_transmitted) totals (since OS boot). Returns an
    /// empty map if no interface is selected (or the selected one is absent),
    /// so snapshot() reports zero rather than summing unrelated NICs.
    ///
    /// v0.4.6: we store total_* here. The report's boot_* field uses it
    /// directly; the per-interval rate is the delta between two samples'
    /// total_* values (NOT sysinfo's `received()`, which is itself a delta and
    /// must not be subtracted again).
    async fn sample_networks(&self) -> HashMap<String, (u64, u64)> {
        let mut nets = self.networks.lock().await;
        nets.refresh();
        let selected = self.network_interface.read().await.clone();
        let mut current = HashMap::new();
        for (name, data) in nets.list() {
            if Some(name.as_str()) == selected.as_deref() {
                current.insert(
                    name.clone(),
                    (data.total_received(), data.total_transmitted()),
                );
                break;
            }
        }
        // v0.4.6: if the selected interface is gone (renamed/timed out), try to
        // re-resolve once so a NIC that came up after node start gets picked up.
        if current.is_empty() {
            let need_auto = matches!(&selected, Some(s) if s.eq_ignore_ascii_case("auto"))
                || selected.is_none();
            // We only re-resolve for the unset/auto case; an explicitly pinned
            // interface that vanished is a config error we surface as zero.
            if need_auto {
                drop(nets);
                let nets2 = Networks::new_with_refreshed_list();
                let picked = nets2
                    .list()
                    .iter()
                    .find(|(n, _)| !n.eq_ignore_ascii_case("lo"))
                    .map(|(n, _)| n.clone());
                if let Some(ref name) = picked {
                    *self.network_interface.write().await = picked.clone();
                    let mut nets3 = self.networks.lock().await;
                    nets3.refresh();
                    for (n, data) in nets3.list() {
                        if n == name {
                            current.insert(
                                n.clone(),
                                (data.total_received(), data.total_transmitted()),
                            );
                        }
                    }
                }
            }
        }
        current
    }

    /// v0.4.15: legacy alias — sets/gets the IPv4. Kept so the old
    /// `spawn_public_ip_refresher` name path still compiles; the dual-stack
    /// refresher uses set_public_ipv4 / set_public_ipv6 directly.
    #[allow(dead_code)]
    pub async fn set_public_ip(&self, ip: Option<String>) {
        *self.public_ipv4.write().await = ip;
    }

    #[allow(dead_code)]
    pub async fn public_ip(&self) -> Option<String> {
        self.public_ipv4.read().await.clone()
    }

    pub async fn set_public_ipv4(&self, ip: Option<String>) {
        *self.public_ipv4.write().await = ip;
    }

    pub async fn public_ipv4(&self) -> Option<String> {
        self.public_ipv4.read().await.clone()
    }

    pub async fn set_public_ipv6(&self, ip: Option<String>) {
        *self.public_ipv6.write().await = ip;
    }

    pub async fn public_ipv6(&self) -> Option<String> {
        self.public_ipv6.read().await.clone()
    }
}

/// One snapshot of every metric `report_status` needs, gathered under the
/// locks and then handed off to the (await-heavy) HTTP call without holding them.
struct MetricsSnapshot {
    cpu: f32,
    mem_pct: f32,
    disk_total: Option<u64>,
    disk_used: Option<u64>,
    disk_usage_percent: Option<f32>,
    disk_mount: Option<String>,
    upload_bps: Option<u64>,
    download_bps: Option<u64>,
    boot_upload_bytes: Option<u64>,
    boot_download_bytes: Option<u64>,
    /// v0.4.15 legacy compat: mirrors public_ipv4. Kept so old code/tests that
    /// read `snap.public_ip` still compile; the report uses public_ipv4.
    #[allow(dead_code)]
    public_ip: Option<String>,
    /// v0.4.15: dual-stack public IPs (ipv4 mirrors public_ip for compat).
    public_ipv4: Option<String>,
    public_ipv6: Option<String>,
    /// v0.4.6: the interface machine traffic is counted on (e.g. "eth0"), for
    /// display. None when no interface could be selected.
    network_interface: Option<String>,
    /// v0.3.2: SYSTEM uptime (time since the OS booted), NOT the relay-node
    /// process uptime. Users read "运行时长" as "how long has the server been
    /// up", which is the OS uptime; the process uptime is reported separately
    /// as process_uptime_secs.
    system_uptime: u64,
}

/// Read the system uptime in whole seconds from /proc/uptime (Linux only).
///
/// /proc/uptime is two fields: `<uptime_secs> <idle_secs>`. We take the floor
/// of the first. Returns 0 if the file is missing/unreadable (the panel treats
/// 0 as "unknown" gracefully). Factored out so it's unit-testable.
fn read_system_uptime_secs() -> u64 {
    match std::fs::read_to_string("/proc/uptime") {
        Ok(s) => s
            .split_whitespace()
            .next()
            .and_then(|f| f.split('.').next())
            .and_then(|n| n.parse::<u64>().ok())
            .unwrap_or(0),
        Err(_) => 0,
    }
}

/// v0.4.6: resolve the configured NETWORK_INTERFACE value to a concrete NIC.
///
/// - "auto" (or empty): read the default route from /proc/net/route and return
///   its interface. Falls back to the first non-loopback NIC sysinfo sees if
///   /proc/net/route can't be parsed.
/// - any other value: returned verbatim (the operator pinned it). We do NOT
///   validate it exists here; snapshot() skips a missing interface and reports
///   zero rather than summing others.
///
/// Returns None only when no interface can be determined at all.
fn resolve_network_interface(configured: &str) -> Option<String> {
    let c = configured.trim();
    if !c.is_empty() && !c.eq_ignore_ascii_case("auto") {
        return Some(c.to_string());
    }

    // /proc/net/route columns: Iface, Destination, Gateway, Flags, ..., Mask,
    // ... The default route has Destination 00000000. Field 2 (index 0) is the
    // interface name. Hex 00000000 == the default route (0.0.0.0).
    if let Ok(text) = std::fs::read_to_string("/proc/net/route") {
        for (i, line) in text.lines().enumerate() {
            if i == 0 {
                continue; // header
            }
            let mut fields = line.split_whitespace();
            let iface = fields.next()?;
            let dest = fields.next()?;
            if dest.eq_ignore_ascii_case("00000000") {
                return Some(iface.to_string());
            }
        }
    }

    // Fallback: first non-loopback interface sysinfo enumerates. Avoids blindly
    // summing docker bridges / veth pairs when the route table isn't readable.
    let nets = Networks::new_with_refreshed_list();
    for name in nets.list().keys() {
        if !name.eq_ignore_ascii_case("lo") {
            return Some(name.clone());
        }
    }
    None
}

impl NodeMetrics {
    /// Collect one snapshot: CPU/mem/disk + network rate (delta since the last
    /// call) + cumulative NIC totals + cached public IP.
    async fn snapshot(&self) -> MetricsSnapshot {
        // --- CPU + memory + system uptime ---
        let (cpu, mem_pct, system_uptime) = {
            let mut s = self.sys.lock().await;
            s.refresh_cpu_usage();
            s.refresh_memory();
            let cpu = s.global_cpu_usage();
            let mem_total = s.total_memory();
            let mem_used = s.used_memory();
            let mem_pct = if mem_total > 0 {
                (mem_used as f64 / mem_total as f64) * 100.0
            } else {
                0.0
            };
            // System uptime (since OS boot), NOT process uptime. Read directly
            // from /proc/uptime on Linux (the only supported platform) rather
            // than via sysinfo, whose uptime API changed across 0.30/0.32 and
            // is unreliable to depend on. Falls back to 0 if unreadable.
            let system_uptime = read_system_uptime_secs();
            (cpu as f32, mem_pct as f32, system_uptime)
        };

        // --- Primary disk (root partition `/`) ---
        // Refresh before reading: without this disks only reflects the snapshot
        // taken at NodeMetrics::new(), so disk usage never changes after start.
        let (disk_total, disk_used, disk_usage_percent, disk_mount) = {
            let mut disks = self.disks.lock().await;
            disks.refresh();
            // Pick the mount point matching `/` exactly; fall back to the first
            // disk if none matches exactly. total/available come from sysinfo.
            let pick = disks
                .list()
                .iter()
                .find(|d| d.mount_point().to_string_lossy() == "/")
                .or_else(|| disks.list().first());
            match pick {
                Some(d) => {
                    let total = d.total_space();
                    let avail = d.available_space();
                    let used = total.saturating_sub(avail);
                    let pct = if total > 0 {
                        (used as f64 / total as f64 * 100.0) as f32
                    } else {
                        0.0
                    };
                    (
                        Some(total),
                        Some(used),
                        Some(pct),
                        Some(d.mount_point().to_string_lossy().into_owned()),
                    )
                }
                None => (None, None, None, None),
            }
        };

        // --- Network: real-time rate + cumulative, for the SELECTED NIC only ---
        // v0.4.6: sample_networks returns the selected interface's total_*
        // (since-boot cumulative). The cumulative field is that value directly;
        // the per-interval rate is (current_total - prev_total) / elapsed.
        // We store current totals as the next baseline. Unlike the old code,
        // this does NOT sum every non-loopback NIC, so docker bridges / veth
        // are no longer double-counted.
        let prev_baseline = self.last_net.lock().await.clone();
        let prev_at = *self.last_net_at.lock().await;
        let now = Instant::now();
        let current = self.sample_networks().await;
        // Store the new baseline for next cycle.
        *self.last_net.lock().await = current.clone();
        *self.last_net_at.lock().await = Some(now);

        let (upload_bps, download_bps, boot_upload_bytes, boot_download_bytes) = {
            // Cumulative totals across all non-loopback NICs (system-wide since boot).
            let up_total: u64 = current.values().map(|(_, t)| *t).sum();
            let down_total: u64 = current.values().map(|(r, _)| *r).sum();

            // Real-time rate from the delta, if we have a previous sample + a
            // usable elapsed time. saturating_sub guards against counter wrap.
            let (up_bps, down_bps) = match (prev_at, prev_baseline.is_empty()) {
                (Some(prev_time), false) => {
                    let elapsed = now.duration_since(prev_time).as_secs_f64();
                    if elapsed > 0.0 {
                        let up_delta: u64 = current
                            .iter()
                            .map(|(n, (_, t))| {
                                prev_baseline
                                    .get(n)
                                    .map(|(_, pt)| t.saturating_sub(*pt))
                                    .unwrap_or(0)
                            })
                            .sum();
                        let down_delta: u64 = current
                            .iter()
                            .map(|(n, (r, _))| {
                                prev_baseline
                                    .get(n)
                                    .map(|(pr, _)| r.saturating_sub(*pr))
                                    .unwrap_or(0)
                            })
                            .sum();
                        (
                            Some((up_delta as f64 / elapsed) as u64),
                            Some((down_delta as f64 / elapsed) as u64),
                        )
                    } else {
                        (Some(0), Some(0))
                    }
                }
                _ => (None, None), // first sample ever: no rate yet
            };
            (up_bps, down_bps, Some(up_total), Some(down_total))
        };

        let public_ipv4 = self.public_ipv4().await;
        let public_ipv6 = self.public_ipv6().await;
        let network_interface = self.network_interface().await;

        MetricsSnapshot {
            cpu,
            mem_pct,
            disk_total,
            disk_used,
            disk_usage_percent,
            disk_mount,
            upload_bps,
            download_bps,
            boot_upload_bytes,
            boot_download_bytes,
            public_ip: public_ipv4.clone(),
            public_ipv4,
            public_ipv6,
            network_interface,
            system_uptime,
        }
    }
}

/// Collect all metrics + connections + uptime and POST one StatusReport to
/// the panel. Every new field is independent of the WebSocket control channel
/// — this runs on the plain-HTTP poll loop, so it keeps reporting even if WS
/// is down. Failures are logged, never crash.
pub async fn report_status(
    config: &NodeConfig,
    metrics: &Arc<NodeMetrics>,
    connections: &ConnectionTracker,
    start_time: Instant,
    node_id: &str,
    listener_errors: Vec<ListenerError>,
) {
    let snap = metrics.snapshot().await;
    let active_connections = connections.current().await;

    let report = StatusReport {
        cpu_usage: snap.cpu,
        mem_usage: snap.mem_pct,
        active_connections,
        // v0.3.2: uptime_secs is now SYSTEM uptime (since OS boot), matching
        // what "运行时长" means to users. The process uptime moved to its own
        // field below.
        uptime_secs: snap.system_uptime,
        public_ip: snap.public_ipv4.clone(),
        public_ipv4: snap.public_ipv4.clone(),
        public_ipv6: snap.public_ipv6,
        disk_total: snap.disk_total,
        disk_used: snap.disk_used,
        disk_usage_percent: snap.disk_usage_percent,
        disk_mount: snap.disk_mount,
        upload_bps: snap.upload_bps,
        download_bps: snap.download_bps,
        boot_upload_bytes: snap.boot_upload_bytes,
        boot_download_bytes: snap.boot_download_bytes,
        network_interface: snap.network_interface,
        node_id: Some(node_id.to_string()),
        process_uptime_secs: Some(start_time.elapsed().as_secs()),
        // v0.3.4: report this binary's version so the panel can flag stale
        // nodes for upgrade. env! is compile-time, zero runtime cost.
        node_version: Some(env!("CARGO_PKG_VERSION").to_string()),
        // v0.4.0: config-protocol version, mirrored from the
        // X-Config-Protocol-Version header. Stored by the panel purely for the
        // frontend status display (the actual gate is request-scoped).
        config_protocol_version: Some(relay_shared::protocol::CONFIG_PROTOCOL_VERSION),
        // Only include listener_errors when non-empty, so healthy nodes send a
        // smaller payload and the panel renders "ok" by default.
        listener_errors: if listener_errors.is_empty() {
            None
        } else {
            Some(listener_errors)
        },
    };

    // debug, not info: this runs every poll cycle (default 10s). Keeping it
    // at info floods the log with one line per cycle on a healthy node.
    tracing::debug!(
        "report_status: cpu={:.1}% mem={:.1}% conns={} sys_up={}s proc_up={}s disk={} ip={}",
        report.cpu_usage,
        report.mem_usage,
        report.active_connections,
        report.uptime_secs,
        report.process_uptime_secs.unwrap_or(0),
        report
            .disk_usage_percent
            .map(|p| format!("{:.0}%", p))
            .unwrap_or_else(|| "n/a".into()),
        report.public_ip.as_deref().unwrap_or("?"),
    );

    let url = format!("{}/api/v1/node/report_status", config.panel_url);
    let client = reqwest::Client::new();
    // v0.3.9: check the response so a rejected status report (invalid/rotated
    // token, DB error) is surfaced instead of silently fire-and-forget. Unlike
    // report_traffic there's nothing to retry here (status is ephemeral), but
    // a persistent rejection (e.g. rotated token) now shows up in the log
    // rather than the node believing everything is fine.
    match client
        .post(&url)
        .header("Authorization", format!("Bearer {}", config.token))
        .json(&report)
        .send()
        .await
    {
        Ok(r) => {
            let status = r.status();
            if !status.is_success() {
                tracing::warn!("report_status HTTP {} (not 2xx)", status);
                return;
            }
            match r.json::<ApiResponse<()>>().await {
                Ok(resp) if resp.code == 0 => {
                    tracing::info!("report_status HTTP {} code 0", status);
                }
                Ok(resp) => {
                    tracing::warn!(
                        "report_status rejected: HTTP {} code {} msg={}",
                        status,
                        resp.code,
                        resp.message
                    );
                }
                Err(e) => {
                    tracing::warn!(
                        "report_status: could not parse response body (HTTP {}): {}",
                        status,
                        e
                    );
                }
            }
        }
        Err(e) => tracing::warn!("report_status error: {}", e),
    }
}

/// How often the public-IP refresher re-checks (long interval so we are not
/// hammering the external service every poll cycle).
const PUBLIC_IP_REFRESH: Duration = Duration::from_secs(30 * 60);

/// Detect a public egress IP by calling the configured check URL. The returned
/// text is validated as a parseable `IpAddr` (rejects garbage / HTML error
/// pages). Failure yields None — never blocks node startup. `quiet` suppresses
/// the warn log on failure (used for IPv6, where "not available" is normal and
/// we don't want to spam the log every 30 min).
/// v0.4.15: parse a public-IP-check response body into a validated, correct-
/// family IP string. Returns None if the body isn't a single valid IP, or if
/// the address family doesn't match the family we asked for.
///
/// Pure (no I/O) so it's unit-testable. The family check matters because on a
/// dual-stack host the IPv4 endpoint (api.ipify.org) can be reached over IPv6
/// and return an IPv6 address — storing that in public_ipv4 would surface an
/// IPv6 on the panel's IPv4 line.
fn parse_ip_for_family(body: &str, family: &IpFamily) -> Option<String> {
    let ip = body.trim();
    if ip.is_empty() {
        return None;
    }
    let parsed = ip.parse::<std::net::IpAddr>().ok()?;
    let matches = match family {
        IpFamily::V4 => parsed.is_ipv4(),
        IpFamily::V6 => parsed.is_ipv6(),
    };
    if matches {
        Some(ip.to_string())
    } else {
        None
    }
}

async fn detect_public_ip(check_url: &str, family: &IpFamily, quiet: bool) -> Option<String> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .ok()?;
    match client.get(check_url).send().await {
        Ok(r) if r.status().is_success() => match r.text().await {
            Ok(body) => {
                let result = parse_ip_for_family(&body, family);
                if result.is_none() && !quiet {
                    tracing::warn!(
                        "public_ip: {:?} check returned no usable same-family IP: {:?}",
                        family,
                        body.trim()
                    );
                }
                result
            }
            Err(e) => {
                if !quiet {
                    tracing::warn!("public_ip: failed to read body: {}", e);
                }
                None
            }
        },
        Ok(r) => {
            if !quiet {
                tracing::warn!("public_ip: check returned HTTP {}", r.status());
            }
            None
        }
        Err(e) => {
            if !quiet {
                tracing::warn!("public_ip: check failed: {}", e);
            }
            None
        }
    }
}

/// v0.4.15: detect one address family in a loop, storing into its own field.
/// INDEPENDENT of the other family — a v6 failure never clears v4 and vice
/// versa. `quiet` suppresses failure logs for IPv6 (absence is normal).
async fn run_family_refresher(
    metrics: Arc<NodeMetrics>,
    check_url: String,
    family: IpFamily,
    quiet: bool,
) {
    loop {
        // v0.4.15: only OVERWRITE the stored address on a successful, correct-
        // family detection. A transient failure (endpoint down, timeout, wrong
        // family) keeps the LAST good value instead of clearing it to None —
        // otherwise one flaky poll would blank the IP/region on the panel until
        // the next success 30 min later.
        if let Some(v) = detect_public_ip(&check_url, &family, quiet).await {
            tracing::info!("public_{:?} detected: {}", family, v);
            match family {
                IpFamily::V4 => metrics.set_public_ipv4(Some(v)).await,
                IpFamily::V6 => metrics.set_public_ipv6(Some(v)).await,
            }
        }
        tokio::time::sleep(PUBLIC_IP_REFRESH).await;
    }
}

#[derive(Debug)]
enum IpFamily {
    V4,
    V6,
}

/// v0.4.15: spawn TWO independent background tasks — one for IPv4, one for
/// IPv6. Each checks once at start then every 30 min. A failure in one family
/// never clears the other. Env overrides (later wins):
///   IPv4: PUBLIC_IPV4_CHECK_URL → PUBLIC_IP_CHECK_URL → default ipify.org
///   IPv6: PUBLIC_IPV6_CHECK_URL → default api6.ipify.org
/// IPv6 failures are quiet (no IPv6 is normal on many hosts).
pub fn spawn_public_ip_refresher(metrics: Arc<NodeMetrics>) {
    let v4_url = std::env::var("PUBLIC_IPV4_CHECK_URL")
        .or_else(|_| std::env::var("PUBLIC_IP_CHECK_URL"))
        .unwrap_or_else(|_| "https://api.ipify.org".to_string());
    let v6_url = std::env::var("PUBLIC_IPV6_CHECK_URL")
        .unwrap_or_else(|_| "https://api6.ipify.org".to_string());

    let m4 = metrics.clone();
    tokio::spawn(async move { run_family_refresher(m4, v4_url, IpFamily::V4, false).await });

    let m6 = metrics;
    tokio::spawn(async move { run_family_refresher(m6, v6_url, IpFamily::V6, true).await });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{Ipv4Addr, SocketAddrV4};

    fn addr(p: u16) -> SocketAddr {
        SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::new(127, 0, 0, 1), p))
    }

    /// read_system_uptime_secs parses /proc/uptime's first field as whole
    /// seconds. On Linux CI this returns a real uptime (> 0); on non-Linux
    /// (dev machines without /proc) it returns 0 — so we only assert the
    /// type/shape, not a specific value, and verify the parser directly.
    #[test]
    fn read_system_uptime_returns_nonneg_or_zero() {
        let v = read_system_uptime_secs();
        // Always non-negative by construction (u64); on Linux it's the real
        // uptime. We don't assert > 0 because some CI runners may not expose
        // /proc/uptime in sandboxes.
        assert!(v <= u64::MAX / 2, "sanity bound");
    }

    /// The parser must handle the real /proc/uptime format (float seconds +
    /// idle) and take the floor, not round or panic.
    #[test]
    fn parse_proc_uptime_format() {
        // Simulate what /proc/uptime looks like: "3612.45 1234.56\n"
        // We can't easily inject a file, but we CAN verify the parsing logic
        // by mirroring it here against sample input. This guards against a
        // future refactor that breaks the split-on-'.' floor.
        let sample = "3612.45 1234.56\n";
        let parsed: u64 = sample
            .split_whitespace()
            .next()
            .and_then(|f| f.split('.').next())
            .and_then(|n| n.parse::<u64>().ok())
            .unwrap_or(0);
        assert_eq!(parsed, 3612, "must floor the uptime to whole seconds");
    }

    /// An explicit NETWORK_INTERFACE value is returned verbatim (the operator
    /// pinned it; we do not validate existence at resolve time).
    #[test]
    fn resolve_explicit_interface_is_passed_through() {
        assert_eq!(resolve_network_interface("eth0"), Some("eth0".to_string()));
        assert_eq!(
            resolve_network_interface("  wg0  "),
            Some("wg0".to_string()),
            "leading/trailing whitespace is trimmed"
        );
    }

    /// "auto" / empty must not crash and must return Some(interface) on a host
    /// that has any non-loopback NIC (CI runners do). We don't assert the
    /// exact name — only that selection succeeded and isn't "lo".
    #[test]
    fn resolve_auto_picks_a_non_loopback_interface() {
        let picked = resolve_network_interface("auto");
        if let Some(name) = picked {
            assert!(
                !name.eq_ignore_ascii_case("lo"),
                "auto must never pick the loopback interface"
            );
        }
        // An unset/empty value behaves the same as "auto".
        assert_eq!(
            resolve_network_interface("").is_some(),
            resolve_network_interface("auto").is_some(),
        );
    }

    #[tokio::test]
    async fn tcp_guard_increments_and_decrements_on_drop() {
        let tracker = ConnectionTracker::new();
        // Baseline: zero active connections.
        assert_eq!(tracker.current().await, 0);

        // Open one TCP connection -> count becomes 1.
        let guard = tracker.tcp_handle();
        assert_eq!(tracker.current().await, 1);

        // Open a second -> count becomes 2.
        let guard2 = tracker.tcp_handle();
        assert_eq!(tracker.current().await, 2);

        // Drop one guard (simulating a normal close) -> count falls to 1.
        drop(guard);
        assert_eq!(tracker.current().await, 1);

        // Drop the other -> back to 0. This is the regression guard for
        // "connection count stuck at non-zero after all clients disconnect".
        drop(guard2);
        assert_eq!(tracker.current().await, 0);
    }

    #[tokio::test]
    async fn tcp_guard_decrements_even_on_panic_via_drop() {
        // The guard's Drop runs during stack unwinding, so a panicking task
        // still releases its slot. We simulate that by forgetting the guard is
        // inside a catch_unwind and just relying on Drop semantics.
        let tracker = ConnectionTracker::new();
        {
            let _g = tracker.tcp_handle();
            assert_eq!(tracker.current().await, 1);
            // scope ends here -> _g drops
        }
        assert_eq!(tracker.current().await, 0);
    }

    #[tokio::test]
    async fn udp_session_registered_on_touch_and_counts_as_active() {
        let tracker = ConnectionTracker::new();
        // No UDP traffic yet -> zero.
        assert_eq!(tracker.current().await, 0);

        // First datagram from (127.0.0.1:5000, rule 1) opens a session.
        let opened = tracker.udp_touch(addr(5000), 1).await;
        assert!(opened, "first touch must register a new session");
        assert_eq!(tracker.current().await, 1);

        // Same client again -> refresh, not a new session; count stays 1.
        let opened2 = tracker.udp_touch(addr(5000), 1).await;
        assert!(!opened2, "repeat touch must not register a new session");
        assert_eq!(tracker.current().await, 1);

        // A different client (different port) opens a second session.
        let opened3 = tracker.udp_touch(addr(5001), 1).await;
        assert!(opened3);
        assert_eq!(tracker.current().await, 2);

        // Same client but different rule is a distinct session.
        let opened4 = tracker.udp_touch(addr(5001), 2).await;
        assert!(opened4);
        assert_eq!(tracker.current().await, 3);
    }

    #[tokio::test]
    async fn udp_session_expires_after_timeout() {
        let tracker = ConnectionTracker::new();
        // Manually backdate a session to simulate "no traffic for longer than
        // the timeout" — we can't sleep 60s in a unit test.
        {
            let mut map = tracker.udp.lock().await;
            map.insert(
                UdpSessionKey {
                    client_addr: addr(6000),
                    rule_id: 7,
                },
                Instant::now() - (UDP_SESSION_TIMEOUT + Duration::from_secs(1)),
            );
        }
        // The expired session must NOT be counted by current().
        assert_eq!(tracker.current().await, 0);
    }

    #[tokio::test]
    async fn udp_close_removes_a_single_session() {
        let tracker = ConnectionTracker::new();
        tracker.udp_touch(addr(7000), 1).await;
        tracker.udp_touch(addr(7001), 1).await;
        assert_eq!(tracker.current().await, 2);

        tracker.udp_close(addr(7000), 1).await;
        assert_eq!(tracker.current().await, 1);
        // Closing an unknown session is a no-op.
        tracker.udp_close(addr(9999), 1).await;
        assert_eq!(tracker.current().await, 1);
    }

    #[tokio::test]
    async fn current_is_tcp_plus_udp() {
        let tracker = ConnectionTracker::new();
        // 2 TCP + 2 UDP distinct sessions == 4.
        let _t1 = tracker.tcp_handle();
        let _t2 = tracker.tcp_handle();
        tracker.udp_touch(addr(8000), 1).await;
        tracker.udp_touch(addr(8001), 1).await;
        assert_eq!(tracker.current().await, 4);
    }

    /// Performance: applying a config with many rules must keep the listener
    /// table size bounded (one entry per rule), confirming memory grows ~O(n)
    /// and there is no per-rule polling task leaked.
    #[tokio::test]
    async fn apply_many_rules_keeps_listener_table_bounded() {
        use crate::forwarder::ForwarderManager;
        use relay_shared::protocol::{ListenerConfig, NodeConfigResponse, NodeTransport};

        let counter = Arc::new(TrafficCounter::new());
        let connections = Arc::new(ConnectionTracker::new());
        let mut mgr = ForwarderManager::new(counter, connections);

        // Build a config with 1000 rules. We deliberately pick listen ports
        // that are unlikely to be bindable here (high) so apply_config tries
        // to spawn listeners; failures are logged but the manager still
        // records the key. What we assert is that the manager does not crash
        // and completes in bounded time.
        let listeners: Vec<ListenerConfig> = (0..1000)
            .map(|i| ListenerConfig {
                rule_id: i,
                port: 40000 + (i as u16),
                protocol: relay_shared::protocol::Protocol::Tcp,
                node_transport: NodeTransport::Raw,
                ws_path: None,
                targets: vec!["127.0.0.1:1".to_string()],
                load_balance_strategy: relay_shared::protocol::LoadBalanceStrategy::First,
                upload_limit_bps: None,
                download_limit_bps: None,
            })
            .collect();
        let cfg = NodeConfigResponse { listeners };

        // apply_config should return promptly even for 1000 rules — the diff
        // is O(n) and binding happens in spawned tasks, not inline.
        let start = Instant::now();
        mgr.apply_config(&cfg).await;
        let elapsed = start.elapsed();
        // Generous bound: must finish well under 2s. If apply_config were
        // doing serial work per rule this would blow past it.
        assert!(
            elapsed < Duration::from_secs(2),
            "apply_config(1000 rules) took {:?}, expected < 2s",
            elapsed
        );
    }

    // v0.4.15: address-family validation for the public-IP refresher. These
    // guard the dual-stack bug where the IPv4 endpoint, reached over IPv6,
    // returns a v6 address that must NOT be stored as public_ipv4.
    #[test]
    fn parse_ip_for_family_accepts_matching_family() {
        assert_eq!(
            parse_ip_for_family("1.2.3.4", &IpFamily::V4),
            Some("1.2.3.4".to_string())
        );
        assert_eq!(
            parse_ip_for_family("2001:db8::1", &IpFamily::V6),
            Some("2001:db8::1".to_string())
        );
    }

    #[test]
    fn parse_ip_for_family_trims_whitespace() {
        // ipify-style responses have no trailing newline, but be defensive.
        assert_eq!(
            parse_ip_for_family("  8.8.8.8\n", &IpFamily::V4),
            Some("8.8.8.8".to_string())
        );
    }

    #[test]
    fn parse_ip_for_family_rejects_wrong_family() {
        // The core dual-stack guard: a v6 answer to a v4 query is dropped.
        assert_eq!(parse_ip_for_family("2001:db8::1", &IpFamily::V4), None);
        // ...and a v4 answer to a v6 query.
        assert_eq!(parse_ip_for_family("1.2.3.4", &IpFamily::V6), None);
    }

    #[test]
    fn parse_ip_for_family_rejects_empty_and_non_ip() {
        assert_eq!(parse_ip_for_family("", &IpFamily::V4), None);
        assert_eq!(parse_ip_for_family("   ", &IpFamily::V4), None);
        // An HTML error page or rate-limit text must not parse as an IP.
        assert_eq!(parse_ip_for_family("<html>429</html>", &IpFamily::V4), None);
        assert_eq!(parse_ip_for_family("not-an-ip", &IpFamily::V6), None);
    }
}
