// v1.2.12: the latency monitor — the node side.
//
// The panel gives every node the same plan (`NodeConfigResponse::latency`): a
// list of targets and an interval. The node probes each target once per
// interval, adds the results up per target and per whole minute, and puts each
// finished minute into its next status report (see `reporter::report_status`).
//
// Probing is a side channel, like diagnose.rs: it opens its own sockets, never
// goes through the forwarder, and so counts no traffic, no connection and no
// rate limit. With no plan (no targets configured, or a panel that does not
// know the feature) nothing is probed at all.
//
//   * TCP: the time `TcpStream::connect` takes to finish the handshake; the
//     stream is dropped at once.
//   * ICMP: an echo request through `surge-ping`, which opens an unprivileged
//     ping socket where the kernel allows it and a raw socket otherwise. When
//     neither can be opened the target is reported `unsupported`, which the
//     panel shows as "this node cannot do ICMP" — never as packet loss.
//   * Domain names are resolved at most every 10 minutes (IPv4 first); a name
//     that does not resolve counts as a lost probe.
//
// Reporting is at-least-once: a finished minute stays in the buffer (at most
// 5 minutes of them) until a report that carried it was accepted, and the
// panel stores a minute idempotently, so re-sending is harmless.

use relay_shared::protocol::{LatencyKind, LatencyProbeConfig, LatencyProbeTarget, LatencySample};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::net::{IpAddr, SocketAddr};
use std::sync::atomic::{AtomicU16, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::net::TcpStream;
use tokio::sync::{watch, Semaphore};
use tokio::task::{JoinHandle, JoinSet};
use tokio::time::{interval, timeout, MissedTickBehavior};

/// How long one probe may take before it counts as lost.
const PROBE_TIMEOUT: Duration = Duration::from_secs(3);
/// Probes in flight at once, across all targets.
const MAX_CONCURRENT_PROBES: usize = 8;
/// A resolved domain name is used for this long before it is looked up again.
const RESOLVE_TTL: Duration = Duration::from_secs(600);
/// A minute is reported once this long after it ended: a probe started in its
/// last seconds can still be waiting for its answer (3 s) or for a free slot.
const MINUTE_GRACE_SECS: i64 = 10;
/// Finished minutes older than this are dropped rather than kept for another
/// try — the panel would refuse them anyway (it takes the last 10 minutes).
const KEEP_SECS: i64 = 5 * 60;

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    // A panic while holding the lock leaves plain counters behind; they are
    // still usable, and refusing to report would be the worse failure.
    m.lock().unwrap_or_else(|e| e.into_inner())
}

fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn minute_of(ts: i64) -> i64 {
    ts - ts.rem_euclid(60)
}

/// What one probe came to.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Outcome {
    /// Answered after this many milliseconds.
    Rtt(f64),
    /// No answer in time (or the name did not resolve).
    Lost,
    /// This node cannot probe the target this way at all.
    Unsupported,
}

/// One target's probes within one minute.
#[derive(Debug, Clone, PartialEq)]
struct Accum {
    sent: u32,
    lost: u32,
    sum_ms: f64,
    min_ms: f64,
    max_ms: f64,
}

impl Accum {
    fn new() -> Self {
        Self {
            sent: 0,
            lost: 0,
            sum_ms: 0.0,
            min_ms: f64::INFINITY,
            max_ms: 0.0,
        }
    }

    fn add(&mut self, rtt: Option<f64>) {
        self.sent += 1;
        match rtt {
            Some(ms) => {
                self.sum_ms += ms;
                self.min_ms = self.min_ms.min(ms);
                self.max_ms = self.max_ms.max(ms);
            }
            None => self.lost += 1,
        }
    }

    fn to_sample(&self, target_id: i64, minute_ts: i64) -> LatencySample {
        let answered = self.sent > self.lost;
        LatencySample {
            target_id,
            minute_ts,
            sent: self.sent,
            lost: self.lost,
            rtt_sum_ms: self.sum_ms,
            // Zero, not infinity, when nothing answered: the panel ignores the
            // round trip of a fully lost minute, and JSON has no infinity.
            rtt_min_ms: if answered { self.min_ms } else { 0.0 },
            rtt_max_ms: if answered { self.max_ms } else { 0.0 },
            unsupported: false,
        }
    }
}

/// What has been measured and not yet acknowledged. All time is passed in, so
/// the bookkeeping is testable without a clock.
#[derive(Default)]
struct Ledger {
    minutes: BTreeMap<(i64, i64), Accum>,
    unsupported: BTreeSet<i64>,
    /// The newest minute the panel has acknowledged. A probe that finishes
    /// after its minute was acknowledged (it was started in time but ran past
    /// the grace) is dropped instead of becoming a one-probe "minute" that
    /// would replace the full one on the panel.
    acked_through: i64,
}

impl Ledger {
    fn record(&mut self, target_id: i64, started_at: i64, rtt: Option<f64>) {
        let minute = minute_of(started_at);
        if minute <= self.acked_through {
            return;
        }
        self.minutes
            .entry((target_id, minute))
            .or_insert_with(Accum::new)
            .add(rtt);
    }

    fn set_unsupported(&mut self, target_id: i64, unsupported: bool) {
        if unsupported {
            self.unsupported.insert(target_id);
        } else {
            self.unsupported.remove(&target_id);
        }
    }

    /// The plan changed: forget the minute in progress (it mixes old and new
    /// targets) and everything about targets that are gone.
    fn reset(&mut self, now: i64, keep: &BTreeSet<i64>) {
        self.minutes
            .retain(|(id, minute), _| keep.contains(id) && minute + 60 <= now);
        self.unsupported.retain(|id| keep.contains(id));
    }

    /// Finished minutes, oldest first, plus an `unsupported` marker for every
    /// target that cannot be probed. Minutes older than the keep window are
    /// discarded here.
    fn pending(&mut self, now: i64) -> Vec<LatencySample> {
        let oldest = minute_of(now) - KEEP_SECS;
        self.minutes.retain(|(_, minute), _| *minute >= oldest);
        let mut ready: Vec<LatencySample> = self
            .minutes
            .iter()
            .filter(|((_, minute), _)| minute + 60 + MINUTE_GRACE_SECS <= now)
            .map(|((id, minute), acc)| acc.to_sample(*id, *minute))
            .collect();
        ready.sort_by_key(|s| (s.minute_ts, s.target_id));
        ready.extend(self.unsupported.iter().map(|id| LatencySample {
            target_id: *id,
            minute_ts: 0,
            sent: 0,
            lost: 0,
            rtt_sum_ms: 0.0,
            rtt_min_ms: 0.0,
            rtt_max_ms: 0.0,
            unsupported: true,
        }));
        ready
    }

    /// The panel accepted these: forget them.
    fn ack(&mut self, sent: &[LatencySample]) {
        for s in sent.iter().filter(|s| !s.unsupported) {
            self.minutes.remove(&(s.target_id, s.minute_ts));
            self.acked_through = self.acked_through.max(s.minute_ts);
        }
    }
}

/// The node's latency monitor: hand it the panel's plan, take finished
/// minutes out for the status report.
pub struct LatencyMonitor {
    ledger: Mutex<Ledger>,
    plan: watch::Sender<Option<LatencyProbeConfig>>,
}

impl LatencyMonitor {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            ledger: Mutex::new(Ledger::default()),
            plan: watch::channel(None).0,
        })
    }

    /// Start the task that follows the plan. Call once.
    pub fn spawn(self: &Arc<Self>) -> JoinHandle<()> {
        let monitor = self.clone();
        let rx = self.plan.subscribe();
        tokio::spawn(async move { monitor.supervise(rx).await })
    }

    /// Give the monitor the panel's latest plan (`None`: probe nothing). An
    /// identical plan changes nothing — config pushes are frequent and must not
    /// restart the probing each time.
    pub fn set_plan(&self, plan: Option<LatencyProbeConfig>) {
        self.plan.send_if_modified(|current| {
            if *current == plan {
                false
            } else {
                *current = plan;
                true
            }
        });
    }

    /// Finished minutes (and unsupported markers) for the next status report.
    pub fn pending(&self) -> Vec<LatencySample> {
        lock(&self.ledger).pending(unix_now())
    }

    /// The panel accepted a report carrying `sent`.
    pub fn ack(&self, sent: &[LatencySample]) {
        lock(&self.ledger).ack(sent);
    }

    async fn supervise(self: Arc<Self>, mut rx: watch::Receiver<Option<LatencyProbeConfig>>) {
        let mut running: Option<JoinHandle<()>> = None;
        loop {
            let plan = rx.borrow_and_update().clone();
            if let Some(handle) = running.take() {
                // Aborting drops the round in progress with its probe tasks.
                handle.abort();
            }
            let keep: BTreeSet<i64> = plan
                .iter()
                .flat_map(|p| p.targets.iter().map(|t| t.id))
                .collect();
            lock(&self.ledger).reset(unix_now(), &keep);
            if let Some(plan) = plan {
                tracing::info!(
                    "latency monitor: probing {} target(s) every {}s",
                    plan.targets.len(),
                    plan.interval_secs
                );
                let monitor = self.clone();
                running = Some(tokio::spawn(async move { monitor.probe_loop(plan).await }));
            } else {
                tracing::debug!("latency monitor: no targets, not probing");
            }
            if rx.changed().await.is_err() {
                break;
            }
        }
    }

    async fn probe_loop(self: Arc<Self>, plan: LatencyProbeConfig) {
        let resolver = Arc::new(Resolver::default());
        let icmp = Arc::new(IcmpProber::default());
        let secs = u64::from(plan.interval_secs).clamp(1, 3600);
        let mut ticker = interval(Duration::from_secs(secs));
        // A round that overran its interval is not made up for with a burst.
        ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);
        loop {
            ticker.tick().await;
            self.run_round(&plan.targets, &resolver, &icmp).await;
        }
    }

    /// Probe every target once (at most 8 at a time) and record the results
    /// under the minute the round started in.
    async fn run_round(
        &self,
        targets: &[LatencyProbeTarget],
        resolver: &Arc<Resolver>,
        icmp: &Arc<IcmpProber>,
    ) {
        let started_at = unix_now();
        let slots = Arc::new(Semaphore::new(MAX_CONCURRENT_PROBES));
        let mut set = JoinSet::new();
        for target in targets.iter().cloned() {
            let (slots, resolver, icmp) = (slots.clone(), resolver.clone(), icmp.clone());
            set.spawn(async move {
                // The semaphore is never closed, so acquiring cannot fail.
                let _slot = slots.acquire_owned().await.ok();
                let outcome = probe(&target, &resolver, &icmp).await;
                (target.id, outcome)
            });
        }
        while let Some(joined) = set.join_next().await {
            let Ok((id, outcome)) = joined else { continue };
            let mut ledger = lock(&self.ledger);
            match outcome {
                Outcome::Rtt(ms) => {
                    ledger.set_unsupported(id, false);
                    ledger.record(id, started_at, Some(ms));
                }
                Outcome::Lost => {
                    ledger.set_unsupported(id, false);
                    ledger.record(id, started_at, None);
                }
                Outcome::Unsupported => ledger.set_unsupported(id, true),
            }
        }
    }
}

async fn probe(target: &LatencyProbeTarget, resolver: &Resolver, icmp: &IcmpProber) -> Outcome {
    let Some(ip) = resolver.resolve(&target.host).await else {
        return Outcome::Lost;
    };
    match target.kind {
        LatencyKind::Tcp => match target.port {
            Some(port) => probe_tcp(SocketAddr::new(ip, port)).await,
            // The panel never sends a TCP target without a port; if one
            // arrives anyway there is nothing to connect to.
            None => Outcome::Unsupported,
        },
        LatencyKind::Icmp => icmp.ping(target.id, ip).await,
    }
}

/// The time the TCP handshake takes. The connection is closed right away, so
/// the target sees a connect and an immediate close — nothing is sent.
async fn probe_tcp(addr: SocketAddr) -> Outcome {
    let started = Instant::now();
    match timeout(PROBE_TIMEOUT, TcpStream::connect(addr)).await {
        Ok(Ok(stream)) => {
            let ms = started.elapsed().as_secs_f64() * 1000.0;
            drop(stream);
            Outcome::Rtt(ms)
        }
        _ => Outcome::Lost,
    }
}

/// Turns hosts into addresses, remembering a looked-up name for 10 minutes.
#[derive(Default)]
struct Resolver {
    cache: Mutex<HashMap<String, (IpAddr, Instant)>>,
}

impl Resolver {
    async fn resolve(&self, host: &str) -> Option<IpAddr> {
        // An address literal needs no lookup — and an IPv6 literal is used as
        // it is, whatever else the machine could reach.
        if let Ok(ip) = host.parse::<IpAddr>() {
            return Some(ip);
        }
        if let Some((ip, at)) = lock(&self.cache).get(host) {
            if at.elapsed() < RESOLVE_TTL {
                return Some(*ip);
            }
        }
        let found: Option<Vec<IpAddr>> =
            match timeout(PROBE_TIMEOUT, tokio::net::lookup_host((host, 0))).await {
                Ok(Ok(addrs)) => Some(addrs.map(|a| a.ip()).collect()),
                _ => None,
            };
        match found.as_deref().and_then(pick_address) {
            Some(ip) => {
                lock(&self.cache).insert(host.to_string(), (ip, Instant::now()));
                Some(ip)
            }
            None => {
                // A failed lookup is a lost probe, and the next round asks again.
                lock(&self.cache).remove(host);
                None
            }
        }
    }
}

/// The first IPv4 address if there is one, else the first of whatever there is.
fn pick_address(addrs: &[IpAddr]) -> Option<IpAddr> {
    addrs
        .iter()
        .find(|a| a.is_ipv4())
        .or_else(|| addrs.first())
        .copied()
}

/// ICMP echo through `surge-ping`, one shared socket per address family.
#[derive(Default)]
struct IcmpProber {
    v4: Mutex<Option<surge_ping::Client>>,
    v6: Mutex<Option<surge_ping::Client>>,
    seq: AtomicU16,
}

impl IcmpProber {
    /// The family's client, opening its socket on first use. `None` when the
    /// socket cannot be opened (no privilege) — tried again next round, so a
    /// node that gains the capability starts working without a restart.
    fn client(&self, v6: bool) -> Option<surge_ping::Client> {
        let mut slot = lock(if v6 { &self.v6 } else { &self.v4 });
        if slot.is_none() {
            let kind = if v6 {
                surge_ping::ICMP::V6
            } else {
                surge_ping::ICMP::V4
            };
            // The default socket type is the unprivileged ping socket, with a
            // raw socket as the fallback.
            let config = surge_ping::Config::builder().kind(kind).build();
            match surge_ping::Client::new(&config) {
                Ok(client) => *slot = Some(client),
                Err(e) => tracing::debug!("latency monitor: no ICMP socket: {}", e),
            }
        }
        slot.clone()
    }

    async fn ping(&self, target_id: i64, ip: IpAddr) -> Outcome {
        let Some(client) = self.client(ip.is_ipv6()) else {
            return Outcome::Unsupported;
        };
        let ident = surge_ping::PingIdentifier((target_id & 0xffff) as u16);
        let mut pinger = client.pinger(ip, ident).await;
        pinger.timeout(PROBE_TIMEOUT);
        let seq = surge_ping::PingSequence(self.seq.fetch_add(1, Ordering::Relaxed));
        match pinger.ping(seq, &[0u8; 16]).await {
            Ok((_, rtt)) => Outcome::Rtt(rtt.as_secs_f64() * 1000.0),
            Err(_) => Outcome::Lost,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{Ipv4Addr, Ipv6Addr};

    /// A minute boundary.
    const T0: i64 = 1_700_000_100;

    fn acc_sample(l: &mut Ledger, target: i64, at: i64, rtt: Option<f64>) {
        l.record(target, at, rtt);
    }

    #[test]
    fn a_minute_adds_up_sent_lost_sum_min_and_max() {
        let mut l = Ledger::default();
        for (at, rtt) in [
            (T0, Some(10.0)),
            (T0 + 10, Some(30.0)),
            (T0 + 20, None),
            (T0 + 59, Some(20.0)),
        ] {
            acc_sample(&mut l, 1, at, rtt);
        }
        // not finished yet
        assert!(l.pending(T0 + 59).is_empty());
        let out = l.pending(T0 + 60 + MINUTE_GRACE_SECS);
        assert_eq!(out.len(), 1);
        let s = &out[0];
        assert_eq!((s.target_id, s.minute_ts, s.sent, s.lost), (1, T0, 4, 1));
        assert_eq!(s.rtt_sum_ms, 60.0);
        assert_eq!((s.rtt_min_ms, s.rtt_max_ms), (10.0, 30.0));
        assert!(!s.unsupported);
    }

    #[test]
    fn a_minute_with_no_answer_reports_zero_round_trips_not_infinity() {
        let mut l = Ledger::default();
        acc_sample(&mut l, 1, T0, None);
        acc_sample(&mut l, 1, T0 + 5, None);
        let s = l.pending(T0 + 100).remove(0);
        assert_eq!((s.sent, s.lost), (2, 2));
        assert_eq!((s.rtt_sum_ms, s.rtt_min_ms, s.rtt_max_ms), (0.0, 0.0, 0.0));
        // and it survives JSON, which has no infinity
        serde_json::to_string(&s).unwrap();
    }

    #[test]
    fn targets_and_minutes_are_kept_apart_and_sent_oldest_first() {
        let mut l = Ledger::default();
        acc_sample(&mut l, 2, T0 + 60, Some(5.0));
        acc_sample(&mut l, 1, T0 + 60, Some(6.0));
        acc_sample(&mut l, 2, T0, Some(7.0));
        let out = l.pending(T0 + 200);
        assert_eq!(
            out.iter()
                .map(|s| (s.minute_ts, s.target_id))
                .collect::<Vec<_>>(),
            vec![(T0, 2), (T0 + 60, 1), (T0 + 60, 2)]
        );
    }

    /// The point of the buffer: a report that fails leaves the minutes in
    /// place, the next report carries them again, and only an accepted one
    /// clears them.
    #[test]
    fn minutes_stay_until_a_report_carrying_them_is_acknowledged() {
        let mut l = Ledger::default();
        acc_sample(&mut l, 1, T0, Some(10.0));
        let first = l.pending(T0 + 100);
        assert_eq!(first.len(), 1);
        // the report failed: nothing was acknowledged
        let again = l.pending(T0 + 110);
        assert_eq!(again, first);
        // a minute finishing meanwhile joins it
        acc_sample(&mut l, 1, T0 + 60, Some(11.0));
        let both = l.pending(T0 + 200);
        assert_eq!(both.len(), 2);
        // the report with both went through
        l.ack(&both);
        assert!(l.pending(T0 + 210).is_empty());
    }

    #[test]
    fn a_minute_reported_after_a_failure_is_not_lost_when_the_next_one_succeeds() {
        let mut l = Ledger::default();
        acc_sample(&mut l, 1, T0, Some(10.0));
        let carried = l.pending(T0 + 100); // sent, then the request failed
        acc_sample(&mut l, 1, T0 + 60, Some(12.0));
        let next = l.pending(T0 + 170);
        assert_eq!(next.len(), 2, "the failed minute is sent again");
        assert_eq!(next[0], carried[0]);
        l.ack(&next);
        assert!(l.minutes.is_empty());
    }

    #[test]
    fn at_most_five_minutes_are_kept() {
        let mut l = Ledger::default();
        for k in 0..10 {
            acc_sample(&mut l, 1, T0 + k * 60, Some(10.0));
        }
        let out = l.pending(T0 + 10 * 60 + 20);
        let minutes: Vec<i64> = out.iter().map(|s| s.minute_ts).collect();
        assert_eq!(minutes.len(), 5, "only the last five minutes are kept");
        assert!(minutes
            .iter()
            .all(|m| *m >= minute_of(T0 + 10 * 60 + 20) - KEEP_SECS));
        assert_eq!(
            l.minutes.len(),
            5,
            "older ones are dropped, not just hidden"
        );
    }

    /// A probe that overran its minute's grace must not resurrect the minute
    /// the panel already holds in full.
    #[test]
    fn a_late_probe_for_an_acknowledged_minute_is_dropped() {
        let mut l = Ledger::default();
        acc_sample(&mut l, 1, T0, Some(10.0));
        let out = l.pending(T0 + 100);
        l.ack(&out);
        acc_sample(&mut l, 1, T0 + 55, Some(999.0)); // finished after the ack
        assert!(l.pending(T0 + 110).is_empty());
        // a later minute is unaffected
        acc_sample(&mut l, 1, T0 + 60, Some(10.0));
        assert_eq!(l.pending(T0 + 200).len(), 1);
    }

    #[test]
    fn a_plan_change_drops_the_minute_in_progress_and_gone_targets() {
        let mut l = Ledger::default();
        acc_sample(&mut l, 1, T0, Some(10.0)); // finished
        acc_sample(&mut l, 2, T0, Some(10.0)); // finished, target about to go
        acc_sample(&mut l, 1, T0 + 60, Some(10.0)); // in progress
        l.set_unsupported(2, true);
        let keep: BTreeSet<i64> = [1].into_iter().collect();
        l.reset(T0 + 90, &keep);
        let out = l.pending(T0 + 100);
        assert_eq!(
            out.iter()
                .map(|s| (s.target_id, s.minute_ts, s.unsupported))
                .collect::<Vec<_>>(),
            vec![(1, T0, false)]
        );
    }

    #[test]
    fn unsupported_targets_are_marked_on_every_report_until_they_work() {
        let mut l = Ledger::default();
        l.set_unsupported(3, true);
        for now in [T0, T0 + 10] {
            let out = l.pending(now);
            assert_eq!(out.len(), 1);
            assert!(out[0].unsupported);
            assert_eq!((out[0].target_id, out[0].sent), (3, 0));
            // acknowledging a report does not clear the marker
            l.ack(&out);
        }
        l.set_unsupported(3, false);
        assert!(l.pending(T0 + 20).is_empty());
    }

    #[test]
    fn addresses_prefer_ipv4_unless_there_is_only_ipv6() {
        let v4 = IpAddr::V4(Ipv4Addr::new(192, 0, 2, 1));
        let v6 = IpAddr::V6(Ipv6Addr::new(0x2001, 0xdb8, 0, 0, 0, 0, 0, 1));
        assert_eq!(pick_address(&[v6, v4]), Some(v4));
        assert_eq!(pick_address(&[v6]), Some(v6));
        assert_eq!(pick_address(&[]), None);
    }

    #[tokio::test]
    async fn address_literals_resolve_without_a_lookup() {
        let r = Resolver::default();
        assert_eq!(
            r.resolve("192.0.2.7").await,
            Some(IpAddr::V4(Ipv4Addr::new(192, 0, 2, 7)))
        );
        assert_eq!(
            r.resolve("2001:db8::1").await,
            Some(IpAddr::V6(Ipv6Addr::new(0x2001, 0xdb8, 0, 0, 0, 0, 0, 1)))
        );
    }

    #[tokio::test]
    async fn a_name_that_does_not_resolve_is_none() {
        // `.invalid` is reserved (RFC 6761): it never resolves.
        let r = Resolver::default();
        assert_eq!(r.resolve("no-such-host.invalid").await, None);
    }

    #[tokio::test]
    async fn tcp_probe_times_the_handshake_and_a_closed_port_is_lost() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let open = listener.local_addr().unwrap();
        match probe_tcp(open).await {
            Outcome::Rtt(ms) => assert!((0.0..3000.0).contains(&ms), "{ms}"),
            other => panic!("expected a round trip, got {other:?}"),
        }
        // bind and drop to find a port nothing listens on
        let closed = {
            let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            l.local_addr().unwrap()
        };
        assert_eq!(probe_tcp(closed).await, Outcome::Lost);
    }

    /// A whole round against local listeners: the results land in the minute
    /// the round started in, and a target that cannot connect counts as lost.
    #[tokio::test]
    async fn a_round_records_each_target_under_its_own_id() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let open = listener.local_addr().unwrap();
        let closed = {
            let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            l.local_addr().unwrap()
        };
        let targets = vec![
            LatencyProbeTarget {
                id: 1,
                kind: LatencyKind::Tcp,
                host: open.ip().to_string(),
                port: Some(open.port()),
            },
            LatencyProbeTarget {
                id: 2,
                kind: LatencyKind::Tcp,
                host: closed.ip().to_string(),
                port: Some(closed.port()),
            },
        ];
        let monitor = LatencyMonitor::new();
        let before = unix_now();
        monitor
            .run_round(
                &targets,
                &Arc::new(Resolver::default()),
                &Arc::new(IcmpProber::default()),
            )
            .await;
        let after = unix_now();
        let mut ledger = lock(&monitor.ledger);
        let out = ledger.pending(after + 200);
        assert_eq!(out.len(), 2);
        assert!(out
            .iter()
            .all(|s| s.minute_ts == minute_of(before) || s.minute_ts == minute_of(after)));
        let by_id = |id| out.iter().find(|s| s.target_id == id).unwrap();
        assert_eq!((by_id(1).sent, by_id(1).lost), (1, 0));
        assert_eq!((by_id(2).sent, by_id(2).lost), (1, 1));
    }

    /// With more targets than slots every one is still probed, and never more
    /// than 8 at once (the semaphore is what bounds it).
    #[tokio::test]
    async fn a_round_probes_every_target_even_with_more_than_the_concurrency_limit() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let open = listener.local_addr().unwrap();
        let targets: Vec<LatencyProbeTarget> = (1..=16)
            .map(|id| LatencyProbeTarget {
                id,
                kind: LatencyKind::Tcp,
                host: open.ip().to_string(),
                port: Some(open.port()),
            })
            .collect();
        let monitor = LatencyMonitor::new();
        monitor
            .run_round(
                &targets,
                &Arc::new(Resolver::default()),
                &Arc::new(IcmpProber::default()),
            )
            .await;
        let out = lock(&monitor.ledger).pending(unix_now() + 200);
        assert_eq!(out.len(), 16);
    }

    /// ICMP either works or says plainly that it cannot; on loopback it is
    /// never "lost". (Which of the two depends on the machine's privileges.)
    #[tokio::test]
    async fn icmp_is_a_round_trip_or_unsupported_never_a_loss_on_loopback() {
        let prober = IcmpProber::default();
        let out = prober.ping(1, IpAddr::V4(Ipv4Addr::LOCALHOST)).await;
        assert!(
            matches!(out, Outcome::Rtt(_) | Outcome::Unsupported),
            "{out:?}"
        );
    }

    /// An identical plan must not restart anything, a changed one must.
    #[test]
    fn set_plan_only_signals_a_real_change() {
        let m = LatencyMonitor::new();
        let rx = m.plan.subscribe();
        let plan = LatencyProbeConfig {
            interval_secs: 10,
            targets: vec![LatencyProbeTarget {
                id: 1,
                kind: LatencyKind::Icmp,
                host: "192.0.2.1".into(),
                port: None,
            }],
        };
        m.set_plan(None);
        assert!(!rx.has_changed().unwrap(), "None over None is nothing");
        m.set_plan(Some(plan.clone()));
        assert!(rx.has_changed().unwrap());
        let mut rx = rx;
        rx.borrow_and_update();
        m.set_plan(Some(plan.clone()));
        assert!(!rx.has_changed().unwrap(), "the same plan again is nothing");
        m.set_plan(Some(LatencyProbeConfig {
            interval_secs: 20,
            ..plan
        }));
        assert!(rx.has_changed().unwrap());
        m.set_plan(None);
        rx.borrow_and_update();
        assert!(rx.borrow().is_none());
    }

    /// The whole path: a plan starts the probing (the first round is
    /// immediate).
    #[tokio::test]
    async fn a_plan_starts_probing() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let open = listener.local_addr().unwrap();
        let m = LatencyMonitor::new();
        let handle = m.spawn();
        m.set_plan(Some(LatencyProbeConfig {
            interval_secs: 5,
            targets: vec![LatencyProbeTarget {
                id: 7,
                kind: LatencyKind::Tcp,
                host: open.ip().to_string(),
                port: Some(open.port()),
            }],
        }));
        let mut probed = false;
        for _ in 0..50 {
            tokio::time::sleep(Duration::from_millis(100)).await;
            if !lock(&m.ledger).minutes.is_empty() {
                probed = true;
                break;
            }
        }
        assert!(probed, "the first round runs at once");

        handle.abort();
    }

    /// No plan: nothing is probed and nothing is reported.
    #[tokio::test]
    async fn without_a_plan_nothing_is_reported() {
        let m = LatencyMonitor::new();
        let handle = m.spawn();
        tokio::task::yield_now().await;
        assert!(m.pending().is_empty());
        handle.abort();
    }
}
