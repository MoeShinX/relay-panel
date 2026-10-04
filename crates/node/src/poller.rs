use crate::config::NodeConfig;
use relay_shared::protocol::{NodeConfigResponse, CONFIG_PROTOCOL_VERSION};
use std::path::PathBuf;

/// Path for the config cache file. Used when the panel is unreachable.
const CACHE_FILE: &str = "config-cache.json";

/// File holding this node's stable identity. Generated once on first start
/// (a random hex string) and reused forever after, so the panel can tell
/// multiple nodes sharing one group token apart (fixes status overwrite:
/// node_status:{group_id} was a single key overwritten by every node).
const NODE_ID_FILE: &str = "node-id";

/// v0.4.0: outcome of a config fetch, distinguishing a permanent protocol
/// mismatch (426) from a transient failure (network/5xx). The caller uses this
/// to decide the poll interval: 426 → long backoff (upgrade needed), transient
/// → keep the normal interval.
pub enum FetchResult {
    /// A valid config was received and cached.
    Ok(NodeConfigResponse),
    /// The panel reports a permanent config-protocol mismatch (426). The node
    /// keeps its cached config; the caller should back off (the only fix is an
    /// upgrade, so polling fast is pointless).
    ProtocolMismatch,
    /// Transient failure (network error, 5xx, non-JSON body). The caller keeps
    /// the cached config and retries on the normal interval.
    Transient,
}

pub async fn fetch_config(config: &NodeConfig) -> FetchResult {
    let url = format!("{}/api/v1/node/config", config.panel_url);
    let client = reqwest::Client::new();

    let resp = match client
        .get(&url)
        .header("Authorization", format!("Bearer {}", config.token))
        // v0.4.0: send our config-protocol version so the panel can refuse to
        // send config we can't deserialize (keeps old nodes on their cached
        // config instead of crashing on unknown fields/enum variants).
        .header("X-Config-Protocol-Version", CONFIG_PROTOCOL_VERSION)
        .timeout(std::time::Duration::from_secs(5))
        .send()
        .await
    {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!("fetch_config: network error: {}", e);
            return FetchResult::Transient;
        }
    };

    let status = resp.status();
    if status == reqwest::StatusCode::UPGRADE_REQUIRED {
        // Permanent: the panel's config protocol doesn't match ours. Parse the
        // structured body for a clear log line, then back off.
        let body: serde_json::Value = resp.json().await.unwrap_or_default();
        let required = body.get("required").and_then(|v| v.as_u64());
        tracing::warn!(
            required = ?required,
            "fetch_config: config protocol mismatch (panel requires v{:?}, node has v{}); \
             keeping cached config — upgrade relay-node",
            required,
            CONFIG_PROTOCOL_VERSION
        );
        return FetchResult::ProtocolMismatch;
    }
    if !status.is_success() {
        tracing::warn!(status = %status, "fetch_config: non-2xx response; keeping cached config");
        return FetchResult::Transient;
    }

    match resp.json::<NodeConfigResponse>().await {
        Ok(cfg) => {
            save_cache(&cfg);
            FetchResult::Ok(cfg)
        }
        Err(e) => {
            tracing::warn!("fetch_config: response parse failed: {}", e);
            FetchResult::Transient
        }
    }
}

/// Load cached config from config-cache.json.
/// Returns None if file doesn't exist or is corrupt.
pub fn load_cache() -> Option<NodeConfigResponse> {
    let path = cache_path();
    let data = std::fs::read_to_string(&path).ok()?;
    let resp: NodeConfigResponse = serde_json::from_str(&data).ok()?;
    tracing::info!(
        "Loaded cached config from {} ({} listeners)",
        path.display(),
        resp.listeners.len()
    );
    Some(resp)
}

/// Save config to config-cache.json (next to the binary or in working dir).
fn save_cache(config: &NodeConfigResponse) {
    let path = cache_path();
    match serde_json::to_string_pretty(config) {
        Ok(json) => {
            if let Err(e) = std::fs::write(&path, json) {
                tracing::warn!("Failed to write config cache to {}: {}", path.display(), e);
            }
        }
        Err(e) => {
            tracing::warn!("Failed to serialize config cache: {}", e);
        }
    }
}

fn cache_path() -> PathBuf {
    state_dir(std::env::var_os("RELAY_NODE_DIR")).join(CACHE_FILE)
}

/// Resolve where the node-id file lives — same directory as cache_path so the
/// two files sit together.
fn node_id_path() -> PathBuf {
    state_dir(std::env::var_os("RELAY_NODE_DIR")).join(NODE_ID_FILE)
}

/// Directory holding the node's state files. v1.2.12: `RELAY_NODE_DIR`, which
/// the installer writes into start.sh, wins — before it, a second instance
/// installed with `-s` (into /opt/<name>) still used /opt/relay-node whenever
/// that existed, sharing the first instance's node id and config cache. Without
/// it (start.sh from older installers) the old order stays: /opt/relay-node
/// if present (production), else the current directory (dev).
fn state_dir(relay_node_dir: Option<std::ffi::OsString>) -> PathBuf {
    if let Some(dir) = relay_node_dir.filter(|d| !d.is_empty()) {
        return PathBuf::from(dir);
    }
    let prod = PathBuf::from("/opt/relay-node");
    if prod.exists() {
        return prod;
    }
    PathBuf::new()
}

/// Get this node's stable identity, generating + persisting it on first call.
///
/// The id is a random hex string generated once and reused across restarts, so
/// the panel can distinguish multiple physical nodes that share one inbound
/// group token (each gets its own node_status:{group_id}:{node_id} key instead
/// of all overwriting node_status:{group_id}).
///
/// Generation uses the OS random source via std; we deliberately do NOT derive
/// it from hostname/MAC (those can change/DHCP) — a stable random id is the
/// contract the panel's status dedup depends on.
pub fn get_or_create_node_id() -> String {
    get_or_create_node_id_at(&node_id_path())
}

/// Inner implementation taking an explicit path, so it's unit-testable without
/// touching the real /opt/relay-node or cwd.
fn get_or_create_node_id_at(path: &std::path::Path) -> String {
    // Try to load an existing id first.
    if let Ok(existing) = std::fs::read_to_string(path) {
        let trimmed = existing.trim();
        if !trimmed.is_empty() {
            return trimmed.to_string();
        }
    }
    // No id yet: generate one (16 random bytes → 32 hex chars). std's
    // fill_bytes uses the OS CSPRNG; we don't need cryptographic strength but
    // it's the most portable "good enough random" available without extra deps.
    // /dev/urandom on Linux (the only supported platform); fall back to a
    // time+pid-based id if unavailable so the node still boots.
    let id = match random_hex_16() {
        Some(id) => id,
        None => {
            tracing::warn!("could not read /dev/urandom for node_id; using fallback");
            fallback_id()
        }
    };
    if let Err(e) = std::fs::write(path, &id) {
        tracing::warn!("failed to persist node_id to {}: {}", path.display(), e);
        // Non-fatal: we return the in-memory id; it'll regenerate next start,
        // which means status may flap for this node until the file is writable.
    } else {
        tracing::info!("generated node_id {} -> {}", id, path.display());
    }
    id
}

/// 16 random bytes from /dev/urandom as 32 hex characters, or `None` where
/// that cannot be read (not Linux).
pub(crate) fn random_hex_16() -> Option<String> {
    use std::io::Read;
    let mut bytes = [0u8; 16];
    std::fs::File::open("/dev/urandom")
        .and_then(|mut f| f.read_exact(&mut bytes))
        .ok()?;
    Some(hex_encode(&bytes))
}

fn hex_encode(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{:02x}", b));
    }
    s
}

/// Fallback id when /dev/urandom is unavailable. Not random, but unique enough
/// per (host, pid, time) to avoid collisions in practice — and only used on
/// broken systems where /dev/urandom is missing (shouldn't happen on Linux).
fn fallback_id() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("node-{}-{}", std::process::id(), now)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// v1.2.12: an instance installed with `-s` keeps its state in its own
    /// install dir (RELAY_NODE_DIR from start.sh), not in /opt/relay-node.
    #[test]
    fn state_dir_follows_relay_node_dir() {
        assert_eq!(
            state_dir(Some("/opt/relay-node-hk".into())),
            PathBuf::from("/opt/relay-node-hk")
        );
        // Unset or empty (start.sh from an older installer): the old lookup.
        let legacy = if PathBuf::from("/opt/relay-node").exists() {
            PathBuf::from("/opt/relay-node")
        } else {
            PathBuf::new()
        };
        assert_eq!(state_dir(None), legacy);
        assert_eq!(state_dir(Some("".into())), legacy);
    }

    /// A node_id generated once must be reused verbatim on every subsequent
    /// call — this stability is the contract the panel's status dedup depends
    /// on. If this breaks, a restarting node would look like a NEW node and its
    /// old status entry would stale forever.
    #[test]
    fn node_id_is_stable_across_calls() {
        let dir = std::env::temp_dir();
        let path = dir.join(format!(
            "relaypanel-test-nodeid-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let first = get_or_create_node_id_at(&path);
        let second = get_or_create_node_id_at(&path);
        assert!(!first.is_empty(), "first id must be non-empty");
        assert_eq!(
            first, second,
            "node_id must be stable: a restart must reuse the persisted id"
        );
        // The file must exist and hold exactly the id (so it survives a real
        // process restart, not just in-memory caching).
        let on_disk = std::fs::read_to_string(&path).unwrap();
        assert_eq!(on_disk.trim(), first);
        let _ = std::fs::remove_file(&path);
    }

    /// Two different nodes (different id files) must get DIFFERENT ids. This is
    /// what lets the panel tell them apart — if they collided, the status
    /// overwrite bug would be back.
    #[test]
    fn distinct_nodes_get_distinct_ids() {
        let dir = std::env::temp_dir();
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path_a = dir.join(format!("relaypanel-test-nodeid-a-{}", stamp));
        let path_b = dir.join(format!("relaypanel-test-nodeid-b-{}", stamp));
        let a = get_or_create_node_id_at(&path_a);
        let b = get_or_create_node_id_at(&path_b);
        assert_ne!(a, b, "two fresh nodes must not share an id");
        let _ = std::fs::remove_file(&path_a);
        let _ = std::fs::remove_file(&path_b);
    }

    /// A pre-existing node-id file must be honored as-is (an operator who set
    /// a specific id, or a node restored from backup, keeps that identity).
    #[test]
    fn existing_node_id_file_is_honored() {
        let dir = std::env::temp_dir();
        let path = dir.join(format!(
            "relaypanel-test-nodeid-existing-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::write(&path, "my-fixed-id-12345").unwrap();
        let id = get_or_create_node_id_at(&path);
        assert_eq!(id, "my-fixed-id-12345");
        let _ = std::fs::remove_file(&path);
    }
}
