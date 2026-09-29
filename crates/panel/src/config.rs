use serde::Deserialize;

/// Sentinel value that, if seen in JWT_SECRET at startup, means the operator
/// forgot to override the example from docker-compose.yaml. We refuse to boot.
const INSECURE_JWT_SECRET: &str = "change-me-jwt-secret";

/// The compiled-in app version. Bumped per release.
///
/// **Single source of truth for the panel version.** This MUST stay in sync
/// with the entries listed in `docs/VERSIONS.md` (the version-sync checklist):
/// `crates/node/Cargo.toml`, `scripts/relay-node-install.sh` (SCRIPT_VERSION),
/// `docker-compose.release.yaml` (GHCR image tags), the README version badges,
/// and this constant.
///
/// Overridable at runtime via the `APP_VERSION` env var — used by CI to inject
/// the version into the Docker image without rebuilding. If the env var is
/// unset, the compiled-in default below is used.
const COMPILED_APP_VERSION: &str = "1.2.11";

/// Resolve the effective app version: the `APP_VERSION` env var if set,
/// otherwise the compiled-in default. Cached for the process lifetime.
pub fn app_version() -> &'static str {
    static CACHED: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    CACHED.get_or_init(|| {
        std::env::var("APP_VERSION").unwrap_or_else(|_| COMPILED_APP_VERSION.to_string())
    })
}

#[derive(Debug, Deserialize, Clone)]
pub struct Config {
    pub database_path: String,
    pub listen: String,
    // License/panel key — read from env so future endpoints can validate
    // installations, but no handler uses it yet in this MVP.
    #[allow(dead_code)]
    pub key: String,
    pub jwt_secret: String,
    pub public_dir: String,
    /// The public-facing panel URL that nodes should connect to.
    /// e.g. "http://203.0.113.10:18888" or "https://panel.example.com".
    /// If empty, the frontend falls back to window.location.origin.
    pub public_panel_url: String,
    /// Whether public self-registration (/auth/register) is allowed.
    /// Default: false (admin must create users). Set REGISTRATION_ENABLED=1
    /// to allow open registration.
    pub registration_enabled: bool,
    /// Allowed CORS origins for the API. Comma-separated, e.g.
    /// "http://localhost:5173,https://panel.example.com". Empty (default) → no
    /// CORS layer is applied at all, which is correct for the standard
    /// same-origin deployment (panel serves the built frontend itself).
    ///
    /// v0.3.9: replaces the old `CorsLayer::permissive()`, which allowed ANY
    /// origin to call the admin API with the browser's credentials. Set this
    /// only for cross-origin dev (Vite dev server) or a split frontend/backend.
    pub cors_origins: Vec<String>,
    /// v0.4.19: GeoIP enrichment (node-level region from public IP). Uses
    /// built-in primary (ipinfo.io Lite) + fallback (ipwho.is) providers;
    /// the GeoIP URL is no longer user-configurable. Caches results in KVS
    /// for `geoip_cache_ttl` seconds. Failure degrades to "未知", never
    /// blocks node status or forwarding.
    pub geoip_enabled: bool,
    pub geoip_cache_ttl: u64,
}

impl Config {
    pub fn load() -> Self {
        // v0.4.3: DATABASE_URL is the canonical env var; DATABASE_PATH is a
        // legacy fallback for deployments created before v0.4.3.  The internal
        // field name stays `database_path` to avoid a wider refactor.
        //
        // Order:
        //   1. DATABASE_URL   (e.g. postgres://user:pass@host/db or sqlite:...)
        //   2. DATABASE_PATH  (legacy, e.g. sqlite:/app/data/data.db?mode=rwc)
        //   3. Default: sqlite:data.db?mode=rwc
        let database_path = std::env::var("DATABASE_URL")
            .or_else(|_| std::env::var("DATABASE_PATH"))
            .unwrap_or_else(|_| "sqlite:data.db?mode=rwc".into());
        let listen = std::env::var("LISTEN").unwrap_or_else(|_| "0.0.0.0:18888".into());
        let key = std::env::var("PANEL_KEY").unwrap_or_else(|_| "default-key".into());
        let jwt_secret =
            std::env::var("JWT_SECRET").unwrap_or_else(|_| uuid::Uuid::new_v4().to_string());
        let public_dir = std::env::var("PUBLIC_DIR").unwrap_or_else(|_| "public".into());
        let public_panel_url = std::env::var("PUBLIC_PANEL_URL").unwrap_or_else(|_| String::new());
        let registration_enabled = std::env::var("REGISTRATION_ENABLED")
            .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
            .unwrap_or(false);
        let cors_origins = std::env::var("CORS_ORIGINS")
            .unwrap_or_default()
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>();

        // v0.4.19: GeoIP is ON by default and uses built-in primary + fallback
        // providers (ipinfo.io Lite → ipwho.is). The lookup URL is no longer
        // user-configurable — only GEOIP_ENABLED and GEOIP_CACHE_TTL remain.
        // Operators who don't want any third-party IP lookup set
        // `GEOIP_ENABLED=false` (or `0`). The lookup is still server-side
        // only, queries just the public IP, caches for GEOIP_CACHE_TTL
        // (default 7 days), and degrades to "unknown" on any failure —
        // forwarding/node status are never affected.
        let geoip_enabled = parse_geoip_enabled(std::env::var("GEOIP_ENABLED").ok());
        let geoip_cache_ttl = std::env::var("GEOIP_CACHE_TTL")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(604_800); // 7 days

        let cfg = Self {
            database_path,
            listen,
            key,
            jwt_secret,
            public_dir,
            public_panel_url,
            registration_enabled,
            cors_origins,
            geoip_enabled,
            geoip_cache_ttl,
        };
        cfg.validate();
        cfg
    }

    /// Refuse to start with an obviously-insecure JWT secret. In production
    /// (where JWT_SECRET is set from docker-compose), the placeholder value
    /// must be replaced. The random fallback above is fine for local dev
    /// because it is generated fresh each run and passes the check.
    fn validate(&self) {
        if let Some(problem) = jwt_secret_problem(&self.jwt_secret) {
            eprintln!(
                "FATAL: JWT_SECRET {}.\n  \
                 Generate one with:  openssl rand -hex 32\n  \
                 Then set JWT_SECRET in your environment / .env and restart.\n  \
                 (deploy.sh replaces a short JWT_SECRET in .env automatically.)",
                problem
            );
            std::process::exit(1);
        }
    }
}

/// v1.2.12: minimum JWT_SECRET length in bytes. Tokens are HS256-signed, so
/// anyone holding one issued token can brute-force the secret offline and then
/// mint an admin token; a short secret falls quickly. 32 bytes is the HS256 key
/// size (`openssl rand -hex 32` gives 64 characters, the uuid dev fallback 36).
const MIN_JWT_SECRET_LEN: usize = 32;

/// Why `secret` is unusable as the JWT signing key, or `None` if it is fine.
fn jwt_secret_problem(secret: &str) -> Option<String> {
    if secret.is_empty() {
        return Some("is empty".into());
    }
    if secret == INSECURE_JWT_SECRET {
        return Some(format!(
            "is still the insecure placeholder \"{INSECURE_JWT_SECRET}\""
        ));
    }
    if secret.len() < MIN_JWT_SECRET_LEN {
        return Some(format!(
            "is too short ({} bytes, at least {MIN_JWT_SECRET_LEN} required)",
            secret.len()
        ));
    }
    None
}

/// v0.4.16: parse `GEOIP_ENABLED` into a boolean. Extracted as a pure function
/// so the truth table is unit-testable.
///
/// Semantics (GeoIP defaults ON since v0.4.16):
/// - `None` (env var unset) → `true` (the new default — opt-out model)
/// - `Some("0")` / `Some("false")` (any case) → `false` (explicit opt-out)
/// - any other value → `true`
///
/// Note the inversion vs v0.4.15's opt-in logic: an UNSET var now enables
/// GeoIP, and only an explicit `0`/`false` disables it. This keeps a fat-finger
/// typo (e.g. `GEOIP_ENABLED=ttru`) from silently disabling GeoIP — it stays
/// on, matching the operator's intent of "default on".
fn parse_geoip_enabled(raw: Option<String>) -> bool {
    match raw {
        Some(v) => !(v == "0" || v.eq_ignore_ascii_case("false")),
        None => true,
    }
}

#[cfg(test)]
mod tests {
    use super::{jwt_secret_problem, parse_geoip_enabled, INSECURE_JWT_SECRET};

    /// v1.2.12: a short JWT_SECRET is brute-forceable offline from any issued
    /// token, so the panel must refuse it — not only the empty/placeholder case.
    #[test]
    fn jwt_secret_must_be_long_enough() {
        assert!(jwt_secret_problem("").is_some());
        assert!(jwt_secret_problem(INSECURE_JWT_SECRET).is_some());
        assert!(jwt_secret_problem("abc").is_some());
        assert!(jwt_secret_problem(&"a".repeat(31)).is_some());

        assert!(jwt_secret_problem(&"a".repeat(32)).is_none());
        // What deploy.sh generates (openssl rand -hex 32).
        assert!(jwt_secret_problem(&"0f".repeat(32)).is_none());
        // The dev fallback when JWT_SECRET is unset.
        assert!(jwt_secret_problem(&uuid::Uuid::new_v4().to_string()).is_none());
    }

    /// v0.4.16: pin the GEOIP_ENABLED truth table. The default flipped from
    /// false (v0.4.15, opt-in) to true (opt-out). This test guards against a
    /// future refactor that silently reverts the default or mis-parses the
    /// opt-out spellings.
    #[test]
    fn geoip_enabled_truth_table() {
        // Unset → ON (the v0.4.16 default flip).
        assert!(parse_geoip_enabled(None));

        // Explicit opt-out spellings → OFF.
        assert!(!parse_geoip_enabled(Some("false".into())));
        assert!(!parse_geoip_enabled(Some("FALSE".into())));
        assert!(!parse_geoip_enabled(Some("False".into())));
        assert!(!parse_geoip_enabled(Some("0".into())));

        // Explicit opt-in (and anything else) → ON.
        assert!(parse_geoip_enabled(Some("true".into())));
        assert!(parse_geoip_enabled(Some("1".into())));
        assert!(parse_geoip_enabled(Some("yes".into())));
        // A typo must NOT silently disable GeoIP (default-on intent).
        assert!(parse_geoip_enabled(Some("ttru".into())));
        assert!(parse_geoip_enabled(Some("".into())));
    }
}
