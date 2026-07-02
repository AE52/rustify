//! Periodic container-status reconciliation (the 30s scheduler task).
//!
//! Behavioural port of Coolify's `ContainerStatusJob`: for every usable server,
//! read `docker ps -a`, map the `rustify.applicationUuid` label back to the
//! application row, and update `applications.status`. Crash-looping containers
//! (restart count ≥ `max_restart_count`) are stopped and marked `crashed`.
//! Any status change is broadcast as `application_status_changed` (Contract C4).

use std::future::Future;
use std::pin::Pin;

use rustify_core::events::WsEvent;
use rustify_db::repos::{ApplicationRepo, ServerRepo};
use rustify_docker::{ManagedContainer, parse_containers};

use crate::{DeployEngineDeps, DeployError};

/// Map a Docker container `State` to the `applications.status` string.
pub fn desired_status(state: &str) -> &'static str {
    match state.to_lowercase().as_str() {
        "running" => "running",
        "restarting" => "restarting",
        "paused" => "paused",
        "created" => "created",
        _ => "exited",
    }
}

/// Build the scheduler closure for [`rustify_jobs::Scheduler::every`]. Runs one
/// reconciliation sweep per tick; errors are logged and swallowed so the loop
/// keeps running.
pub fn status_sync_task(
    deps: DeployEngineDeps,
) -> impl Fn() -> Pin<Box<dyn Future<Output = ()> + Send>> + Send + 'static {
    move || {
        let deps = deps.clone();
        Box::pin(async move {
            if let Err(e) = sync_all(&deps).await {
                tracing::warn!(error = %e, "status sync sweep failed");
            }
        })
    }
}

/// One reconciliation sweep across all usable servers.
pub async fn sync_all(deps: &DeployEngineDeps) -> Result<(), DeployError> {
    let servers: Vec<(i64, String)> =
        sqlx::query_as("SELECT id, uuid FROM servers WHERE usable = true ORDER BY id")
            .fetch_all(&deps.pool)
            .await?;
    let server_repo = ServerRepo::new(deps.pool.clone());
    let app_repo = ApplicationRepo::new(deps.pool.clone());

    for (server_id, uuid) in servers {
        let Some(server) = server_repo.get_by_uuid(&uuid).await? else {
            continue;
        };
        let settings = server_repo.settings(server_id).await?;
        let ct = settings
            .map(|s| s.connection_timeout.max(1) as u32)
            .unwrap_or(10);
        let conn = crate::engine::build_conn(&deps.pool, &server, ct).await;

        let ps = "docker ps -a --filter label=rustify.managed=true --format '{{json .}}'";
        let Ok(out) = deps
            .executor
            .exec(&conn, ps, rustify_core::ExecOpts::default())
            .await
        else {
            continue; // server transiently unreachable; try next sweep
        };
        let containers = parse_containers(&out.stdout);
        reconcile(deps, &app_repo, &conn, &containers).await;
    }
    Ok(())
}

/// Reconcile one server's containers against the application rows.
async fn reconcile(
    deps: &DeployEngineDeps,
    app_repo: &ApplicationRepo,
    conn: &rustify_core::ServerConn,
    containers: &[ManagedContainer],
) {
    for c in containers {
        let Some(app_uuid) = &c.application_uuid else {
            continue;
        };
        let Ok(Some(app)) = app_repo.get_by_uuid(app_uuid).await else {
            continue;
        };

        // Crash-loop detection: inspect the restart count and, once it reaches
        // the application's cap, stop the container and mark it crashed.
        let restart_count = inspect_restart_count(deps, conn, &c.name).await;
        let new_status = if restart_count >= app.max_restart_count && app.max_restart_count > 0 {
            let _ = deps
                .executor
                .exec(
                    conn,
                    &format!("docker stop {} >/dev/null 2>&1 || true", c.name),
                    rustify_core::ExecOpts::default(),
                )
                .await;
            "crashed".to_string()
        } else {
            desired_status(&c.state).to_string()
        };

        let _ = sqlx::query("UPDATE applications SET restart_count = $2 WHERE id = $1")
            .bind(app.id)
            .bind(restart_count)
            .execute(&deps.pool)
            .await;

        if app.status != new_status && app_repo.set_status(app.id, &new_status).await.is_ok() {
            let _ = deps
                .events
                .send(WsEvent::application_status_changed(app_uuid, &new_status));
        }
    }
}

/// `docker inspect --format '{{.RestartCount}}'`, defaulting to 0 on any error.
async fn inspect_restart_count(
    deps: &DeployEngineDeps,
    conn: &rustify_core::ServerConn,
    name: &str,
) -> i32 {
    let cmd = format!("docker inspect --format='{{{{.RestartCount}}}}' {name}");
    deps.executor
        .exec(conn, &cmd, rustify_core::ExecOpts::default())
        .await
        .ok()
        .and_then(|o| o.stdout.trim().parse::<i32>().ok())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_mapping() {
        assert_eq!(desired_status("running"), "running");
        assert_eq!(desired_status("Running"), "running");
        assert_eq!(desired_status("exited"), "exited");
        assert_eq!(desired_status("restarting"), "restarting");
        assert_eq!(desired_status("dead"), "exited");
    }
}
