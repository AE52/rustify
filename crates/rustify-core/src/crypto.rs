//! AES-256-GCM encryption for at-rest secrets (private keys, env var values).
//!
//! Key: `RUSTIFY_SECRET_KEY` env var, base64-encoded 32 bytes, read once via
//! `OnceLock`. Blob layout: `nonce (12 bytes) || ciphertext (incl. 16-byte tag)`.
//! Tampering with any byte makes `decrypt` return an error.

use std::sync::OnceLock;

use aes_gcm::aead::{Aead, AeadCore, KeyInit, OsRng};
use aes_gcm::{Aes256Gcm, Key, Nonce};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use hmac::{Hmac, Mac};
use sha2::Sha256;

use crate::error::CoreError;

pub const KEY_ENV: &str = "RUSTIFY_SECRET_KEY";

/// AES-GCM nonce size in bytes (96 bits).
const NONCE_LEN: usize = 12;
/// AES-GCM authentication tag size in bytes.
const TAG_LEN: usize = 16;

static KEY: OnceLock<Result<[u8; 32], CoreError>> = OnceLock::new();

/// Read and parse `RUSTIFY_SECRET_KEY` exactly once; the result (including a
/// configuration error) is cached for the process lifetime.
fn key() -> Result<[u8; 32], CoreError> {
    KEY.get_or_init(|| {
        let raw = std::env::var(KEY_ENV).map_err(|_| CoreError::MissingKey)?;
        parse_key(&raw)
    })
    .clone()
}

fn parse_key(raw: &str) -> Result<[u8; 32], CoreError> {
    let bytes = BASE64
        .decode(raw.trim())
        .map_err(|e| CoreError::InvalidKey(format!("not valid base64: {e}")))?;
    <[u8; 32]>::try_from(bytes.as_slice())
        .map_err(|_| CoreError::InvalidKey(format!("expected 32 bytes, got {}", bytes.len())))
}

/// Bypass the environment in unit tests (setting env vars is `unsafe` in
/// edition 2024 and racy across parallel tests).
#[cfg(test)]
fn set_key_for_tests(key: [u8; 32]) {
    let _ = KEY.set(Ok(key));
}

/// Encrypt `plain` with a fresh random nonce. Returns `nonce || ciphertext`.
///
/// # Panics
/// The pinned signature is infallible, so a missing/invalid
/// `RUSTIFY_SECRET_KEY` (a fatal deployment misconfiguration) panics.
pub fn encrypt(plain: &[u8]) -> Vec<u8> {
    match try_encrypt(plain) {
        Ok(blob) => blob,
        Err(e) => panic!("rustify crypto misconfiguration: {e}"),
    }
}

fn try_encrypt(plain: &[u8]) -> Result<Vec<u8>, CoreError> {
    let key = key()?;
    let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(&key));
    let nonce = Aes256Gcm::generate_nonce(&mut OsRng);
    let ciphertext = cipher
        .encrypt(&nonce, plain)
        .map_err(|_| CoreError::Encrypt)?;
    let mut blob = Vec::with_capacity(NONCE_LEN + ciphertext.len());
    blob.extend_from_slice(&nonce);
    blob.extend_from_slice(&ciphertext);
    Ok(blob)
}

/// Decrypt a blob produced by [`encrypt`]. Fails on truncation or tampering.
pub fn decrypt(blob: &[u8]) -> Result<Vec<u8>, CoreError> {
    if blob.len() < NONCE_LEN + TAG_LEN {
        return Err(CoreError::Decrypt);
    }
    let key = key()?;
    let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(&key));
    let (nonce, ciphertext) = blob.split_at(NONCE_LEN);
    cipher
        .decrypt(Nonce::from_slice(nonce), ciphertext)
        .map_err(|_| CoreError::Decrypt)
}

/// Deterministic HMAC-SHA256 fingerprint of a set of build-time variables, used
/// as the Railpack `--build-arg secrets-hash=` cache buster.
///
/// Parity with Coolify's `generate_secrets_hash` (ApplicationDeploymentJob.php:4086):
/// the variables are rendered as `KEY=VALUE`, sorted by key, joined with `|`,
/// then HMAC-SHA256'd. Coolify keys the HMAC on `config('app.key')`; here the
/// deterministic key is the decoded `RUSTIFY_SECRET_KEY`, so the digest is
/// stable across deployments (preserving Docker build cache) yet secret. The
/// output is lowercase hex, matching PHP's `hash_hmac`.
///
/// Only the sorted `KEY=VALUE` string is hashed — never logged — so values are
/// never exposed even though the digest changes whenever a value does.
pub fn secrets_hash(vars: &[(String, String)]) -> Result<String, CoreError> {
    let key = key()?;
    let mut pairs: Vec<String> = vars.iter().map(|(k, v)| format!("{k}={v}")).collect();
    pairs.sort();
    let joined = pairs.join("|");
    let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(&key)
        .map_err(|_| CoreError::InvalidKey("hmac".into()))?;
    mac.update(joined.as_bytes());
    Ok(to_hex(&mac.finalize().into_bytes()))
}

/// Lowercase-hex encode, matching PHP's `hash_hmac('sha256', ...)` output.
fn to_hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        let _ = write!(s, "{b:02x}");
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    const NONCE_LEN: usize = 12;
    const TAG_LEN: usize = 16;

    fn init_key() {
        set_key_for_tests(*b"0123456789abcdef0123456789abcdef");
    }

    #[test]
    fn roundtrip() {
        init_key();
        let plain = b"super secret private key material";
        let blob = encrypt(plain);
        assert_ne!(&blob[NONCE_LEN..], plain.as_slice());
        assert_eq!(blob.len(), plain.len() + NONCE_LEN + TAG_LEN);
        assert_eq!(decrypt(&blob).unwrap(), plain);
    }

    #[test]
    fn roundtrip_empty_plaintext() {
        init_key();
        let blob = encrypt(b"");
        assert_eq!(blob.len(), NONCE_LEN + TAG_LEN);
        assert_eq!(decrypt(&blob).unwrap(), b"");
    }

    #[test]
    fn nonce_is_random_per_call() {
        init_key();
        let a = encrypt(b"same input");
        let b = encrypt(b"same input");
        assert_ne!(a, b, "two encryptions must not share a nonce/ciphertext");
    }

    #[test]
    fn tampered_ciphertext_fails() {
        init_key();
        let mut blob = encrypt(b"payload");
        let last = blob.len() - 1;
        blob[last] ^= 0x01; // flip one bit in the tag
        assert_eq!(decrypt(&blob), Err(CoreError::Decrypt));

        let mut blob = encrypt(b"payload");
        blob[NONCE_LEN] ^= 0x01; // flip one bit in the ciphertext body
        assert_eq!(decrypt(&blob), Err(CoreError::Decrypt));

        let mut blob = encrypt(b"payload");
        blob[0] ^= 0x01; // flip one bit in the nonce
        assert_eq!(decrypt(&blob), Err(CoreError::Decrypt));
    }

    #[test]
    fn truncated_blob_fails() {
        init_key();
        let blob = encrypt(b"payload");
        assert_eq!(decrypt(&blob[..blob.len() - 1]), Err(CoreError::Decrypt));
        assert_eq!(decrypt(&[]), Err(CoreError::Decrypt));
        assert_eq!(decrypt(&blob[..NONCE_LEN]), Err(CoreError::Decrypt));
    }

    #[test]
    fn parse_key_accepts_base64_32_bytes() {
        // base64 of 32 zero bytes
        let raw = "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=";
        assert_eq!(parse_key(raw).unwrap(), [0u8; 32]);
        // surrounding whitespace tolerated
        assert_eq!(parse_key(&format!(" {raw}\n")).unwrap(), [0u8; 32]);
    }

    #[test]
    fn secrets_hash_is_deterministic_and_order_independent() {
        init_key();
        let a = secrets_hash(&[("B".into(), "2".into()), ("A".into(), "1".into())]).unwrap();
        let b = secrets_hash(&[("A".into(), "1".into()), ("B".into(), "2".into())]).unwrap();
        assert_eq!(a, b, "sorting by key makes input order irrelevant");
        // 32-byte digest → 64 lowercase-hex chars.
        assert_eq!(a.len(), 64);
        assert!(
            a.chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
        );
    }

    #[test]
    fn secrets_hash_changes_when_a_value_changes() {
        init_key();
        let base = secrets_hash(&[("TOKEN".into(), "old".into())]).unwrap();
        let bumped = secrets_hash(&[("TOKEN".into(), "new".into())]).unwrap();
        assert_ne!(base, bumped, "a changed secret must bust the cache");
    }

    #[test]
    fn parse_key_rejects_bad_input() {
        assert!(matches!(
            parse_key("not-base64!!!"),
            Err(CoreError::InvalidKey(_))
        ));
        // valid base64 but only 16 bytes
        assert!(matches!(
            parse_key("AAAAAAAAAAAAAAAAAAAAAA=="),
            Err(CoreError::InvalidKey(_))
        ));
    }
}
