//! One-click-service deploy & stop handlers.
//!
//! Behavioural port of Coolify's `StartService` / `StopService`
//! (app/Actions/Service/*), reduced to the Phase-2 surface:
//!
//! - [`ServiceDeployHandler`] (kind `"service_deploy"`, payload
//!   `{"service_uuid": ".."}`): resolve the service, run the compose template
//!   through [`rustify_docker::parse_and_mutate_service`], persist the generated
//!   env (encrypted, persist-once) and the mutated compose, write both to
//!   `{data}/services/{uuid}/` on the target server and
//!   `docker compose --project-name {uuid} up -d --remove-orphans`.
//! - [`ServiceStopHandler`] (kind `"service_stop"`): `docker compose ... down`.
//!
//! Both stream to the `service:<uuid>` channel and emit
//! [`WsEvent::service_status_changed`].

use std::collections::BTreeMap;

use async_trait::async_trait;
use serde_json::Value;
use sha2::{Digest, Sha256};
use tokio::sync::mpsc;

use rustify_core::events::WsEvent;
use rustify_core::{ExecEvent, ExecOpts, ExecOutput, ServerConn};
use rustify_db::repos::{EnvVarRepo, NewEnvVar, Server, ServerRepo, Service, ServiceRepo};
use rustify_docker::service_compose::SERVICE_RESOURCE_KIND;
use rustify_docker::{MutatedService, parse_and_mutate_service};
use rustify_jobs::JobHandler;

use crate::{DeployEngineDeps, DeployError};

pub const SERVICE_DEPLOY_KIND: &str = "service_deploy";
pub const SERVICE_STOP_KIND: &str = "service_stop";

/// Kind `"service_deploy"`.
pub struct ServiceDeployHandler {
    deps: DeployEngineDeps,
}

impl ServiceDeployHandler {
    pub fn new(deps: DeployEngineDeps) -> Self {
        Self { deps }
    }
}

#[async_trait]
impl JobHandler for ServiceDeployHandler {
    async fn run(&self, payload: Value) -> anyhow::Result<()> {
        let uuid = service_uuid(&payload)?;
        deploy_service(&self.deps, &uuid)
            .await
            .map_err(|e| anyhow::anyhow!(e.to_string()))?;
        Ok(())
    }
}

/// Kind `"service_stop"`.
pub struct ServiceStopHandler {
    deps: DeployEngineDeps,
}

impl ServiceStopHandler {
    pub fn new(deps: DeployEngineDeps) -> Self {
        Self { deps }
    }
}

#[async_trait]
impl JobHandler for ServiceStopHandler {
    async fn run(&self, payload: Value) -> anyhow::Result<()> {
        let uuid = service_uuid(&payload)?;
        stop_service(&self.deps, &uuid)
            .await
            .map_err(|e| anyhow::anyhow!(e.to_string()))?;
        Ok(())
    }
}

fn service_uuid(payload: &Value) -> anyhow::Result<String> {
    payload
        .get("service_uuid")
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .ok_or_else(|| anyhow::anyhow!("service payload missing service_uuid"))
}

/// Remote data directory for a service's compose + env (Contract: per-resource
/// dir `{data}/services/{uuid}`).
fn service_dir(service_uuid: &str) -> String {
    format!("/data/rustify/services/{service_uuid}")
}

/// Deploy (or redeploy) a service to its target server.
pub async fn deploy_service(
    deps: &DeployEngineDeps,
    service_uuid: &str,
) -> Result<(), DeployError> {
    let repo = ServiceRepo::new(deps.pool.clone());
    let service = repo
        .get_by_uuid(service_uuid)
        .await?
        .ok_or_else(|| DeployError::NotFound(service_uuid.to_string()))?;
    let server = resolve_server(deps, &service).await?;
    let mut ctx = Ctx::new(deps, service.uuid.clone(), &server).await;

    ctx.status("deploying").await;
    ctx.log("info", "Preparing service compose.").await;

    // 1. Load already-persisted env (persist-once) and mutate the template.
    let env_repo = EnvVarRepo::new(deps.pool.clone());
    let existing: BTreeMap<String, String> = env_repo
        .list(SERVICE_RESOURCE_KIND, service.id)
        .await?
        .into_iter()
        .map(|e| (e.key, e.value))
        .collect();

    let fqdn_base = fqdn_base(deps, &service).await;
    let mutated: MutatedService = parse_and_mutate_service(
        &service.compose_raw,
        &service.uuid,
        &service.template_key,
        &fqdn_base,
        &existing,
    )
    .map_err(|e| DeployError::Build(format!("compose parse failed: {e}")))?;

    // 2. Persist generated env (encrypted at rest, secrets shown once).
    for (key, value, is_shown_once) in &mutated.env {
        env_repo
            .upsert(NewEnvVar {
                resource_kind: SERVICE_RESOURCE_KIND.into(),
                resource_id: service.id,
                key: key.clone(),
                value: value.clone(),
                is_buildtime: false,
                is_literal: false,
                is_shown_once: *is_shown_once,
            })
            .await?;
    }

    // 3. Persist the mutated compose + a config hash for change detection.
    let env_body = render_env(&mutated.env);
    let config_hash = hash_config(&mutated.compose_mutated, &env_body);
    repo.set_mutated(service.id, &mutated.compose_mutated, &config_hash)
        .await?;
    record_applications(&repo, &service, &mutated.compose_mutated).await;

    // 4. Write compose + .env to the server, then bring the stack up.
    let dir = service_dir(&service.uuid);
    ctx.exec(&format!("mkdir -p {dir}"), true).await?;
    ctx.upload_text(&dir, "docker-compose.yml", &mutated.compose_mutated)
        .await?;
    ctx.upload_text(&dir, ".env", &env_body).await?;

    ctx.log("info", "Starting compose stack.").await;
    let up = format!(
        "cd {dir} && docker compose --project-name {uuid} up -d --remove-orphans",
        uuid = service.uuid
    );
    match ctx.exec(&up, false).await {
        Ok(_) => {
            repo.set_status(service.id, "running").await?;
            ctx.status("running").await;
            ctx.log("info", "Service is running.").await;
            Ok(())
        }
        Err(e) => {
            repo.set_status(service.id, "exited").await?;
            ctx.status("exited").await;
            ctx.log("stderr", &format!("Service deploy failed: {e}"))
                .await;
            Err(e)
        }
    }
}

/// Stop a running service stack.
pub async fn stop_service(deps: &DeployEngineDeps, service_uuid: &str) -> Result<(), DeployError> {
    let repo = ServiceRepo::new(deps.pool.clone());
    let service = repo
        .get_by_uuid(service_uuid)
        .await?
        .ok_or_else(|| DeployError::NotFound(service_uuid.to_string()))?;
    let server = resolve_server(deps, &service).await?;
    let mut ctx = Ctx::new(deps, service.uuid.clone(), &server).await;

    ctx.log("info", "Stopping service stack.").await;
    let dir = service_dir(&service.uuid);
    let down = format!(
        "cd {dir} && docker compose --project-name {uuid} down",
        uuid = service.uuid
    );
    // A stopped/never-deployed stack must not fail the job.
    ctx.exec(&down, true).await?;
    repo.set_status(service.id, "exited").await?;
    ctx.status("exited").await;
    ctx.log("info", "Service stopped.").await;
    Ok(())
}

/// Resolve the target server via the service's destination.
async fn resolve_server(deps: &DeployEngineDeps, service: &Service) -> Result<Server, DeployError> {
    let servers = ServerRepo::new(deps.pool.clone());
    let destination = servers
        .destination_by_id(service.destination_id)
        .await?
        .ok_or_else(|| DeployError::Missing(format!("destination {}", service.destination_id)))?;
    servers
        .get_by_id(destination.server_id)
        .await?
        .ok_or_else(|| DeployError::Missing(format!("server {}", destination.server_id)))
}

/// Base host that `SERVICE_FQDN_*` / `SERVICE_URL_*` resolve against:
/// `<uuid>.<wildcard_domain>` when the instance has a wildcard configured, else
/// an `sslip.io` wildcard on the server IP so the stack is reachable in dev.
async fn fqdn_base(deps: &DeployEngineDeps, service: &Service) -> String {
    let wildcard: Option<String> =
        sqlx::query_scalar("SELECT wildcard_domain FROM instance_settings ORDER BY id LIMIT 1")
            .fetch_optional(&deps.pool)
            .await
            .ok()
            .flatten();
    match wildcard.filter(|w| !w.is_empty()) {
        Some(w) => format!("{}.{}", &service.uuid, w.trim_start_matches("*.")),
        None => format!("{}.sslip.io", &service.uuid),
    }
}

/// Extract the service's child containers from the mutated compose and persist
/// them as `service_applications` (best-effort; never fails the deploy).
async fn record_applications(repo: &ServiceRepo, service: &Service, compose: &str) {
    let Ok(doc) = serde_yaml::from_str::<serde_yaml::Value>(compose) else {
        return;
    };
    let Some(services) = doc.get("services").and_then(|v| v.as_mapping()) else {
        return;
    };
    for (name, svc) in services {
        let Some(name) = name.as_str() else { continue };
        let image = svc.get("image").and_then(|v| v.as_str());
        let is_database = image.map(is_database_image).unwrap_or(false);
        let _ = repo
            .upsert_application(service.id, name, None, image, is_database)
            .await;
    }
}

/// Heuristic: does the image look like a database (so it is not FQDN-exposed)?
fn is_database_image(image: &str) -> bool {
    let img = image.to_lowercase();
    [
        "postgres",
        "mysql",
        "mariadb",
        "mongo",
        "redis",
        "clickhouse",
        "keydb",
        "dragonfly",
    ]
    .iter()
    .any(|db| img.contains(db))
}

/// Render an env-var list as a `KEY=VALUE` file body (sorted by the caller).
fn render_env(env: &[(String, String, bool)]) -> String {
    let mut body = String::new();
    for (key, value, _) in env {
        body.push_str(key);
        body.push('=');
        body.push_str(value);
        body.push('\n');
    }
    body
}

/// SHA-256 of the compose + env body, for change detection.
fn hash_config(compose: &str, env: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(compose.as_bytes());
    hasher.update(b"\0");
    hasher.update(env.as_bytes());
    let digest = hasher.finalize();
    let mut hex = String::with_capacity(64);
    for b in digest {
        hex.push(char::from_digit((b >> 4) as u32, 16).expect("nibble"));
        hex.push(char::from_digit((b & 0x0f) as u32, 16).expect("nibble"));
    }
    hex
}

/// Streaming context for a single service task.
struct Ctx<'a> {
    deps: &'a DeployEngineDeps,
    conn: ServerConn,
    service_uuid: String,
}

impl<'a> Ctx<'a> {
    async fn new(deps: &'a DeployEngineDeps, service_uuid: String, server: &Server) -> Ctx<'a> {
        let settings = ServerRepo::new(deps.pool.clone())
            .settings(server.id)
            .await
            .ok()
            .flatten();
        let connection_timeout = settings
            .as_ref()
            .map(|s| s.connection_timeout.max(1) as u32)
            .unwrap_or(10);
        let conn = crate::engine::build_conn(&deps.pool, server, connection_timeout).await;
        Ctx {
            deps,
            conn,
            service_uuid,
        }
    }

    async fn status(&self, status: &str) {
        let _ = self
            .deps
            .events
            .send(WsEvent::service_status_changed(&self.service_uuid, status));
    }

    async fn log(&self, kind: &str, content: &str) {
        let _ = self
            .deps
            .events
            .send(WsEvent::service_log(&self.service_uuid, kind, content));
    }

    /// Upload text by writing a scratch file and scp-ing it into `dir`.
    async fn upload_text(&self, dir: &str, name: &str, content: &str) -> Result<(), DeployError> {
        let scratch = std::env::temp_dir()
            .join("rustify-service")
            .join(&self.service_uuid);
        std::fs::create_dir_all(&scratch)
            .map_err(|e| DeployError::Missing(format!("scratch dir: {e}")))?;
        let local = scratch.join(name);
        std::fs::write(&local, content)
            .map_err(|e| DeployError::Missing(format!("write {name}: {e}")))?;
        let remote = format!("{dir}/{name}");
        self.deps
            .executor
            .upload(&self.conn, &local, &remote)
            .await?;
        Ok(())
    }

    /// Run a command, streaming output to the service channel. A non-zero exit
    /// is an error unless `allow_failure`.
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
        let out = match result {
            Some(r) => r?,
            None => {
                return Err(DeployError::Exec(rustify_core::ExecError::Io(
                    "exec stream closed before the command resolved".into(),
                )));
            }
        };
        if !allow_failure && out.exit_code != 0 {
            return Err(DeployError::Build(format!(
                "service command exited {}: {}",
                out.exit_code,
                out.stderr.trim()
            )));
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn database_image_heuristic() {
        assert!(is_database_image("postgres:16-alpine"));
        assert!(is_database_image("docker.io/library/redis:7"));
        assert!(!is_database_image("ghcr.io/umami-software/umami:latest"));
    }

    #[test]
    fn env_body_is_key_value_lines() {
        let env = vec![
            ("A".to_string(), "1".to_string(), false),
            ("B".to_string(), "2".to_string(), true),
        ];
        assert_eq!(render_env(&env), "A=1\nB=2\n");
    }

    #[test]
    fn config_hash_is_stable_and_sensitive() {
        let a = hash_config("compose", "env");
        assert_eq!(a, hash_config("compose", "env"));
        assert_ne!(a, hash_config("compose2", "env"));
        assert_eq!(a.len(), 64);
    }
}
