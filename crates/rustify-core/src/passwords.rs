//! Random password generation for database credentials.
//!
//! Behavioural analogue of Coolify's `Str::password` usage across the
//! `Start*.php` database actions (which seed `POSTGRES_PASSWORD`,
//! `MYSQL_ROOT_PASSWORD`, `redis_password`, ... with a random alphanumeric
//! string on first create). Rustify uses a 64-char alphanumeric password by
//! default; symbols are opt-in.

use rand::Rng;

const ALNUM: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";
const SYMBOLS: &[u8] = b"!#%*+-=?_";

/// Generate a random password of `length` characters. When `symbols` is true a
/// small set of URL-safe symbols is added to the alphabet; otherwise the
/// password is strictly `[A-Za-z0-9]` (safe to embed unquoted in a connection
/// URL or a shell command).
pub fn gen_password(length: usize, symbols: bool) -> String {
    let mut alphabet = ALNUM.to_vec();
    if symbols {
        alphabet.extend_from_slice(SYMBOLS);
    }
    let mut rng = rand::thread_rng();
    (0..length)
        .map(|_| alphabet[rng.gen_range(0..alphabet.len())] as char)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn length_and_alphabet_are_respected() {
        let p = gen_password(64, false);
        assert_eq!(p.chars().count(), 64);
        assert!(
            p.chars().all(|c| c.is_ascii_alphanumeric()),
            "no-symbols password is strictly alphanumeric: {p}"
        );
    }

    #[test]
    fn two_passwords_differ() {
        assert_ne!(gen_password(64, false), gen_password(64, false));
    }

    #[test]
    fn symbols_widen_the_alphabet() {
        // Statistically a 4096-char password with symbols enabled will contain
        // at least one symbol; this asserts the branch is wired.
        let p = gen_password(4096, true);
        assert!(p.chars().any(|c| !c.is_ascii_alphanumeric()));
    }
}
