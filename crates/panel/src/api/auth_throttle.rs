// v1.2.12: throttling for the unauthenticated endpoints that run bcrypt
// (login, and register when it is open).
//
// Before this, login was limited only per USERNAME (5/min), while an unknown
// username still costs a full bcrypt-12 check (so the response time does not
// reveal which usernames exist). Rotating random usernames therefore bypassed
// the limit and bought ~250 ms of CPU per request — and bcrypt ran directly on
// the async runtime's worker threads, so a handful of concurrent requests
// stalled the whole panel, node reports included. Three layers now:
//
//   1. input size: a login that cannot match (username > 64 bytes, password >
//      1 KiB) is refused before anything else, so it never costs bcrypt or a
//      rate-limit entry;
//   2. attempts per client IP and per username, each in a bounded table;
//   3. BCRYPT_PERMITS concurrent bcrypt runs in total, on the blocking pool.
//      Past that, callers wait up to BCRYPT_WAIT and then get "busy" — an
//      attacker can make logging in slow, but not take the panel down.

use axum::http::HeaderMap;
use once_cell::sync::Lazy;
use std::collections::HashMap;
use std::hash::Hash;
use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::Semaphore;

/// Longest username that can exist (see `validate_username`).
pub const MAX_LOGIN_USERNAME: usize = 64;
/// Passwords are at most 72 bytes when set (bcrypt's limit); this is only a
/// generous bound on what a login request may carry.
pub const MAX_LOGIN_PASSWORD: usize = 1024;

/// Failed-or-not attempts allowed per key within one window.
pub struct AttemptLimiter<K> {
    max: u32,
    window: Duration,
    cap: usize,
    map: Mutex<HashMap<K, (u32, Instant)>>,
}

impl<K: Eq + Hash + Clone> AttemptLimiter<K> {
    pub fn new(max: u32, window: Duration, cap: usize) -> Self {
        Self {
            max,
            window,
            cap,
            map: Mutex::new(HashMap::new()),
        }
    }

    /// Count an attempt for `key`; true when it is over the limit.
    ///
    /// The table holds at most `cap` keys. When it is full even after dropping
    /// expired windows, a NEW key is not tracked (let through) rather than
    /// evicting existing counters — a flood of fresh keys must not reset the
    /// count of a key that is being brute-forced. Keys already tracked keep
    /// counting, and the bcrypt permits still bound the CPU cost.
    pub fn hit(&self, key: &K) -> bool {
        let now = Instant::now();
        let mut map = self.map.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((count, start)) = map.get_mut(key) {
            if now.duration_since(*start) < self.window {
                *count += 1;
                return *count > self.max;
            }
            *count = 1;
            *start = now;
            return false;
        }
        if map.len() >= self.cap {
            let window = self.window;
            map.retain(|_, (_, start)| now.duration_since(*start) < window);
            if map.len() >= self.cap {
                return false;
            }
        }
        map.insert(key.clone(), (1, now));
        false
    }

    /// Forget `key` (a successful login clears its username counter).
    pub fn clear(&self, key: &K) {
        self.map
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(key);
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.map.lock().unwrap().len()
    }
}

/// 5 attempts per username per minute (unchanged from before v1.2.12).
pub static USERNAME_LIMITER: Lazy<AttemptLimiter<String>> =
    Lazy::new(|| AttemptLimiter::new(5, Duration::from_secs(60), 10_000));

/// 20 login/register attempts per client IP per minute.
pub static IP_LIMITER: Lazy<AttemptLimiter<IpAddr>> =
    Lazy::new(|| AttemptLimiter::new(20, Duration::from_secs(60), 100_000));

/// The client's address for per-IP limiting, or `None` when it cannot be told.
///
/// Normally the TCP peer. When the peer is loopback or a private address, the
/// panel sits behind Caddy (compose network) or a reverse proxy on the host —
/// then the LAST X-Forwarded-For entry, the one the proxy itself appended, is
/// the client. A public peer's X-Forwarded-For is ignored: that client could
/// write anything there.
///
/// A private peer WITHOUT a usable X-Forwarded-For (a proxy configured without
/// it) gives `None`, not the proxy's own address: every user would share that
/// one counter, and anyone could lock them all out with 20 bad logins a
/// minute. Such requests are covered by the per-username limit and the bcrypt
/// permits only. (Caveat: if Docker's userland proxy publishes the port, a
/// direct client also appears as a private address and could spoof the
/// header; the permits still cap the damage.)
pub fn client_ip(peer: Option<SocketAddr>, headers: &HeaderMap) -> Option<IpAddr> {
    let peer = peer?.ip();
    if !is_local(peer) {
        return Some(peer);
    }
    headers
        .get("x-forwarded-for")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.rsplit(',').next())
        .and_then(|last| last.trim().parse::<IpAddr>().ok())
}

fn is_local(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => v4.is_loopback() || v4.is_private() || v4.is_link_local(),
        IpAddr::V6(v6) => match v6.to_ipv4_mapped() {
            Some(v4) => is_local(IpAddr::V4(v4)),
            // ::1, or fc00::/7 unique-local.
            None => v6.is_loopback() || (v6.segments()[0] & 0xfe00) == 0xfc00,
        },
    }
}

/// Concurrent bcrypt runs allowed across login + register: half the cores,
/// so the rest of the panel always has CPU left.
static BCRYPT_PERMITS: Lazy<Arc<Semaphore>> = Lazy::new(|| {
    let cores = std::thread::available_parallelism().map_or(2, |n| n.get());
    Arc::new(Semaphore::new((cores / 2).max(1)))
});
const BCRYPT_WAIT: Duration = Duration::from_secs(3);

/// Run a bcrypt job on the blocking pool under the global permit. `None` when
/// no permit freed up within BCRYPT_WAIT (the caller answers "busy").
///
/// The permit moves INTO the blocking job and is released when bcrypt
/// finishes. Held by the request instead, it would be released as soon as a
/// client disconnects (the request future is dropped) while its bcrypt keeps
/// running — and connect-then-hang-up would run more bcrypt at once than the
/// permits allow.
pub async fn run_bcrypt<T: Send + 'static>(job: impl FnOnce() -> T + Send + 'static) -> Option<T> {
    run_under(&BCRYPT_PERMITS, BCRYPT_WAIT, job).await
}

async fn run_under<T: Send + 'static>(
    permits: &Arc<Semaphore>,
    wait: Duration,
    job: impl FnOnce() -> T + Send + 'static,
) -> Option<T> {
    let permit = tokio::time::timeout(wait, permits.clone().acquire_owned())
        .await
        .ok()?
        .ok()?;
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        job()
    })
    .await
    .ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    fn xff(v: &str) -> HeaderMap {
        let mut h = HeaderMap::new();
        h.insert("x-forwarded-for", HeaderValue::from_str(v).unwrap());
        h
    }

    #[test]
    fn a_public_peer_is_the_client_whatever_it_claims() {
        let peer: SocketAddr = "203.0.113.7:5555".parse().unwrap();
        assert_eq!(
            client_ip(Some(peer), &xff("198.51.100.1")),
            Some("203.0.113.7".parse().unwrap())
        );
    }

    #[test]
    fn behind_a_local_proxy_the_last_forwarded_hop_is_the_client() {
        let caddy: SocketAddr = "172.18.0.3:40000".parse().unwrap();
        // A client-supplied entry first, then the one the proxy appended.
        assert_eq!(
            client_ip(Some(caddy), &xff("1.2.3.4, 203.0.113.9")),
            Some("203.0.113.9".parse().unwrap())
        );
        let host_proxy: SocketAddr = "127.0.0.1:40000".parse().unwrap();
        assert_eq!(
            client_ip(Some(host_proxy), &xff("2001:db8::1")),
            Some("2001:db8::1".parse().unwrap())
        );
    }

    /// A proxy that does not send X-Forwarded-For must not put every user
    /// under its own address: one shared counter would let anyone lock all
    /// users out. Those requests get no per-IP limit instead.
    #[test]
    fn a_local_proxy_without_forwarded_for_gives_no_client_ip() {
        let proxy: SocketAddr = "172.17.0.1:40000".parse().unwrap();
        assert_eq!(client_ip(Some(proxy), &HeaderMap::new()), None);
        assert_eq!(client_ip(Some(proxy), &xff("not-an-ip")), None);
    }

    /// The permit stays taken until bcrypt itself finishes, even when the
    /// request that started it is gone (client hung up).
    #[tokio::test]
    async fn a_cancelled_request_keeps_its_permit_until_bcrypt_finishes() {
        let permits = Arc::new(Semaphore::new(1));
        let (release, wait) = std::sync::mpsc::channel::<()>();
        let (started_tx, started_rx) = tokio::sync::oneshot::channel::<()>();
        let task = tokio::spawn({
            let permits = permits.clone();
            async move {
                run_under(&permits, Duration::from_secs(1), move || {
                    let _ = started_tx.send(());
                    let _ = wait.recv(); // "bcrypt" running
                })
                .await
            }
        });
        started_rx.await.unwrap();
        task.abort(); // the client disconnected
        let _ = task.await;
        assert_eq!(
            permits.available_permits(),
            0,
            "still held: the job is still running"
        );
        release.send(()).unwrap();
        for _ in 0..100 {
            if permits.available_permits() == 1 {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("the permit must come back once the job ends");
    }

    #[test]
    fn no_peer_means_no_per_ip_limit() {
        assert_eq!(client_ip(None, &xff("1.2.3.4")), None);
    }

    #[test]
    fn the_limit_applies_per_key_across_calls() {
        let l = AttemptLimiter::new(3, Duration::from_secs(60), 100);
        let a: IpAddr = "203.0.113.1".parse().unwrap();
        let b: IpAddr = "203.0.113.2".parse().unwrap();
        assert!(!l.hit(&a) && !l.hit(&a) && !l.hit(&a));
        assert!(l.hit(&a), "the 4th attempt in the window is over the limit");
        assert!(!l.hit(&b), "another client has its own count");
        l.clear(&a);
        assert!(!l.hit(&a));
    }

    #[test]
    fn a_full_table_does_not_let_new_keys_evict_old_counts() {
        let l = AttemptLimiter::new(1, Duration::from_secs(60), 2);
        assert!(!l.hit(&"victim".to_string()));
        assert!(!l.hit(&"k1".to_string()));
        // Full: a flood of fresh keys is let through untracked...
        for i in 0..100 {
            assert!(!l.hit(&format!("flood{i}")));
        }
        assert_eq!(l.len(), 2);
        // ...and the victim's count survived it.
        assert!(l.hit(&"victim".to_string()));
    }

    #[tokio::test]
    async fn bcrypt_runs_off_the_runtime_and_returns_its_result() {
        assert_eq!(run_bcrypt(|| 40 + 2).await, Some(42));
    }
}
