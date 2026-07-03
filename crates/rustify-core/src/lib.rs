#![forbid(unsafe_code)]

//! rustify-core: shared domain types and helpers for all Rustify crates.
//!
//! Contains the pinned contracts C1 (execution trait), C2 (deployment state
//! machine) and C3 (log line shape), plus crypto, id generation and secret
//! redaction helpers.

pub mod backup_cmd;
pub mod cron;
pub mod crypto;
pub mod db_engine;
pub mod deployment;
pub mod error;
pub mod events;
pub mod exec;
pub mod github_jwt;
pub mod ids;
pub mod logline;
pub mod notify;
pub mod passwords;
pub mod railpack;
pub mod redact;
pub mod retention;
pub mod role;
pub mod service_vars;
pub mod webhook;

pub use db_engine::{DatabaseCredentials, DatabaseEngine, EngineDescriptor};
pub use deployment::{BuildPack, DeploymentStatus};
pub use error::CoreError;
pub use events::WsEvent;
pub use exec::{CommandExecutor, ExecError, ExecEvent, ExecOpts, ExecOutput, ServerConn};
pub use logline::LogLine;
pub use notify::{Channel, NotifEvent, default_event_matrix, should_send};
pub use passwords::gen_password;
pub use redact::redact;
pub use retention::{ExecMeta, select_for_deletion};
pub use role::Role;
