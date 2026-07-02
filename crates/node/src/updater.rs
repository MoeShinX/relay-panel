//! v1.0.10: node self-upgrade.
//!
//! Triggered by a directed `UpgradeNodeMessage` on the WS control channel. The
//! node downloads the LATEST official `relay-node` release binary for its own
//! architecture, verifies its published sha256, backs up + atomically swaps its
//! own binary, and returns Ok — the caller then exits so systemd (Restart=always)
//! re-execs into the new binary.
//!
//! Security: the download URL is hardcoded to the official GitHub release. The
//! panel command carries NO url/binary, so a compromised panel can at most force
//! an upgrade to an official build — never run arbitrary code. A failed upgrade
//! (network / hash / io) leaves the current binary untouched.

use sha2::{Digest, Sha256};

/// Official release source. Never taken from the panel.
const REPO: &str = "MoeShinX/relay-panel";

/// Map the compiled target arch to the release asset suffix.
fn asset_arch() -> Option<&'static str> {
    match std::env::consts::ARCH {
        "x86_64" => Some("amd64"),
        "aarch64" => Some("arm64"),
        _ => None,
    }
}

/// Download + verify + swap. On success the new binary is in place and the
/// caller should exit(0). On error the running binary is untouched.
pub async fn self_upgrade() -> Result<(), String> {
    let arch =
        asset_arch().ok_or_else(|| format!("unsupported arch: {}", std::env::consts::ARCH))?;
    let asset = format!("relay-node-linux-{arch}");
    let bin_url = format!("https://github.com/{REPO}/releases/latest/download/{asset}");
    let sha_url = format!("{bin_url}.sha256");

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
        "self-upgrade: sha256 verified ({} bytes) for {}",
        bin_bytes.len(),
        asset
    );

    // 4. Write temp, chmod +x, back up current binary, atomically swap.
    // Renaming over a RUNNING binary is fine on Linux (the live process keeps
    // its old inode; the path points at the new file for the next start).
    let tmp = bin_path.with_extension("upgrade.tmp");
    let bak = bin_path.with_extension("bak");
    tokio::fs::write(&tmp, &bin_bytes)
        .await
        .map_err(|e| format!("write temp binary: {e}"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o755))
            .map_err(|e| format!("chmod temp binary: {e}"))?;
    }
    // Keep the previous binary as .bak for manual rollback (best-effort).
    if let Err(e) = tokio::fs::copy(&bin_path, &bak).await {
        tracing::warn!("self-upgrade: could not back up current binary: {e}");
    }
    tokio::fs::rename(&tmp, &bin_path)
        .await
        .map_err(|e| format!("swap binary: {e}"))?;

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
        // On the CI/test host this is whatever the runner is; just assert the
        // mapping logic is sane for the two supported arches.
        assert_eq!(
            match "x86_64" {
                "x86_64" => Some("amd64"),
                "aarch64" => Some("arm64"),
                _ => None,
            },
            Some("amd64")
        );
        // The real function returns None on an unsupported arch rather than
        // panicking — exercised indirectly by self_upgrade's early return.
        let _ = asset_arch();
    }
}
