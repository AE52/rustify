#![forbid(unsafe_code)]

//! Rustify server entrypoint.
//!
//! Wiring order (contract F): pool → migrate → seed → event bus →
//! JobQueue workers (empty registry; Task Z registers `deploy` /
//! `server_validate` / status handlers) → axum on `0.0.0.0:8000`.

use std::net::SocketAddr;

use tokio::net::TcpListener;
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;

use rustify_db::pool::MIGRATOR;
use rustify_db::repos::seed_default;
use rustify_jobs::{JobQueue, JobRegistry};
use rustify_server::app::{AppState, Config};
use rustify_server::build_router;

/// Number of concurrent job workers.
const JOB_WORKERS: usize = 4;
/// Capacity of the WS broadcast channel (contract F).
const EVENT_CHANNEL_CAP: usize = 1024;

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

    // job workers with an EMPTY registry (Task Z registers real handlers)
    let queue = JobQueue::new(pool.clone());
    let shutdown = CancellationToken::new();
    let worker_handle = {
        let queue = queue.clone();
        let shutdown = shutdown.clone();
        tokio::spawn(async move {
            queue.run(JOB_WORKERS, JobRegistry::new(), shutdown).await;
        })
    };

    let state = AppState {
        pool,
        queue,
        events,
        config: Config::from_env(),
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

    // Let in-flight jobs drain.
    shutdown.cancel();
    let _ = worker_handle.await;
    Ok(())
}
