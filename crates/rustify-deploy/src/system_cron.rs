//! The essential system cron set — the periodic housekeeping jobs Coolify runs
//! from `app/Console/Kernel.php`, reduced to the self-hosted surface (no
//! cloud/stripe/CDN/update-check jobs):
//!
//! - [`docker_cleanup_task`] — per-server `docker image prune -af` + `docker
//!   builder prune -af` (Coolify `DockerCleanupJob` / `CleanupDocker`),
//!   default hourly.
//! - [`ssh_mux_cleanup_task`] — reap stale ControlMaster sockets from the mux
//!   dir (`CleanupStaleMultiplexedConnections`), hourly.
//! - [`daily_cleanup_task`] — prune old `deployment_logs` and
//!   `scheduled_task_executions` (`cleanup:database`), daily.
//!
//! `server-health` is the existing [`crate::status_sync_task`], kept as-is.

use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::time::{Duration, SystemTime};

use rustify_core::ExecOpts;
use rustify_db::repos::ServerRepo;
use sqlx::PgPool;

use crate::{DeployEngineDeps, DeployError};

/// Default retention for the daily record prune.
pub const DEFAULT_RETENTION_DAYS: i64 = 7;

type Task = Pin<Box<dyn Future<Output = ()> + Send>>;

/// Per-server docker image/builder prune sweep (default hourly).
pub fn docker_cleanup_task(deps: DeployEngineDeps) -> impl Fn() -> Task + Send + 'static {
    move || {
        let deps = deps.clone();
        Box::pin(async move {
            if let Err(e) = cleanup_docker_all(&deps).await {
                tracing::warn!(error = %e, "docker cleanup sweep failed");
            }
        })
    }
}

/// Run `docker image prune -af` and `docker builder prune -af` on every usable
/// server. Individual server failures are logged and skipped.
pub async fn cleanup_docker_all(deps: &DeployEngineDeps) -> Result<(), DeployError> {
    let servers: Vec<(i64, String)> =
        sqlx::query_as("SELECT id, uuid FROM servers WHERE usable = true ORDER BY id")
            .fetch_all(&deps.pool)
            .await?;
    let server_repo = ServerRepo::new(deps.pool.clone());

    for (server_id, uuid) in servers {
        let Some(server) = server_repo.get_by_uuid(&uuid).await? else {
            continue;
        };
        let ct = server_repo
            .settings(server_id)
            .await?
            .map(|s| s.connection_timeout.max(1) as u32)
            .unwrap_or(10);
        let conn = crate::engine::build_conn(&deps.pool, &server, ct).await;
        for cmd in DOCKER_CLEANUP_COMMANDS {
            let _ = deps.executor.exec(&conn, cmd, ExecOpts::default()).await;
        }
    }
    Ok(())
}

/// The prune commands run per server (CleanupDocker.php:53, :94).
pub const DOCKER_CLEANUP_COMMANDS: [&str; 2] =
    ["docker image prune -af", "docker builder prune -af"];

/// Reap stale ControlMaster sockets older than `max_age` from `mux_dir` (hourly).
pub fn ssh_mux_cleanup_task(
    mux_dir: PathBuf,
    max_age: Duration,
) -> impl Fn() -> Task + Send + 'static {
    move || {
        let mux_dir = mux_dir.clone();
        Box::pin(async move {
            let removed = remove_stale_mux_sockets(&mux_dir, max_age);
            if removed > 0 {
                tracing::info!(removed, "reaped stale ssh mux sockets");
            }
        })
    }
}

/// Remove mux socket files whose last-modified age exceeds `max_age`. Returns
/// the number removed. A missing directory is a no-op.
pub fn remove_stale_mux_sockets(mux_dir: &Path, max_age: Duration) -> usize {
    let Ok(entries) = std::fs::read_dir(mux_dir) else {
        return 0;
    };
    let now = SystemTime::now();
    let mut removed = 0;
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(meta) = entry.metadata() else { continue };
        if !meta.is_file() {
            continue;
        }
        let age = meta
            .modified()
            .ok()
            .and_then(|m| now.duration_since(m).ok())
            .unwrap_or(Duration::ZERO);
        if age >= max_age && std::fs::remove_file(&path).is_ok() {
            removed += 1;
        }
    }
    removed
}

/// Daily prune of old `deployment_logs` and `scheduled_task_executions`.
pub fn daily_cleanup_task(pool: PgPool, retention_days: i64) -> impl Fn() -> Task + Send + 'static {
    move || {
        let pool = pool.clone();
        Box::pin(async move {
            if let Err(e) = cleanup_old_records(&pool, retention_days).await {
                tracing::warn!(error = %e, "daily record cleanup failed");
            }
        })
    }
}

/// Delete `deployment_logs` and finished `scheduled_task_executions` older than
/// `retention_days`. Returns `(logs_deleted, executions_deleted)`.
pub async fn cleanup_old_records(
    pool: &PgPool,
    retention_days: i64,
) -> Result<(u64, u64), DeployError> {
    let logs = sqlx::query(
        "DELETE FROM deployment_logs WHERE created_at < now() - ($1 || ' days')::interval",
    )
    .bind(retention_days.to_string())
    .execute(pool)
    .await?
    .rows_affected();
    let execs = sqlx::query(
        "DELETE FROM scheduled_task_executions
         WHERE started_at < now() - ($1 || ' days')::interval
           AND finished_at IS NOT NULL",
    )
    .bind(retention_days.to_string())
    .execute(pool)
    .await?
    .rows_affected();
    Ok((logs, execs))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn docker_cleanup_commands_are_image_and_builder_prune() {
        assert_eq!(DOCKER_CLEANUP_COMMANDS[0], "docker image prune -af");
        assert_eq!(DOCKER_CLEANUP_COMMANDS[1], "docker builder prune -af");
    }

    #[test]
    fn stale_sockets_removed_by_age() {
        let dir = std::env::temp_dir().join(format!("rustify-mux-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let a = dir.join("mux_a");
        let b = dir.join("mux_b");
        std::fs::write(&a, b"").unwrap();
        std::fs::write(&b, b"").unwrap();

        // A huge max_age keeps everything.
        assert_eq!(
            remove_stale_mux_sockets(&dir, Duration::from_secs(86_400)),
            0
        );
        assert!(a.exists() && b.exists());

        // max_age of zero treats every socket as stale.
        assert_eq!(remove_stale_mux_sockets(&dir, Duration::ZERO), 2);
        assert!(!a.exists() && !b.exists());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_mux_dir_is_noop() {
        let dir = std::env::temp_dir().join("rustify-mux-does-not-exist-xyz");
        assert_eq!(remove_stale_mux_sockets(&dir, Duration::ZERO), 0);
    }
}
