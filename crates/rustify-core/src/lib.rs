#![forbid(unsafe_code)]

//! rustify-core: shared domain types and helpers for all Rustify crates.
//!
//! Contains the pinned contracts C1 (execution trait), C2 (deployment state
//! machine) and C3 (log line shape), plus crypto, id generation and secret
//! redaction helpers.

pub mod crypto;
pub mod deployment;
pub mod error;
pub mod exec;
pub mod ids;
pub mod logline;
pub mod redact;

pub use deployment::{BuildPack, DeploymentStatus};
pub use error::CoreError;
pub use exec::{CommandExecutor, ExecError, ExecEvent, ExecOpts, ExecOutput, ServerConn};
pub use logline::LogLine;
pub use redact::redact;
