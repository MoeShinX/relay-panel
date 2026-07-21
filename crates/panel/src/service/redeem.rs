//! v1.2.0: redeem-code generation and input normalization.
//!
//! Pure functions — no DB, no I/O — so the format rules are exhaustively unit
//! tested here and the repo layer only deals with storage.
//!
//! ## Why not a UUID
//!
//! Node tokens are `Uuid::new_v4()`, but those are copy-pasted once into a
//! config file. A redeem code is READ ALOUD, written on a card, and typed by
//! hand — 36 characters with `0`/`O` and `1`/`l` in them is a support-ticket
//! generator. The alphabet below is Crockford Base32: 32 symbols with `I`, `L`,
//! `O` and `U` removed (`U` is dropped so no random string spells something
//! unfortunate), which also means decoding can FORGIVE the classic
//! misreadings — `O` → `0`, `I`/`L` → `1` — instead of rejecting them.
//!
//! Randomness comes from `Uuid::new_v4()`'s bytes rather than a new `rand`
//! dependency; v4 UUIDs are generated from the OS CSPRNG via `getrandom`.

/// Crockford Base32. Deliberately excludes I, L, O, U.
const ALPHABET: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";

/// Symbols per code. 16 × 5 bits = 80 bits of entropy — brute-forcing a hit
/// against even a million live codes is not a practical attack, and the API
/// layer refuses to distinguish "no such code" from "already used" so an
/// attacker gets no oracle either.
const CODE_LEN: usize = 16;

/// Characters between dashes in the display form (`XXXX-XXXX-XXXX-XXXX`).
const GROUP: usize = 4;

/// Generate one code in display form.
pub fn generate_code() -> String {
    // 16 symbols × 5 bits = 80 bits; a v4 UUID carries 122 random bits, so one
    // UUID comfortably covers a single code.
    let bytes = *uuid::Uuid::new_v4().as_bytes();
    let mut out = String::with_capacity(CODE_LEN + CODE_LEN / GROUP);
    for (i, b) in bytes.iter().take(CODE_LEN).enumerate() {
        if i > 0 && i % GROUP == 0 {
            out.push('-');
        }
        // Fold each byte into the 32-symbol alphabet. Taking the low 5 bits
        // discards 3 bits per byte but keeps the mapping uniform — every symbol
        // is reachable from exactly 8 of the 256 byte values.
        out.push(ALPHABET[(b & 0x1F) as usize] as char);
    }
    out
}

/// Normalize user input to the canonical STORED form (upper-case, no dashes).
///
/// A code arrives from a human: lower-cased by a phone keyboard, wrapped in
/// whitespace by a copy-paste, with or without the dashes, and with `O`/`I`/`l`
/// where `0`/`1` were meant. All of those are the same code — rejecting them
/// would make redemption fail for reasons the user cannot see. Anything outside
/// the alphabet after that folding is dropped, so punctuation or a stray
/// invisible character can't break a paste.
///
/// Returns `None` when nothing usable is left, so the caller can reject empty
/// input without a DB round-trip.
pub fn normalize_code(input: &str) -> Option<String> {
    let mut out = String::with_capacity(CODE_LEN);
    for ch in input.chars() {
        let up = ch.to_ascii_uppercase();
        let folded = match up {
            // Crockford's forgiving decode: these are misreadings, not values.
            'O' => '0',
            'I' | 'L' => '1',
            c => c,
        };
        if ALPHABET.contains(&(folded as u8)) {
            out.push(folded);
        }
        // Everything else (dashes, spaces, punctuation) is dropped silently.
    }
    if out.is_empty() {
        None
    } else {
        Some(out)
    }
}

/// The stored form of a freshly generated code (dashes stripped).
///
/// Codes are STORED without dashes and looked up that way, so a user who types
/// the dashes and a user who doesn't hit the same row. The dashed form exists
/// only for display/export.
pub fn to_stored(display: &str) -> String {
    display.chars().filter(|c| *c != '-').collect()
}

/// Re-insert dashes for display/export.
pub fn to_display(stored: &str) -> String {
    let mut out = String::with_capacity(stored.len() + stored.len() / GROUP);
    for (i, c) in stored.chars().enumerate() {
        if i > 0 && i % GROUP == 0 {
            out.push('-');
        }
        out.push(c);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn generated_code_has_the_expected_shape() {
        let c = generate_code();
        assert_eq!(c.len(), CODE_LEN + 3, "16 symbols + 3 dashes");
        assert_eq!(c.matches('-').count(), 3);
        let stored = to_stored(&c);
        assert_eq!(stored.len(), CODE_LEN);
        assert!(
            stored.bytes().all(|b| ALPHABET.contains(&b)),
            "every symbol must come from the alphabet: {stored}"
        );
    }

    /// The alphabet must not contain the characters people misread, or the
    /// forgiving normalization below would be ambiguous (is this `0` or `O`?).
    #[test]
    fn alphabet_excludes_confusable_letters() {
        for bad in [b'I', b'L', b'O', b'U'] {
            assert!(
                !ALPHABET.contains(&bad),
                "{} must not be in the alphabet",
                bad as char
            );
        }
        assert_eq!(ALPHABET.len(), 32);
        // No duplicates — a repeated symbol would skew the distribution.
        assert_eq!(ALPHABET.iter().collect::<HashSet<_>>().len(), 32);
    }

    /// Everything a human might realistically type for one code must normalize
    /// to the same stored string. This is the difference between "redemption
    /// just works" and a support ticket the user can't diagnose.
    #[test]
    fn normalize_accepts_every_plausible_human_rendering() {
        let canonical = "0123456789ABCDEF";
        for variant in [
            "0123-4567-89AB-CDEF",     // display form with dashes
            "0123456789ABCDEF",        // no dashes
            "0123456789abcdef",        // lower case
            "  0123-4567-89AB-CDEF  ", // copy-paste whitespace
            "0123 4567 89AB CDEF",     // spaces instead of dashes
            "O123-4567-89AB-CDEF",     // typed letter O for zero
        ] {
            assert_eq!(
                normalize_code(variant).as_deref(),
                Some(canonical),
                "variant {variant:?} must normalize to the canonical form"
            );
        }
    }

    /// I and L both mean 1 — a card read over the phone.
    #[test]
    fn normalize_folds_i_and_l_to_one() {
        assert_eq!(normalize_code("IL").as_deref(), Some("11"));
        assert_eq!(normalize_code("il").as_deref(), Some("11"));
    }

    #[test]
    fn normalize_rejects_input_with_nothing_usable() {
        assert_eq!(normalize_code(""), None);
        assert_eq!(normalize_code("   "), None);
        assert_eq!(normalize_code("---"), None);
        assert_eq!(normalize_code("!@#$"), None);
    }

    #[test]
    fn display_and_stored_round_trip() {
        let c = generate_code();
        assert_eq!(to_display(&to_stored(&c)), c);
    }

    /// Two generated codes must not collide. Not a statistical proof — just a
    /// guard against a refactor that accidentally makes generation
    /// deterministic (e.g. seeding from a fixed value).
    #[test]
    fn generation_is_not_deterministic() {
        let codes: HashSet<String> = (0..500).map(|_| generate_code()).collect();
        assert_eq!(codes.len(), 500, "generated codes must be unique");
    }
}
