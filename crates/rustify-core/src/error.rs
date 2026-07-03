/// Errors produced by `rustify-core` helpers (currently only `crypto`).
///
/// `Clone` is required because the parsed secret key is cached in a
/// `OnceLock` and a config error must be returned to every caller.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CoreError {
    #[error("RUSTIFY_SECRET_KEY is not set")]
    MissingKey,
    #[error("invalid RUSTIFY_SECRET_KEY: {0}")]
    InvalidKey(String),
    #[error("encryption failed")]
    Encrypt,
    #[error("decryption failed: blob is invalid or has been tampered with")]
    Decrypt,
    #[error("github app jwt error: {0}")]
    Jwt(String),
}
