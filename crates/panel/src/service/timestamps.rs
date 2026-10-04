//! v1.2.12: the one stored format for expiry-style timestamps.
//!
//! Plan expiry, redeem-code expiry and announcement expiry are stored as TEXT
//! and compared as TEXT against the current time (`datetime('now')` /
//! `now_utc()`), which is `YYYY-MM-DD HH:MM:SS`, UTC, zero-padded. Any other
//! spelling sorts wrong: `2026-9-1 00:00:00` sorts AFTER `2026-09-30 …`
//! ('9' > '0'), so something meant to expire on 1 September stayed live for
//! the rest of the month. Checking that a value merely parses is not enough —
//! chrono also accepts unpadded fields — so values are parsed and then written
//! back in the canonical form, and only that form is stored.

/// The stored format (UTC).
pub const FORMAT: &str = "%Y-%m-%d %H:%M:%S";

/// `raw` rewritten in the canonical stored form, or `None` if it is not a
/// valid date-time in that layout. Surrounding whitespace is ignored.
pub fn canonical_utc(raw: &str) -> Option<String> {
    chrono::NaiveDateTime::parse_from_str(raw.trim(), FORMAT)
        .ok()
        .map(|t| t.format(FORMAT).to_string())
}

#[cfg(test)]
mod tests {
    use super::canonical_utc;

    #[test]
    fn canonical_values_pass_through_unchanged() {
        for v in ["2026-10-01 00:00:00", "2099-12-31 23:59:59"] {
            assert_eq!(canonical_utc(v).as_deref(), Some(v));
        }
    }

    /// The case from the pre-release review: an unpadded date parses, and was
    /// stored as typed — sorting after every later day of the month.
    #[test]
    fn unpadded_or_padded_with_spaces_is_rewritten() {
        assert_eq!(
            canonical_utc("2026-9-1 00:00:00").as_deref(),
            Some("2026-09-01 00:00:00")
        );
        assert_eq!(
            canonical_utc("2026-9-1 0:0:0").as_deref(),
            Some("2026-09-01 00:00:00")
        );
        assert_eq!(
            canonical_utc("  2026-10-01 00:00:00 ").as_deref(),
            Some("2026-10-01 00:00:00")
        );
        // What the text comparison now sees: 1 September is before the 30th.
        assert!(canonical_utc("2026-9-1 00:00:00").unwrap().as_str() < "2026-09-30 00:00:00");
    }

    #[test]
    fn anything_else_is_refused() {
        for bad in [
            "2026-10-01T00:00:00Z",
            "2026-10-01T00:00:00+08:00",
            "2026/10/01 00:00:00",
            "2026-10-01",
            "never",
            "",
            "2026-13-01 00:00:00",
            "2026-02-30 00:00:00",
            "2026-10-01 24:00:00",
        ] {
            assert_eq!(canonical_utc(bad), None, "must refuse {bad:?}");
        }
    }
}
