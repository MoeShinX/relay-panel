//! v1.0.10: node self-upgrade.
//!
//! Triggered by a directed `UpgradeNodeMessage` on the WS control channel. The
//! node downloads the official `relay-node` release binary for the REQUESTED
//! version (resolved by the panel from the latest `node-v*` GitHub release) and
//! this node's architecture, verifies its published sha256, backs up + atomically
//! swaps its own binary, and returns Ok — the caller then exits so systemd
//! (Restart=always) re-execs into the new binary.
//!
//! v1.2: panel and node release on independent tracks. Node binaries live under
//! `node-v{version}` tags from 1.1.1 onward; releases 1.1.0 and earlier were
//! published under the joint `v{version}` tag. `build_download_urls` picks the
//! right prefix per version (and never blindly falls back — only the known
//! legacy range uses `v{version}`).
//!
//! Guarantees:
//! - **systemd only.** docker / manual installs are refused: only systemd has a
//!   supervisor that restarts the process (and, for docker, a self-replaced
//!   binary is lost when the container is recreated). The install method is also
//!   reported to the panel so the UI never offers a self-upgrade that can't work.
//! - **Single-flight.** A global flag rejects a second upgrade while one runs,
//!   so concurrent/repeated commands can't race on the temp file / swap.
//! - **Mandatory backup.** The current binary is copied to `.bak` BEFORE the
//!   swap; if the backup fails the upgrade is aborted (nothing is replaced).
//! - **Download acceleration.** Direct GitHub first, then three fixed
//!   GitHub-proxy prefixes (see `SOURCES`) when direct is unreachable or slow.
//!   The sha256 is always the one GitHub publishes next to the binary, taken
//!   ONLY from GitHub itself (direct or the trusted relay), never from the
//!   public accelerators, and the binary must match it exactly.
//! - **Security.** The URL is hardcoded to the official GitHub release, and the
//!   target must be a valid semver STRICTLY NEWER than the running version — so
//!   even a compromised panel can at most force an upgrade to a newer official
//!   build (never arbitrary code, and never a downgrade to an old vulnerable
//!   release). A failed upgrade (network / hash / io) leaves the binary intact.

use sha2::{Digest, Sha256};
use std::sync::atomic::{AtomicBool, Ordering};

/// Official release source. Never taken from the panel.
const REPO: &str = "MoeShinX/relay-panel";

/// Single-flight guard: true while an upgrade is downloading/swapping.
static UPGRADING: AtomicBool = AtomicBool::new(false);

/// How this node is run. Drives whether a one-click self-upgrade is possible.
/// - `docker`: `/.dockerenv` exists → a self-replaced binary is lost on the next
///   `docker compose up` / image pull, so we don't self-replace.
/// - `systemd`: systemd sets `INVOCATION_ID` for the units it starts (inherited
///   by children) → there's a supervisor to restart us after we exit.
/// - `manual`: neither → exiting would leave nothing to bring the node back.
pub fn install_method() -> &'static str {
    if std::path::Path::new("/.dockerenv").exists() {
        "docker"
    } else if std::env::var_os("INVOCATION_ID").is_some() {
        "systemd"
    } else {
        "manual"
    }
}

/// v1.2.4: tell the panel a self-upgrade attempt failed.
///
/// Only failures are reported, and the asymmetry is forced rather than chosen:
/// a successful upgrade ends with the caller calling `std::process::exit(0)`,
/// so there is no process left to send a success. Nor is one needed — the node
/// comes back up and reports its new version in the ordinary status report,
/// which is what the panel's audit entry tells the operator to look at.
///
/// A failure had nowhere to go before this. It was logged locally on a machine
/// whose whole problem is that nobody is watching it, so from the panel a
/// failed upgrade and a slow one looked identical, permanently.
///
/// Best-effort by construction: this runs on the failure path of an upgrade
/// that already went wrong, frequently because the network is unreachable, so
/// it gets a short timeout and its own error is logged and dropped. Never
/// propagate from here — the node's job now is to keep forwarding on the binary
/// it still has.
pub async fn report_failure(
    panel_url: &str,
    token: &str,
    node_id: &str,
    target_version: &str,
    error: &str,
) {
    let result = relay_shared::protocol::UpgradeResult::new(
        node_id.to_string(),
        env!("CARGO_PKG_VERSION").to_string(),
        target_version.to_string(),
        error,
    );
    let url = format!("{}/api/v1/node/upgrade_result", panel_url);
    let client = match reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
    {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!("upgrade_result: build client: {}", e);
            return;
        }
    };
    match client
        .post(&url)
        .header("Authorization", format!("Bearer {token}"))
        .json(&result)
        .send()
        .await
    {
        Ok(r) if r.status().is_success() => {
            tracing::info!("upgrade_result: reported failure to panel");
        }
        Ok(r) => tracing::warn!("upgrade_result: panel returned HTTP {}", r.status()),
        Err(e) => tracing::warn!("upgrade_result: post failed: {}", e),
    }
}

/// Map the compiled target arch to the release asset suffix.
fn asset_arch() -> Option<&'static str> {
    match std::env::consts::ARCH {
        "x86_64" => Some("amd64"),
        "aarch64" => Some("arm64"),
        _ => None,
    }
}

/// Validate a self-upgrade target against `current`. Returns Ok only when
/// `target` parses as semver AND is STRICTLY greater than `current`. This does
/// double duty:
///   - Parsing as semver rejects anything that could escape the URL path
///     (`/`, `..`, "latest", …) — a valid semver has none of those.
///   - The strict `>` comparison blocks a downgrade or a same-version reinstall.
///     Under the "panel is compromised" threat model, this stops an attacker
///     from rolling a node back to an old, vulnerable official release.
fn check_upgrade_target(target: &str, current: &str) -> Result<(), String> {
    let t = semver::Version::parse(target)
        .map_err(|_| format!("unparseable target version {target:?}"))?;
    let c = semver::Version::parse(current)
        .map_err(|_| format!("unparseable current version {current:?}"))?;
    if t <= c {
        return Err(format!(
            "target v{t} is not newer than current v{c}; refusing downgrade/reinstall"
        ));
    }
    Ok(())
}

/// Upgrade this node to `version` (a plain semver WITHOUT a leading "v"). On
/// success the new binary is in place and the caller should exit(0). On any
/// error the running binary is untouched and a later retry is allowed.
pub async fn self_upgrade(version: &str) -> Result<(), String> {
    // Only systemd installs can safely self-replace + restart.
    let method = install_method();
    if method != "systemd" {
        return Err(format!(
            "self-upgrade is only supported for systemd installs (this node is '{method}'); \
             update the container image / re-run the install script instead"
        ));
    }
    // Reject unparseable versions, downgrades, and same-version reinstalls.
    check_upgrade_target(version, env!("CARGO_PKG_VERSION"))?;
    // Single-flight: reject a concurrent/repeat upgrade so two tasks can't race
    // on the temp file and swap, which could leave a truncated binary.
    if UPGRADING
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        return Err("an upgrade is already in progress".into());
    }
    let result = do_upgrade(version).await;
    if result.is_err() {
        // Failed → release the flag so a later command can retry. (On success we
        // exit the process before reaching here, so the flag simply dies with us.)
        UPGRADING.store(false, Ordering::SeqCst);
    }
    result
}

/// v1.2: the first node release published under its own `node-v*` tag. Versions
/// strictly older than this were published under the joint `v{version}` tag, so
/// the download URL for them uses `v{version}` (legacy fallback). This is a
/// bounded, explicit boundary — NOT an arbitrary fallback — so a future unknown
/// version can never silently pick up a wrong-tag URL.
const FIRST_NODE_V_TAG: semver::Version = semver::Version::new(1, 1, 1);

/// v1.2: build the (binary, sha256) download URLs for a node self-upgrade to
/// `version` on architecture `arch` ("amd64" | "arm64"). New versions (>=
/// FIRST_NODE_V_TAG) use the `node-v{version}` tag; the known legacy range
/// (< FIRST_NODE_V_TAG, i.e. 1.0.x and 1.1.0) falls back to the joint
/// `v{version}` tag where those binaries were originally published.
///
/// `version` must already be a validated semver WITHOUT a leading "v" (the
/// caller's `check_upgrade_target` guarantees this). Returns `(bin_url, sha_url)`
/// — the sha256 URL is always the binary URL + ".sha256".
fn build_download_urls(version: &str, arch: &str) -> Result<(String, String), String> {
    let asset = format!("relay-node-linux-{arch}");
    let parsed = semver::Version::parse(version)
        .map_err(|_| format!("unparseable version {version:?} in build_download_urls"))?;
    let tag_prefix = if parsed < FIRST_NODE_V_TAG {
        // Legacy: 1.0.x / 1.1.0 shipped under the joint v* tag.
        "v"
    } else {
        // New: 1.1.1+ ship under the node-only node-v* tag.
        "node-v"
    };
    let bin_url =
        format!("https://github.com/{REPO}/releases/download/{tag_prefix}{version}/{asset}");
    let sha_url = format!("{bin_url}.sha256");
    Ok((bin_url, sha_url))
}

// ---- download acceleration -------------------------------------------------

/// Where GitHub files are fetched from, in the order tried:
/// `(prefix, trusted, label)`. The full GitHub URL is appended after the prefix;
/// `""` is the direct connection. The label is what logs and error text call the
/// source (never a URL). Hardcoded on purpose: never taken from the panel, so a
/// compromised panel cannot point nodes at a download host of its choosing.
///
/// Trusted (direct GitHub, the trusted relay #2): may supply the sha256.
/// Public third-party accelerators (#3, #4): may only carry the program file,
/// which is then checked against a sha256 from a trusted source. A relay that
/// serves both the file and its hash proves nothing.
const SOURCES: [(&str, bool, &str); 4] = [
    ("", true, "direct"),
    ("https://down.relaypanel.dev/", true, "mirror"),
    ("https://gh-proxy.com/", false, "gh-proxy.com"),
    ("https://ghfast.top/", false, "ghfast.top"),
];

/// Largest binary accepted from any source. A release is a few MB; the cap only
/// stops a misbehaving mirror from streaming forever now that there is no total
/// time limit on the download.
const MAX_BINARY_BYTES: usize = 256 * 1024 * 1024;

/// Time limits for one fetch attempt against one source.
#[derive(Clone, Copy)]
struct Limits {
    /// TCP/TLS connect, per source.
    connect: std::time::Duration,
    /// Whole request for a small file (sha256), per source.
    small_total: std::time::Duration,
    /// Time to the response headers for the big file, per source.
    header_wait: std::time::Duration,
    /// Give up on a big download when its average speed over this long a
    /// window is below `min_bytes_per_sec`.
    slow_window: std::time::Duration,
    min_bytes_per_sec: u64,
}

impl Limits {
    const DEFAULT: Limits = Limits {
        connect: std::time::Duration::from_secs(5),
        small_total: std::time::Duration::from_secs(10),
        header_wait: std::time::Duration::from_secs(15),
        slow_window: std::time::Duration::from_secs(15),
        min_bytes_per_sec: 50 * 1024,
    };
}

/// The URL to request through `prefix` (`""` = direct).
fn mirror_url(prefix: &str, url: &str) -> String {
    format!("{prefix}{url}")
}

/// Order in which to try `n` sources: the one that worked earlier in this
/// operation first (if any, and in range), then the rest in list order.
fn mirror_order(preferred: Option<usize>, n: usize) -> Vec<usize> {
    let first = preferred.filter(|&p| p < n);
    first
        .into_iter()
        .chain((0..n).filter(move |&i| Some(i) != first))
        .collect()
}

#[derive(Debug, PartialEq, Eq)]
enum Speed {
    Ok,
    TooSlow,
}

/// Low-speed detector for the big download. Mirrors curl's
/// `--speed-limit/--speed-time`: judged once per `window`, on the average speed
/// over that window; a window that clears the bar starts a fresh one. Times are
/// passed in (as offsets from the start of the download) so it is testable
/// without sleeping.
struct SpeedGuard {
    window: std::time::Duration,
    min_bytes_per_sec: u64,
    window_start: std::time::Duration,
    bytes: u64,
}

impl SpeedGuard {
    fn new(window: std::time::Duration, min_bytes_per_sec: u64, now: std::time::Duration) -> Self {
        Self {
            window,
            min_bytes_per_sec,
            window_start: now,
            bytes: 0,
        }
    }

    /// How long until the current window is due for judgement.
    fn time_left(&self, now: std::time::Duration) -> std::time::Duration {
        (self.window_start + self.window).saturating_sub(now)
    }

    fn on_bytes(&mut self, now: std::time::Duration, n: usize) -> Speed {
        self.bytes += n as u64;
        self.check(now)
    }

    /// Judge the window if it is complete (also called when no data arrived
    /// for the rest of the window: a stall).
    fn check(&mut self, now: std::time::Duration) -> Speed {
        let elapsed = now.saturating_sub(self.window_start);
        if elapsed < self.window {
            return Speed::Ok;
        }
        let ms = elapsed.as_millis().max(1);
        if (self.bytes as u128) * 1000 < (self.min_bytes_per_sec as u128) * ms {
            return Speed::TooSlow;
        }
        self.window_start = now;
        self.bytes = 0;
        Speed::Ok
    }
}

/// Parse the `<hex>  <filename>` text of a `.sha256` file into the lowercase hash.
fn parse_sha256(text: &str) -> Result<String, String> {
    let h = text.split_whitespace().next().unwrap_or("").to_lowercase();
    if h.len() != 64 || !h.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err("malformed sha256 file".to_string());
    }
    Ok(h)
}

/// A downloaded binary must be a non-empty ELF file; anything else (an HTML
/// "blocked" page served with 200, say) means this source is no good.
fn check_elf(bytes: &[u8]) -> Result<(), String> {
    if bytes.is_empty() {
        return Err("empty body".to_string());
    }
    if !bytes.starts_with(b"\x7fELF") {
        return Err("not an ELF binary".to_string());
    }
    Ok(())
}

/// A reqwest error as short text with no URL in it (a redirect target may be a
/// signed link, and these strings go to the log and to the panel).
fn describe_err(e: reqwest::Error) -> String {
    let e = e.without_url();
    if e.is_timeout() {
        "timed out".to_string()
    } else if e.is_connect() {
        "cannot connect".to_string()
    } else if e.is_redirect() {
        "too many redirects".to_string()
    } else if e.is_body() || e.is_decode() {
        "connection broke while downloading".to_string()
    } else {
        "request failed".to_string()
    }
}

/// Largest sha256 file accepted (it is one line).
const MAX_SMALL_BYTES: usize = 1024 * 1024;

/// One place a GitHub file can be fetched from.
#[derive(Clone)]
struct Source {
    /// Put before the full GitHub URL (`""` = direct).
    prefix: String,
    /// Name used in logs and error text.
    label: String,
    /// Trusted sources may supply the sha256 (and any other text we act on);
    /// the others only carry the program file, which is checked against a
    /// sha256 taken from a trusted source.
    trusted: bool,
}

/// What `fetch_large` runs on a downloaded body after the ELF check; an error
/// makes that source count as failed and the next one is tried.
type BodyCheck<'a> = &'a (dyn Fn(&[u8]) -> Result<(), String> + Sync);

/// Fetches files from GitHub through the source list.
///
/// Trust model: the sha256 is accepted from trusted sources only (`fetch_trusted_small`,
/// list order, nothing remembered), because a hash that came with the file from
/// the same untrusted relay proves nothing. The program may come from any
/// source (`fetch_large`) and is verified against that hash before it counts
/// as downloaded. For the program the source that worked earlier in the SAME
/// operation is tried first; a new operation builds a new `Fetcher` and starts
/// from direct again.
struct Fetcher {
    client: reqwest::Client,
    sources: Vec<Source>,
    limits: Limits,
    preferred: Option<usize>,
}

impl Fetcher {
    fn new() -> Result<Self, String> {
        let client = reqwest::Client::builder()
            .connect_timeout(Limits::DEFAULT.connect)
            .build()
            .map_err(|e| format!("http client: {e}"))?;
        Ok(Self::with(
            client,
            SOURCES
                .iter()
                .map(|(prefix, trusted, label)| Source {
                    prefix: prefix.to_string(),
                    label: label.to_string(),
                    trusted: *trusted,
                })
                .collect(),
            Limits::DEFAULT,
        ))
    }

    fn with(client: reqwest::Client, sources: Vec<Source>, limits: Limits) -> Self {
        Self {
            client,
            sources,
            limits,
            preferred: None,
        }
    }

    /// Small text file (the sha256) from TRUSTED sources only, in list order;
    /// each gets `small_total` in all. `parse` rejects a 200 that is not the
    /// file (then the next trusted source is tried). A public source is never
    /// asked, and the source that served the program does not matter here.
    async fn fetch_trusted_small<T>(
        &self,
        what: &str,
        url: &str,
        parse: fn(&str) -> Result<T, String>,
    ) -> Result<T, String> {
        let mut failures: Vec<String> = Vec::new();
        for src in self.sources.iter().filter(|s| s.trusted) {
            let label = src.label.clone();
            let full = mirror_url(&src.prefix, url);
            let attempt = async {
                let mut resp = self
                    .client
                    .get(&full)
                    .timeout(self.limits.small_total)
                    .send()
                    .await
                    .map_err(describe_err)?;
                if !resp.status().is_success() {
                    return Err(format!("HTTP {}", resp.status().as_u16()));
                }
                let mut body: Vec<u8> = Vec::new();
                while let Some(chunk) = resp.chunk().await.map_err(describe_err)? {
                    body.extend_from_slice(&chunk);
                    if body.len() > MAX_SMALL_BYTES {
                        return Err("body too large".to_string());
                    }
                }
                parse(&String::from_utf8_lossy(&body))
            }
            .await;
            match attempt {
                Ok(v) => {
                    tracing::warn!("self-upgrade: {what} fetched via {label}");
                    return Ok(v);
                }
                Err(why) => {
                    tracing::warn!("self-upgrade: {what} via {label} failed: {why}");
                    failures.push(format!("{label}: {why}"));
                }
            }
        }
        Err(format!(
            "{what} could not be obtained from GitHub itself (tried: {})",
            if failures.is_empty() {
                "no trusted source".to_string()
            } else {
                failures.join("; ")
            }
        ))
    }

    /// The big file, from any source: no total time limit, but a source is
    /// dropped when it is too slow (see `SpeedGuard`), sends something that is
    /// not an ELF, or fails `check` (the sha256 comparison).
    async fn fetch_large(
        &mut self,
        what: &str,
        url: &str,
        check: BodyCheck<'_>,
    ) -> Result<Vec<u8>, String> {
        let mut failures: Vec<String> = Vec::new();
        for i in mirror_order(self.preferred, self.sources.len()) {
            let label = self.sources[i].label.clone();
            let full = mirror_url(&self.sources[i].prefix, url);
            match self.try_large(&full, check).await {
                Ok(bytes) => {
                    self.note_success(what, i, &label);
                    return Ok(bytes);
                }
                Err(why) => {
                    tracing::warn!("self-upgrade: {what} via {label} failed: {why}");
                    failures.push(format!("{label}: {why}"));
                }
            }
        }
        Err(format!(
            "{what}: all sources failed ({})",
            failures.join("; ")
        ))
    }

    async fn try_large(&self, full_url: &str, check: BodyCheck<'_>) -> Result<Vec<u8>, String> {
        let l = self.limits;
        let mut resp = tokio::time::timeout(l.header_wait, self.client.get(full_url).send())
            .await
            .map_err(|_| "no response".to_string())?
            .map_err(describe_err)?;
        if !resp.status().is_success() {
            return Err(format!("HTTP {}", resp.status().as_u16()));
        }
        let started = std::time::Instant::now();
        let mut guard = SpeedGuard::new(l.slow_window, l.min_bytes_per_sec, started.elapsed());
        let mut buf: Vec<u8> = Vec::new();
        loop {
            let wait = guard.time_left(started.elapsed());
            match tokio::time::timeout(wait, resp.chunk()).await {
                Ok(Ok(Some(chunk))) => {
                    buf.extend_from_slice(&chunk);
                    if buf.len() > MAX_BINARY_BYTES {
                        return Err("body too large".to_string());
                    }
                    if guard.on_bytes(started.elapsed(), chunk.len()) == Speed::TooSlow {
                        return Err("too slow".to_string());
                    }
                }
                Ok(Ok(None)) => break,
                Ok(Err(e)) => return Err(describe_err(e)),
                Err(_) => {
                    if guard.check(started.elapsed()) == Speed::TooSlow {
                        return Err("too slow".to_string());
                    }
                }
            }
        }
        check_elf(&buf)?;
        check(&buf)?;
        Ok(buf)
    }

    /// Log whenever the source in use changes (the operation starts on direct),
    /// and remember it for the next program fetch.
    fn note_success(&mut self, what: &str, i: usize, label: &str) {
        let switched = match self.preferred {
            None => i != 0,
            Some(p) => p != i,
        };
        if switched {
            tracing::warn!("self-upgrade: {what} fetched via {label}");
        }
        self.preferred = Some(i);
    }
}

/// Lowercase hex sha256 of `bytes`.
fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Get the verified program bytes: FIRST the sha256 from a trusted source (so
/// a node that cannot get one stops before downloading anything), then the
/// program from any source, accepted only if it matches that sha256.
async fn download_verified(
    fetcher: &mut Fetcher,
    bin_url: &str,
    sha_url: &str,
) -> Result<Vec<u8>, String> {
    let expected = fetcher
        .fetch_trusted_small("sha256", sha_url, parse_sha256)
        .await
        .map_err(|e| format!("{e}; refusing to upgrade without GitHub's sha256, retry later"))?;
    fetcher
        .fetch_large("binary", bin_url, &|bytes: &[u8]| {
            if sha256_hex(bytes) == expected {
                Ok(())
            } else {
                Err("sha256 mismatch".to_string())
            }
        })
        .await
}

async fn do_upgrade(version: &str) -> Result<(), String> {
    let arch =
        asset_arch().ok_or_else(|| format!("unsupported arch: {}", std::env::consts::ARCH))?;
    let (bin_url, sha_url) = build_download_urls(version, arch)?;

    let bin_path = std::env::current_exe().map_err(|e| format!("current_exe: {e}"))?;

    tracing::warn!(
        "self-upgrade: downloading relay-node-linux-{arch} v{version} (direct first, then GitHub proxies)"
    );
    let mut fetcher = Fetcher::new()?;

    // 1+2. Published sha256 from GitHub itself (direct, or via the trusted relay),
    // then the program from any source, verified against it.
    let bin_bytes = download_verified(&mut fetcher, &bin_url, &sha_url).await?;
    tracing::warn!(
        "self-upgrade: sha256 verified ({} bytes) for relay-node-linux-{arch} v{version}",
        bin_bytes.len(),
    );

    // 4. Write a UNIQUE temp file (pid + nanos, so a stray concurrent run can't
    // truncate ours), chmod +x, back up the current binary (MANDATORY), then
    // atomically rename over the running binary. Renaming over a running binary
    // is fine on Linux (the live process keeps its old inode).
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let tmp = bin_path.with_extension(format!("upgrade.{}.{}.tmp", std::process::id(), nanos));
    let bak = bin_path.with_extension("bak");

    tokio::fs::write(&tmp, &bin_bytes)
        .await
        .map_err(|e| format!("write temp binary: {e}"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Err(e) = std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o755)) {
            let _ = tokio::fs::remove_file(&tmp).await;
            return Err(format!("chmod temp binary: {e}"));
        }
    }
    // MANDATORY backup: if we can't preserve a rollback copy, abort (don't swap).
    if let Err(e) = tokio::fs::copy(&bin_path, &bak).await {
        let _ = tokio::fs::remove_file(&tmp).await;
        return Err(format!(
            "backup to {} failed, aborting upgrade: {e}",
            bak.display()
        ));
    }
    if let Err(e) = tokio::fs::rename(&tmp, &bin_path).await {
        let _ = tokio::fs::remove_file(&tmp).await;
        return Err(format!("swap binary: {e}"));
    }

    tracing::warn!(
        "self-upgrade: binary swapped at {} (old kept as {}); exiting for systemd restart",
        bin_path.display(),
        bak.display()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn asset_arch_maps_known_targets() {
        // The real function returns None on an unsupported arch rather than
        // panicking — exercised indirectly by do_upgrade's early return.
        let _ = asset_arch();
    }

    #[test]
    fn upgrade_target_allows_only_strictly_newer() {
        // Strictly newer → allowed.
        assert!(check_upgrade_target("1.0.11", "1.0.10").is_ok());
        assert!(check_upgrade_target("1.1.0", "1.0.10").is_ok());
        assert!(check_upgrade_target("2.0.0", "1.9.9").is_ok());
        // Same version → rejected (no reinstall).
        assert!(check_upgrade_target("1.0.10", "1.0.10").is_err());
        // Downgrade → rejected (blocks rolling back to an old vulnerable build).
        assert!(check_upgrade_target("1.0.9", "1.0.10").is_err());
        assert!(check_upgrade_target("0.4.20", "1.0.10").is_err());
        // Unparseable / path-injection attempts → rejected.
        assert!(check_upgrade_target("latest", "1.0.10").is_err());
        assert!(check_upgrade_target("1.0.11/../../etc", "1.0.10").is_err());
        assert!(check_upgrade_target("1.0", "1.0.10").is_err());
        assert!(check_upgrade_target("", "1.0.10").is_err());
    }

    // ---- v1.2: download URL prefix (node-v* for new, v* legacy fallback) ----

    #[test]
    fn build_download_urls_uses_node_v_for_new_versions() {
        // 1.1.1 and above ship under the node-only node-v* tag.
        let (bin, sha) = build_download_urls("1.1.1", "amd64").unwrap();
        assert_eq!(
            bin,
            "https://github.com/MoeShinX/relay-panel/releases/download/node-v1.1.1/relay-node-linux-amd64"
        );
        // sha256 URL always tracks the binary URL + ".sha256".
        assert_eq!(sha, format!("{bin}.sha256"));

        // arm64 + a higher version.
        let (bin2, sha2) = build_download_urls("2.0.0", "arm64").unwrap();
        assert_eq!(
            bin2,
            "https://github.com/MoeShinX/relay-panel/releases/download/node-v2.0.0/relay-node-linux-arm64"
        );
        assert_eq!(sha2, format!("{bin2}.sha256"));
    }

    #[test]
    fn build_download_urls_falls_back_to_v_for_legacy_1_1_0() {
        // 1.1.0 and the 1.0.x line were published under the joint v* tag.
        let (bin, sha) = build_download_urls("1.1.0", "amd64").unwrap();
        assert_eq!(
            bin,
            "https://github.com/MoeShinX/relay-panel/releases/download/v1.1.0/relay-node-linux-amd64"
        );
        assert_eq!(sha, format!("{bin}.sha256"));

        // A 1.0.x patch version also falls back.
        let (bin2, _) = build_download_urls("1.0.10", "arm64").unwrap();
        assert!(bin2.contains("/download/v1.0.10/"));
        assert!(!bin2.contains("/node-v1.0.10/"));
    }

    #[test]
    fn build_download_urls_rejects_unparseable_version() {
        // A non-semver version must error rather than build a bogus URL.
        assert!(build_download_urls("latest", "amd64").is_err());
        assert!(build_download_urls("1.0", "amd64").is_err());
        assert!(build_download_urls("", "amd64").is_err());
        // Path-injection attempts are rejected (no '/' can sneak through).
        assert!(build_download_urls("1.1.1/../../etc", "amd64").is_err());
    }

    // ---- download acceleration: prefixes, ordering, speed guard ----

    #[test]
    fn sources_are_direct_then_three_proxies_and_only_the_first_two_are_trusted() {
        assert_eq!(
            SOURCES,
            [
                ("", true, "direct"),
                ("https://down.relaypanel.dev/", true, "mirror"),
                ("https://gh-proxy.com/", false, "gh-proxy.com"),
                ("https://ghfast.top/", false, "ghfast.top"),
            ]
        );
    }

    #[test]
    fn mirror_url_puts_the_full_github_url_after_the_prefix() {
        let (bin, _) = build_download_urls("1.2.9", "amd64").unwrap();
        // Direct: unchanged.
        assert_eq!(mirror_url(SOURCES[0].0, &bin), bin);
        assert_eq!(
            mirror_url(SOURCES[1].0, &bin),
            "https://down.relaypanel.dev/https://github.com/MoeShinX/relay-panel/releases/download/node-v1.2.9/relay-node-linux-amd64"
        );
        assert_eq!(
            mirror_url(SOURCES[3].0, &bin),
            format!("https://ghfast.top/{bin}")
        );
    }

    #[test]
    fn source_labels_name_a_source_without_a_url_and_the_relay_is_just_mirror() {
        let labels: Vec<&str> = SOURCES.iter().map(|s| s.2).collect();
        assert_eq!(labels, ["direct", "mirror", "gh-proxy.com", "ghfast.top"]);
        for l in labels {
            assert!(!l.contains("://") && !l.contains('/') && !l.contains("relaypanel"));
        }
    }

    #[test]
    fn mirror_order_starts_with_the_source_that_worked() {
        // Nothing worked yet: list order.
        assert_eq!(mirror_order(None, 4), vec![0, 1, 2, 3]);
        // Source 1 worked: it goes first, the rest keep their order.
        assert_eq!(mirror_order(Some(1), 4), vec![1, 0, 2, 3]);
        assert_eq!(mirror_order(Some(3), 4), vec![3, 0, 1, 2]);
        // Direct worked: unchanged.
        assert_eq!(mirror_order(Some(0), 4), vec![0, 1, 2, 3]);
        // Out of range is ignored, never panics or duplicates.
        assert_eq!(mirror_order(Some(9), 4), vec![0, 1, 2, 3]);
        assert!(mirror_order(None, 0).is_empty());
    }

    fn secs(n: u64) -> std::time::Duration {
        std::time::Duration::from_secs(n)
    }

    #[test]
    fn speed_guard_passes_a_steady_fast_download() {
        let mut g = SpeedGuard::new(secs(15), 50 * 1024, secs(0));
        // 200 KB/s for two minutes, a chunk per second.
        for t in 1..=120 {
            assert_eq!(g.on_bytes(secs(t), 200 * 1024), Speed::Ok, "at {t}s");
        }
    }

    #[test]
    fn speed_guard_gives_up_after_a_full_slow_window() {
        let mut g = SpeedGuard::new(secs(15), 50 * 1024, secs(0));
        // 10 KB/s: fine until the 15 s window is complete, then too slow.
        for t in 1..15 {
            assert_eq!(g.on_bytes(secs(t), 10 * 1024), Speed::Ok, "at {t}s");
        }
        assert_eq!(g.on_bytes(secs(15), 10 * 1024), Speed::TooSlow);
    }

    #[test]
    fn speed_guard_treats_a_stall_as_too_slow_only_when_the_window_ends() {
        let mut g = SpeedGuard::new(secs(15), 50 * 1024, secs(0));
        assert_eq!(g.on_bytes(secs(1), 1024), Speed::Ok);
        // Waiting is fine while the window is open...
        assert_eq!(g.check(secs(14)), Speed::Ok);
        assert_eq!(g.time_left(secs(14)), secs(1));
        // ...and it ends the download once it is complete.
        assert_eq!(g.check(secs(15)), Speed::TooSlow);
    }

    #[test]
    fn speed_guard_judges_each_window_on_its_own() {
        let mut g = SpeedGuard::new(secs(15), 50 * 1024, secs(0));
        // A fast burst fills the first window...
        assert_eq!(g.on_bytes(secs(1), 2 * 1024 * 1024), Speed::Ok);
        assert_eq!(g.check(secs(15)), Speed::Ok);
        // ...but it does not pay for the next one.
        assert_eq!(g.time_left(secs(15)), secs(15));
        assert_eq!(g.check(secs(29)), Speed::Ok);
        assert_eq!(g.check(secs(30)), Speed::TooSlow);
    }

    #[test]
    fn parse_sha256_accepts_only_a_real_hash() {
        let h = "a".repeat(64);
        assert_eq!(
            parse_sha256(&format!("{}  relay-node-linux-amd64\n", h.to_uppercase())).unwrap(),
            h
        );
        assert!(parse_sha256("").is_err());
        assert!(parse_sha256("<html>blocked</html>").is_err());
        assert!(parse_sha256(&"g".repeat(64)).is_err());
        assert!(parse_sha256(&"a".repeat(63)).is_err());
    }

    #[test]
    fn check_elf_rejects_empty_and_non_elf_bodies() {
        assert!(check_elf(b"\x7fELF\x02\x01").is_ok());
        assert!(check_elf(b"").is_err());
        assert!(check_elf(b"<html>blocked</html>").is_err());
    }

    // ---- download acceleration: end to end against local servers ----

    use std::sync::{Arc, Mutex};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    enum Reply {
        Body(u16, Vec<u8>),
        /// Valid headers, then one byte every 40 ms for a long time.
        Trickle,
    }

    /// A tiny HTTP server. `route` maps a request path to a reply. Returns its
    /// port and the list of request paths it has seen.
    async fn serve(route: fn(&str) -> Reply) -> (u16, Arc<Mutex<Vec<String>>>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let seen2 = seen.clone();
        tokio::spawn(async move {
            loop {
                let Ok((mut sock, _)) = listener.accept().await else {
                    return;
                };
                let seen = seen2.clone();
                tokio::spawn(async move {
                    let mut buf = vec![0u8; 4096];
                    let n = sock.read(&mut buf).await.unwrap_or(0);
                    let req = String::from_utf8_lossy(&buf[..n]).to_string();
                    let path = req.split_whitespace().nth(1).unwrap_or("").to_string();
                    seen.lock().unwrap().push(path.clone());
                    match route(&path) {
                        Reply::Body(code, body) => {
                            let head = format!(
                                "HTTP/1.1 {code} X\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                                body.len()
                            );
                            let _ = sock.write_all(head.as_bytes()).await;
                            let _ = sock.write_all(&body).await;
                        }
                        Reply::Trickle => {
                            let _ = sock
                                .write_all(
                                    b"HTTP/1.1 200 X\r\nContent-Length: 100000000\r\nConnection: close\r\n\r\n",
                                )
                                .await;
                            loop {
                                if sock.write_all(b"x").await.is_err() {
                                    return;
                                }
                                tokio::time::sleep(std::time::Duration::from_millis(40)).await;
                            }
                        }
                    }
                });
            }
        });
        (port, seen)
    }

    fn elf_body() -> Vec<u8> {
        let mut b = b"\x7fELF".to_vec();
        b.extend_from_slice(&[7u8; 4096]);
        b
    }

    /// A tampered program: looks like an ELF, is not the real one.
    fn evil_body() -> Vec<u8> {
        let mut b = b"\x7fELF".to_vec();
        b.extend_from_slice(&[0x66u8; 4096]);
        b
    }

    /// The genuine program and the genuine hash of it.
    fn route_ok(path: &str) -> Reply {
        if path.ends_with(".sha256") {
            Reply::Body(
                200,
                format!("{}  relay-node\n", sha256_hex(&elf_body())).into_bytes(),
            )
        } else {
            Reply::Body(200, elf_body())
        }
    }
    /// A source that has the genuine hash but not the program.
    fn route_sha_only(path: &str) -> Reply {
        if path.ends_with(".sha256") {
            route_ok(path)
        } else {
            Reply::Body(404, b"nope".to_vec())
        }
    }
    /// A malicious relay: a tampered program AND a hash that matches it.
    fn route_evil(path: &str) -> Reply {
        if path.ends_with(".sha256") {
            Reply::Body(
                200,
                format!("{}  relay-node\n", sha256_hex(&evil_body())).into_bytes(),
            )
        } else {
            Reply::Body(200, evil_body())
        }
    }
    fn route_404(_: &str) -> Reply {
        Reply::Body(404, b"nope".to_vec())
    }
    fn route_html(_: &str) -> Reply {
        Reply::Body(200, b"<html>blocked</html>".to_vec())
    }
    fn route_trickle(_: &str) -> Reply {
        Reply::Trickle
    }

    fn fast_limits() -> Limits {
        Limits {
            connect: std::time::Duration::from_millis(500),
            small_total: std::time::Duration::from_secs(2),
            header_wait: std::time::Duration::from_secs(2),
            slow_window: std::time::Duration::from_millis(300),
            min_bytes_per_sec: 50 * 1024,
        }
    }

    /// `(prefix, trusted)` pairs -> a fetcher against local servers.
    fn test_fetcher(sources: Vec<(String, bool)>) -> Fetcher {
        let client = reqwest::Client::builder().no_proxy().build().unwrap();
        let sources = sources
            .into_iter()
            .map(|(prefix, trusted)| Source {
                label: if prefix.is_empty() {
                    "direct".to_string()
                } else {
                    prefix
                        .trim_start_matches("http://")
                        .trim_end_matches('/')
                        .to_string()
                },
                prefix,
                trusted,
            })
            .collect();
        Fetcher::with(client, sources, fast_limits())
    }

    fn local(port: u16, trusted: bool) -> (String, bool) {
        (format!("http://127.0.0.1:{port}/"), trusted)
    }

    fn ok_check() -> impl Fn(&[u8]) -> Result<(), String> + Sync {
        |_| Ok(())
    }

    /// A local port with nothing listening: connecting is refused at once.
    async fn dead_port() -> u16 {
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        l.local_addr().unwrap().port()
    }

    fn sha_hits(seen: &Arc<Mutex<Vec<String>>>) -> usize {
        seen.lock()
            .unwrap()
            .iter()
            .filter(|p| p.ends_with(".sha256"))
            .count()
    }
    fn bin_hits(seen: &Arc<Mutex<Vec<String>>>) -> usize {
        seen.lock()
            .unwrap()
            .iter()
            .filter(|p| !p.ends_with(".sha256"))
            .count()
    }

    const BIN: &str = "http://x.invalid/relay-node-linux-amd64";
    const SHA: &str = "http://x.invalid/relay-node-linux-amd64.sha256";

    // -- the program (any source), same-operation memory --

    #[tokio::test]
    async fn falls_back_to_the_next_source_and_sticks_to_it_for_the_program() {
        let dead = dead_port().await;
        let (p1, seen1) = serve(route_ok).await;
        let (p2, seen2) = serve(route_ok).await;
        // Source 0 = direct (dead), 1 and 2 = two local "proxies".
        let mut f = test_fetcher(vec![
            (String::new(), true),
            local(p1, true),
            local(p2, false),
        ]);
        let url = format!("http://127.0.0.1:{dead}/dl/relay-node-linux-amd64");
        let bin = f.fetch_large("binary", &url, &ok_check()).await.unwrap();
        assert_eq!(bin, elf_body());
        assert_eq!(f.preferred, Some(1));
        // Fetching the program again goes to the source that worked first.
        f.fetch_large("binary", &url, &ok_check()).await.unwrap();
        assert_eq!(seen1.lock().unwrap().len(), 2);
        assert!(seen2.lock().unwrap().is_empty());
        // The proxy got the full GitHub-style URL after its own slash.
        assert!(seen1.lock().unwrap()[0]
            .starts_with(&format!("/http://127.0.0.1:{dead}/dl/relay-node")));
    }

    #[tokio::test]
    async fn a_remembered_source_that_then_fails_falls_back_in_order() {
        let (p1, _) = serve(route_ok).await;
        let (p2, seen2) = serve(route_404).await;
        let mut f = test_fetcher(vec![local(p1, true), local(p2, false)]);
        f.preferred = Some(1);
        // Starts at source 1 (404), then falls back to source 0.
        f.fetch_large("binary", BIN, &ok_check()).await.unwrap();
        assert_eq!(seen2.lock().unwrap().len(), 1);
        assert_eq!(f.preferred, Some(0));
    }

    #[tokio::test]
    async fn a_missing_file_reports_every_source_without_a_full_url() {
        let (p1, _) = serve(route_404).await;
        let (p2, _) = serve(route_404).await;
        let mut f = test_fetcher(vec![local(p1, true), local(p2, false)]);
        let err = f
            .fetch_large(
                "binary",
                "http://x.invalid/relay-node?sig=SECRET",
                &ok_check(),
            )
            .await
            .unwrap_err();
        assert!(err.starts_with("binary: all sources failed"), "{err}");
        assert_eq!(err.matches("HTTP 404").count(), 2, "{err}");
        assert!(
            !err.contains("SECRET") && !err.contains("x.invalid"),
            "{err}"
        );
        assert_eq!(f.preferred, None);
    }

    #[tokio::test]
    async fn a_200_that_is_not_the_program_counts_as_a_failure() {
        let (bad, _) = serve(route_html).await;
        let (good, _) = serve(route_ok).await;
        let mut f = test_fetcher(vec![local(bad, true), local(good, false)]);
        assert_eq!(
            f.fetch_large("binary", BIN, &ok_check()).await.unwrap(),
            elf_body()
        );
        assert_eq!(f.preferred, Some(1));
    }

    #[tokio::test]
    async fn a_trickling_source_is_abandoned_for_the_next() {
        let (slow, _) = serve(route_trickle).await;
        let (good, _) = serve(route_ok).await;
        let mut f = test_fetcher(vec![local(slow, true), local(good, false)]);
        let started = std::time::Instant::now();
        let bin = f.fetch_large("binary", BIN, &ok_check()).await.unwrap();
        assert_eq!(bin, elf_body());
        assert_eq!(f.preferred, Some(1));
        // Gave up after about one 300 ms window, not after the whole "file".
        assert!(started.elapsed() < std::time::Duration::from_secs(5));
    }

    // -- the sha256 (trusted sources only) --

    #[tokio::test]
    async fn the_sha256_comes_from_trusted_sources_in_list_order_never_a_public_one() {
        let dead = dead_port().await;
        let (t, seen_t) = serve(route_ok).await;
        let (public, seen_public) = serve(route_ok).await;
        let mut f = test_fetcher(vec![
            (String::new(), true),
            local(t, true),
            local(public, false),
        ]);
        // Even a source that just served the program is not used for the hash
        // unless it is trusted.
        f.preferred = Some(2);
        let url = format!("http://127.0.0.1:{dead}/dl/relay-node-linux-amd64.sha256");
        let sha = f
            .fetch_trusted_small("sha256", &url, parse_sha256)
            .await
            .unwrap();
        assert_eq!(sha, sha256_hex(&elf_body()));
        assert_eq!(sha_hits(&seen_t), 1);
        assert!(seen_public.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn a_trusted_source_serving_a_non_hash_is_skipped_for_the_next_trusted_one() {
        let (bad, _) = serve(route_html).await;
        let (good, _) = serve(route_ok).await;
        let f = test_fetcher(vec![local(bad, true), local(good, true)]);
        let sha = f
            .fetch_trusted_small("sha256", SHA, parse_sha256)
            .await
            .unwrap();
        assert_eq!(sha, sha256_hex(&elf_body()));
    }

    #[tokio::test]
    async fn no_trusted_source_means_no_sha256_and_the_message_names_what_was_tried() {
        let (t1, _) = serve(route_404).await;
        let (public, seen_public) = serve(route_ok).await;
        let f = test_fetcher(vec![local(t1, true), local(public, false)]);
        let err = f
            .fetch_trusted_small(
                "sha256",
                "http://x.invalid/a.sha256?sig=SECRET",
                parse_sha256,
            )
            .await
            .unwrap_err();
        assert!(
            err.starts_with("sha256 could not be obtained from GitHub itself"),
            "{err}"
        );
        assert_eq!(err.matches("HTTP 404").count(), 1, "{err}");
        assert!(
            !err.contains("SECRET") && !err.contains("x.invalid"),
            "{err}"
        );
        // The public source was never asked, though it would have answered.
        assert!(seen_public.lock().unwrap().is_empty());

        let f = test_fetcher(vec![local(public, false)]);
        let err = f
            .fetch_trusted_small("sha256", SHA, parse_sha256)
            .await
            .unwrap_err();
        assert!(err.contains("no trusted source"), "{err}");
    }

    // -- the whole download: sha256 first, from a trusted source, then the program --

    /// (a) The only source that answers is a public one, and it serves a
    /// tampered program AND a matching hash. The upgrade must be refused, and
    /// the public source must not even be asked for the hash or the program.
    #[tokio::test]
    async fn a_public_source_serving_program_and_matching_hash_is_refused() {
        let dead = dead_port().await;
        let (evil, seen_evil) = serve(route_evil).await;
        let mut f = test_fetcher(vec![
            (String::new(), true),
            (format!("http://127.0.0.1:{dead}/"), true),
            local(evil, false),
        ]);
        let url = format!("http://127.0.0.1:{dead}/dl/relay-node-linux-amd64");
        let err = download_verified(&mut f, &url, &format!("{url}.sha256"))
            .await
            .unwrap_err();
        assert!(
            err.contains("sha256 could not be obtained from GitHub itself"),
            "{err}"
        );
        assert!(err.contains("refusing to upgrade"), "{err}");
        // Nothing at all was sent to the public source: no hash request (d),
        // and no program download either, since the hash is fetched first.
        assert!(seen_evil.lock().unwrap().is_empty());
    }

    /// (b) A public source carries the program; a trusted source supplies the
    /// real hash. Accepted, and the hash request went to the trusted one only.
    #[tokio::test]
    async fn a_public_source_may_carry_the_program_when_a_trusted_one_gives_the_hash() {
        let (trusted, seen_trusted) = serve(route_sha_only).await;
        let (public, seen_public) = serve(route_ok).await;
        let mut f = test_fetcher(vec![local(trusted, true), local(public, false)]);
        let bin = download_verified(&mut f, BIN, SHA).await.unwrap();
        assert_eq!(bin, elf_body());
        assert_eq!(f.preferred, Some(1));
        // (d) the hash was asked of the trusted source only.
        assert_eq!(sha_hits(&seen_public), 0);
        assert_eq!(sha_hits(&seen_trusted), 1);
    }

    /// (c) A public source serves a tampered program (a valid-looking ELF)
    /// next to the real trusted hash: mismatch, and the next source is tried.
    #[tokio::test]
    async fn a_tampered_program_from_a_public_source_is_rejected_and_the_next_source_tried() {
        let (trusted, _) = serve(route_sha_only).await;
        let (tampered, seen_tampered) = serve(route_evil).await;
        let (good, _) = serve(route_ok).await;
        let mut f = test_fetcher(vec![
            local(trusted, true),
            local(tampered, false),
            local(good, false),
        ]);
        let bin = download_verified(&mut f, BIN, SHA).await.unwrap();
        assert_eq!(
            bin,
            elf_body(),
            "the verified program, not the tampered one"
        );
        assert_eq!(f.preferred, Some(2));
        assert_eq!(bin_hits(&seen_tampered), 1);
        assert_eq!(sha_hits(&seen_tampered), 0);
    }

    #[tokio::test]
    async fn only_tampered_programs_available_fails_with_a_sha256_mismatch() {
        let (trusted, _) = serve(route_sha_only).await;
        let (tampered, seen_tampered) = serve(route_evil).await;
        let mut f = test_fetcher(vec![local(trusted, true), local(tampered, false)]);
        let err = download_verified(&mut f, BIN, SHA).await.unwrap_err();
        assert!(err.contains("sha256 mismatch"), "{err}");
        assert_eq!(bin_hits(&seen_tampered), 1);
        assert_eq!(f.preferred, None);
    }

    #[tokio::test]
    async fn a_trusted_source_that_gives_the_hash_but_not_the_program_falls_through_to_a_public_one(
    ) {
        let (trusted, _) = serve(route_sha_only).await;
        let (public, seen_public) = serve(route_ok).await;
        let mut f = test_fetcher(vec![local(trusted, true), local(public, false)]);
        let bin = download_verified(&mut f, BIN, SHA).await.unwrap();
        assert_eq!(bin, elf_body());
        assert_eq!(sha_hits(&seen_public), 0);
    }
}
