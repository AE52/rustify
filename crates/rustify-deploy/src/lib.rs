#![forbid(unsafe_code)]

//! rustify-deploy: the deployment engine — a behavioural port of Coolify's
//! `ApplicationDeploymentJob` (app/Jobs/ApplicationDeploymentJob.php).
//!
//! The engine drives a deployment end to end against a
//! [`rustify_core::CommandExecutor`] (real SSH in production, a scripted fake in
//! tests): claim the queued row, bring up the build helper container, resolve
//! and clone the git ref, build via the selected buildpack, write the runtime
//! config, roll the new container in with a health gate, and always tear the
//! helper down. Every step streams [`rustify_core::LogLine`]s to the database
//! and broadcasts [`WsEvent`]s, and checks for cancellation before every remote
//! command.
//!
//! Public surface:
//! - [`DeployEngineDeps`] — the shared dependency bundle (executor, pool, event bus).
//! - [`DeployJobHandler`] — [`rustify_jobs::JobHandler`] for kind `"deploy"`.
//! - [`ServerSetupHandler`] — [`rustify_jobs::JobHandler`] for kind `"server_validate"`.
//! - [`status_sync_task`] — the 30s scheduler closure factory.
//! - [`run_deployment`] — the engine entry point (used by the handler and tests).

use std::sync::Arc;

use rustify_core::CommandExecutor;
use rustify_core::events::WsEvent;
use sqlx::PgPool;
use tokio::sync::broadcast;

pub mod admission;
pub mod app_lifecycle;
pub mod backup;
pub mod build_server;
pub mod buildpacks;
pub mod cloudflared;
pub mod database;
pub mod engine;
pub mod envfile;
pub mod git;
pub mod github;
pub mod metrics;
pub mod pr_comment;
pub mod preview;
pub mod preview_cleanup;
pub mod proxy;
pub mod rolling;
pub mod scheduled_task;
pub mod server_setup;
pub mod service;
pub mod status_sync;
pub mod system_cron;

pub use app_lifecycle::{
    APP_RESTART_KIND, APP_STOP_KIND, RestartApplicationHandler, StopApplicationHandler,
    restart_application, stop_application,
};
pub use backup::{
    DATABASE_BACKUP_KIND, DatabaseBackupHandler, backup_dispatcher_task, cron_is_due,
    dispatch_due_backups, run_backup,
};
pub use build_server::{
    BuildTargets, plan_build_targets, pull_image_command, push_image_command, push_then_pull,
    registry_image_ref,
};
pub use cloudflared::{
    CONFIGURE_CLOUDFLARED_KIND, ConfigureCloudflaredHandler, cloudflared_compose,
    configure_cloudflared, disable_cloudflared,
};
pub use database::{StartDatabaseHandler, StopDatabaseHandler, start_database, stop_database};
pub use engine::{DeployJobHandler, run_deployment};
pub use metrics::{metrics_collector_task, metrics_retention_task};
pub use preview_cleanup::{PREVIEW_CLEANUP_KIND, PreviewCleanupHandler, cleanup_preview};
pub use proxy::{
    PROXY_RESTART_KIND, PROXY_START_KIND, PROXY_STOP_KIND, ProxyRestartHandler, ProxyStartHandler,
    ProxyStopHandler, restart_proxy, start_proxy, stop_proxy,
};
pub use scheduled_task::{
    SCHEDULED_TASK_KIND, ScheduledTaskHandler, dispatch_due_tasks, run_scheduled_task,
    task_dispatcher_task,
};
pub use server_setup::ServerSetupHandler;
pub use service::{
    SERVICE_DEPLOY_KIND, SERVICE_STOP_KIND, ServiceDeployHandler, ServiceStopHandler,
    deploy_service, stop_service,
};
pub use status_sync::status_sync_task;
pub use system_cron::{
    cleanup_docker_all, cleanup_old_records, daily_cleanup_task, docker_cleanup_task,
    remove_stale_mux_sockets, ssh_mux_cleanup_task,
};

/// Broadcast channel of realtime events (Contract C4).
pub type EventBus = broadcast::Sender<WsEvent>;

/// Shared dependencies every engine entry point needs. Cheap to clone: the
/// executor is an `Arc`, the pool and broadcast sender are handle types.
#[derive(Clone)]
pub struct DeployEngineDeps {
    pub executor: Arc<dyn CommandExecutor>,
    pub pool: PgPool,
    pub events: EventBus,
}

impl DeployEngineDeps {
    pub fn new(executor: Arc<dyn CommandExecutor>, pool: PgPool, events: EventBus) -> Self {
        Self {
            executor,
            pool,
            events,
        }
    }
}

/// Errors the engine surfaces. Deployment-level failures (build/unhealthy) are
/// recorded as a `Failed` status and are *not* propagated as job errors;
/// infrastructure errors (DB, missing rows) are, so the job queue may retry.
#[derive(Debug, thiserror::Error)]
pub enum DeployError {
    #[error("database: {0}")]
    Db(#[from] rustify_db::DbError),
    #[error("sqlx: {0}")]
    Sqlx(#[from] sqlx::Error),
    #[error("exec: {0}")]
    Exec(#[from] rustify_core::ExecError),
    #[error("deployment {0} not found")]
    NotFound(String),
    #[error("required row missing: {0}")]
    Missing(String),
    #[error("deployment cancelled")]
    Cancelled,
    #[error("build failed: {0}")]
    Build(String),
    #[error("new container failed its health check")]
    Unhealthy,
    #[error("invalid deployment payload: {0}")]
    Payload(String),
    #[error("job queue: {0}")]
    Jobs(String),
}
