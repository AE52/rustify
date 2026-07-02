#![forbid(unsafe_code)]

//! rustify-db: PostgreSQL persistence layer for Rustify.
//!
//! Owns the schema (migration `0001_init.sql`, contract C6), the connection
//! pool + embedded [`MIGRATOR`](pool::MIGRATOR), and one repository struct per
//! aggregate. Secrets (private keys, env var values) are encrypted with
//! [`rustify_core::crypto`] before they touch the database and never logged.

pub mod pool;
pub mod repos;

pub use pool::{MIGRATOR, connect};

/// Errors surfaced by the persistence layer.
#[derive(Debug, thiserror::Error)]
pub enum DbError {
    #[error("database: {0}")]
    Sqlx(#[from] sqlx::Error),
    #[error("migration: {0}")]
    Migrate(#[from] sqlx::migrate::MigrateError),
    #[error("crypto: {0}")]
    Crypto(#[from] rustify_core::CoreError),
    #[error("password hashing failed")]
    PasswordHash,
    #[error("stored value is not valid utf-8")]
    Utf8,
    #[error("configuration: {0}")]
    Config(String),
    #[error("not found")]
    NotFound,
}

/// Convenience alias for fallible repository operations.
pub type DbResult<T> = Result<T, DbError>;
