//! v1.3.0: operator-configurable site identity.
//!
//! The brand string was hardcoded in three places (login page, sidebar, browser
//! title), so every operator running this panel showed "RelayPanel" to their own
//! users. This makes name, subtitle, announcement and support contact editable
//! from the panel.
//!
//! Stored as one JSON blob in the kvs table, exactly like the notify config —
//! a handful of free-text fields don't earn their own columns, and a new field
//! then costs no migration.

use serde::{Deserialize, Serialize};

pub const SITE_CONFIG_KEY: &str = "site:config";

/// Length caps. These are not security boundaries — they stop an accidental
/// paste of a whole document from becoming a row every page load has to read.
pub const MAX_NAME: usize = 64;
pub const MAX_SUBTITLE: usize = 128;
pub const MAX_ANNOUNCEMENT: usize = 4000;
pub const MAX_CONTACT: usize = 256;

/// Falls back to the current hardcoded brand, so an operator who never opens
/// the page sees exactly what they saw before.
pub const DEFAULT_NAME: &str = "RelayPanel";

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct SiteConfig {
    /// Shown on the login page, in the sidebar, and as the browser tab title.
    pub site_name: String,
    /// Small text under the name on the login page. Empty = the frontend keeps
    /// its own translated default, which is why this is not defaulted here.
    pub subtitle: String,
    /// Free text shown to signed-in users on the dashboard and account page.
    /// Empty = the banner is not rendered at all, rather than an empty box.
    pub announcement: String,
    /// How users reach the operator (Telegram handle, email, whatever).
    pub contact: String,
}

impl SiteConfig {
    /// Tolerates a missing, empty, or corrupt row — the panel must render its
    /// login page even if this value is garbage, since a blank brand would make
    /// the site look broken to everyone.
    pub fn from_json(raw: Option<&str>) -> Self {
        let mut cfg: Self = raw
            .and_then(|r| serde_json::from_str(r).ok())
            .unwrap_or_default();
        if cfg.site_name.trim().is_empty() {
            cfg.site_name = DEFAULT_NAME.to_string();
        }
        cfg
    }

    /// Trim and clamp every field. Applied on write so the stored row is always
    /// already within bounds and readers never have to defend themselves.
    ///
    /// Truncation is by `char`, not by byte: slicing a multi-byte character in
    /// half would panic, and every one of these fields is expected to hold CJK.
    pub fn sanitized(&self) -> Self {
        fn clamp(s: &str, max: usize) -> String {
            s.trim().chars().take(max).collect()
        }
        let mut out = Self {
            site_name: clamp(&self.site_name, MAX_NAME),
            subtitle: clamp(&self.subtitle, MAX_SUBTITLE),
            announcement: clamp(&self.announcement, MAX_ANNOUNCEMENT),
            contact: clamp(&self.contact, MAX_CONTACT),
        };
        if out.site_name.is_empty() {
            out.site_name = DEFAULT_NAME.to_string();
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A missing or damaged row must still yield a usable brand. Rendering an
    /// empty site name would make every page look broken, including the login
    /// page an operator would use to go fix it.
    #[test]
    fn from_json_always_yields_a_name() {
        for raw in [
            None,
            Some(""),
            Some("not json"),
            Some("{}"),
            Some("[]"),
            Some(r#"{"site_name":"   "}"#),
        ] {
            let cfg = SiteConfig::from_json(raw);
            assert_eq!(cfg.site_name, DEFAULT_NAME, "for {raw:?}");
        }
    }

    /// A stored name is kept as-is; only blank falls back.
    #[test]
    fn from_json_keeps_a_configured_name() {
        let cfg = SiteConfig::from_json(Some(r#"{"site_name":"我的中转","contact":"tg"}"#));
        assert_eq!(cfg.site_name, "我的中转");
        assert_eq!(cfg.contact, "tg");
    }

    /// Truncation counts characters, not bytes. A byte slice at MAX_NAME would
    /// land mid-character on CJK input and panic — the exact input this panel
    /// expects most.
    #[test]
    fn sanitize_truncates_multibyte_text_without_panicking() {
        let cfg = SiteConfig {
            site_name: "中".repeat(MAX_NAME + 10),
            announcement: "公".repeat(MAX_ANNOUNCEMENT + 10),
            ..Default::default()
        };
        let out = cfg.sanitized();
        assert_eq!(out.site_name.chars().count(), MAX_NAME);
        assert_eq!(out.announcement.chars().count(), MAX_ANNOUNCEMENT);
    }

    /// Whitespace-only input is the same as clearing the field, and clearing
    /// the name falls back rather than storing "".
    #[test]
    fn sanitize_trims_and_falls_back_on_blank_name() {
        let cfg = SiteConfig {
            site_name: "   ".into(),
            subtitle: "  hi  ".into(),
            ..Default::default()
        };
        let out = cfg.sanitized();
        assert_eq!(out.site_name, DEFAULT_NAME);
        assert_eq!(out.subtitle, "hi");
    }
}
