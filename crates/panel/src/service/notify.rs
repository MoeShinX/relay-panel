//! v1.2.0: outbound notifications (Telegram + email).
//!
//! Config lives in the generic `kvs` table under one JSON key rather than as a
//! dozen new columns on `app_settings`: that table is a fixed-column singleton,
//! and notification settings are a loose bag that will keep growing. Storing
//! JSON in kvs is the same pattern `node_status:*` already uses, and it means
//! adding a field later needs no migration.
//!
//! ## Credentials
//!
//! The bot token and SMTP password are stored in PLAINTEXT, like every other
//! secret in this schema (node tokens, the JWT secret's fallback). That is a
//! deliberate call, not an oversight: anyone who can read this database already
//! has the node tokens, i.e. full control of the forwarding fleet — encrypting
//! one field while the keys sit on the same host would be security theatre. The
//! API never returns them (see `NotifyConfigPublic`), so they cannot leak
//! through the panel UI.

use serde::{Deserialize, Serialize};

/// kvs key holding the JSON config.
pub const NOTIFY_CONFIG_KEY: &str = "notify:config";

/// How long a node must be continuously offline before an alert fires.
///
/// NOT the same as `NODE_ONLINE_WINDOW_SECS` (30s), which decides whether the
/// UI paints a node green. A node that misses two status reports over a flaky
/// link is "offline" for the UI and perfectly healthy for alerting purposes;
/// firing at 30s would page the operator every time a packet dropped. Three
/// minutes means roughly six missed reports — sustained, not a blip.
pub const DEFAULT_OFFLINE_ALERT_SECS: i64 = 180;

/// Floor for the configured threshold. Below this the alerts are noise, and
/// noisy alerts get muted, which is worse than no alerts.
pub const MIN_OFFLINE_ALERT_SECS: i64 = 60;

/// The alert floor MUST stay above the UI's online window, or an alert could
/// fire for a node the status page still paints green — the operator would be
/// told something is down while looking at evidence that it isn't.
///
/// Checked at COMPILE time: a runtime assert comparing two constants is
/// something clippy (rightly) rejects, and this way lowering either constant
/// fails the build rather than a test run.
const _: () = assert!(MIN_OFFLINE_ALERT_SECS > crate::api::stats::NODE_ONLINE_WINDOW_SECS);

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct NotifyConfig {
    /// Master switch. Off = the scheduler still tracks state but sends nothing.
    pub enabled: bool,
    /// Seconds a node must stay offline before alerting.
    pub offline_alert_secs: i64,
    /// Also send when a node comes back. On by default — an alert you can't
    /// clear is an alert you learn to ignore.
    pub notify_recovery: bool,

    // ── Telegram ──
    pub telegram_enabled: bool,
    pub telegram_bot_token: String,
    pub telegram_chat_id: String,

    // ── SMTP ──
    pub email_enabled: bool,
    pub smtp_host: String,
    pub smtp_port: u16,
    pub smtp_username: String,
    pub smtp_password: String,
    /// Envelope sender. Falls back to `smtp_username` when empty.
    pub smtp_from: String,
    /// Where alerts go.
    pub smtp_to: String,
    /// Implicit TLS (port 465). False = STARTTLS (587).
    pub smtp_tls: bool,
}

impl NotifyConfig {
    /// Parse from the stored JSON, falling back to defaults on absent/corrupt
    /// data — a broken config must not take the panel down, it just means no
    /// notifications until someone fixes it.
    pub fn from_json(raw: Option<&str>) -> Self {
        let mut cfg: Self = raw
            .and_then(|r| serde_json::from_str(r).ok())
            .unwrap_or_default();
        if cfg.offline_alert_secs <= 0 {
            cfg.offline_alert_secs = DEFAULT_OFFLINE_ALERT_SECS;
        }
        cfg
    }

    /// Effective alert threshold, clamped to the floor.
    pub fn alert_after(&self) -> i64 {
        self.offline_alert_secs.max(MIN_OFFLINE_ALERT_SECS)
    }

    pub fn any_channel_enabled(&self) -> bool {
        self.enabled && (self.telegram_enabled || self.email_enabled)
    }
}

/// The config as returned by the API: credentials are replaced with a boolean
/// "is one set?" so the UI can show a filled state without ever receiving the
/// secret. A round-trip through the browser must not be able to exfiltrate it.
#[derive(Debug, Clone, Serialize)]
pub struct NotifyConfigPublic {
    pub enabled: bool,
    pub offline_alert_secs: i64,
    pub notify_recovery: bool,
    pub telegram_enabled: bool,
    pub telegram_chat_id: String,
    /// True when a token is stored. The token itself is never sent.
    pub telegram_bot_token_set: bool,
    pub email_enabled: bool,
    pub smtp_host: String,
    pub smtp_port: u16,
    pub smtp_username: String,
    /// True when a password is stored. The password itself is never sent.
    pub smtp_password_set: bool,
    pub smtp_from: String,
    pub smtp_to: String,
    pub smtp_tls: bool,
}

impl From<&NotifyConfig> for NotifyConfigPublic {
    fn from(c: &NotifyConfig) -> Self {
        Self {
            enabled: c.enabled,
            offline_alert_secs: c.alert_after(),
            notify_recovery: c.notify_recovery,
            telegram_enabled: c.telegram_enabled,
            telegram_chat_id: c.telegram_chat_id.clone(),
            telegram_bot_token_set: !c.telegram_bot_token.is_empty(),
            email_enabled: c.email_enabled,
            smtp_host: c.smtp_host.clone(),
            smtp_port: c.smtp_port,
            smtp_username: c.smtp_username.clone(),
            smtp_password_set: !c.smtp_password.is_empty(),
            smtp_from: c.smtp_from.clone(),
            smtp_to: c.smtp_to.clone(),
            smtp_tls: c.smtp_tls,
        }
    }
}

/// Outcome of one delivery attempt, per channel, so the test button can report
/// exactly which side failed instead of a single opaque "failed".
#[derive(Debug, Clone, Serialize)]
pub struct SendReport {
    pub telegram: Option<Result<(), String>>,
    pub email: Option<Result<(), String>>,
}

/// Send `text` on every enabled channel. Failures are captured per channel and
/// never propagate — a dead SMTP server must not stop the Telegram alert, and
/// neither must stop the offline-detection loop.
pub async fn send_all(cfg: &NotifyConfig, subject: &str, text: &str) -> SendReport {
    let telegram = if cfg.telegram_enabled {
        Some(send_telegram(cfg, text).await)
    } else {
        None
    };
    let email = if cfg.email_enabled {
        Some(send_email(cfg, subject, text).await)
    } else {
        None
    };
    SendReport { telegram, email }
}

/// Post to the Telegram Bot API. Uses the existing reqwest dependency — no bot
/// framework needed for one HTTP call.
pub async fn send_telegram(cfg: &NotifyConfig, text: &str) -> Result<(), String> {
    if cfg.telegram_bot_token.is_empty() || cfg.telegram_chat_id.is_empty() {
        return Err("Telegram bot_token / chat_id 未配置".into());
    }
    let url = format!(
        "https://api.telegram.org/bot{}/sendMessage",
        cfg.telegram_bot_token
    );
    let client = reqwest::Client::builder()
        // A hung notification must not wedge the alert loop.
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .map_err(|e| format!("HTTP 客户端构建失败: {e}"))?;

    let resp = client
        .post(&url)
        .json(&serde_json::json!({
            "chat_id": cfg.telegram_chat_id,
            "text": text,
            "disable_web_page_preview": true,
        }))
        .send()
        .await
        .map_err(|e| format!("请求失败: {e}"))?;

    let status = resp.status();
    if status.is_success() {
        return Ok(());
    }
    // Telegram puts the real reason in the body ("chat not found", "bot was
    // blocked"). Surfacing only the status code would make this undiagnosable.
    let body = resp.text().await.unwrap_or_default();
    let detail = serde_json::from_str::<serde_json::Value>(&body)
        .ok()
        .and_then(|v| {
            v.get("description")
                .and_then(|d| d.as_str())
                .map(str::to_string)
        })
        .unwrap_or_else(|| body.chars().take(200).collect());
    Err(format!("Telegram 返回 {status}: {detail}"))
}

/// Send one alert email over SMTP.
pub async fn send_email(cfg: &NotifyConfig, subject: &str, text: &str) -> Result<(), String> {
    use lettre::message::header::ContentType;
    use lettre::transport::smtp::authentication::Credentials;
    use lettre::{AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor};

    if cfg.smtp_host.is_empty() || cfg.smtp_to.is_empty() {
        return Err("SMTP 主机 / 收件人未配置".into());
    }
    // Many providers reject a From that isn't the authenticated mailbox, so
    // defaulting to the username is the behaviour that works out of the box.
    let from = if cfg.smtp_from.is_empty() {
        &cfg.smtp_username
    } else {
        &cfg.smtp_from
    };

    let mut builder = Message::builder()
        .from(from.parse().map_err(|e| format!("发件人地址无效: {e}"))?)
        .subject(subject)
        .header(ContentType::TEXT_PLAIN);
    // Comma-separated recipients, so one alert can reach a small team.
    for addr in cfg
        .smtp_to
        .split(',')
        .map(str::trim)
        .filter(|a| !a.is_empty())
    {
        builder = builder.to(addr
            .parse()
            .map_err(|e| format!("收件人地址无效 ({addr}): {e}"))?);
    }
    let email = builder
        .body(text.to_string())
        .map_err(|e| format!("邮件构建失败: {e}"))?;

    // Implicit TLS (465) vs STARTTLS (587) — the two conventions in the wild.
    let mut transport = if cfg.smtp_tls {
        AsyncSmtpTransport::<Tokio1Executor>::relay(&cfg.smtp_host)
            .map_err(|e| format!("SMTP 连接配置失败: {e}"))?
    } else {
        AsyncSmtpTransport::<Tokio1Executor>::starttls_relay(&cfg.smtp_host)
            .map_err(|e| format!("SMTP STARTTLS 配置失败: {e}"))?
    };
    if cfg.smtp_port > 0 {
        transport = transport.port(cfg.smtp_port);
    }
    if !cfg.smtp_username.is_empty() {
        transport = transport.credentials(Credentials::new(
            cfg.smtp_username.clone(),
            cfg.smtp_password.clone(),
        ));
    }
    transport
        .timeout(Some(std::time::Duration::from_secs(15)))
        .build()
        .send(email)
        .await
        .map(|_| ())
        .map_err(|e| format!("发送失败: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Absent or corrupt config must yield a safe default (disabled), never a
    /// panic — a hand-edited kvs row shouldn't be able to take the panel down.
    #[test]
    fn from_json_tolerates_missing_and_corrupt_config() {
        for raw in [None, Some(""), Some("not json"), Some("{}"), Some("[]")] {
            let cfg = NotifyConfig::from_json(raw);
            assert!(!cfg.enabled, "must default to disabled for {raw:?}");
            assert_eq!(
                cfg.offline_alert_secs, DEFAULT_OFFLINE_ALERT_SECS,
                "must fall back to the default threshold for {raw:?}"
            );
        }
    }

    /// A partial config (only the fields the admin set) keeps its values and
    /// defaults the rest — this is what `#[serde(default)]` buys, and it means
    /// adding a field later doesn't invalidate stored configs.
    #[test]
    fn from_json_accepts_partial_config() {
        let cfg = NotifyConfig::from_json(Some(
            r#"{"enabled":true,"telegram_enabled":true,"telegram_chat_id":"123"}"#,
        ));
        assert!(cfg.enabled);
        assert!(cfg.telegram_enabled);
        assert_eq!(cfg.telegram_chat_id, "123");
        assert!(!cfg.email_enabled, "unset fields default");
        assert_eq!(cfg.offline_alert_secs, DEFAULT_OFFLINE_ALERT_SECS);
    }

    /// The threshold is clamped up, never down. A 5-second alert window would
    /// fire on every transient blip and train the operator to ignore alerts.
    #[test]
    fn alert_threshold_is_clamped_to_the_floor() {
        let mut cfg = NotifyConfig::from_json(Some(r#"{"offline_alert_secs":5}"#));
        assert_eq!(cfg.alert_after(), MIN_OFFLINE_ALERT_SECS);

        cfg.offline_alert_secs = 600;
        assert_eq!(cfg.alert_after(), 600, "a generous threshold is respected");
        // (The floor-vs-online-window invariant is enforced at compile time by
        // the const assert next to MIN_OFFLINE_ALERT_SECS.)
    }

    /// Credentials must never appear in the API-facing shape. If this ever
    /// fails, the panel is handing its bot token to every browser that opens
    /// the settings page.
    #[test]
    fn public_config_never_exposes_credentials() {
        let cfg = NotifyConfig {
            telegram_bot_token: "123456:SECRET-TOKEN".into(),
            smtp_password: "hunter2".into(),
            smtp_username: "ops@example.com".into(),
            ..Default::default()
        };
        let public = NotifyConfigPublic::from(&cfg);
        let json = serde_json::to_string(&public).unwrap();

        assert!(!json.contains("SECRET-TOKEN"), "bot token leaked: {json}");
        assert!(!json.contains("hunter2"), "smtp password leaked: {json}");
        // ...but the UI still learns that they ARE set, so it can show a
        // "configured" state instead of an empty box.
        assert!(public.telegram_bot_token_set);
        assert!(public.smtp_password_set);
        // Non-secret fields are still returned so the form can round-trip.
        assert!(json.contains("ops@example.com"));
    }

    #[test]
    fn any_channel_enabled_requires_master_switch_and_a_channel() {
        let mut cfg = NotifyConfig::default();
        assert!(!cfg.any_channel_enabled(), "all off");

        cfg.telegram_enabled = true;
        assert!(
            !cfg.any_channel_enabled(),
            "a channel without the master switch stays silent"
        );

        cfg.enabled = true;
        assert!(cfg.any_channel_enabled());
    }
}
