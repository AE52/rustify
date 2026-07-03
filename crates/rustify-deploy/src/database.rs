//! Standalone-database lifecycle handlers.
//!
//! Behavioural port of Coolify's `StartDatabase`/`StopDatabase` actions
//! (app/Actions/Database/StartDatabase.php, StopDatabase.php) and the per-engine
//! `Start*.php` command sequence (StartPostgresql.php:194-204): write the
//! generated compose over SSH (base64), `docker compose pull`, stop/remove the
//! old container, `docker compose up -d`. When the database is public a nginx
//! stream proxy sidecar is brought up too (StartDatabaseProxy.php).
//!
//! Both handlers reuse [`DeployEngineDeps`]; secrets are decrypted only to build
//! the compose file (written base64-encoded over SSH) and are never logged.

use async_trait::async_trait;
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use serde_json::Value;

use rustify_core::events::WsEvent;
use rustify_core::{DatabaseEngine, ExecOpts, ExecOutput, ServerConn};
use rustify_db::repos::{DatabaseRepo, ServerRepo, StandaloneDatabase};
use rustify_docker::{
    DatabaseComposeInput, generate_database_compose, generate_db_proxy_compose,
    generate_db_proxy_nginx_conf,
};
use rustify_jobs::JobHandler;

use crate::engine::build_conn;
use crate::{DeployEngineDeps, DeployError};

/// Server-side data root (matches the application engine's `/data/rustify/...`).
const DATA_DIR: &str = "/data/rustify";

/// [`JobHandler`] for kind `"database_start"`, payload `{"database_uuid": ".."}`.
pub struct StartDatabaseHandler {
    deps: DeployEngineDeps,
}

impl StartDatabaseHandler {
    pub fn new(deps: DeployEngineDeps) -> Self {
        Self { deps }
    }
}

#[async_trait]
impl JobHandler for StartDatabaseHandler {
    async fn run(&self, payload: Value) -> anyhow::Result<()> {
        let uuid = database_uuid(&payload)?;
        start_database(&self.deps, uuid)
            .await
            .map_err(|e| anyhow::anyhow!(e.to_string()))?;
        Ok(())
    }
}

/// [`JobHandler`] for kind `"database_stop"`, payload `{"database_uuid": ".."}`.
pub struct StopDatabaseHandler {
    deps: DeployEngineDeps,
}

impl StopDatabaseHandler {
    pub fn new(deps: DeployEngineDeps) -> Self {
        Self { deps }
    }
}

#[async_trait]
impl JobHandler for StopDatabaseHandler {
    async fn run(&self, payload: Value) -> anyhow::Result<()> {
        let uuid = database_uuid(&payload)?;
        stop_database(&self.deps, uuid)
            .await
            .map_err(|e| anyhow::anyhow!(e.to_string()))?;
        Ok(())
    }
}

fn database_uuid(payload: &Value) -> anyhow::Result<&str> {
    payload
        .get("database_uuid")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("database job payload missing database_uuid"))
}

/// Resolve the database, its server connection and destination network.
async fn resolve(
    deps: &DeployEngineDeps,
    uuid: &str,
) -> Result<(StandaloneDatabase, ServerConn, String), DeployError> {
    let repo = DatabaseRepo::new(deps.pool.clone());
    let db = repo
        .get_by_uuid(uuid)
        .await?
        .ok_or_else(|| DeployError::NotFound(uuid.to_string()))?;

    let (server_id, network) = destination(&deps.pool, db.destination_id).await?;
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
    Ok((db, conn, network))
}

/// Resolve `(server_id, network)` for a destination id.
async fn destination(
    pool: &sqlx::PgPool,
    destination_id: i64,
) -> Result<(i64, String), DeployError> {
    let row: Option<(i64, String)> =
        sqlx::query_as("SELECT server_id, network FROM destinations WHERE id = $1")
            .bind(destination_id)
            .fetch_optional(pool)
            .await?;
    row.ok_or_else(|| DeployError::Missing(format!("destination {destination_id}")))
}

/// Start (or restart) a standalone database: write the compose, pull, replace
/// the container, and bring up the public proxy when configured.
pub async fn start_database(deps: &DeployEngineDeps, uuid: &str) -> Result<(), DeployError> {
    let (db, conn, network) = resolve(deps, uuid).await?;
    let engine = DatabaseEngine::parse(&db.engine)
        .ok_or_else(|| DeployError::Payload(format!("unknown database engine {}", db.engine)))?;

    let repo = DatabaseRepo::new(deps.pool.clone());
    let credentials = repo.decrypt_credentials(uuid).await?;

    let dir = format!("{DATA_DIR}/databases/{uuid}");
    let compose = generate_database_compose(&DatabaseComposeInput {
        uuid: db.uuid.clone(),
        engine,
        image: db.image.clone(),
        network: network.clone(),
        credentials,
        limits_memory: db.limits_memory.clone(),
        limits_cpus: db.limits_cpus.clone(),
        health_check_enabled: db.health_check_enabled,
        health_check_interval: db.health_check_interval.max(1) as u32,
        health_check_timeout: db.health_check_timeout.max(1) as u32,
        health_check_retries: db.health_check_retries.max(1) as u32,
        health_check_start_period: db.health_check_start_period.max(0) as u32,
        ports_mappings: split_ports(db.ports_mappings.as_deref()),
    });

    exec(deps, &conn, &format!("mkdir -p {dir}")).await?;
    exec(
        deps,
        &conn,
        &write_file(&format!("{dir}/docker-compose.yml"), &compose),
    )
    .await?;
    exec(
        deps,
        &conn,
        &format!("docker compose -f {dir}/docker-compose.yml pull"),
    )
    .await?;
    // Replace any previous container (StartPostgresql.php:200-201).
    exec_ignore(
        deps,
        &conn,
        &format!("docker stop -t 10 {uuid} 2>/dev/null || true"),
    )
    .await;
    exec_ignore(
        deps,
        &conn,
        &format!("docker rm -f {uuid} 2>/dev/null || true"),
    )
    .await;
    exec(
        deps,
        &conn,
        &format!("docker compose -f {dir}/docker-compose.yml up -d"),
    )
    .await?;

    // Public TCP proxy sidecar (StartDatabase.php:57-59 -> StartDatabaseProxy).
    if db.is_public
        && let Some(public_port) = db.public_port
    {
        let public_port = public_port as u16;
        let internal_port = engine.descriptor().internal_port;
        let timeout = db.public_port_timeout.max(1) as u32;
        let proxy_dir = format!("{dir}/proxy");
        let nginx = generate_db_proxy_nginx_conf(uuid, public_port, internal_port, timeout);
        let proxy_compose =
            generate_db_proxy_compose(uuid, public_port, internal_port, timeout, &network);

        exec(deps, &conn, &format!("mkdir -p {proxy_dir}")).await?;
        exec(
            deps,
            &conn,
            &write_file(&format!("{proxy_dir}/nginx.conf"), &nginx),
        )
        .await?;
        exec(
            deps,
            &conn,
            &write_file(&format!("{proxy_dir}/docker-compose.yml"), &proxy_compose),
        )
        .await?;
        exec_ignore(
            deps,
            &conn,
            &format!("docker rm -f {uuid}-proxy 2>/dev/null || true"),
        )
        .await;
        exec(
            deps,
            &conn,
            &format!("docker compose -f {proxy_dir}/docker-compose.yml up -d"),
        )
        .await?;
    }

    repo.set_status(db.id, "running").await?;
    repo.mark_started(db.id).await?;
    let _ = deps
        .events
        .send(WsEvent::database_status_changed(uuid, "running"));
    Ok(())
}

/// Stop a standalone database and its proxy (StopDatabase.php:53-60).
pub async fn stop_database(deps: &DeployEngineDeps, uuid: &str) -> Result<(), DeployError> {
    let (db, conn, _network) = resolve(deps, uuid).await?;

    exec_ignore(deps, &conn, &format!("docker stop -t 30 {uuid}")).await;
    exec_ignore(deps, &conn, &format!("docker rm -f {uuid}")).await;
    if db.is_public {
        exec_ignore(deps, &conn, &format!("docker stop -t 30 {uuid}-proxy")).await;
        exec_ignore(deps, &conn, &format!("docker rm -f {uuid}-proxy")).await;
    }

    DatabaseRepo::new(deps.pool.clone())
        .set_status(db.id, "exited")
        .await?;
    let _ = deps
        .events
        .send(WsEvent::database_status_changed(uuid, "exited"));
    Ok(())
}

/// Run a remote command; a non-zero exit (or a connection error) is fatal.
async fn exec(
    deps: &DeployEngineDeps,
    conn: &ServerConn,
    script: &str,
) -> Result<ExecOutput, DeployError> {
    let out = deps
        .executor
        .exec(conn, script, ExecOpts::default())
        .await?;
    if out.exit_code != 0 {
        return Err(DeployError::Build(format!(
            "database command exited {}: {}",
            out.exit_code,
            out.stderr.trim()
        )));
    }
    Ok(out)
}

/// Run a remote command, ignoring failures (used for best-effort stop/remove).
async fn exec_ignore(deps: &DeployEngineDeps, conn: &ServerConn, script: &str) {
    let _ = deps.executor.exec(conn, script, ExecOpts::default()).await;
}

/// A base64 write command: `echo '<b64>' | base64 -d | tee <path> > /dev/null`
/// (StartPostgresql.php:196). Encoding keeps the (secret-bearing) compose off
/// the shell command line in plaintext.
fn write_file(path: &str, content: &str) -> String {
    let b64 = BASE64.encode(content.as_bytes());
    format!("echo '{b64}' | base64 -d | tee {path} > /dev/null")
}

/// Split a comma/space-separated `ports_mappings` string into a vec.
fn split_ports(raw: Option<&str>) -> Vec<String> {
    raw.unwrap_or("")
        .split([',', ' '])
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_file_is_base64_decode_tee() {
        let cmd = write_file("/data/rustify/databases/x/docker-compose.yml", "hello");
        assert!(cmd.starts_with("echo '"));
        assert!(cmd.contains("| base64 -d | tee /data/rustify/databases/x/docker-compose.yml"));
        // The plaintext must not appear on the command line.
        assert!(!cmd.contains("hello"));
        assert!(cmd.contains(&BASE64.encode("hello")));
    }

    #[test]
    fn split_ports_handles_separators() {
        assert_eq!(
            split_ports(Some("5433:5432, 5434:5432")),
            vec!["5433:5432".to_string(), "5434:5432".to_string()]
        );
        assert!(split_ports(None).is_empty());
    }
}
