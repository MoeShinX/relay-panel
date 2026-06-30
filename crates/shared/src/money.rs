//! Strict balance / money parsing.
//!
//! The v0.3.x schema stores `users.balance` as TEXT for backward compatibility
//! (v0.3.4 started allowing admins to edit it via `PUT /admin/users/{id}`).
//! Until the column migrates to a real decimal type, this module is the single
//! source of truth for "what is a legal balance value" — used by the admin
//! update handler (Rust). The frontend copy (InputNumber) mirrors these
//! limits; see frontend/src/pages/Users.tsx.
//!
//! Rules (deliberately conservative for an MVP accounting field):
//!
//! * ASCII decimal notation only — `0`, `12`, `12.34`. No exponent (`1e3`),
//!   no leading `+` / `-` (negative balances are rejected outright), no
//!   leading / trailing whitespace, no locale-specific separators.
//! * At most 2 digits after the decimal point (matches currency / GB-metering
//!   conventions used elsewhere in the UI; finer precision is not actionable).
//! * Capped at a maximum of 9 999 999 999.99 so we never overflow when the
//!   field eventually becomes a numeric column, and so a runaway script can't
//!   poison the dashboard with a billion-figure row.
//! * Output is canonicalised: `0012.30` -> `12.30`, `12.30` -> `12.30`,
//!   `12.3` -> `12.30`. The DB stores the canonical form so admin-edited rows
//!   look the same regardless of what the user typed.
//!
//! Returns `Err(&'static str)` with a user-readable reason on rejection. The
//! caller should surface it as a 400.

/// Maximum allowed balance value: 9 999 999 999.99 (= 2^33.16-ish, well below
/// any numeric type we'll move to). Anything larger is rejected.
pub const MAX_BALANCE: &str = "9999999999.99";

/// Total length of `MAX_BALANCE` (used for length checks before parsing).
const MAX_LEN: usize = 14;

/// Validate a balance string and return the canonical (trimmed, two-decimal)
/// form, or an error describing why the input was rejected.
///
/// Pure function — no DB, no I/O — so it is exhaustively unit-tested in the
/// `tests` submodule below.
pub fn parse_balance(input: &str) -> Result<String, &'static str> {
    // Empty input is rejected outright. The optional field on the wire is
    // represented by a JSON `null` (None in Rust), not by an empty string.
    if input.is_empty() {
        return Err("balance must not be empty");
    }
    // Cheap pre-checks before pulling in the integer parser.
    if input.len() > MAX_LEN {
        return Err("balance is too long (max 9999999999.99)");
    }
    // ASCII decimal only — no exponent, no sign, no locale separators, no
    // whitespace. This also catches "NaN" / "Infinity" up front.
    if input.bytes().any(|b| !b.is_ascii_digit() && b != b'.') {
        return Err("balance must be a non-negative decimal (e.g. 12.34)");
    }
    // Exactly one '.', and not at the very start / very end.
    let dot_count = input.bytes().filter(|&b| b == b'.').count();
    if dot_count > 1 {
        return Err("balance must contain at most one decimal point");
    }

    let (int_part, frac_part) = match input.split_once('.') {
        Some((i, f)) => (i, f),
        None => (input, ""),
    };
    if int_part.is_empty() {
        return Err("balance must have digits before the decimal point");
    }
    if frac_part.len() > 2 {
        return Err("balance may have at most 2 digits after the decimal point");
    }

    // Strip leading zeros for the magnitude check; "" becomes "0" so we never
    // panic on a `.5`-style input here (already rejected above, but be safe).
    let canonical_int = int_part.trim_start_matches('0');
    let canonical_int = if canonical_int.is_empty() {
        "0"
    } else {
        canonical_int
    };
    if exceeds_max(canonical_int, frac_part) {
        return Err("balance exceeds maximum (9999999999.99)");
    }

    // Build the canonical form: drop leading zeros, keep up-to-2 decimals.
    let canonical_frac = if frac_part.is_empty() {
        String::new()
    } else {
        let mut buf = String::with_capacity(2);
        let mut chars = frac_part.chars();
        let d1 = chars.next().unwrap_or('0');
        let d2 = chars.next().unwrap_or('0');
        buf.push(d1);
        buf.push(d2);
        buf
    };
    Ok(if canonical_frac.is_empty() {
        canonical_int.to_string()
    } else {
        format!("{}.{}", canonical_int, canonical_frac)
    })
}

/// True iff `int_part.frac_part` > `MAX_BALANCE` (= "9999999999.99").
fn exceeds_max(int_part: &str, frac_part: &str) -> bool {
    let mut f = frac_part.to_string();
    while f.len() < 2 {
        f.push('0');
    }
    let f = &f[..2];

    if int_part.len() > 10 {
        return true;
    }
    if int_part.len() < 10 {
        return false;
    }
    let max_int = "9999999999";
    if int_part != max_int {
        return int_part > max_int;
    }
    f > "99"
}

/// v1.0.8: convert a canonical balance string ("12.34" / "12" / "0") to integer
/// cents (1234). Used by the purchase path to compare/deduct balances in
/// integer arithmetic (no floating point). Input MUST be canonical (no leading
/// zeros except "0", at most 2 fraction digits, no sign) — every balance in
/// the DB is canonical because parse_balance canonicalizes on write. Returns
/// None on a non-canonical string (the caller treats that as a data-integrity
/// error and refuses the purchase rather than silently mis-billing).
pub fn balance_to_cents(s: &str) -> Option<i64> {
    if s.is_empty() {
        return None;
    }
    let (int_part, frac_part) = s.split_once('.').unwrap_or((s, ""));
    if int_part.is_empty() || frac_part.len() > 2 {
        return None;
    }
    // Reject non-canonical leading zeros ("012") except the single "0".
    if int_part.len() > 1 && int_part.starts_with('0') {
        return None;
    }
    if int_part.bytes().any(|b| !b.is_ascii_digit()) || frac_part.bytes().any(|b| !b.is_ascii_digit())
    {
        return None;
    }
    let int_cents: i64 = int_part.parse().ok()?;
    let frac_cents: i64 = match frac_part.len() {
        0 => 0,
        1 => frac_part.parse::<i64>().ok()? * 10,
        2 => frac_part.parse::<i64>().ok()?,
        _ => return None,
    };
    Some(int_cents * 100 + frac_cents)
}

/// v1.0.8: convert integer cents back to a canonical balance string. The
/// inverse of [`balance_to_cents`]. Used to persist the post-deduction balance.
pub fn cents_to_balance(cents: i64) -> String {
    let neg = cents < 0;
    let abs = cents.unsigned_abs();
    let int_part = abs / 100;
    let frac = abs % 100;
    let s = if frac == 0 {
        int_part.to_string()
    } else {
        format!("{}.{:02}", int_part, frac)
    };
    if neg {
        format!("-{}", s)
    } else {
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_and_canonicalises_valid_balances() {
        assert_eq!(parse_balance("0").unwrap(), "0");
        assert_eq!(parse_balance("00").unwrap(), "0");
        assert_eq!(parse_balance("12").unwrap(), "12");
        assert_eq!(parse_balance("12.34").unwrap(), "12.34");
        assert_eq!(parse_balance("0012.30").unwrap(), "12.30");
        assert_eq!(parse_balance("12.3").unwrap(), "12.30");
        assert_eq!(parse_balance("0.00").unwrap(), "0.00");
        assert_eq!(parse_balance("0.0").unwrap(), "0.00");
        assert_eq!(parse_balance("9999999999.99").unwrap(), "9999999999.99");
        assert_eq!(parse_balance("9999999999").unwrap(), "9999999999");
    }

    #[test]
    fn rejects_known_invalid_balances() {
        let bad: &[(&str, &str)] = &[
            ("", "empty"),
            ("-1", "non-negative"),
            ("-0.01", "non-negative"),
            ("+1", "non-negative"),
            ("1e3", "non-negative"),
            ("1E3", "non-negative"),
            ("NaN", "non-negative"),
            ("Infinity", "non-negative"),
            ("abc", "non-negative"),
            ("12abc", "non-negative"),
            ("1,000.00", "non-negative"),
            ("1 000.00", "non-negative"),
            ("\t12.34", "non-negative"),
            ("12.34\n", "non-negative"),
            (".5", "digits before"),
            ("1.2.3", "at most one"),
            ("12.345", "at most 2"),
            ("12.3456", "at most 2"),
            // Note: too-many-decimals is checked BEFORE max magnitude, so this
            // first fails the fraction-length check.
            ("9999999999.991", "at most 2"),
            ("10000000000", "exceeds"),
            ("10000000000.00", "exceeds"),
        ];
        for (input, needle) in bad {
            let err = parse_balance(input).unwrap_err();
            assert!(
                err.contains(needle),
                "input {input:?} should fail with {needle:?}, got {err:?}"
            );
        }
    }

    #[test]
    fn rejects_obviously_too_long_input() {
        let big = "1".repeat(MAX_LEN + 1);
        assert!(parse_balance(&big).unwrap_err().contains("too long"));
    }
}
