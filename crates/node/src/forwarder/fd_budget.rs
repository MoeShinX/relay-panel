//! Shared node-wide file descriptor budget for TCP and UDP forwarding.
//!
//! Previously UDP tracked its own node-wide cap (~nofile/2) while TCP only had
//! per-rule optional `max_connections`. Under combined load, TCP + UDP could
//! exceed `RLIMIT_NOFILE` and starve panel reporting, DNS lookups, and accept
//! loops.
//!
//! This module coordinates both protocols against one shared atomic budget:
//! - `reserve = (nofile / 8).clamp(256, 4096).min(nofile / 2)` stays free for
//!   listeners, panel HTTP/WS control channels, DNS, and logging.
//! - `budget = nofile - reserve`.
//! - TCP has NO fixed protocol sub-share: it may use up to the entire shared
//!   `budget` (minus any fds currently held by UDP), reserving each
//!   connection's actual path cost (`6` fds on Linux `splice` unlimited rules =
//!   2 sockets + 2 pipes; `2` fds on rate-limited or non-Linux rules). Optional
//!   `TCP_MAX_CONNECTIONS` can further tighten live TCP connections.
//! - UDP keeps a cap of at most `budget / 2` (1 fd per UDP session) to prevent
//!   forged-source floods from filling the node-wide budget.
//! - `tcp_fds + udp_fds <= budget` always holds atomically.
//! - `TCP_MAX_CONNECTIONS` and `UDP_MAX_SESSIONS` may only tighten below the
//!   safe budget caps; values above the safe cap are clamped with a warning.

use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::OnceLock;

use super::limiter::RateLimit;

/// Conservative fallback when `RLIMIT_NOFILE` cannot be read (e.g. non-Unix).
pub const FALLBACK_NOFILE: u64 = 4096;

/// Minimum fd cost of a single forwarded TCP connection (inbound + outbound socket).
const TCP_MIN_FDS: usize = 2;

/// Linux `splice` unlimited TCP connection fd cost: 2 sockets + 2 pipes (4 pipe fds).
#[cfg(target_os = "linux")]
const TCP_SPLICE_FDS: usize = 6;

/// How often node-wide TCP fd exhaustion warnings may be logged.
const TCP_FD_WARN_INTERVAL_SECS: u64 = 10;

/// Compute the fd cost of one TCP connection based on its actual forwarding path.
///
/// Linux unlimited rules use `splice(2)` with two private pipes (2 sockets +
/// 4 pipe fds = 6 fds). Rate-limited rules and non-Linux platforms use userspace
/// copies (2 sockets = 2 fds).
pub fn tcp_fd_cost(rate_limit: &RateLimit) -> usize {
    #[cfg(target_os = "linux")]
    {
        if matches!(rate_limit, RateLimit::Unlimited) {
            return TCP_SPLICE_FDS;
        }
    }
    let _ = rate_limit;
    TCP_MIN_FDS
}

#[cfg(unix)]
#[allow(clippy::unnecessary_cast)]
pub fn nofile_limit() -> Option<u64> {
    let mut lim = libc::rlimit {
        rlim_cur: 0,
        rlim_max: 0,
    };
    // SAFETY: getrlimit writes into a local rlimit struct we own.
    (unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, &mut lim) } == 0).then_some(lim.rlim_cur as u64)
}

#[cfg(not(unix))]
pub fn nofile_limit() -> Option<u64> {
    None
}

/// Resolved budget parameters computed from `nofile` and env overrides.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BudgetConfig {
    pub nofile: u64,
    pub nofile_detected: bool,
    pub reserve: usize,
    pub budget: usize,
    pub tcp_conn_cap: usize,
    pub udp_fd_cap: usize,
    pub warnings: Vec<String>,
}

impl BudgetConfig {
    /// Compute budget limits from `nofile` and optional `TCP_MAX_CONNECTIONS` /
    /// `UDP_MAX_SESSIONS` settings. Env overrides may only tighten limits and
    /// never inflate caps past the fd budget — even on tiny `nofile` values.
    pub fn compute(
        nofile_opt: Option<u64>,
        tcp_max_env: Option<&str>,
        udp_max_env: Option<&str>,
    ) -> Self {
        let mut warnings = Vec::new();
        let (nofile, nofile_detected) = match nofile_opt {
            Some(n) if n > 0 => (n, true),
            _ => {
                warnings.push(format!(
                    "could not read RLIMIT_NOFILE; using conservative fallback {}",
                    FALLBACK_NOFILE
                ));
                (FALLBACK_NOFILE, false)
            }
        };

        let nofile_capped = nofile.min(1_048_576) as usize;
        // Reserve clamp(nofile / 8, 256, 4096), bounded by nofile / 2 so tiny
        // nofile limits (< 512) still retain a non-zero forwarding budget.
        let reserve = (nofile_capped / 8).clamp(256, 4096).min(nofile_capped / 2);
        let budget = nofile_capped.saturating_sub(reserve);

        // TCP uses the full shared budget (no fixed 3/4 sub-share); UDP stays
        // capped at half the budget to limit forged-source session floods.
        let default_udp_cap = budget / 2;
        let safe_tcp_conn_cap = budget / TCP_MIN_FDS;

        let tcp_conn_cap = apply_tighten_only_env(
            "TCP_MAX_CONNECTIONS",
            tcp_max_env,
            safe_tcp_conn_cap,
            &mut warnings,
        );
        let udp_fd_cap = apply_tighten_only_env(
            "UDP_MAX_SESSIONS",
            udp_max_env,
            default_udp_cap,
            &mut warnings,
        );

        Self {
            nofile: nofile_capped as u64,
            nofile_detected,
            reserve,
            budget,
            tcp_conn_cap,
            udp_fd_cap,
            warnings,
        }
    }
}

fn apply_tighten_only_env(
    name: &str,
    raw: Option<&str>,
    safe_cap: usize,
    warnings: &mut Vec<String>,
) -> usize {
    let Some(s) = raw.map(str::trim).filter(|s| !s.is_empty()) else {
        return safe_cap;
    };
    match s.parse::<usize>() {
        Ok(0) => {
            warnings.push(format!(
                "{}=0 is invalid; using default safe cap {}",
                name, safe_cap
            ));
            safe_cap
        }
        Ok(v) if v > safe_cap => {
            warnings.push(format!(
                "{}={} exceeds safe fd-budget cap {}; clamping to {}",
                name, v, safe_cap, safe_cap
            ));
            safe_cap
        }
        Ok(v) => v,
        Err(_) => {
            warnings.push(format!(
                "invalid {}={:?}; using default safe cap {}",
                name, s, safe_cap
            ));
            safe_cap
        }
    }
}

/// Shared atomic fd budget across TCP and UDP listeners.
pub struct FdBudget {
    config: BudgetConfig,
    used_fds: AtomicUsize,
    tcp_fds: AtomicUsize,
    tcp_conns: AtomicUsize,
    udp_fds: AtomicUsize,
}

impl FdBudget {
    pub fn new(config: BudgetConfig) -> Self {
        Self {
            config,
            used_fds: AtomicUsize::new(0),
            tcp_fds: AtomicUsize::new(0),
            tcp_conns: AtomicUsize::new(0),
            udp_fds: AtomicUsize::new(0),
        }
    }

    pub fn config(&self) -> &BudgetConfig {
        &self.config
    }

    #[cfg(test)]
    pub fn snapshot_usage(&self) -> (usize, usize, usize, usize) {
        (
            self.used_fds.load(Ordering::Acquire),
            self.tcp_fds.load(Ordering::Acquire),
            self.tcp_conns.load(Ordering::Acquire),
            self.udp_fds.load(Ordering::Acquire),
        )
    }

    /// Reserve `cost` fds and 1 TCP connection slot. Returns `None` if adding
    /// this connection would exceed `tcp_conn_cap` or the shared node-wide fd
    /// `budget`.
    pub fn acquire_tcp(&'static self, cost: usize) -> Option<TcpFdGuard> {
        let cost = cost.max(1);
        // 1. Optional tightened TCP connection cap (defaults to budget / 2).
        self.tcp_conns
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |n| {
                (n < self.config.tcp_conn_cap).then_some(n + 1)
            })
            .ok()?;

        // 2. Shared node-wide fd budget (TCP can use up to 100% of `budget`).
        if self
            .used_fds
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |n| {
                n.checked_add(cost)
                    .filter(|&next| next <= self.config.budget)
            })
            .is_err()
        {
            self.tcp_conns.fetch_sub(1, Ordering::AcqRel);
            return None;
        }

        self.tcp_fds.fetch_add(cost, Ordering::AcqRel);
        Some(TcpFdGuard { budget: self, cost })
    }

    /// Reserve 1 fd for a new UDP outbound session. Returns `None` if adding
    /// this session would exceed the UDP share cap (`budget / 2`) or the shared
    /// fd budget.
    pub fn acquire_udp(&'static self) -> Option<UdpFdGuard> {
        // 1. UDP share cap (at most budget / 2).
        self.udp_fds
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |n| {
                (n < self.config.udp_fd_cap).then_some(n + 1)
            })
            .ok()?;

        // 2. Shared node-wide fd budget.
        if self
            .used_fds
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |n| {
                (n < self.config.budget).then_some(n + 1)
            })
            .is_err()
        {
            self.udp_fds.fetch_sub(1, Ordering::AcqRel);
            return None;
        }

        Some(UdpFdGuard { budget: self })
    }
}

/// RAII guard holding one TCP connection's fd reservation; always releases on drop.
pub struct TcpFdGuard {
    budget: &'static FdBudget,
    cost: usize,
}

impl Drop for TcpFdGuard {
    fn drop(&mut self) {
        self.budget.used_fds.fetch_sub(self.cost, Ordering::AcqRel);
        self.budget.tcp_fds.fetch_sub(self.cost, Ordering::AcqRel);
        self.budget.tcp_conns.fetch_sub(1, Ordering::AcqRel);
    }
}

/// RAII guard holding one UDP session's fd reservation; always releases on drop.
pub struct UdpFdGuard {
    budget: &'static FdBudget,
}

impl Drop for UdpFdGuard {
    fn drop(&mut self) {
        self.budget.used_fds.fetch_sub(1, Ordering::AcqRel);
        self.budget.udp_fds.fetch_sub(1, Ordering::AcqRel);
    }
}

/// Process-wide shared fd budget initialized once on first use (after
/// `raise_nofile_limit()` in `main()`).
pub fn global() -> &'static FdBudget {
    static BUDGET: OnceLock<FdBudget> = OnceLock::new();
    BUDGET.get_or_init(|| {
        let tcp_env = std::env::var("TCP_MAX_CONNECTIONS").ok();
        let udp_env = std::env::var("UDP_MAX_SESSIONS").ok();
        let cfg = BudgetConfig::compute(nofile_limit(), tcp_env.as_deref(), udp_env.as_deref());
        for w in &cfg.warnings {
            tracing::warn!("fd_budget: {}", w);
        }
        tracing::info!(
            "fd_budget: nofile={} (detected={}), reserve={}, budget={}, tcp_max_connections={}, udp_max_sessions={}",
            cfg.nofile,
            cfg.nofile_detected,
            cfg.reserve,
            cfg.budget,
            cfg.tcp_conn_cap,
            cfg.udp_fd_cap
        );
        FdBudget::new(cfg)
    })
}

/// Initialize the shared budget at startup so its summary INFO line is logged
/// immediately after `tracing_subscriber::fmt::init()`.
pub fn init_and_log() {
    let _ = global();
}

static TCP_FD_REFUSED: AtomicU64 = AtomicU64::new(0);
static LAST_TCP_FD_WARN: AtomicU64 = AtomicU64::new(0);

/// Log a rate-limited warning when a TCP connection is rejected due to the
/// shared node-wide fd budget or `TCP_MAX_CONNECTIONS`.
pub fn note_tcp_refused(budget: &FdBudget, rule_id: i64, client_addr: SocketAddr) {
    TCP_FD_REFUSED.fetch_add(1, Ordering::Relaxed);
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let prev = LAST_TCP_FD_WARN.load(Ordering::Relaxed);
    if now.saturating_sub(prev) >= TCP_FD_WARN_INTERVAL_SECS
        && LAST_TCP_FD_WARN
            .compare_exchange(prev, now, Ordering::Relaxed, Ordering::Relaxed)
            .is_ok()
    {
        let cfg = budget.config();
        tracing::warn!(
            "TCP node fd budget reached (budget={} fds, tcp_max_connections={}); \
             refused {} connection(s) since last warning (latest rule {} from {}). \
             Raise LimitNOFILE if this is expected traffic.",
            cfg.budget,
            cfg.tcp_conn_cap,
            TCP_FD_REFUSED.swap(0, Ordering::Relaxed),
            rule_id,
            client_addr
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn budget_calculation_handles_small_standard_and_missing_nofile() {
        // Tiny nofile = 512: reserve = 256, budget = 256, udp = 128, tcp_conn = 128
        let b512 = BudgetConfig::compute(Some(512), None, None);
        assert_eq!(b512.reserve, 256);
        assert_eq!(b512.budget, 256);
        assert_eq!(b512.udp_fd_cap, 128);
        assert_eq!(b512.tcp_conn_cap, 128);
        assert!(b512.udp_fd_cap <= b512.budget);

        // Very small nofile = 256: reserve capped at nofile/2 = 128 so budget > 0
        let b256 = BudgetConfig::compute(Some(256), None, None);
        assert_eq!(b256.reserve, 128);
        assert_eq!(b256.budget, 128);
        assert_eq!(b256.udp_fd_cap, 64);
        assert_eq!(b256.tcp_conn_cap, 64);

        // Common container nofile = 1024: reserve = 256, budget = 768
        let b1024 = BudgetConfig::compute(Some(1024), None, None);
        assert_eq!(b1024.reserve, 256);
        assert_eq!(b1024.budget, 768);
        assert_eq!(b1024.udp_fd_cap, 384);
        assert_eq!(b1024.tcp_conn_cap, 384);

        // systemd default nofile = 65536: reserve = 4096, budget = 61440
        // TCP has no fixed 3/4 share (can use all 61440 fds = 10240 splice conns);
        // UDP stays capped at budget/2 = 30720.
        let b64k = BudgetConfig::compute(Some(65_536), None, None);
        assert_eq!(b64k.reserve, 4096);
        assert_eq!(b64k.budget, 61_440);
        assert_eq!(b64k.udp_fd_cap, 30_720);
        assert_eq!(b64k.tcp_conn_cap, 30_720);

        // Missing nofile uses conservative fallback and records a warning.
        let bnone = BudgetConfig::compute(None, None, None);
        assert!(!bnone.nofile_detected);
        assert_eq!(bnone.nofile, FALLBACK_NOFILE);
        assert!(!bnone.warnings.is_empty());
    }

    #[test]
    fn env_overrides_can_only_tighten_never_exceed_budget() {
        // Under nofile = 512: budget = 256, udp safe cap = 128, tcp_conn safe cap = 128.
        let tightened = BudgetConfig::compute(Some(512), Some("40"), Some("50"));
        assert_eq!(tightened.tcp_conn_cap, 40);
        assert_eq!(tightened.udp_fd_cap, 50);
        assert!(tightened.warnings.is_empty());

        // Oversized env vars are clamped to safe caps with warnings.
        let clamped = BudgetConfig::compute(Some(512), Some("5000"), Some("5000"));
        assert_eq!(clamped.tcp_conn_cap, 128);
        assert_eq!(clamped.udp_fd_cap, 128);
        assert_eq!(clamped.warnings.len(), 2);

        // Invalid / zero values fall back to safe caps with warnings.
        let bad = BudgetConfig::compute(Some(1024), Some("0"), Some("abc"));
        assert_eq!(bad.tcp_conn_cap, 384);
        assert_eq!(bad.udp_fd_cap, 384);
        assert_eq!(bad.warnings.len(), 2);
    }

    #[tokio::test]
    async fn shared_budget_lets_tcp_use_full_budget_caps_udp_at_half_and_preserves_reserve() {
        // Small injected budget: total budget = 18 fds (reserve = 16),
        // TCP uses full budget (no 3/4 cap), UDP capped at budget/2 = 9.
        let budget: &'static FdBudget = Box::leak(Box::new(FdBudget::new(BudgetConfig {
            nofile: 34,
            nofile_detected: true,
            reserve: 16,
            budget: 18,
            tcp_conn_cap: 9,
            udp_fd_cap: 9,
            warnings: Vec::new(),
        })));

        // When UDP is idle, TCP can consume 100% of the shared budget (3 × 6 = 18 fds).
        let t1 = budget.acquire_tcp(6).expect("tcp 1");
        let t2 = budget.acquire_tcp(6).expect("tcp 2");
        let t3 = budget.acquire_tcp(6).expect("tcp 3 fills full budget");
        assert!(budget.acquire_tcp(2).is_none(), "shared budget is full");
        assert!(budget.acquire_udp().is_none(), "shared budget is full");
        assert_eq!(budget.snapshot_usage(), (18, 18, 3, 0));

        // Release one TCP connection (6 fds free): UDP can take up to 6 sessions
        // before hitting the shared budget (even though udp_fd_cap is 9).
        drop(t3);
        let mut udp_guards = Vec::new();
        for _ in 0..6 {
            udp_guards.push(budget.acquire_udp().expect("udp within remaining budget"));
        }
        assert!(
            budget.acquire_udp().is_none(),
            "combined TCP+UDP must not exceed shared budget"
        );
        assert_eq!(budget.snapshot_usage(), (18, 12, 2, 6));

        // While forwarding budget is completely full, control-plane sockets
        // (up to `reserve`) can still be opened cleanly.
        let mut control_sockets = Vec::new();
        for _ in 0..budget.config().reserve {
            let s = tokio::net::UdpSocket::bind("127.0.0.1:0")
                .await
                .expect("control socket must still open within reserve");
            control_sockets.push(s);
        }
        drop(control_sockets);

        // Release TCP and verify UDP is still capped at its 1/2 share (9 sessions).
        drop(t1);
        drop(t2);
        for _ in 0..3 {
            udp_guards.push(budget.acquire_udp().expect("udp up to half budget"));
        }
        assert!(
            budget.acquire_udp().is_none(),
            "UDP must not exceed its budget/2 share even when TCP is idle"
        );
        assert_eq!(budget.snapshot_usage(), (9, 0, 0, 9));

        drop(udp_guards);
        assert_eq!(budget.snapshot_usage(), (0, 0, 0, 0));
    }
}
