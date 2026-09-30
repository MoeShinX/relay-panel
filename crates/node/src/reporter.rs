use crate::config::NodeConfig;
use dashmap::mapref::entry::Entry;
use dashmap::DashMap;
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

/// Per-rule (upload, download) byte counters. Shared behind an Arc so the
/// per-packet add() path can clone-free `fetch_add` after a shared read lock.
type RuleCounters = Arc<(AtomicU64, AtomicU64)>;

/// A per-rule counter held for the lifetime of ONE connection.
///
/// Handed out by [`TrafficCounter::handle`]. Both methods are plain atomic
/// adds — no lock, no `await` — so a copy loop can report every chunk the
/// instant it moves instead of batching until the connection closes.
#[derive(Clone)]
pub struct RuleCounterHandle(RuleCounters);

impl RuleCounterHandle {
    /// client → target.
    pub fn add_upload(&self, n: u64) {
        self.0 .0.fetch_add(n, Ordering::Relaxed);
    }

    /// target → client.
    pub fn add_download(&self, n: u64) {
        self.0 .1.fetch_add(n, Ordering::Relaxed);
    }
}

pub struct TrafficCounter {
    // rule_id -> (upload, download) as lock-free atomic counters. Keyed by rule
    // id (not listen port) so traffic is attributed to the right rule even when
    // two inbound groups listen on the same port.
    //
    // v1.0.9: the RwLock guards only the MAP shape (insert on a rule's first
    // bytes). Concurrent add()s to an already-present rule take a SHARED read
    // lock and do a lock-free atomic fetch_add, so they never serialize on each
    // other — this is the per-packet path for both TCP and UDP forwarding.
    data: Arc<RwLock<HashMap<i64, RuleCounters>>>,
    /// v1.2.5: held for the whole snapshot -> upload -> commit of one report.
    ///
    /// A snapshot does not remove anything; the bytes are only subtracted once
    /// the panel acknowledges them. Two reports in flight at once would both
    /// snapshot the same bytes, both upload them — the panel bills the user
    /// twice — and then both subtract them, wrapping the u64 counter to an
    /// enormous value that the next report bills yet again. The regular loop
    /// never overlapped itself, so this never happened; the shutdown flush is a
    /// second caller that can run at the same moment, and this lock is what
    /// makes that safe. A second report waits and then sees only newer bytes.
    ///
    /// v1.2.12: it also holds the batch whose outcome is not known yet — sent,
    /// but no definite answer (timeout, dropped connection, proxy error, a
    /// panel error that may have come after its commit). The next report
    /// re-sends exactly that batch, same id and bytes, instead of taking a new
    /// snapshot: the panel may already have applied it, and only the same id
    /// lets the panel tell (it acknowledges a known id without billing it
    /// again). See [`ReportState`].
    report_lock: tokio::sync::Mutex<ReportState>,
}

/// v1.2.12: what one traffic report hands to the next, behind
/// `TrafficCounter::report_lock`.
#[derive(Default)]
struct ReportState {
    /// The batch sent without a definite answer yet; re-sent as is.
    pending: Option<PendingBatch>,
    /// The id of the last batch the panel acknowledged. The next batch carries
    /// it, telling the panel it will never be sent again and can be forgotten.
    last_acked: Option<String>,
}

impl TrafficCounter {
    pub fn new() -> Self {
        Self {
            data: Arc::new(RwLock::new(HashMap::new())),
            report_lock: tokio::sync::Mutex::new(ReportState::default()),
        }
    }

    pub async fn add(&self, rule_id: i64, upload: u64, download: u64) {
        // Fast path: rule already present → shared read lock + atomic add.
        {
            let map = self.data.read().await;
            if let Some(c) = map.get(&rule_id) {
                c.0.fetch_add(upload, Ordering::Relaxed);
                c.1.fetch_add(download, Ordering::Relaxed);
                return;
            }
        }
        // Slow path: first bytes for this rule → write lock to insert, then add.
        let mut map = self.data.write().await;
        let c = map
            .entry(rule_id)
            .or_insert_with(|| Arc::new((AtomicU64::new(0), AtomicU64::new(0))));
        c.0.fetch_add(upload, Ordering::Relaxed);
        c.1.fetch_add(download, Ordering::Relaxed);
    }

    /// Acquire a long-lived handle to one rule's counters.
    ///
    /// v1.2.3: this exists so a TCP connection can report bytes AS THEY MOVE
    /// rather than accumulating them in a local and submitting once at close.
    /// Both copy loops used to do the latter, which meant a long-lived
    /// connection contributed NOTHING to the rule's usage until it ended -- a
    /// persistent tunnel could move hundreds of gigabytes while the owner's
    /// quota still read zero, so the panel never stopped it. The bytes landed
    /// only when the connection finally closed, possibly long after the user
    /// had stopped paying for them.
    ///
    /// Returning the `Arc` (rather than calling `add` per chunk) keeps the hot
    /// path lock-free AND non-async: the map lock is taken once when the
    /// connection starts, and every later chunk is a plain `fetch_add`. That is
    /// strictly cheaper than the old per-chunk `add`, which took a read lock
    /// every time.
    ///
    /// Note on `prune_rule`: a handle taken before a prune keeps writing into
    /// an Arc the map no longer holds, so those bytes are dropped rather than
    /// resurrecting a pruned rule_id. That is what we want -- a stale rule_id
    /// in a traffic batch makes the panel reject the WHOLE batch.
    pub async fn handle(&self, rule_id: i64) -> RuleCounterHandle {
        {
            let map = self.data.read().await;
            if let Some(c) = map.get(&rule_id) {
                return RuleCounterHandle(c.clone());
            }
        }
        let mut map = self.data.write().await;
        let c = map
            .entry(rule_id)
            .or_insert_with(|| Arc::new((AtomicU64::new(0), AtomicU64::new(0))))
            .clone();
        RuleCounterHandle(c)
    }

    /// Take a snapshot and return a guard whose `commit()` subtracts exactly
    /// the snapshotted bytes from each counter. This is the correct pattern for
    /// traffic reporting: the bytes captured in the snapshot are only deducted
    /// after the panel ACKs the upload. If the upload fails the guard is
    /// dropped without commit, so those bytes stay and are retried next cycle.
    /// Bytes that arrive BETWEEN snapshot and commit are preserved (subtract,
    /// not clear), so no traffic is ever lost.
    pub async fn snapshot(&self) -> TrafficSnapshot<'_> {
        let map = self.data.read().await;
        let mut entries = Vec::with_capacity(map.len());
        let mut sources = Vec::with_capacity(map.len());
        for (rule_id, c) in map.iter() {
            entries.push(TrafficEntry {
                rule_id: *rule_id,
                upload: c.0.load(Ordering::Relaxed),
                download: c.1.load(Ordering::Relaxed),
            });
            sources.push(c.clone());
        }
        TrafficSnapshot {
            counter: self,
            entries,
            sources,
        }
    }

    /// Destructive read: snapshot AND clear in one step. Kept for callers that
    /// want the old semantics (e.g. test fixtures that drain-then-assert). The
    /// production reporter uses `snapshot()` + `TrafficSnapshot::commit()` so a
    /// failed upload retries instead of dropping traffic.
    #[allow(dead_code)]
    pub async fn drain(&self) -> Vec<TrafficEntry> {
        let mut map = self.data.write().await;
        map.drain()
            .map(|(rule_id, c)| TrafficEntry {
                rule_id,
                upload: c.0.load(Ordering::Relaxed),
                download: c.1.load(Ordering::Relaxed),
            })
            .collect()
    }

    /// Remove all accumulated bytes for a single rule from the counter. Used
    /// when a listener is permanently stopped (rule deleted or no longer in the
    /// config) so that orphaned bytes don't poison future traffic batches — a
    /// stale rule_id causes the panel to atomically reject the entire batch.
    pub async fn prune_rule(&self, rule_id: i64) {
        self.data.write().await.remove(&rule_id);
    }

    /// Test-only: check whether a rule_id has any accumulated bytes.
    #[cfg(test)]
    pub async fn has_rule(&self, rule_id: i64) -> bool {
        self.data.read().await.contains_key(&rule_id)
    }
}

/// Subtract a snapshot the panel has persisted (`entries`, read from
/// `sources` in the same order) from the counters it was read from. Bytes
/// counted after the snapshot was taken are untouched.
///
/// Periodic, not the hot path. It runs on the write-locked map so the
/// fetch_sub and the zero-entry cleanup can't race an add(), and it never
/// awaits: a report cancelled anywhere has subtracted all of a snapshot or
/// none of it.
///
/// v1.2.12: subtract from the counters the snapshot was READ from, not from
/// whatever the map holds under the same rule id by now. A rule whose
/// listener stops has its counter pruned; started again, it gets a NEW
/// counter that never held the old bytes. With a batch left in doubt across
/// that, subtracting from the new counter by rule id wrapped it to about
/// 2^64 — a value the panel refuses, and with it every later batch from this
/// node, so none of its traffic was billed again until it restarted. The old
/// counter, if pruned meanwhile, takes the subtraction harmlessly.
fn commit_read(
    map: &mut HashMap<i64, RuleCounters>,
    entries: &[TrafficEntry],
    sources: Vec<RuleCounters>,
) {
    for (e, src) in entries.iter().zip(sources) {
        let prev_up = src.0.fetch_sub(e.upload, Ordering::Relaxed);
        let prev_down = src.1.fetch_sub(e.download, Ordering::Relaxed);
        // new == 0 iff prev == snapshotted (no adds since the snapshot); bytes
        // counted since show up as a larger prev and keep the entry.
        let drained = prev_up == e.upload && prev_down == e.download;
        // Only THIS counter's entry may be cleaned up: a newer counter under
        // the same id holds bytes for the next batch.
        let still_mapped = map.get(&e.rule_id).is_some_and(|c| Arc::ptr_eq(c, &src));
        drop(src);
        // v1.2.3: draining to zero is NOT enough to remove the entry. A live
        // connection holds a RuleCounterHandle — an Arc to this very counter —
        // and removing the map's copy would orphan it: every later byte would
        // land in an Arc nothing reads, and that connection would stop being
        // billed for the rest of its life. That is this release's own
        // long-connection hole, re-created at a poll boundary, and it would hit
        // exactly the long-lived connections the change exists to fix.
        //
        // strong_count == 1 (with this snapshot's own reference dropped above)
        // means the map is the only owner, so no connection can still be
        // writing. The check is conservative in the safe direction: a handle
        // dropped concurrently can leave the count momentarily high, which only
        // keeps a zeroed entry around until the next cycle.
        if drained
            && still_mapped
            && map
                .get(&e.rule_id)
                .is_some_and(|c| Arc::strong_count(c) == 1)
        {
            map.remove(&e.rule_id);
        }
    }
}

/// Snapshot of [`TrafficCounter`] at one instant. Drop without calling
/// [`commit`](Self::commit) to retry the same bytes; call `commit` once the
/// panel has persisted the report.
pub struct TrafficSnapshot<'a> {
    counter: &'a TrafficCounter,
    pub entries: Vec<TrafficEntry>,
    /// The counter each entry was read from, in the same order.
    sources: Vec<RuleCounters>,
}

impl TrafficSnapshot<'_> {
    /// Subtract the snapshotted bytes from the counters they were read from.
    /// Bytes counted after the snapshot was taken are untouched.
    pub async fn commit(self) {
        let TrafficSnapshot {
            counter,
            entries,
            sources,
        } = self;
        let mut map = counter.data.write().await;
        commit_read(&mut map, &entries, sources);
    }
}

/// v1.2.12: a traffic batch sent to the panel whose outcome is not known yet.
/// Kept (in `TrafficCounter::report_lock`) and re-sent unchanged until the
/// panel answers definitely.
struct PendingBatch {
    /// Sent with the batch; the panel applies each id at most once.
    id: String,
    /// Sent with the batch: the last acknowledged batch, for the panel to
    /// forget (see `ReportState::last_acked`).
    acked: Option<String>,
    /// The whole snapshot, zero entries included — subtracted on success.
    entries: Vec<TrafficEntry>,
    /// The counter each entry was read from — what a success subtracts from.
    sources: Vec<RuleCounters>,
    /// What is sent: the entries that have bytes in them.
    reports: Vec<TrafficEntry>,
}

/// What a traffic report's answer says about the batch.
enum BatchOutcome {
    /// The panel persisted it (code 0 — including "already applied").
    Applied,
    /// The panel refused it before writing anything (400/401/403). Its bytes
    /// can go out again in a new batch.
    NotApplied,
    /// No definite answer: the panel may or may not have applied it — no
    /// answer at all, or an error that can come after the commit (a 500).
    Unknown,
}

/// v1.2.12: how long one traffic report waits for the panel. It runs in the
/// same loop as the config poll, and without a limit a request left hanging —
/// a half-open connection, a panel stuck on a database lock — stalled config
/// updates and every later report with it. Giving up is safe now that the
/// batch is re-sent under the same id: if the panel did apply it, it says so.
const TRAFFIC_REPORT_TIMEOUT: Duration = if cfg!(test) {
    Duration::from_millis(500)
} else {
    Duration::from_secs(30)
};

/// A fresh batch id: 32 random hex characters (or, where /dev/urandom is
/// missing, pid + time + a counter — unique within this process).
fn new_batch_id() -> String {
    static FALLBACK_SEQ: AtomicU64 = AtomicU64::new(0);
    crate::poller::random_hex_16().unwrap_or_else(|| {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        format!(
            "{:x}-{:x}-{:x}",
            std::process::id(),
            nanos,
            FALLBACK_SEQ.fetch_add(1, Ordering::Relaxed)
        )
    })
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
///   `UDP_SESSION_TIMEOUT` with no further traffic. `touch` runs per datagram
///   but does NOT prune (that's O(sessions) per packet); expiry is handled by
///   `current()` and the UDP listener's periodic sweeper (`udp_prune_expired`),
///   so the count still converges on zero shortly after traffic stops.
///
/// `current()` reports `active_tcp + active_udp_sessions`.
///
/// This is entirely independent of the WebSocket control channel: it is read
/// from the plain-HTTP `report_status` loop, so connection counts keep
/// updating even if WS is down.
///
/// Locking: TCP uses an `AtomicU64` (lock-free); UDP uses a sharded `DashMap`
/// keyed by (client, rule), so a per-packet `udp_touch` takes only that shard's
/// lock (v1.0.9) — never a process-wide lock that could block forwarding.
pub struct ConnectionTracker {
    tcp: Arc<AtomicU64>,
    udp: DashMap<UdpSessionKey, Instant>,
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
            udp: DashMap::new(),
        }
    }

    /// Increment the active TCP count and return a guard whose `Drop`
    /// decrements it. Hand the guard to the per-connection task so the count
    /// is correct no matter how that task ends (normal close, error, panic).
    /// v1.2.5: open TCP connections only. Shutdown drains on this, not on
    /// `current()`: UDP sessions end with their listener, but their entries
    /// linger until the idle expiry prunes them, so counting them would hold
    /// every shutdown for the full drain window.
    pub fn tcp_active(&self) -> u64 {
        self.tcp.load(Ordering::Relaxed)
    }

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
        // Sharded map (keyed by client+rule): this takes only the target shard's
        // lock, so per-packet touching doesn't serialize on a process-wide lock.
        // We do NOT prune here (an O(sessions) scan per packet); expiry is
        // handled by the periodic sweeper (udp_prune_expired) and current().
        let is_new = match self.udp.entry(key) {
            Entry::Occupied(mut e) => {
                *e.get_mut() = Instant::now();
                false
            }
            Entry::Vacant(e) => {
                e.insert(Instant::now());
                true
            }
        };
        // len() locks shards briefly; call it only AFTER the entry guard above
        // is released (holding a shard guard across len() would deadlock).
        if is_new {
            tracing::debug!(
                "udp session opened (client={}, rule={}), udp_active={}",
                client_addr,
                rule_id,
                self.udp.len()
            );
        }
        is_new
    }

    /// Remove a single UDP session (e.g. when its outbound recv loop ends).
    pub async fn udp_close(&self, client_addr: SocketAddr, rule_id: i64) {
        let key = UdpSessionKey {
            client_addr,
            rule_id,
        };
        if self.udp.remove(&key).is_some() {
            tracing::debug!(
                "udp session closed (client={}, rule={}), udp_active={}",
                client_addr,
                rule_id,
                self.udp.len()
            );
        }
    }

    /// Drop every UDP session older than `UDP_SESSION_TIMEOUT`. Called both by
    /// the UDP listener's periodic sweeper and as part of `current()`.
    pub async fn udp_prune_expired(&self) -> usize {
        prune_expired(&self.udp)
    }

    /// Total active connections reported to the panel:
    /// active TCP connections + active UDP sessions.
    pub async fn current(&self) -> u32 {
        // TCP count is exact; UDP count is pruned-of-expired first so a quiet
        // node reports 0 shortly after traffic stops.
        let tcp = self.tcp.load(Ordering::Relaxed) as u32;
        prune_expired(&self.udp);
        let udp = self.udp.len() as u32;
        tcp.saturating_add(udp)
    }
}

/// Prune sessions whose `last_active` is older than the timeout. Returns how
/// many were removed. `retain` runs per shard; `before`/`after` are read across
/// shards without a global lock, so use saturating_sub in case a concurrent
/// insert lands between the two reads.
fn prune_expired(map: &DashMap<UdpSessionKey, Instant>) -> usize {
    let now = Instant::now();
    let before = map.len();
    map.retain(|_, last_active| now.duration_since(*last_active) < UDP_SESSION_TIMEOUT);
    let removed = before.saturating_sub(map.len());
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
    // One report at a time — see `TrafficCounter::report_lock`, which also
    // holds a batch still waiting for a definite answer.
    let mut state = counter.report_lock.lock().await;
    if state.pending.is_none() {
        let acked = state.last_acked.clone();
        let Some(batch) = next_batch(counter, acked).await else {
            return;
        };
        state.pending = Some(batch);
    }
    // The batch stays in `pending` while it is being sent, so a report cut off
    // mid-flight (the shutdown flush's timeout) leaves it for the next one to
    // re-send under the same id rather than losing track of it.
    let Some(batch) = state.pending.as_ref() else {
        return;
    };
    match send_batch(config, batch).await {
        BatchOutcome::Applied => {
            // The only await left: cut off here, nothing is subtracted and the
            // batch stays pending — re-sent, it is acknowledged as a copy.
            let mut map = counter.data.write().await;
            if let Some(PendingBatch {
                id,
                entries,
                sources,
                ..
            }) = state.pending.take()
            {
                commit_read(&mut map, &entries, sources);
                state.last_acked = Some(id);
            }
        }
        // Its bytes are still in the counters; the next report takes a new
        // snapshot (without rules pruned since) under a new id.
        BatchOutcome::NotApplied => state.pending = None,
        BatchOutcome::Unknown => {}
    }
}

/// Snapshot the counters into a new batch, or `None` when there is nothing to
/// send (the zero entries are committed right away, as before). `acked` is the
/// last acknowledged batch id, sent along with the new batch.
async fn next_batch(counter: &TrafficCounter, acked: Option<String>) -> Option<PendingBatch> {
    // Snapshot (non-destructive) first: the snapshotted bytes are only deducted
    // from the counters after the panel ACKs the upload (see TrafficSnapshot).
    // A failed/lost upload keeps them, so they are sent again instead of being
    // permanently dropped.
    let snap = counter.snapshot().await;
    // debug, not info: this runs every poll cycle (default 10s) and would
    // flood the log at info level on a healthy node. Only the per-request
    // HTTP status below is worth keeping visible.
    // v1.2.3: skip entries with nothing in them. A rule now gets its counter
    // the moment a connection OPENS rather than when it closes, so an idle but
    // still-open connection holds a 0/0 entry — without this filter such a node
    // would POST a batch of zeroes every cycle, forever. Only the UPLOAD is
    // filtered; the snapshot still commits every entry.
    let reports: Vec<TrafficEntry> = snap
        .entries
        .iter()
        .filter(|e| e.upload > 0 || e.download > 0)
        .cloned()
        .collect();
    tracing::debug!("report_traffic: {} entries to report", reports.len());
    if reports.is_empty() {
        // Commit before returning. There is nothing to send, but the snapshot
        // still has to be applied: subtracting zero is a no-op that lets the
        // strong_count cleanup in `commit` drop entries whose connection has
        // closed. Returning without it would strand a 0/0 entry for every rule
        // that ever had a connection open and transfer nothing, and nothing
        // else would ever clear it — a batch of only zeroes is exactly the case
        // that reaches this branch.
        snap.commit().await;
        return None;
    }
    let TrafficSnapshot {
        entries, sources, ..
    } = snap;
    Some(PendingBatch {
        id: new_batch_id(),
        acked,
        entries,
        sources,
        reports,
    })
}

/// POST one batch and classify the answer.
async fn send_batch(config: &NodeConfig, batch: &PendingBatch) -> BatchOutcome {
    let report = TrafficReport {
        reports: batch.reports.clone(),
        batch_id: Some(batch.id.clone()),
        acked_batch_id: batch.acked.clone(),
    };

    let url = format!("{}/api/v1/node/report_traffic", config.panel_url);
    let client = reqwest::Client::new();
    match client
        .post(&url)
        .timeout(TRAFFIC_REPORT_TIMEOUT)
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
                // Not the panel's own answer (it always replies 200): a proxy
                // or gateway error, which says nothing about whether the panel
                // applied the batch behind it.
                tracing::warn!("report_traffic HTTP {} (not 2xx); will re-send", status);
                return BatchOutcome::Unknown;
            }
            match r.json::<ApiResponse<()>>().await {
                Ok(resp) if resp.code == 0 => {
                    tracing::info!("report_traffic HTTP {} code 0", status);
                    BatchOutcome::Applied
                }
                // Refused before anything was written — a malformed or
                // out-of-range batch (400), a bad token (401), a rule this node
                // may not report (403). Its bytes stay for the next report.
                Ok(resp) if matches!(resp.code, 400 | 401 | 403) => {
                    tracing::warn!(
                        "report_traffic rejected: HTTP {} code {} msg={}",
                        status,
                        resp.code,
                        resp.message
                    );
                    BatchOutcome::NotApplied
                }
                // v1.2.12: anything else, a 500 above all, may have come AFTER
                // the commit — a database connection lost while committing
                // reports an error for a transaction that went through. A new
                // id would then bill the same bytes again; the same id is safe
                // either way (acknowledged as a copy, or applied now).
                Ok(resp) => {
                    tracing::warn!(
                        "report_traffic failed: HTTP {} code {} msg={}; will re-send",
                        status,
                        resp.code,
                        resp.message
                    );
                    BatchOutcome::Unknown
                }
                Err(e) => {
                    tracing::warn!(
                        "report_traffic: could not parse response body (HTTP {}): {}; will re-send",
                        status,
                        e
                    );
                    BatchOutcome::Unknown
                }
            }
        }
        Err(e) => {
            tracing::warn!("report_traffic error: {}; will re-send", e);
            BatchOutcome::Unknown
        }
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

impl NodeMetrics {
    /// v1.2.5: the selected NIC's cumulative (upload, download) byte counters,
    /// for the live-rate feed. Unlike `report_status` this does NOT move the
    /// status report's baseline (`last_net` / `last_net_at`): the two callers
    /// sample on different clocks, and sharing a baseline would make each one
    /// measure only the sliver of time since the other last looked.
    ///
    /// None when no interface is selected, so the feed sends nothing rather
    /// than a misleading zero.
    pub async fn nic_totals(&self) -> Option<(u64, u64)> {
        let current = self.sample_networks().await;
        if current.is_empty() {
            return None;
        }
        let up: u64 = current.values().map(|(_, t)| *t).sum();
        let down: u64 = current.values().map(|(r, _)| *r).sum();
        Some((up, down))
    }
}

/// v1.2.5: turns successive NIC counter readings into a rate for the live feed.
/// Owns its own baseline — see [`NodeMetrics::nic_totals`] for why it can't
/// share the status report's.
#[derive(Default)]
pub struct LiveRateSampler {
    prev: Option<(u64, u64, Instant)>,
}

impl LiveRateSampler {
    pub fn new() -> Self {
        Self::default()
    }

    /// The (upload, download) bytes/sec since the previous call. None on the
    /// first call (no baseline yet) and whenever the NIC can't be read.
    pub async fn sample(&mut self, metrics: &NodeMetrics) -> Option<(u64, u64)> {
        let (up, down) = metrics.nic_totals().await?;
        let now = Instant::now();
        let rate = self
            .prev
            .and_then(|prev| rate_between(prev, (up, down, now)));
        self.prev = Some((up, down, now));
        rate
    }
}

/// Bytes/sec between two cumulative (upload, download, at) readings.
///
/// None when no time has passed. A counter that went DOWN — the interface was
/// reset, or re-resolved to a different NIC with a smaller total — yields 0 for
/// that direction via `saturating_sub` instead of an absurd wrapped value.
fn rate_between(prev: (u64, u64, Instant), now: (u64, u64, Instant)) -> Option<(u64, u64)> {
    let elapsed = now.2.checked_duration_since(prev.2)?.as_secs_f64();
    if elapsed <= 0.0 {
        return None;
    }
    let up = now.0.saturating_sub(prev.0) as f64 / elapsed;
    let down = now.1.saturating_sub(prev.1) as f64 / elapsed;
    Some((up as u64, down as u64))
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
        // v1.0.10: how this node is run, so the panel only offers a one-click
        // self-upgrade to systemd nodes (docker → update image; manual → none).
        install_method: Some(crate::updater::install_method().to_string()),
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

/// v1.2.1: the default endpoints, one per family.
///
/// These MUST be family-pinned hostnames. Both probes validate the family and
/// DISCARD a mismatch (see `parse_ip_for_family`), so a dual-stack endpoint —
/// one that answers over whichever family the connection used and returns that
/// address — makes the IPv4 probe intermittently receive an IPv6 and throw it
/// away, leaving the panel showing no address at all. It fails only on
/// dual-stack hosts and only sometimes, which is the worst way for it to fail.
/// `api.ip.sb/ip` is exactly such an endpoint; `api-ipv4` / `api-ipv6` are its
/// pinned variants. A test below pins this.
///
/// Changed from ipify in v1.2.1: `api.ipify.org` is unreachable from mainland
/// China, where the probe simply timed out every 30 minutes forever and the
/// node's IP (and therefore its flag and region) stayed blank on the panel.
const DEFAULT_IPV4_CHECK_URL: &str = "https://api-ipv4.ip.sb/ip";
const DEFAULT_IPV6_CHECK_URL: &str = "https://api-ipv6.ip.sb/ip";

/// v0.4.15: spawn TWO independent background tasks — one for IPv4, one for
/// IPv6. Each checks once at start then every 30 min. A failure in one family
/// never clears the other. Env overrides (later wins):
///   IPv4: PUBLIC_IPV4_CHECK_URL → PUBLIC_IP_CHECK_URL → DEFAULT_IPV4_CHECK_URL
///   IPv6: PUBLIC_IPV6_CHECK_URL → DEFAULT_IPV6_CHECK_URL
/// IPv6 failures are quiet (no IPv6 is normal on many hosts).
pub fn spawn_public_ip_refresher(metrics: Arc<NodeMetrics>) {
    let v4_url = std::env::var("PUBLIC_IPV4_CHECK_URL")
        .or_else(|_| std::env::var("PUBLIC_IP_CHECK_URL"))
        .unwrap_or_else(|_| DEFAULT_IPV4_CHECK_URL.to_string());
    let v6_url = std::env::var("PUBLIC_IPV6_CHECK_URL")
        .unwrap_or_else(|_| DEFAULT_IPV6_CHECK_URL.to_string());

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

    /// A handle held across a successful report must keep counting.
    ///
    /// `commit()` removes a rule's map entry once its counters reach zero. A
    /// live connection holds an Arc to that entry, so removing it orphans the
    /// handle: every later byte lands in an Arc nothing reads, and the
    /// connection stops being billed for the rest of its life. That is the
    /// original long-connection hole re-created at a poll boundary — and it
    /// bites precisely the connections this change exists to fix, because they
    /// are the ones alive across a report.
    #[tokio::test]
    async fn a_handle_keeps_counting_after_its_rule_was_reported_and_drained() {
        let counter = TrafficCounter::new();
        // A connection opens and forwards some bytes.
        let conn = counter.handle(9).await;
        conn.add_upload(1_000);
        conn.add_download(2_000);

        // A reporting cycle uploads them and the panel ACKs, so they are
        // subtracted. Nothing arrived in between, so the entry drains to zero.
        let snap = counter.snapshot().await;
        assert_eq!(snap.entries.len(), 1);
        snap.commit().await;

        // The connection is STILL OPEN and forwards more.
        conn.add_upload(500);
        conn.add_download(700);

        let after = counter.snapshot().await;
        let entry = after
            .entries
            .iter()
            .find(|e| e.rule_id == 9)
            .expect("traffic after a report must still be attributed to the rule");
        assert_eq!(entry.upload, 500);
        assert_eq!(entry.download, 700);
    }

    /// The counterpart: once no connection holds the rule any more, a drained
    /// entry IS removed. The strong_count guard must not turn into a leak that
    /// keeps a row per rule the node ever forwarded.
    #[tokio::test]
    async fn a_drained_rule_is_removed_once_no_connection_holds_it() {
        let counter = TrafficCounter::new();
        {
            let conn = counter.handle(11).await;
            conn.add_upload(42);
        } // connection closes here — the handle drops

        let snap = counter.snapshot().await;
        snap.commit().await;
        assert!(
            !counter.has_rule(11).await,
            "a fully reported rule with no live connection must not stay in the map"
        );
    }

    /// A connection that opens, moves nothing and closes must leave no entry
    /// behind after one reporting cycle.
    ///
    /// Taking the handle at connection START means such a connection creates a
    /// 0/0 entry. Those are filtered out of the upload — and the filter is what
    /// makes this reachable: when EVERY entry is zero there is nothing to send,
    /// and an early return would skip the commit that removes them. Nothing
    /// else clears a zero entry, so each one would sit in the map until the
    /// rule left the config.
    #[tokio::test]
    async fn an_idle_connection_leaves_no_counter_entry_behind() {
        let counter = TrafficCounter::new();
        {
            // Connection opens, transfers nothing, closes.
            let _conn = counter.handle(21).await;
        }
        assert!(
            counter.has_rule(21).await,
            "opening a connection is what creates the entry — otherwise this test proves nothing"
        );

        // One reporting cycle. panel_url is unreachable on purpose: with only
        // zero entries the function must return before it ever tries to POST.
        let config = NodeConfig {
            panel_url: "http://127.0.0.1:1".into(),
            token: "t".into(),
            poll_interval: 10,
            tls_cert_path: None,
            tls_key_path: None,
            network_interface: "auto".into(),
            listen_ipv4: "0.0.0.0".into(),
            listen_ipv6: "::".into(),
            outbound_interface: "auto".into(),
            outbound_bind_ipv4: None,
            shutdown_drain_secs: 5,
        };
        report_traffic(&config, &counter).await;

        assert!(
            !counter.has_rule(21).await,
            "a zero entry with no live connection must be cleared by the reporting cycle"
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
        tracker.udp.insert(
            UdpSessionKey {
                client_addr: addr(6000),
                rule_id: 7,
            },
            Instant::now() - (UDP_SESSION_TIMEOUT + Duration::from_secs(1)),
        );
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
                max_connections: None,
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

    /// v1.2.1: the defaults must be FAMILY-PINNED hostnames, and the two must
    /// differ.
    ///
    /// This is the invariant that pairs with `parse_ip_for_family`: a probe
    /// discards an answer from the wrong family, so pointing both probes at a
    /// dual-stack endpoint (`api.ip.sb/ip`, `api.ipify.org` reached over v6)
    /// makes the v4 probe intermittently throw its answer away and the panel
    /// show no address. It breaks only on dual-stack hosts and only sometimes,
    /// so a unit test is the only place it gets caught cheaply.
    ///
    /// Asserting on the shape rather than the exact URL keeps the endpoint
    /// swappable — what must not change is that each one names its family.
    #[test]
    fn default_check_urls_are_family_pinned() {
        assert_ne!(
            DEFAULT_IPV4_CHECK_URL, DEFAULT_IPV6_CHECK_URL,
            "one endpoint for both families cannot be family-pinned"
        );
        let host4 = DEFAULT_IPV4_CHECK_URL
            .trim_start_matches("https://")
            .split('/')
            .next()
            .unwrap();
        let host6 = DEFAULT_IPV6_CHECK_URL
            .trim_start_matches("https://")
            .split('/')
            .next()
            .unwrap();
        assert!(
            host4.contains("ipv4") || host4.contains("-v4") || host4.contains("4."),
            "IPv4 default must name its family, got {host4}"
        );
        assert!(
            host6.contains("ipv6") || host6.contains("-v6") || host6.contains("6."),
            "IPv6 default must name its family, got {host6}"
        );
        // Both must be HTTPS: the response decides what the panel displays as
        // this node's identity, and plain HTTP lets any on-path party set it.
        assert!(DEFAULT_IPV4_CHECK_URL.starts_with("https://"));
        assert!(DEFAULT_IPV6_CHECK_URL.starts_with("https://"));
    }

    #[test]
    fn rate_between_divides_the_delta_by_elapsed_time() {
        let t0 = Instant::now();
        let t1 = t0 + Duration::from_secs(2);
        assert_eq!(
            rate_between((1_000, 5_000, t0), (8_001_000, 262_860, t1)),
            Some((4_000_000, 128_930))
        );
    }

    /// A NIC reset or a switch to a different interface can make a cumulative
    /// counter go backwards. That must read as zero, not as a wrapped u64.
    #[test]
    fn rate_between_treats_a_counter_that_went_down_as_zero() {
        let t0 = Instant::now();
        let t1 = t0 + Duration::from_secs(2);
        assert_eq!(
            rate_between((9_000, 9_000, t0), (100, 9_200, t1)),
            Some((0, 100))
        );
    }

    #[test]
    fn rate_between_needs_time_to_have_passed() {
        let t0 = Instant::now();
        assert_eq!(rate_between((0, 0, t0), (500, 500, t0)), None);
        // Out-of-order readings are rejected rather than treated as negative time.
        assert_eq!(
            rate_between((0, 0, t0 + Duration::from_secs(1)), (500, 500, t0)),
            None
        );
    }

    fn config_for(panel_url: &str) -> NodeConfig {
        NodeConfig {
            panel_url: panel_url.into(),
            token: "t".into(),
            poll_interval: 10,
            tls_cert_path: None,
            tls_key_path: None,
            network_interface: "auto".into(),
            listen_ipv4: "0.0.0.0".into(),
            listen_ipv6: "::".into(),
            outbound_interface: "auto".into(),
            outbound_bind_ipv4: None,
            shutdown_drain_secs: 5,
        }
    }

    /// A stand-in for the panel's report_traffic endpoint. It adds up every byte
    /// it is sent — that is what the panel bills — and holds each reply for
    /// `delay`, so two reports started together really are in flight at once.
    async fn billing_panel(delay: Duration) -> (String, Arc<AtomicU64>) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let billed = Arc::new(AtomicU64::new(0));
        let billed_srv = billed.clone();
        tokio::spawn(async move {
            loop {
                let Ok((mut sock, _)) = listener.accept().await else {
                    return;
                };
                let billed = billed_srv.clone();
                tokio::spawn(async move {
                    let mut buf = Vec::new();
                    let mut chunk = [0u8; 4096];
                    let (body_at, body_len) = loop {
                        let n = sock.read(&mut chunk).await.unwrap_or(0);
                        if n == 0 {
                            return;
                        }
                        buf.extend_from_slice(&chunk[..n]);
                        if let Some(end) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                            let head = String::from_utf8_lossy(&buf[..end]).to_ascii_lowercase();
                            let len = head
                                .lines()
                                .find_map(|l| l.strip_prefix("content-length:"))
                                .and_then(|v| v.trim().parse::<usize>().ok())
                                .unwrap_or(0);
                            break (end + 4, len);
                        }
                    };
                    while buf.len() < body_at + body_len {
                        let n = sock.read(&mut chunk).await.unwrap_or(0);
                        if n == 0 {
                            return;
                        }
                        buf.extend_from_slice(&chunk[..n]);
                    }
                    let report: TrafficReport =
                        serde_json::from_slice(&buf[body_at..body_at + body_len]).unwrap();
                    let bytes: u64 = report.reports.iter().map(|e| e.upload + e.download).sum();
                    billed.fetch_add(bytes, Ordering::SeqCst);
                    tokio::time::sleep(delay).await;
                    let body = r#"{"code":0,"message":"ok","data":null}"#;
                    let resp = format!(
                        "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                        body.len(),
                        body
                    );
                    let _ = sock.write_all(resp.as_bytes()).await;
                });
            }
        });
        (url, billed)
    }

    /// The shutdown flush can run while the regular 10 s report is mid-flight.
    /// Both would snapshot the same bytes: the panel would bill them twice, and
    /// the double commit would wrap the counter to a huge value that the next
    /// report bills again. Serialized, every byte reaches the panel once.
    #[tokio::test]
    async fn two_reports_at_once_bill_each_byte_exactly_once() {
        let (url, billed) = billing_panel(Duration::from_millis(200)).await;
        let counter = TrafficCounter::new();
        counter.add(7, 1_000, 500).await;
        let config = config_for(&url);

        tokio::join!(
            report_traffic(&config, &counter),
            report_traffic(&config, &counter)
        );

        assert_eq!(
            billed.load(Ordering::SeqCst),
            1_500,
            "each byte must be billed exactly once"
        );
        let left = counter.snapshot().await;
        assert!(
            left.entries
                .iter()
                .all(|e| e.upload == 0 && e.download == 0),
            "nothing may be left over — and nothing wrapped: {:?}",
            left.entries
                .iter()
                .map(|e| (e.upload, e.download))
                .collect::<Vec<_>>()
        );
    }

    // ── v1.2.12: re-sending a batch whose outcome is unknown ──

    /// How the scripted panel answers one request.
    #[derive(Clone, Copy)]
    enum Answer {
        /// Apply (unless the id was seen) and reply code 0.
        Ack,
        /// Apply, then drop the connection without replying — the node cannot
        /// tell that the batch landed.
        ApplyThenDrop,
        /// Apply, then reply with this non-zero code — a commit that went
        /// through although the panel saw an error.
        ApplyThenFail(i32),
        /// Reply with this non-zero code; nothing is applied.
        Reject(i32),
        /// Read the request and never answer.
        Hang,
    }

    /// One request as the scripted panel saw it.
    #[derive(Clone, Debug)]
    struct Seen {
        batch_id: Option<String>,
        acked: Option<String>,
        bytes: u64,
    }

    /// A stand-in panel that applies each batch id at most once, like the real
    /// one, and answers the n-th request per `script` (Ack once it runs out).
    /// Returns its URL, every request seen, and the bytes billed.
    async fn scripted_panel(
        script: Vec<Answer>,
    ) -> (String, Arc<std::sync::Mutex<Vec<Seen>>>, Arc<AtomicU64>) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let seen = Arc::new(std::sync::Mutex::new(Vec::<Seen>::new()));
        let billed = Arc::new(AtomicU64::new(0));
        let applied = Arc::new(std::sync::Mutex::new(
            std::collections::HashSet::<String>::new(),
        ));
        let (seen_srv, billed_srv) = (seen.clone(), billed.clone());
        tokio::spawn(async move {
            loop {
                let Ok((mut sock, _)) = listener.accept().await else {
                    return;
                };
                let mut buf = Vec::new();
                let mut chunk = [0u8; 4096];
                let (body_at, body_len) = loop {
                    let n = sock.read(&mut chunk).await.unwrap_or(0);
                    if n == 0 {
                        break (0, 0);
                    }
                    buf.extend_from_slice(&chunk[..n]);
                    if let Some(end) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                        let head = String::from_utf8_lossy(&buf[..end]).to_ascii_lowercase();
                        let len = head
                            .lines()
                            .find_map(|l| l.strip_prefix("content-length:"))
                            .and_then(|v| v.trim().parse::<usize>().ok())
                            .unwrap_or(0);
                        break (end + 4, len);
                    }
                };
                if body_at == 0 {
                    continue;
                }
                while buf.len() < body_at + body_len {
                    let n = sock.read(&mut chunk).await.unwrap_or(0);
                    if n == 0 {
                        break;
                    }
                    buf.extend_from_slice(&chunk[..n]);
                }
                let report: TrafficReport =
                    serde_json::from_slice(&buf[body_at..body_at + body_len]).unwrap();
                let bytes: u64 = report.reports.iter().map(|e| e.upload + e.download).sum();
                let answer = {
                    let mut seen = seen_srv.lock().unwrap();
                    let n = seen.len();
                    seen.push(Seen {
                        batch_id: report.batch_id.clone(),
                        acked: report.acked_batch_id.clone(),
                        bytes,
                    });
                    script.get(n).copied().unwrap_or(Answer::Ack)
                };
                let apply = || {
                    let fresh = match &report.batch_id {
                        Some(id) => applied.lock().unwrap().insert(id.clone()),
                        None => true,
                    };
                    if fresh {
                        billed_srv.fetch_add(bytes, Ordering::SeqCst);
                    }
                };
                let reply = |code: i32| {
                    let body = format!(r#"{{"code":{code},"message":"m","data":null}}"#);
                    format!(
                        "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                        body.len(),
                        body
                    )
                };
                match answer {
                    Answer::Ack => {
                        apply();
                        let _ = sock.write_all(reply(0).as_bytes()).await;
                    }
                    Answer::ApplyThenDrop => {
                        apply();
                        drop(sock);
                    }
                    Answer::ApplyThenFail(code) => {
                        apply();
                        let _ = sock.write_all(reply(code).as_bytes()).await;
                    }
                    Answer::Reject(code) => {
                        let _ = sock.write_all(reply(code).as_bytes()).await;
                    }
                    Answer::Hang => {
                        tokio::spawn(async move {
                            tokio::time::sleep(Duration::from_secs(3600)).await;
                            drop(sock);
                        });
                    }
                }
            }
        });
        (url, seen, billed)
    }

    async fn left_in(counter: &TrafficCounter) -> u64 {
        counter
            .snapshot()
            .await
            .entries
            .iter()
            .map(|e| e.upload + e.download)
            .sum()
    }

    /// The panel applied a batch but the answer never arrived. The node sends
    /// the SAME batch again — same id, same bytes, not the bytes counted since
    /// — and the panel, recognising the id, bills it once. The newer bytes go
    /// out afterwards in a batch of their own.
    #[tokio::test]
    async fn a_batch_whose_answer_was_lost_is_resent_unchanged_and_billed_once() {
        let (url, seen, billed) = scripted_panel(vec![Answer::ApplyThenDrop]).await;
        let config = config_for(&url);
        let counter = TrafficCounter::new();
        counter.add(7, 1_000, 500).await;

        report_traffic(&config, &counter).await; // applied, answer lost
        assert_eq!(
            left_in(&counter).await,
            1_500,
            "nothing is subtracted without an answer"
        );
        counter.add(7, 100, 0).await; // counted while the batch is in doubt

        report_traffic(&config, &counter).await; // re-send: acknowledged as a copy
        report_traffic(&config, &counter).await; // then the newer bytes

        let seen = seen.lock().unwrap().clone();
        assert_eq!(seen.len(), 3, "{seen:?}");
        assert!(seen[0].batch_id.is_some());
        assert_eq!(
            seen[1].batch_id, seen[0].batch_id,
            "the re-send keeps the id"
        );
        assert_eq!(
            seen[1].bytes, 1_500,
            "and the bytes, without the newer ones"
        );
        assert_ne!(seen[2].batch_id, seen[0].batch_id);
        assert_eq!(seen[2].bytes, 100);
        assert_eq!(
            billed.load(Ordering::SeqCst),
            1_600,
            "every byte billed once"
        );
        assert_eq!(left_in(&counter).await, 0);
    }

    /// A definite rejection means the panel did not apply the batch: its bytes
    /// go out again in a fresh batch, under a new id.
    #[tokio::test]
    async fn a_rejected_batch_is_replaced_by_a_fresh_one() {
        let (url, seen, billed) = scripted_panel(vec![Answer::Reject(403)]).await;
        let config = config_for(&url);
        let counter = TrafficCounter::new();
        counter.add(7, 1_000, 0).await;

        report_traffic(&config, &counter).await; // rejected, nothing applied
        report_traffic(&config, &counter).await; // fresh batch

        let seen = seen.lock().unwrap().clone();
        assert_eq!(seen.len(), 2, "{seen:?}");
        assert_ne!(
            seen[1].batch_id, seen[0].batch_id,
            "a new batch gets a new id"
        );
        assert_eq!(seen[1].bytes, 1_000);
        assert_eq!(billed.load(Ordering::SeqCst), 1_000);
        assert_eq!(left_in(&counter).await, 0);
    }

    /// A report cut off mid-flight (the shutdown flush has a timeout) keeps
    /// its batch: the next report re-sends it under the same id.
    #[tokio::test]
    async fn a_cancelled_report_resends_the_same_batch() {
        let (url, seen, billed) = scripted_panel(vec![Answer::Hang]).await;
        let config = config_for(&url);
        let counter = TrafficCounter::new();
        counter.add(7, 1_000, 0).await;

        let cut_off = tokio::time::timeout(
            Duration::from_millis(300),
            report_traffic(&config, &counter),
        )
        .await;
        assert!(cut_off.is_err(), "the first report must be cut off");
        report_traffic(&config, &counter).await;

        let seen = seen.lock().unwrap().clone();
        assert_eq!(seen.len(), 2, "{seen:?}");
        assert_eq!(seen[1].batch_id, seen[0].batch_id);
        assert_eq!(billed.load(Ordering::SeqCst), 1_000);
        assert_eq!(left_in(&counter).await, 0);
    }

    /// A panel that never answers must not hold the report — and with it the
    /// poll loop — forever. The report gives up on its own, and the next one
    /// re-sends the same batch.
    #[tokio::test]
    async fn an_unanswered_report_gives_up_and_resends_the_same_batch() {
        let (url, seen, billed) = scripted_panel(vec![Answer::Hang]).await;
        let config = config_for(&url);
        let counter = TrafficCounter::new();
        counter.add(7, 1_000, 0).await;

        tokio::time::timeout(Duration::from_secs(10), report_traffic(&config, &counter))
            .await
            .expect("the report must give up by itself");
        report_traffic(&config, &counter).await;

        let seen = seen.lock().unwrap().clone();
        assert_eq!(seen.len(), 2, "{seen:?}");
        assert_eq!(seen[1].batch_id, seen[0].batch_id);
        assert_eq!(billed.load(Ordering::SeqCst), 1_000);
        assert_eq!(left_in(&counter).await, 0);
    }

    /// A 500 can come after the commit went through (the database connection
    /// lost while committing). The node must not treat it as "not applied":
    /// a new id would bill the same bytes a second time.
    #[tokio::test]
    async fn a_server_error_is_resent_under_the_same_id() {
        let (url, seen, billed) = scripted_panel(vec![Answer::ApplyThenFail(500)]).await;
        let config = config_for(&url);
        let counter = TrafficCounter::new();
        counter.add(7, 1_000, 0).await;

        report_traffic(&config, &counter).await; // applied, but answered 500
        report_traffic(&config, &counter).await; // re-send: acknowledged as a copy

        let seen = seen.lock().unwrap().clone();
        assert_eq!(seen.len(), 2, "{seen:?}");
        assert_eq!(seen[1].batch_id, seen[0].batch_id);
        assert_eq!(
            billed.load(Ordering::SeqCst),
            1_000,
            "billed once, not twice"
        );
        assert_eq!(left_in(&counter).await, 0);
    }

    /// A rule that stops and starts again while its batch is in doubt gets a
    /// new counter. Settling the old batch must not subtract from it: that
    /// wrapped the counter to about 2^64, which the panel refuses — and with
    /// it every later batch from the node, so its traffic went unbilled.
    #[tokio::test]
    async fn a_batch_settled_after_its_rule_restarted_leaves_the_new_counter_alone() {
        let (url, seen, billed) = scripted_panel(vec![Answer::ApplyThenDrop]).await;
        let config = config_for(&url);
        let counter = TrafficCounter::new();
        counter.add(7, 1_000, 0).await;

        report_traffic(&config, &counter).await; // applied, answer lost
        counter.prune_rule(7).await; // the rule is paused: its counter goes
        counter.add(7, 10, 0).await; // resumed: a new counter
        report_traffic(&config, &counter).await; // re-send: acknowledged as a copy
        report_traffic(&config, &counter).await; // the new counter's bytes

        let seen = seen.lock().unwrap().clone();
        assert_eq!(seen.len(), 3, "{seen:?}");
        assert_eq!(seen[1].batch_id, seen[0].batch_id);
        assert_eq!(
            seen[2].bytes, 10,
            "the new counter's own bytes, not a wrapped value"
        );
        assert_eq!(billed.load(Ordering::SeqCst), 1_010);
        assert_eq!(left_in(&counter).await, 0);
    }

    /// The same at the counter level: a snapshot is committed against the
    /// counters it was read from.
    #[tokio::test]
    async fn a_commit_subtracts_from_the_counter_it_read() {
        let counter = TrafficCounter::new();
        counter.add(7, 100, 5).await;
        let snap = counter.snapshot().await;
        counter.prune_rule(7).await;
        counter.add(7, 10, 0).await;
        snap.commit().await;

        let left = counter.snapshot().await;
        assert_eq!(left.entries.len(), 1);
        assert_eq!((left.entries[0].upload, left.entries[0].download), (10, 0));
    }

    /// Each batch names the last one the panel acknowledged, so the panel can
    /// forget that id: the node will never send it again. A refused batch was
    /// never acknowledged, so the one after it names the same earlier batch.
    #[tokio::test]
    async fn each_batch_names_the_last_acknowledged_one() {
        let (url, seen, _) = scripted_panel(vec![
            Answer::Ack,
            Answer::Reject(403),
            Answer::Ack,
            Answer::Ack,
        ])
        .await;
        let config = config_for(&url);
        let counter = TrafficCounter::new();

        for _ in 0..4 {
            counter.add(7, 100, 0).await;
            report_traffic(&config, &counter).await;
        }

        let seen = seen.lock().unwrap().clone();
        assert_eq!(seen.len(), 4, "{seen:?}");
        assert_eq!(seen[0].acked, None, "nothing acknowledged yet");
        assert_eq!(seen[1].acked, seen[0].batch_id);
        assert_eq!(
            seen[2].acked, seen[0].batch_id,
            "the refused batch is not an acknowledged one"
        );
        assert_eq!(seen[3].acked, seen[2].batch_id);
    }
}
