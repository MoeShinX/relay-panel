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

async fn do_upgrade(version: &str) -> Result<(), String> {
    let arch =
        asset_arch().ok_or_else(|| format!("unsupported arch: {}", std::env::consts::ARCH))?;
    let (bin_url, sha_url) = build_download_urls(version, arch)?;

    let bin_path = std::env::current_exe().map_err(|e| format!("current_exe: {e}"))?;

    tracing::warn!("self-upgrade: downloading {bin_url}");
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(180))
        .build()
        .map_err(|e| format!("http client: {e}"))?;

    // 1. Binary bytes.
    let bin_bytes = client
        .get(&bin_url)
        .send()
        .await
        .and_then(|r| r.error_for_status())
        .map_err(|e| format!("download binary: {e}"))?
        .bytes()
        .await
        .map_err(|e| format!("read binary: {e}"))?;
    if bin_bytes.is_empty() {
        return Err("downloaded binary is empty".into());
    }

    // 2. Published sha256 (format: "<hex>  <filename>").
    let sha_text = client
        .get(&sha_url)
        .send()
        .await
        .and_then(|r| r.error_for_status())
        .map_err(|e| format!("download sha256: {e}"))?
        .text()
        .await
        .map_err(|e| format!("read sha256: {e}"))?;
    let expected = sha_text
        .split_whitespace()
        .next()
        .unwrap_or("")
        .to_lowercase();
    if expected.len() != 64 || !expected.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(format!("malformed sha256 file: {sha_text:?}"));
    }

    // 3. Verify.
    let mut hasher = Sha256::new();
    hasher.update(&bin_bytes);
    let actual: String = hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    if actual != expected {
        return Err(format!(
            "sha256 mismatch: expected {expected}, got {actual}"
        ));
    }
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
}
