#![forbid(unsafe_code)]

//! rustify-core: shared domain types and helpers for all Rustify crates.
//!
//! Contains the pinned contracts C1 (execution trait), C2 (deployment state
//! machine), C3 (log line shape) and C4 (WS event envelope), plus crypto,
//! id generation and secret redaction helpers.

pub mod crypto;
pub mod deployment;
pub mod error;
pub mod events;
pub mod exec;
pub mod ids;
pub mod logline;
pub mod redact;

pub use deployment::{BuildPack, DeploymentStatus};
pub use error::CoreError;
pub use events::WsEvent;
pub use exec::{CommandExecutor, ExecError, ExecEvent, ExecOpts, ExecOutput, ServerConn};
pub use logline::LogLine;
pub use redact::redact;
