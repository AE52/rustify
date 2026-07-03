//! Application stop/restart lifecycle handlers.
//!
//! Behavioural port of Coolify's `StopApplication` /`RestartApplication`
//! (app/Actions/Application/StopApplication.php, RestartApplication.php): stop is
//! `docker stop` + `docker rm` of every container carrying the app's
//! `rustify.applicationUuid=<uuid>` label on its server; restart stops the old
//! container(s) and brings the stored compose back up. Both reuse
//! [`DeployEngineDeps`] and run over the server's [`rustify_ssh::SshExecutor`].

use async_trait::async_trait;
use serde_json::Value;

use rustify_core::events::WsEvent;
use rustify_core::{ExecOpts, ServerConn};
use rustify_db::repos::{Application, ApplicationRepo, ServerRepo};
use rustify_jobs::JobHandler;

use crate::engine::build_conn;
use crate::{DeployEngineDeps, DeployError};

/// Job kind: stop an application's containers.
pub const APP_STOP_KIND: &str = "app_stop";
/// Job kind: restart an application (stop, then bring the stored compose up).
pub const APP_RESTART_KIND: &str = "app_restart";

/// Server-side app config root (matches the deploy engine's `app_config_dir`).
const APP_CONFIG_DIR: &str = "/data/rustify/applications";

/// [`JobHandler`] for kind `"app_stop"`, payload `{"application_uuid": ".."}`.
pub struct StopApplicationHandler {
    deps: DeployEngineDeps,
}

impl StopApplicationHandler {
    pub fn new(deps: DeployEngineDeps) -> Self {
        Self { deps }
    }
}

#[async_trait]
impl JobHandler for StopApplicationHandler {
    async fn run(&self, payload: Value) -> anyhow::Result<()> {
        let uuid = application_uuid(&payload)?;
        stop_application(&self.deps, uuid)
            .await
            .map_err(|e| anyhow::anyhow!(e.to_string()))?;
        Ok(())
    }
}

/// [`JobHandler`] for kind `"app_restart"`, payload `{"application_uuid": ".."}`.
pub struct RestartApplicationHandler {
    deps: DeployEngineDeps,
}

impl RestartApplicationHandler {
    pub fn new(deps: DeployEngineDeps) -> Self {
        Self { deps }
    }
}

#[async_trait]
impl JobHandler for RestartApplicationHandler {
    async fn run(&self, payload: Value) -> anyhow::Result<()> {
        let uuid = application_uuid(&payload)?;
        restart_application(&self.deps, uuid)
            .await
            .map_err(|e| anyhow::anyhow!(e.to_string()))?;
        Ok(())
    }
}

fn application_uuid(payload: &Value) -> anyhow::Result<&str> {
    payload
        .get("application_uuid")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("application lifecycle payload missing application_uuid"))
}

/// Resolve the application and a connection to the server it deploys onto.
async fn resolve(
    deps: &DeployEngineDeps,
    uuid: &str,
) -> Result<(Application, ServerConn), DeployError> {
    let app = ApplicationRepo::new(deps.pool.clone())
        .get_by_uuid(uuid)
        .await?
        .ok_or_else(|| DeployError::NotFound(uuid.to_string()))?;

    let server_id: Option<i64> =
        sqlx::query_scalar("SELECT server_id FROM destinations WHERE id = $1")
            .bind(app.destination_id)
            .fetch_optional(&deps.pool)
            .await?;
    let server_id = server_id
        .ok_or_else(|| DeployError::Missing(format!("destination {}", app.destination_id)))?;

    let server_repo = ServerRepo::new(deps.pool.clone());
    let server_uuid: Option<String> = sqlx::query_scalar("SELECT uuid FROM servers WHERE id = $1")
        .bind(server_id)
        .fetch_optional(&deps.pool)
        .await?;
    let server_uuid =
        server_uuid.ok_or_else(|| DeployError::Missing(format!("server {server_id}")))?;
    let server = server_repo
        .get_by_uuid(&server_uuid)
        .await?
        .ok_or_else(|| DeployError::Missing(format!("server {server_uuid}")))?;
    let connection_timeout = server_repo
        .settings(server.id)
        .await?
        .map(|s| s.connection_timeout.max(1) as u32)
        .unwrap_or(10);
    let conn = build_conn(&deps.pool, &server, connection_timeout).await;
    Ok((app, conn))
}

/// Stop+remove every container labelled for this application (production ones:
/// `rustify.pullRequestId=0`), then mark the application `exited`.
pub async fn stop_application(deps: &DeployEngineDeps, uuid: &str) -> Result<(), DeployError> {
    let (app, conn) = resolve(deps, uuid).await?;
    deps.executor
        .exec(&conn, &stop_script(uuid), ExecOpts::default())
        .await?;
    ApplicationRepo::new(deps.pool.clone())
        .set_status(app.id, "exited")
        .await?;
    let _ = deps
        .events
        .send(WsEvent::application_status_changed(uuid, "exited"));
    Ok(())
}

/// Restart: stop the old container(s), then bring the stored compose back up.
pub async fn restart_application(deps: &DeployEngineDeps, uuid: &str) -> Result<(), DeployError> {
    let (app, conn) = resolve(deps, uuid).await?;
    deps.executor
        .exec(&conn, &stop_script(uuid), ExecOpts::default())
        .await?;
    let dir = format!("{APP_CONFIG_DIR}/{uuid}");
    deps.executor
        .exec(
            &conn,
            &format!("cd {dir} && docker compose -f docker-compose.yml up -d"),
            ExecOpts::default(),
        )
        .await?;
    ApplicationRepo::new(deps.pool.clone())
        .set_status(app.id, "running")
        .await?;
    let _ = deps
        .events
        .send(WsEvent::application_status_changed(uuid, "running"));
    Ok(())
}

/// Stop+force-remove every production container labelled for this application.
/// The `|| true` guards keep the command exit 0 when no container exists.
fn stop_script(uuid: &str) -> String {
    format!(
        "for c in $(docker ps -aq \
         --filter label=rustify.applicationUuid={uuid} \
         --filter label=rustify.pullRequestId=0); do \
         docker stop -t 30 \"$c\" 2>/dev/null || true; \
         docker rm -f \"$c\" 2>/dev/null || true; done"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stop_script_filters_by_app_label() {
        let s = stop_script("app-uuid-1");
        assert!(s.contains("--filter label=rustify.applicationUuid=app-uuid-1"));
        assert!(s.contains("--filter label=rustify.pullRequestId=0"));
        assert!(s.contains("docker stop -t 30"));
        assert!(s.contains("docker rm -f"));
    }
}
