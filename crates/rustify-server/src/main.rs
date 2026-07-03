#![forbid(unsafe_code)]

//! Rustify server entrypoint.
//!
//! Wiring order (contract F): pool → migrate → seed → event bus →
//! JobQueue workers (registry with `deploy` + `server_validate` handlers) →
//! status-sync scheduler (30s) → axum on `0.0.0.0:8000`.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use tokio::net::TcpListener;
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;

use rustify_core::CommandExecutor;
use rustify_db::pool::MIGRATOR;
use rustify_db::repos::seed_default;
use rustify_deploy::admission::DEPLOY_JOB_KIND;
use rustify_deploy::{
    DeployEngineDeps, DeployJobHandler, SERVICE_DEPLOY_KIND, SERVICE_STOP_KIND, ServerSetupHandler,
    ServiceDeployHandler, ServiceStopHandler, StartDatabaseHandler, StopDatabaseHandler,
    status_sync_task,
};
use rustify_jobs::{JobQueue, JobRegistry, Scheduler};
use rustify_server::app::{AppState, Config};
use rustify_server::build_router;
use rustify_ssh::SshExecutor;

/// Number of concurrent job workers.
const JOB_WORKERS: usize = 4;
/// Capacity of the WS broadcast channel (contract F).
const EVENT_CHANNEL_CAP: usize = 1024;
/// How often the container-status reconciliation sweep runs (Coolify: 30s).
const STATUS_SYNC_PERIOD: Duration = Duration::from_secs(30);

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt().init();

    let database_url =
        std::env::var("DATABASE_URL").map_err(|_| "DATABASE_URL must be set (postgres:// DSN)")?;

    // pool → migrate → seed
    let pool = rustify_db::connect(&database_url).await?;
    MIGRATOR.run(&pool).await?;
    seed_default(&pool).await?;
    tracing::info!("database migrated and seeded");

    // event bus
    let (events, _rx) = broadcast::channel(EVENT_CHANNEL_CAP);

    // Runtime configuration and the on-disk SSH working directories. The mux
    // dir is handed to the executor; the key dir is where the deploy engine
    // materialises each server's private key `0600` on demand.
    let config = Config::from_env();
    tokio::fs::create_dir_all(&config.ssh_mux_dir).await?;
    tokio::fs::create_dir_all(&config.ssh_key_dir).await?;

    // The deploy engine's shared dependency bundle: one SSH executor for all
    // servers (each call passes a per-server ServerConn), the DB pool, and the
    // WS event bus.
    let executor: Arc<dyn CommandExecutor> = Arc::new(SshExecutor::new(config.ssh_mux_dir.clone()));
    let deps = DeployEngineDeps::new(executor, pool.clone(), events.clone());

    // Job workers with the real registry: `deploy` runs a deployment, and
    // `server_validate` provisions/validates a server.
    let queue = JobQueue::new(pool.clone());
    let shutdown = CancellationToken::new();
    let mut registry = JobRegistry::new();
    registry.register(
        DEPLOY_JOB_KIND,
        Arc::new(DeployJobHandler::new(deps.clone(), shutdown.clone())),
    );
    registry.register(
        "server_validate",
        Arc::new(ServerSetupHandler::new(deps.clone())),
    );
    registry.register(
        "database_start",
        Arc::new(StartDatabaseHandler::new(deps.clone())),
    );
    registry.register(
        "database_stop",
        Arc::new(StopDatabaseHandler::new(deps.clone())),
    );
    registry.register(
        SERVICE_DEPLOY_KIND,
        Arc::new(ServiceDeployHandler::new(deps.clone())),
    );
    registry.register(
        SERVICE_STOP_KIND,
        Arc::new(ServiceStopHandler::new(deps.clone())),
    );
    let worker_handle = {
        let queue = queue.clone();
        let shutdown = shutdown.clone();
        tokio::spawn(async move {
            queue.run(JOB_WORKERS, registry, shutdown).await;
        })
    };

    // Container-status reconciliation sweep every 30s, stopped on shutdown.
    let mut scheduler = Scheduler::new(shutdown.clone());
    scheduler.every(STATUS_SYNC_PERIOD, "status_sync", status_sync_task(deps));

    let state = AppState {
        pool,
        queue,
        events,
        config,
    };
    let app = build_router(state);

    let addr = SocketAddr::from(([0, 0, 0, 0], 8000));
    let listener = TcpListener::bind(addr).await?;
    tracing::info!(%addr, "rustify-server listening");

    let server_shutdown = shutdown.clone();
    axum::serve(listener, app)
        .with_graceful_shutdown(async move {
            let _ = tokio::signal::ctrl_c().await;
            tracing::info!("shutdown signal received, draining");
            server_shutdown.cancel();
        })
        .await?;

    // Let in-flight jobs drain and the scheduler stop.
    shutdown.cancel();
    let _ = worker_handle.await;
    scheduler.join().await;
    Ok(())
}
