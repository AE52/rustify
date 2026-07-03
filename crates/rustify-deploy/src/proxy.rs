//! Proxy start/stop/restart lifecycle handlers.
//!
//! Behavioural port of Coolify's `StartProxy` / `StopProxy` / `RestartProxy`
//! (app/Actions/Proxy/*.php): run `rustify-proxy`'s start/stop shell scripts over
//! the target server's [`rustify_ssh::SshExecutor`], then persist the proxy's
//! runtime status. Start regenerates the Traefik compose (preserving any saved
//! custom command flags) so a config change is picked up on restart.

use async_trait::async_trait;
use serde_json::Value;

use rustify_core::{ExecOpts, ServerConn};
use rustify_db::repos::{Server, ServerRepo};
use rustify_jobs::JobHandler;
use rustify_proxy::{extract_custom_commands, generate_proxy_compose, start_script, stop_script};

use crate::engine::build_conn;
use crate::{DeployEngineDeps, DeployError};

/// Job kind: start the Traefik proxy on a server.
pub const PROXY_START_KIND: &str = "proxy_start";
/// Job kind: stop the Traefik proxy on a server.
pub const PROXY_STOP_KIND: &str = "proxy_stop";
/// Job kind: restart the Traefik proxy on a server.
pub const PROXY_RESTART_KIND: &str = "proxy_restart";

/// [`JobHandler`] for kind `"proxy_start"`, payload `{"server_uuid": ".."}`.
pub struct ProxyStartHandler {
    deps: DeployEngineDeps,
}

impl ProxyStartHandler {
    pub fn new(deps: DeployEngineDeps) -> Self {
        Self { deps }
    }
}

#[async_trait]
impl JobHandler for ProxyStartHandler {
    async fn run(&self, payload: Value) -> anyhow::Result<()> {
        let uuid = server_uuid(&payload)?;
        start_proxy(&self.deps, uuid)
            .await
            .map_err(|e| anyhow::anyhow!(e.to_string()))?;
        Ok(())
    }
}

/// [`JobHandler`] for kind `"proxy_stop"`, payload `{"server_uuid": ".."}`.
pub struct ProxyStopHandler {
    deps: DeployEngineDeps,
}

impl ProxyStopHandler {
    pub fn new(deps: DeployEngineDeps) -> Self {
        Self { deps }
    }
}

#[async_trait]
impl JobHandler for ProxyStopHandler {
    async fn run(&self, payload: Value) -> anyhow::Result<()> {
        let uuid = server_uuid(&payload)?;
        stop_proxy(&self.deps, uuid)
            .await
            .map_err(|e| anyhow::anyhow!(e.to_string()))?;
        Ok(())
    }
}

/// [`JobHandler`] for kind `"proxy_restart"`, payload `{"server_uuid": ".."}`.
pub struct ProxyRestartHandler {
    deps: DeployEngineDeps,
}

impl ProxyRestartHandler {
    pub fn new(deps: DeployEngineDeps) -> Self {
        Self { deps }
    }
}

#[async_trait]
impl JobHandler for ProxyRestartHandler {
    async fn run(&self, payload: Value) -> anyhow::Result<()> {
        let uuid = server_uuid(&payload)?;
        restart_proxy(&self.deps, uuid)
            .await
            .map_err(|e| anyhow::anyhow!(e.to_string()))?;
        Ok(())
    }
}

fn server_uuid(payload: &Value) -> anyhow::Result<&str> {
    payload
        .get("server_uuid")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("proxy lifecycle payload missing server_uuid"))
}

/// Resolve the server and a connection to it.
async fn conn_for(
    deps: &DeployEngineDeps,
    server_uuid: &str,
) -> Result<(Server, ServerConn), DeployError> {
    let repo = ServerRepo::new(deps.pool.clone());
    let server = repo
        .get_by_uuid(server_uuid)
        .await?
        .ok_or_else(|| DeployError::NotFound(server_uuid.to_string()))?;
    let ct = repo
        .settings(server.id)
        .await?
        .map(|s| s.connection_timeout.max(1) as u32)
        .unwrap_or(10);
    let conn = build_conn(&deps.pool, &server, ct).await;
    Ok((server, conn))
}

/// Start the proxy: regenerate the compose (preserving saved custom flags), run
/// the start script, then persist `proxy_status = running`.
pub async fn start_proxy(deps: &DeployEngineDeps, server_uuid: &str) -> Result<(), DeployError> {
    let (server, conn) = conn_for(deps, server_uuid).await?;
    let repo = ServerRepo::new(deps.pool.clone());
    let custom = repo
        .settings(server.id)
        .await?
        .and_then(|s| s.proxy_custom_config)
        .map(|c| extract_custom_commands(&c))
        .unwrap_or_default();
    let compose = generate_proxy_compose(&custom);
    deps.executor
        .exec(&conn, &start_script(&compose), ExecOpts::default())
        .await?;
    repo.set_proxy_status(server.id, "running").await?;
    Ok(())
}

/// Stop the proxy: run the stop script, then persist `proxy_status = exited`.
pub async fn stop_proxy(deps: &DeployEngineDeps, server_uuid: &str) -> Result<(), DeployError> {
    let (server, conn) = conn_for(deps, server_uuid).await?;
    deps.executor
        .exec(&conn, &stop_script(), ExecOpts::default())
        .await?;
    ServerRepo::new(deps.pool.clone())
        .set_proxy_status(server.id, "exited")
        .await?;
    Ok(())
}

/// Restart the proxy: stop, then start.
pub async fn restart_proxy(deps: &DeployEngineDeps, server_uuid: &str) -> Result<(), DeployError> {
    stop_proxy(deps, server_uuid).await?;
    start_proxy(deps, server_uuid).await?;
    Ok(())
}
