//! v1.3.0: site announcements.
//!
//! Replaces the single `announcement` string in the site:config blob. That
//! field held one notice, overwrote it on every edit, and kept no history — so
//! "what did we tell users last week" had no answer, and neither operators nor
//! users could look back.

/// Reuses the site config's severities: the two fields feed the same antd
/// Alert, and a second list would drift from the first.
pub use crate::service::site::{ANNOUNCEMENT_TYPES, DEFAULT_ANNOUNCEMENT_TYPE};

pub const MAX_TITLE: usize = 120;
pub const MAX_CONTENT: usize = 4000;

/// Trim and clamp, truncating by `char` rather than by byte — these fields are
/// expected to hold CJK and a byte slice would panic mid-character.
pub fn clamp(s: &str, max: usize) -> String {
    s.trim().chars().take(max).collect()
}

/// Coerce anything outside the four known severities. The value is handed
/// straight to antd's Alert `type`, so an unknown one renders unstyled.
pub fn normalize_kind(kind: &str) -> String {
    if ANNOUNCEMENT_TYPES.contains(&kind) {
        kind.to_string()
    } else {
        DEFAULT_ANNOUNCEMENT_TYPE.to_string()
    }
}

/// Validate an optional expiry.
///
/// `Ok(None)` = never expires. An unparseable value is an error rather than a
/// silent None: expiry is compared as TEXT, so a differently formatted string
/// would sort wrongly and the notice would hide or linger unpredictably.
pub fn parse_expiry(raw: Option<&str>) -> Result<Option<String>, &'static str> {
    match raw.map(str::trim) {
        None | Some("") => Ok(None),
        Some(v) => {
            if chrono::NaiveDateTime::parse_from_str(v, "%Y-%m-%d %H:%M:%S").is_err() {
                Err("过期时间格式应为 YYYY-MM-DD HH:MM:SS (UTC)")
            } else {
                Ok(Some(v.to_string()))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kind_falls_back_for_anything_unknown() {
        for k in ["", "chartreuse", "red; background:url(x)", "INFO"] {
            assert_eq!(normalize_kind(k), DEFAULT_ANNOUNCEMENT_TYPE, "for {k:?}");
        }
        for k in ANNOUNCEMENT_TYPES {
            assert_eq!(normalize_kind(k), k);
        }
    }

    /// Truncation counts characters. A byte slice at MAX_TITLE would land
    /// mid-character on CJK input and panic.
    #[test]
    fn clamp_truncates_multibyte_text_without_panicking() {
        assert_eq!(
            clamp(&"公".repeat(MAX_TITLE + 10), MAX_TITLE)
                .chars()
                .count(),
            MAX_TITLE
        );
        assert_eq!(clamp("  hi  ", MAX_TITLE), "hi");
    }

    /// A blank expiry means "never", but a malformed one is rejected rather
    /// than silently treated as never — expiry is compared as TEXT, so a
    /// different format would sort wrong and hide the notice at random.
    #[test]
    fn expiry_accepts_blank_and_rejects_malformed() {
        assert_eq!(parse_expiry(None), Ok(None));
        assert_eq!(parse_expiry(Some("   ")), Ok(None));
        assert_eq!(
            parse_expiry(Some("2026-08-01 12:00:00")),
            Ok(Some("2026-08-01 12:00:00".to_string()))
        );
        for bad in [
            "2026-08-01",
            "2026-08-01T12:00:00Z",
            "tomorrow",
            "01/08/2026",
        ] {
            assert!(parse_expiry(Some(bad)).is_err(), "must reject {bad:?}");
        }
    }
}
