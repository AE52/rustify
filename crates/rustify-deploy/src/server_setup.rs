//! Server validation & setup (`ServerSetupHandler`, kind `"server_validate"`).
//!
//! Behavioural port of Coolify's server validation
//! (app/Actions/Server/ValidateServer.php + InstallDocker), reduced to Phase 1:
//!
//! 1. uptime probe (is the host reachable over SSH?)
//! 2. `command -v docker` — install via `get.docker.com` if missing
//! 3. `docker network create --attachable rustify || true`
//! 4. mark the server reachable/usable in the DB (Contract C6)
//! 5. start the Traefik proxy by running `rustify_proxy`'s start script
//!    directly through the executor
//!
//! Everything streams to the `server:<uuid>` WebSocket channel (Contract C4).

use async_trait::async_trait;
use serde_json::Value;
use tokio::sync::mpsc;

use rustify_core::events::WsEvent;
use rustify_core::{ExecEvent, ExecOpts, ExecOutput, ServerConn};
use rustify_db::repos::ServerRepo;
use rustify_jobs::JobHandler;
use rustify_proxy::{generate_proxy_compose, start_script};

use crate::{DeployEngineDeps, DeployError};

/// The default destination network (Contract C7).
const NETWORK: &str = "rustify";

/// [`rustify_jobs::JobHandler`] for kind `"server_validate"`, payload
/// `{"server_uuid": ".."}`.
pub struct ServerSetupHandler {
    deps: DeployEngineDeps,
}

impl ServerSetupHandler {
    pub fn new(deps: DeployEngineDeps) -> Self {
        Self { deps }
    }
}

#[async_trait]
impl JobHandler for ServerSetupHandler {
    async fn run(&self, payload: Value) -> anyhow::Result<()> {
        let uuid = payload
            .get("server_uuid")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("server_validate payload missing server_uuid"))?;
        validate_server(&self.deps, uuid)
            .await
            .map_err(|e| anyhow::anyhow!(e.to_string()))?;
        Ok(())
    }
}

/// Validate and set up `server_uuid`. Reachability failure is recorded (not an
/// error); only missing rows / DB failures propagate as `Err`.
pub async fn validate_server(
    deps: &DeployEngineDeps,
    server_uuid: &str,
) -> Result<(), DeployError> {
    let repo = ServerRepo::new(deps.pool.clone());
    let server = repo
        .get_by_uuid(server_uuid)
        .await?
        .ok_or_else(|| DeployError::NotFound(server_uuid.to_string()))?;

    let settings = repo.settings(server.id).await?;
    let connection_timeout = settings
        .as_ref()
        .map(|s| s.connection_timeout.max(1) as u32)
        .unwrap_or(10);
    let conn = crate::engine::build_conn(&deps.pool, &server, connection_timeout).await;

    let mut ctx = SetupCtx {
        deps,
        conn,
        server_uuid: server_uuid.to_string(),
        logs: String::new(),
    };

    ctx.log("info", "Validating server connectivity.").await;

    // Step 1: reachability probe.
    if ctx.exec("uptime", true).await.is_err() {
        ctx.log("stderr", "Server is not reachable over SSH.").await;
        repo.set_reachability(server.id, false, false, Some(&ctx.logs))
            .await?;
        let _ = deps.events.send(WsEvent::server_reachability_changed(
            server_uuid,
            false,
            false,
        ));
        return Ok(());
    }

    // Step 2: ensure docker is installed.
    let docker = ctx.exec("command -v docker || true", true).await?;
    if docker.stdout.trim().is_empty() {
        ctx.log("info", "Docker not found; installing via get.docker.com.")
            .await;
        ctx.exec("curl -fsSL https://get.docker.com | sh", false)
            .await?;
    } else {
        ctx.log("info", "Docker is already installed.").await;
    }

    // Step 3: ensure the destination network exists.
    ctx.exec(
        &format!("docker network create --attachable {NETWORK} || true"),
        false,
    )
    .await?;

    // Step 4: mark reachable + usable.
    repo.set_reachability(server.id, true, true, Some(&ctx.logs))
        .await?;
    let _ = deps.events.send(WsEvent::server_reachability_changed(
        server_uuid,
        true,
        true,
    ));
    ctx.log("info", "Server marked reachable and usable.").await;

    // Step 5: start the Traefik proxy.
    ctx.log("info", "Starting the reverse proxy.").await;
    let script = start_script(&generate_proxy_compose(&[]));
    ctx.exec(&script, false).await?;
    sqlx::query("UPDATE server_settings SET proxy_status = 'running', updated_at = now() WHERE server_id = $1")
        .bind(server.id)
        .execute(&deps.pool)
        .await?;
    ctx.log("info", "Proxy started.").await;

    Ok(())
}

/// Streaming context for a single server-setup run.
struct SetupCtx<'a> {
    deps: &'a DeployEngineDeps,
    conn: ServerConn,
    server_uuid: String,
    logs: String,
}

impl SetupCtx<'_> {
    /// Emit a synthetic log line to the `server:<uuid>` channel and accumulate
    /// it into `validation_logs`.
    async fn log(&mut self, kind: &str, content: &str) {
        self.logs.push_str(content);
        self.logs.push('\n');
        let _ = self
            .deps
            .events
            .send(WsEvent::server_log(&self.server_uuid, kind, content));
    }

    /// Run a command, streaming each output line to the server channel. A
    /// non-zero exit is an error unless `allow_failure`.
    async fn exec(&mut self, script: &str, allow_failure: bool) -> Result<ExecOutput, DeployError> {
        let (tx, mut rx) = mpsc::channel::<ExecEvent>(256);
        let executor = self.deps.executor.clone();
        let conn = self.conn.clone();
        let owned = script.to_string();
        let fut = async move {
            executor
                .exec_streaming(&conn, &owned, ExecOpts::default(), tx)
                .await
        };
        tokio::pin!(fut);

        let mut result: Option<Result<ExecOutput, rustify_core::ExecError>> = None;
        loop {
            tokio::select! {
                biased;
                evt = rx.recv() => match evt {
                    Some(ExecEvent::Stdout(l)) => self.log("stdout", &l).await,
                    Some(ExecEvent::Stderr(l)) => self.log("stderr", &l).await,
                    None => if result.is_some() { break; },
                },
                r = &mut fut, if result.is_none() => { result = Some(r); }
            }
        }
        let out = result.expect("exec future resolved before channel closed")?;
        if !allow_failure && out.exit_code != 0 {
            return Err(DeployError::Build(format!(
                "server setup command exited {}: {}",
                out.exit_code,
                out.stderr.trim()
            )));
        }
        Ok(out)
    }
}
