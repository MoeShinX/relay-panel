#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PasswordValidationError {
    TooShort,
    TooLong,
}

pub fn validate_password(password: &str) -> Result<(), PasswordValidationError> {
    if password.len() < 8 {
        return Err(PasswordValidationError::TooShort);
    }
    if password.len() > 72 {
        return Err(PasswordValidationError::TooLong);
    }
    Ok(())
}

/// Character classes for a generated recovery password.
///
/// Ambiguous glyphs are left out on purpose — `0/O` and `1/l/I` are read off a
/// terminal and retyped by hand often enough that the confusion costs more than
/// the handful of bits. Quotes, backslash and backtick are out too: the value
/// gets pasted through shells on its way to a browser, and those are exactly
/// the characters that break when it does.
const GEN_LOWER: &[u8] = b"abcdefghijkmnopqrstuvwxyz";
const GEN_UPPER: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZ";
const GEN_DIGIT: &[u8] = b"23456789";
const GEN_SYMBOL: &[u8] = b"!@#%^&*-_=+?";

/// Length of a generated password. Long enough that this stays acceptable as a
/// permanent password, since the reset flow does not force a change.
pub const GENERATED_PASSWORD_LEN: usize = 16;

/// Draw `n` bytes from the OS CSPRNG.
///
/// Panics rather than falling back to anything weaker: a "random" password from
/// a degraded source is worse than no password reset at all, because the
/// operator would never know.
fn random_bytes(n: usize) -> Vec<u8> {
    let mut buf = vec![0u8; n];
    getrandom::getrandom(&mut buf).expect("OS randomness unavailable");
    buf
}

/// Uniformly pick one byte from `set`, without modulo bias.
///
/// Rejection sampling: bytes at or above the largest whole multiple of the set
/// size are discarded. Taking `% len` directly would make the first
/// `256 % len` characters measurably likelier.
fn pick(set: &[u8]) -> u8 {
    let len = set.len();
    let limit = 256 - (256 % len);
    loop {
        let b = random_bytes(1)[0] as usize;
        if b < limit {
            return set[b % len];
        }
    }
}

/// A random password containing at least one lowercase, uppercase, digit and
/// symbol.
///
/// The four classes are placed first and then shuffled in, rather than hoped
/// for: drawing 16 characters from the combined set leaves a real chance of
/// producing one with no digit at all, and an operator who is told the password
/// has a digit should get one.
pub fn generate_password() -> String {
    let mut chars: Vec<u8> = vec![
        pick(GEN_LOWER),
        pick(GEN_UPPER),
        pick(GEN_DIGIT),
        pick(GEN_SYMBOL),
    ];
    let all: Vec<u8> = [GEN_LOWER, GEN_UPPER, GEN_DIGIT, GEN_SYMBOL].concat();
    while chars.len() < GENERATED_PASSWORD_LEN {
        chars.push(pick(&all));
    }

    // Fisher-Yates, so the guaranteed four are not always in positions 0..4.
    for i in (1..chars.len()).rev() {
        let j = {
            let limit = 256 - (256 % (i + 1));
            loop {
                let b = random_bytes(1)[0] as usize;
                if b < limit {
                    break b % (i + 1);
                }
            }
        };
        chars.swap(i, j);
    }

    String::from_utf8(chars).expect("charset is ASCII")
}

pub fn hash_password(password: &str) -> Result<String, bcrypt::BcryptError> {
    bcrypt::hash(password, 12)
}

pub fn verify_password(password: &str, hash: &str) -> bool {
    bcrypt::verify(password, hash).unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every generated password contains all four classes. Drawing 16 characters
    /// from the combined set would leave a real chance of producing one with no
    /// digit, and an operator told the password has digits should get one.
    /// Repeated because this is a probabilistic property — one sample proves
    /// nothing.
    #[test]
    fn generated_password_always_has_all_four_classes() {
        for _ in 0..200 {
            let p = generate_password();
            assert_eq!(p.chars().count(), GENERATED_PASSWORD_LEN);
            assert!(
                p.chars().any(|c| c.is_ascii_lowercase()),
                "no lowercase in {p}"
            );
            assert!(
                p.chars().any(|c| c.is_ascii_uppercase()),
                "no uppercase in {p}"
            );
            assert!(p.chars().any(|c| c.is_ascii_digit()), "no digit in {p}");
            assert!(
                p.chars().any(|c| GEN_SYMBOL.contains(&(c as u8))),
                "no symbol in {p}"
            );
        }
    }

    /// Ambiguous glyphs stay out. These get read off a terminal and retyped, and
    /// 0/O or 1/l/I costs more in failed logins than it buys in entropy.
    #[test]
    fn generated_password_avoids_ambiguous_characters() {
        for _ in 0..200 {
            let p = generate_password();
            for bad in ['0', 'O', '1', 'l', 'I'] {
                assert!(!p.contains(bad), "{p} contains ambiguous {bad}");
            }
        }
    }

    /// Nothing that breaks when the value is pasted through a shell on its way
    /// to a browser.
    #[test]
    fn generated_password_has_no_quoting_hazards() {
        for _ in 0..200 {
            let p = generate_password();
            for bad in ['\'', '"', '\\', '`', '$', ' '] {
                assert!(!p.contains(bad), "{p} contains shell-hostile {bad}");
            }
        }
    }

    /// Two calls must not agree. A generator seeded from a clock or returning a
    /// constant would sail through the class checks above.
    #[test]
    fn generated_passwords_differ() {
        let mut seen = std::collections::HashSet::new();
        for _ in 0..100 {
            assert!(seen.insert(generate_password()), "generated a duplicate");
        }
    }

    /// The generated password must satisfy the panel's own validator — otherwise
    /// the reset would hand out something the change-password form rejects.
    #[test]
    fn generated_password_passes_validation() {
        for _ in 0..50 {
            assert!(validate_password(&generate_password()).is_ok());
        }
    }

    #[test]
    fn password_boundaries_are_enforced() {
        assert_eq!(
            validate_password("1234567"),
            Err(PasswordValidationError::TooShort)
        );
        assert!(validate_password("12345678").is_ok());
        assert!(validate_password(&"a".repeat(72)).is_ok());
        assert_eq!(
            validate_password(&"a".repeat(73)),
            Err(PasswordValidationError::TooLong)
        );
    }
}
