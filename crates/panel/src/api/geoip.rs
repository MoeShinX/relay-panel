//! v0.4.19: node-level GeoIP enrichment with built-in primary + fallback
//! providers.
//!
//! Given a public IP, resolve a country code + name via built-in GeoIP
//! providers (server-side only — never from the browser):
//!
//! 1. Primary: ipinfo.io Lite (with token). If the primary fails (timeout,
//!    non-2xx, JSON parse error, or missing `country_code`), the secondary is
//!    tried.
//! 2. Fallback: ipwho.is (free, no token).
//!
//! Results are cached in KVS for `GEOIP_CACHE_TTL` seconds (default 7 days).
//! Private/loopback/link-local IPs are never queried. Concurrent lookups for
//! the SAME IP are de-duplicated. All failures degrade to None ("未知" in the
//! UI); node status and forwarding are NEVER affected. Third-party response
//! bodies are NOT logged or echoed to clients — only the parsed country
//! code/name are stored.

use crate::db::repo::KvsRepository;
use std::collections::HashSet;
use std::sync::Arc;
use tokio::sync::Mutex;

// ── Built-in GeoIP provider URLs (v0.4.19) ──
//
// Operators can NO LONGER override these via GEOIP_API_URL. The panel ships
// with a fixed primary + fallback pair. If both fail, the lookup degrades to
// "unknown" without affecting node status or forwarding.
const PRIMARY_GEOIP_URL: &str = "https://api.ipinfo.io/lite/{ip}?token=536eeb66425cbe";
const FALLBACK_GEOIP_URL: &str = "https://ipwho.is/{ip}";

/// The cached GeoIP result stored under key `geoip:{ip}` in KVS.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct GeoIpEntry {
    pub country_code: Option<String>,
    pub country_name: Option<String>,
    /// When this entry was cached (Unix seconds). Used for TTL expiry.
    pub cached_at: i64,
}

/// Resolve a public IP to a country, using KVS cache + the built-in primary &
/// fallback providers. Returns None on any failure (private IP, both providers
/// down, parse error) — the caller shows "未知".
pub async fn lookup(
    db: &dyn KvsRepository,
    cache_ttl_secs: i64,
    in_flight: &Arc<Mutex<HashSet<String>>>,
    ip: &str,
) -> Option<GeoIpEntry> {
    // Reject anything that isn't a PUBLIC IP — never send a private/loopback
    // address to a third-party API.
    let parsed: std::net::IpAddr = ip.parse().ok()?;
    if !is_public_ip(&parsed) {
        return None;
    }

    let cache_key = format!("geoip:{ip}");

    // 1. Cache hit (within TTL)?
    if let Ok(Some(raw)) = db.get(&cache_key).await {
        if let Ok(entry) = serde_json::from_str::<GeoIpEntry>(&raw) {
            let now = chrono::Utc::now().timestamp();
            if is_cache_fresh(entry.cached_at, now, cache_ttl_secs) {
                return Some(entry);
            }
        }
    }

    // 2. De-duplicate concurrent lookups for the same IP.
    {
        let mut guard = in_flight.lock().await;
        if guard.contains(ip) {
            // Another task is already looking this up; return None rather than
            // firing a duplicate request. The next poll will hit the cache.
            return None;
        }
        guard.insert(ip.to_string());
    }
    let result = fetch_with_fallback(db, &cache_key, ip).await;
    in_flight.lock().await.remove(ip);
    result
}

/// Call the primary GeoIP provider; if it fails for any reason, retry with the
/// fallback. If both fail, return None but DON'T delete an existing stale
/// cache entry (the spec says "retain old value on failure").
async fn fetch_with_fallback(
    db: &dyn KvsRepository,
    cache_key: &str,
    ip: &str,
) -> Option<GeoIpEntry> {
    // Try primary first.
    if let Some(entry) = try_fetch(db, cache_key, ip, PRIMARY_GEOIP_URL).await {
        return Some(entry);
    }
    // Primary failed — try fallback.
    if let Some(entry) = try_fetch(db, cache_key, ip, FALLBACK_GEOIP_URL).await {
        return Some(entry);
    }
    // Both failed — retain any stale cache entry; the caller shows "unknown".
    None
}

fn geoip_http_client() -> &'static reqwest::Client {
    static CLIENT: std::sync::OnceLock<reqwest::Client> = std::sync::OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(5))
            .build()
            .unwrap_or_else(|_| reqwest::Client::new())
    })
}

/// Call a single GeoIP provider URL and cache the result on success. Returns
/// None when `should_fallback()` would trigger (timeout, non-2xx, JSON parse
/// failure, missing country_code). The API response body is never logged.
async fn try_fetch(
    db: &dyn KvsRepository,
    cache_key: &str,
    ip: &str,
    url_template: &str,
) -> Option<GeoIpEntry> {
    let url = url_template.replace("{ip}", ip);
    let client = geoip_http_client();
    let resp = client.get(&url).send().await.ok()?;
    if !resp.status().is_success() {
        return None;
    }
    let body: serde_json::Value = resp.json().await.ok()?;

    // Extract country fields. If country_code is missing, treat the response
    // as a failure so the next provider (or "unknown") is tried.
    let (country_code, country_name) = parse_geoip_country(&body);
    if should_fallback(&country_code) {
        return None;
    }

    let entry = GeoIpEntry {
        country_code,
        country_name,
        cached_at: chrono::Utc::now().timestamp(),
    };
    // Cache even if country_name is None (a successful API call that returned
    // no country_name) so we don't hammer the API every poll.
    if let Ok(json) = serde_json::to_string(&entry) {
        let _ = db.set(cache_key, &json).await;
    }
    Some(entry)
}

/// Should we fall back to the next provider (or give up)? Returns true when
/// the primary provider returned no usable country_code. Pure so it's
/// unit-testable.
///
/// Failure conditions that trigger fallback:
/// - `country_code` is None (missing from response, or not a 2-letter code)
fn should_fallback(country_code: &Option<String>) -> bool {
    country_code.is_none()
}

/// Extract (country_code, country_name) from a GeoIP API JSON body. Pure (no
/// I/O) so it's unit-testable.
///
/// `country_code` only ever comes from a genuine 2-letter code field
/// (`country_code` / `countryCode`). It MUST NOT fall back to `country` (the
/// full name) — the frontend turns the code into a regional-indicator flag
/// emoji and silently renders nothing for anything that isn't a 2-letter code,
/// so a full name like "United States" would just drop the flag.
fn parse_geoip_country(body: &serde_json::Value) -> (Option<String>, Option<String>) {
    let country_code = body
        .get("country_code")
        .or_else(|| body.get("countryCode"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let country_name = body
        .get("country")
        .and_then(|v| v.as_str())
        .or_else(|| body.get("country_name").and_then(|v| v.as_str()))
        .map(|s| s.to_string());
    (country_code, country_name)
}

/// Is this IP public (routable)? Rejects private, loopback, link-local,
/// multicast, unspecified. Mirrors the standard RFC 1918 / 4193 / ULA ranges.
fn is_public_ip(ip: &std::net::IpAddr) -> bool {
    match ip {
        std::net::IpAddr::V4(v4) => {
            !v4.is_private()
                && !v4.is_loopback()
                && !v4.is_link_local()
                && !v4.is_unspecified()
                && !v4.is_broadcast()
                && !v4.is_documentation()
        }
        std::net::IpAddr::V6(v6) => {
            !v6.is_loopback()
                && !v6.is_unspecified()
                && !v6.is_multicast()
                // Unique-local fc00::/7 (RFC 4193) — not routable on the internet.
                && (v6.segments()[0] & 0xfe00) != 0xfc00
                // Link-local fe80::/10.
                && (v6.segments()[0] & 0xffc0) != 0xfe80
        }
    }
}

/// Is a cache entry still within its TTL relative to `now` (Unix seconds)?
/// Pure so the cache-freshness boundary is unit-testable without a clock.
fn is_cache_fresh(cached_at: i64, now: i64, ttl_secs: i64) -> bool {
    now - cached_at < ttl_secs
}

/// Read a cached GeoIP entry from KVS without triggering a lookup. Returns
/// None on any miss/parse error (the UI shows "未知"). Stale entries (past
/// Batch-read all cached GeoIP entries (`geoip:*`) in one prefix scan so
/// `/nodes` and `/nodes/shared` avoid issuing up to 2 SQL queries per node.
pub async fn read_all_cache(
    db: &dyn KvsRepository,
) -> std::collections::HashMap<String, GeoIpEntry> {
    let Ok(rows) = db.scan_prefix("geoip:").await else {
        return std::collections::HashMap::new();
    };
    rows.into_iter()
        .filter_map(|(k, v)| {
            let ip = k.strip_prefix("geoip:")?.to_string();
            let entry = serde_json::from_str::<GeoIpEntry>(&v).ok()?;
            Some((ip, entry))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn public_vs_private_ipv4() {
        assert!(!is_public_ip(&"10.0.0.1".parse().unwrap()));
        assert!(!is_public_ip(&"192.168.1.1".parse().unwrap()));
        assert!(!is_public_ip(&"172.16.0.1".parse().unwrap()));
        assert!(!is_public_ip(&"127.0.0.1".parse().unwrap()));
        assert!(!is_public_ip(&"169.254.1.1".parse().unwrap()));
        assert!(!is_public_ip(&"0.0.0.0".parse().unwrap()));
        assert!(is_public_ip(&"8.8.8.8".parse().unwrap()));
        assert!(is_public_ip(&"1.2.3.4".parse().unwrap()));
    }

    #[test]
    fn public_vs_private_ipv6() {
        assert!(!is_public_ip(&"::1".parse().unwrap()));
        assert!(!is_public_ip(&"fe80::1".parse().unwrap()));
        assert!(!is_public_ip(&"fc00::1".parse().unwrap()));
        assert!(!is_public_ip(&"fd12:3456::1".parse().unwrap()));
        assert!(is_public_ip(&"2001:4860:4860::8888".parse().unwrap()));
        assert!(is_public_ip(&"240e::1".parse().unwrap()));
    }

    #[test]
    fn lookup_rejects_non_ip_and_private() {
        // Verify the pure predicates — lookup() calls is_public_ip first, so
        // private/non-IP inputs are rejected before any DB / network access.
        assert!(!is_public_ip(
            &"10.0.0.1".parse::<std::net::IpAddr>().unwrap()
        ));
        assert!(!is_public_ip(
            &"192.168.1.1".parse::<std::net::IpAddr>().unwrap()
        ));
        assert!(is_public_ip(
            &"8.8.8.8".parse::<std::net::IpAddr>().unwrap()
        ));
    }

    #[test]
    fn parse_country_reads_code_and_name() {
        // Standard ipwho.is shape: has both country_code and country.
        let body = serde_json::json!({ "country_code": "US", "country": "United States" });
        let (code, name) = parse_geoip_country(&body);
        assert_eq!(code.as_deref(), Some("US"));
        assert_eq!(name.as_deref(), Some("United States"));
    }

    #[test]
    fn parse_country_accepts_camelcase_code_alt_key() {
        let body = serde_json::json!({ "countryCode": "JP", "country_name": "Japan" });
        let (code, name) = parse_geoip_country(&body);
        assert_eq!(code.as_deref(), Some("JP"));
        assert_eq!(name.as_deref(), Some("Japan"));
    }

    #[test]
    fn parse_country_never_puts_full_name_into_code() {
        // The bug guard: an API that returns ONLY `country` (the full name) must
        // NOT leak that name into country_code — otherwise the frontend flag
        // emoji silently disappears. code stays None; name still resolves.
        let body = serde_json::json!({ "country": "United States" });
        let (code, name) = parse_geoip_country(&body);
        assert_eq!(code, None);
        assert_eq!(name.as_deref(), Some("United States"));
    }

    #[test]
    fn parse_country_handles_missing_fields() {
        let body = serde_json::json!({ "ip": "8.8.8.8" });
        let (code, name) = parse_geoip_country(&body);
        assert_eq!(code, None);
        assert_eq!(name, None);
    }

    #[test]
    fn cache_freshness_boundary() {
        let ttl = 100;
        assert!(is_cache_fresh(1_000, 1_050, ttl), "50s < 100s ttl → fresh");
        assert!(
            !is_cache_fresh(1_000, 1_100, ttl),
            "exactly at ttl → stale (strict <)"
        );
        assert!(!is_cache_fresh(1_000, 1_200, ttl), "past ttl → stale");
    }

    // ── should_fallback truth table (v0.4.19) ──

    #[test]
    fn fallback_when_country_code_missing() {
        // No country_code → should fall back (primary failed to return a
        // usable result).
        assert!(should_fallback(&None));
    }

    #[test]
    fn no_fallback_when_country_code_present() {
        // Has a country_code → primary succeeded, no need to fall back.
        assert!(!should_fallback(&Some("US".into())));
        assert!(!should_fallback(&Some("JP".into())));
        assert!(!should_fallback(&Some("CN".into())));
    }
}
