//! One-click-service "magic" environment-variable engine.
//!
//! Behavioural port of Coolify's `generateEnvValue`
//! (bootstrap/helpers/shared.php:1424-1523) and its key parser
//! `parseCommandFromMagicEnvVariable` (bootstrap/helpers/shared.php:1356-1380).
//!
//! A service template declares placeholder variables named
//! `SERVICE_<COMMAND>_<IDENTIFIER>`. When a service is created Rustify replaces
//! each placeholder with a freshly generated value determined solely by the
//! `<COMMAND>` token (a password, random string, hex blob, JWT, ...). The
//! generated secrets are stored encrypted at rest and reused on subsequent
//! deploys (persist-once), so this module only produces *fresh* material — the
//! reuse logic lives in `rustify-docker`'s compose mutator.

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use hmac::{Hmac, Mac};
use rand::Rng;
use rand::seq::SliceRandom;
use sha2::Sha256;

/// Alphanumeric alphabet used by Laravel's `Str::random` (base62).
const ALNUM: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";
/// Symbol set added by Laravel's `Str::password(symbols: true)`.
const SYMBOLS: &[u8] = b"~!#$%^&*()-_.,<>?/\\{}[]|:;";

/// Base64URL alphabet without padding, for JWT segments (RFC 7515).
const BASE64URL: base64::engine::general_purpose::GeneralPurpose =
    base64::engine::general_purpose::URL_SAFE_NO_PAD;

/// A random alphanumeric string of length `n` (Laravel `Str::random`).
fn random_alnum(n: usize) -> String {
    let mut rng = rand::thread_rng();
    (0..n)
        .map(|_| *ALNUM.choose(&mut rng).expect("ALNUM is non-empty") as char)
        .collect()
}

/// A random password of length `n`, optionally including symbols. The character
/// classes are shuffled together exactly as Laravel's `Str::password` does.
fn random_password(n: usize, symbols: bool) -> String {
    let mut rng = rand::thread_rng();
    let mut alphabet: Vec<u8> = ALNUM.to_vec();
    if symbols {
        alphabet.extend_from_slice(SYMBOLS);
    }
    let mut out: Vec<u8> = (0..n)
        .map(|_| *alphabet.choose(&mut rng).expect("alphabet is non-empty"))
        .collect();
    out.shuffle(&mut rng);
    String::from_utf8(out).expect("alphabet is ASCII")
}

/// `n` random bytes.
fn random_bytes(n: usize) -> Vec<u8> {
    let mut rng = rand::thread_rng();
    (0..n).map(|_| rng.r#gen::<u8>()).collect()
}

/// Lowercase hex encoding of `bytes`.
fn hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push(char::from_digit((b >> 4) as u32, 16).expect("nibble"));
        s.push(char::from_digit((b & 0x0f) as u32, 16).expect("nibble"));
    }
    s
}

/// Generate a value for a magic `<COMMAND>` token. Returns `None` for unknown
/// commands and for the Supabase JWT commands (which require the JWT signing
/// key — use [`supabase_jwt`] for those). Port of `generateEnvValue`.
pub fn generate_service_var(command: &str) -> Option<String> {
    let value = match command {
        // Passwords (bootstrap/helpers/shared.php:1427-1439).
        "PASSWORD" => random_password(32, false),
        "PASSWORD_64" => random_password(64, false),
        "PASSWORDWITHSYMBOLS" => random_password(32, true),
        "PASSWORDWITHSYMBOLS_64" => random_password(64, true),
        // "BASE64" is actually a plain random string (shared.php:1440-1449).
        "BASE64" | "BASE64_32" => random_alnum(32),
        "BASE64_64" => random_alnum(64),
        "BASE64_128" => random_alnum(128),
        // Real base64 of random bytes (shared.php:1450-1460).
        "REALBASE64" | "REALBASE64_32" => BASE64.encode(random_bytes(32)),
        "REALBASE64_64" => BASE64.encode(random_bytes(64)),
        "REALBASE64_128" => BASE64.encode(random_bytes(128)),
        // Hex of random bytes (shared.php:1461-1469).
        "HEX_32" => hex(&random_bytes(16)),
        "HEX_64" => hex(&random_bytes(32)),
        "HEX_128" => hex(&random_bytes(64)),
        // Users (shared.php:1470-1475).
        "USER" => random_alnum(16),
        "LOWERCASEUSER" => random_alnum(16).to_lowercase(),
        // Supabase JWTs need the signing key; handled by the caller.
        _ => return None,
    };
    Some(value)
}

/// True when a `<COMMAND>` yields a secret that must never be shown twice in the
/// UI (passwords, tokens, keys). FQDN/URL/user values are not secret.
pub fn is_secret_command(command: &str) -> bool {
    matches!(
        command,
        "PASSWORD"
            | "PASSWORD_64"
            | "PASSWORDWITHSYMBOLS"
            | "PASSWORDWITHSYMBOLS_64"
            | "BASE64"
            | "BASE64_32"
            | "BASE64_64"
            | "BASE64_128"
            | "REALBASE64"
            | "REALBASE64_32"
            | "REALBASE64_64"
            | "REALBASE64_128"
            | "HEX_32"
            | "HEX_64"
            | "HEX_128"
            | "SUPABASEANON"
            | "SUPABASESERVICE"
    )
}

/// Build a Supabase-style HS256 JWT signed with `signing_key`, carrying the
/// given `role` (`anon` or `service_role`), issuer `supabase`, issued now and
/// expiring in ~100 years. Port of the `SUPABASEANON`/`SUPABASESERVICE` arms of
/// `generateEnvValue` (shared.php:1476-1516).
pub fn supabase_jwt(role: &str, signing_key: &str) -> String {
    // Header: {"typ":"JWT","alg":"HS256"} (compact, key order fixed).
    let header = r#"{"typ":"JWT","alg":"HS256"}"#;
    let now = chrono::Utc::now();
    // Coolify truncates to the minute; mirror that so re-issues within a minute
    // are stable.
    let iat = now.timestamp() - (now.timestamp() % 60);
    // +100 years, approximated in seconds (matches Coolify's "+100 year").
    let exp = iat + 100 * 365 * 24 * 60 * 60;
    let payload = serde_json::json!({
        "role": role,
        "iss": "supabase",
        "iat": iat,
        "exp": exp,
    });
    let payload = serde_json::to_string(&payload).unwrap_or_default();

    let signing_input = format!(
        "{}.{}",
        BASE64URL.encode(header.as_bytes()),
        BASE64URL.encode(payload.as_bytes())
    );
    let mut mac = Hmac::<Sha256>::new_from_slice(signing_key.as_bytes())
        .expect("HMAC accepts any key length");
    mac.update(signing_input.as_bytes());
    let signature = BASE64URL.encode(mac.finalize().into_bytes());
    format!("{signing_input}.{signature}")
}

/// Extract the `<COMMAND>` token from a magic `SERVICE_*` variable name, or
/// `None` when the name is not a magic variable (it must have exactly 2 or 3
/// underscores). Port of `parseCommandFromMagicEnvVariable`
/// (shared.php:1356-1380).
///
/// - `SERVICE_FQDN_UMAMI`        → `FQDN`   (count 2)
/// - `SERVICE_URL_UMAMI_3000`    → `URL`    (count 3, FQDN/URL keep first token)
/// - `SERVICE_PASSWORD_UMAMI`    → `PASSWORD`
/// - `SERVICE_BASE64_64_UMAMI`   → `BASE64_64`
pub fn parse_command(key: &str) -> Option<String> {
    if !key.starts_with("SERVICE_") {
        return None;
    }
    let count = key.matches('_').count();
    if count != 2 && count != 3 {
        return None;
    }
    let rest = key.strip_prefix("SERVICE_")?;
    let is_fqdn_or_url = key.starts_with("SERVICE_FQDN") || key.starts_with("SERVICE_URL");
    let command = if count == 3 && is_fqdn_or_url {
        // SERVICE_FQDN_UMAMI_1000 → FQDN (first token after SERVICE_).
        rest.split('_').next()?.to_string()
    } else {
        // Everything up to the last underscore.
        before_last(rest, '_').to_string()
    };
    Some(command)
}

/// The substring before the last occurrence of `sep`, or the whole string.
fn before_last(s: &str, sep: char) -> &str {
    match s.rfind(sep) {
        Some(i) => &s[..i],
        None => s,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_command_underscore_counts() {
        // count 2
        assert_eq!(parse_command("SERVICE_FQDN_UMAMI").as_deref(), Some("FQDN"));
        assert_eq!(parse_command("SERVICE_URL_UMAMI").as_deref(), Some("URL"));
        assert_eq!(
            parse_command("SERVICE_PASSWORD_UMAMI").as_deref(),
            Some("PASSWORD")
        );
        assert_eq!(parse_command("SERVICE_USER_DB").as_deref(), Some("USER"));
        // count 3
        assert_eq!(
            parse_command("SERVICE_FQDN_UMAMI_1000").as_deref(),
            Some("FQDN")
        );
        assert_eq!(
            parse_command("SERVICE_URL_APP_3000").as_deref(),
            Some("URL")
        );
        assert_eq!(
            parse_command("SERVICE_BASE64_64_UMAMI").as_deref(),
            Some("BASE64_64")
        );
        assert_eq!(
            parse_command("SERVICE_PASSWORD_64_ADMIN").as_deref(),
            Some("PASSWORD_64")
        );
    }

    #[test]
    fn parse_command_rejects_non_magic() {
        assert_eq!(parse_command("POSTGRES_DB"), None); // no SERVICE_ prefix
        assert_eq!(parse_command("SERVICE_PASSWORD"), None); // only 1 underscore
        assert_eq!(parse_command("SERVICE_A_B_C_D"), None); // 4 underscores
    }

    #[test]
    fn password_lengths_and_charset() {
        assert_eq!(generate_service_var("PASSWORD").unwrap().len(), 32);
        assert_eq!(generate_service_var("PASSWORD_64").unwrap().len(), 64);
        let no_sym = generate_service_var("PASSWORD").unwrap();
        assert!(
            no_sym.chars().all(|c| c.is_ascii_alphanumeric()),
            "PASSWORD has no symbols: {no_sym}"
        );
        assert_eq!(
            generate_service_var("PASSWORDWITHSYMBOLS").unwrap().len(),
            32
        );
        assert_eq!(
            generate_service_var("PASSWORDWITHSYMBOLS_64")
                .unwrap()
                .len(),
            64
        );
    }

    #[test]
    fn base64_is_plain_alnum() {
        for (cmd, len) in [
            ("BASE64", 32),
            ("BASE64_32", 32),
            ("BASE64_64", 64),
            ("BASE64_128", 128),
        ] {
            let v = generate_service_var(cmd).unwrap();
            assert_eq!(v.len(), len, "{cmd}");
            assert!(v.chars().all(|c| c.is_ascii_alphanumeric()), "{cmd}: {v}");
        }
    }

    #[test]
    fn realbase64_decodes_to_expected_byte_lengths() {
        for (cmd, bytes) in [
            ("REALBASE64", 32),
            ("REALBASE64_32", 32),
            ("REALBASE64_64", 64),
            ("REALBASE64_128", 128),
        ] {
            let v = generate_service_var(cmd).unwrap();
            let decoded = BASE64.decode(v.as_bytes()).expect("valid base64");
            assert_eq!(decoded.len(), bytes, "{cmd}");
        }
    }

    #[test]
    fn hex_lengths_and_charset() {
        for (cmd, len) in [("HEX_32", 32), ("HEX_64", 64), ("HEX_128", 128)] {
            let v = generate_service_var(cmd).unwrap();
            assert_eq!(v.len(), len, "{cmd}");
            assert!(
                v.chars()
                    .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
            );
        }
    }

    #[test]
    fn user_variants() {
        let u = generate_service_var("USER").unwrap();
        assert_eq!(u.len(), 16);
        assert!(u.chars().all(|c| c.is_ascii_alphanumeric()));
        let lu = generate_service_var("LOWERCASEUSER").unwrap();
        assert_eq!(lu.len(), 16);
        assert!(
            lu.chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        );
    }

    #[test]
    fn unknown_and_supabase_return_none() {
        assert_eq!(generate_service_var("NOPE"), None);
        assert_eq!(generate_service_var("FQDN"), None);
        assert_eq!(generate_service_var("SUPABASEANON"), None);
    }

    #[test]
    fn values_are_random_per_call() {
        assert_ne!(
            generate_service_var("PASSWORD"),
            generate_service_var("PASSWORD")
        );
        assert_ne!(
            generate_service_var("HEX_32"),
            generate_service_var("HEX_32")
        );
    }

    #[test]
    fn supabase_jwt_structure_and_signature() {
        let key = "my-super-secret-jwt-signing-key";
        let jwt = supabase_jwt("anon", key);
        let parts: Vec<&str> = jwt.split('.').collect();
        assert_eq!(parts.len(), 3, "header.payload.signature");

        let header = BASE64URL.decode(parts[0]).unwrap();
        let header: serde_json::Value = serde_json::from_slice(&header).unwrap();
        assert_eq!(header["alg"], "HS256");
        assert_eq!(header["typ"], "JWT");

        let payload = BASE64URL.decode(parts[1]).unwrap();
        let payload: serde_json::Value = serde_json::from_slice(&payload).unwrap();
        assert_eq!(payload["role"], "anon");
        assert_eq!(payload["iss"], "supabase");
        assert!(payload["exp"].as_i64().unwrap() > payload["iat"].as_i64().unwrap());

        // Signature must verify with the same key.
        let signing_input = format!("{}.{}", parts[0], parts[1]);
        let mut mac = Hmac::<Sha256>::new_from_slice(key.as_bytes()).unwrap();
        mac.update(signing_input.as_bytes());
        let expected = BASE64URL.encode(mac.finalize().into_bytes());
        assert_eq!(parts[2], expected);

        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(
                &BASE64URL
                    .decode(supabase_jwt("service_role", key).split('.').nth(1).unwrap())
                    .unwrap()
            )
            .unwrap()["role"],
            "service_role"
        );
    }

    #[test]
    fn is_secret_command_classification() {
        assert!(is_secret_command("PASSWORD"));
        assert!(is_secret_command("HEX_64"));
        assert!(is_secret_command("SUPABASEANON"));
        assert!(!is_secret_command("USER"));
        assert!(!is_secret_command("FQDN"));
        assert!(!is_secret_command("URL"));
    }
}
