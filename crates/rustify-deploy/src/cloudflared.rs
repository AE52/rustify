//! Cloudflare Tunnel configuration on a server (`ConfigureCloudflaredHandler`,
//! kind `"configure_cloudflared"`).
//!
//! Behavioural port of Coolify's `ConfigureCloudflared`
//! (app/Actions/Server/ConfigureCloudflared.php): write a one-service compose
//! that runs `cloudflare/cloudflared:latest` in host-network mode with the
//! tunnel token, pull, (re)create the container, then verify it reports healthy
//! before flipping `server_settings.is_cloudflare_tunnel = true`. Rustify names
//! the container `rustify-cloudflared` (Coolify: `coolify-cloudflared`).
//!
//! The tunnel token is a secret: it is embedded in the compose and shipped via a
//! base64 heredoc so it never appears verbatim in a process listing, and the
//! compose is never streamed to the log/event bus.

use async_trait::async_trait;
use base64::Engine as _;
use serde::Serialize;
use serde_json::Value;

use rustify_core::{ExecOpts, ServerConn};
use rustify_db::repos::ServerRepo;
use rustify_jobs::JobHandler;

use crate::{DeployEngineDeps, DeployError};

/// Job kind for the Cloudflare-tunnel configure/disable action.
pub const CONFIGURE_CLOUDFLARED_KIND: &str = "configure_cloudflared";

/// Container (and compose service) name for the tunnel agent.
pub const CLOUDFLARED_CONTAINER: &str = "rustify-cloudflared";
/// Working directory for the tunnel compose on the server.
const CLOUDFLARED_DIR: &str = "/tmp/cloudflared";
/// Metrics endpoint the healthcheck probes.
const METRICS: &str = "127.0.0.1:60123";
/// Number of `docker inspect` health probes before giving up.
const HEALTH_ATTEMPTS: u32 = 3;

#[derive(Serialize)]
struct Compose {
    services: std::collections::BTreeMap<String, CloudflaredService>,
}

#[derive(Serialize)]
struct CloudflaredService {
    container_name: String,
    image: String,
    restart: String,
    network_mode: String,
    command: String,
    environment: Vec<String>,
    healthcheck: Healthcheck,
}

#[derive(Serialize)]
struct Healthcheck {
    test: Vec<String>,
    interval: String,
    timeout: String,
    retries: u32,
}

/// Render the cloudflared `docker-compose.yml` for `tunnel_token`.
pub fn cloudflared_compose(tunnel_token: &str) -> String {
    let service = CloudflaredService {
        container_name: CLOUDFLARED_CONTAINER.to_string(),
        image: "cloudflare/cloudflared:latest".to_string(),
        restart: "unless-stopped".to_string(),
        network_mode: "host".to_string(),
        command: "tunnel run".to_string(),
        environment: vec![
            format!("TUNNEL_TOKEN={tunnel_token}"),
            format!("TUNNEL_METRICS={METRICS}"),
        ],
        healthcheck: Healthcheck {
            test: vec![
                "CMD".to_string(),
                "cloudflared".to_string(),
                "tunnel".to_string(),
                "--metrics".to_string(),
                METRICS.to_string(),
                "ready".to_string(),
            ],
            interval: "5s".to_string(),
            timeout: "30s".to_string(),
            retries: 5,
        },
    };
    let mut services = std::collections::BTreeMap::new();
    services.insert(CLOUDFLARED_CONTAINER.to_string(), service);
    serde_yaml::to_string(&Compose { services }).unwrap_or_default()
}

/// The shell script that installs and starts the tunnel container. The compose
/// (which carries the secret token) is delivered base64-encoded so the token is
/// never a plaintext argument.
pub fn configure_script(compose_yaml: &str) -> String {
    let b64 = base64::engine::general_purpose::STANDARD.encode(compose_yaml.as_bytes());
    format!(
        "mkdir -p {dir}\n\
         cd {dir}\n\
         echo '{b64}' | base64 -d | tee docker-compose.yml > /dev/null\n\
         docker compose pull\n\
         docker rm -f {name} || true\n\
         docker compose up --wait --wait-timeout 15 --remove-orphans || docker logs {name}\n",
        dir = CLOUDFLARED_DIR,
        name = CLOUDFLARED_CONTAINER,
    )
}

/// `docker inspect` command reporting the container health status (empty when
/// absent).
pub fn health_inspect_command() -> String {
    format!(
        "docker inspect --format '{{{{.State.Health.Status}}}}' {CLOUDFLARED_CONTAINER} 2>/dev/null || true"
    )
}

/// The teardown script: force-remove the tunnel container.
pub fn disable_script() -> String {
    format!("docker rm -f {CLOUDFLARED_CONTAINER} || true\n")
}

/// [`rustify_jobs::JobHandler`] for kind `"configure_cloudflared"`, payload
/// `{"server_uuid": "..", "tunnel_token": "..", "action": "configure"|"disable"}`.
pub struct ConfigureCloudflaredHandler {
    deps: DeployEngineDeps,
}

impl ConfigureCloudflaredHandler {
    pub fn new(deps: DeployEngineDeps) -> Self {
        Self { deps }
    }
}

#[async_trait]
impl JobHandler for ConfigureCloudflaredHandler {
    async fn run(&self, payload: Value) -> anyhow::Result<()> {
        let server_uuid = payload
            .get("server_uuid")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("configure_cloudflared payload missing server_uuid"))?;
        let action = payload
            .get("action")
            .and_then(|v| v.as_str())
            .unwrap_or("configure");
        if action == "disable" {
            disable_cloudflared(&self.deps, server_uuid)
                .await
                .map_err(|e| anyhow::anyhow!(e.to_string()))?;
        } else {
            let token = payload
                .get("tunnel_token")
                .and_then(|v| v.as_str())
                .ok_or_else(|| {
                    anyhow::anyhow!("configure_cloudflared payload missing tunnel_token")
                })?;
            configure_cloudflared(&self.deps, server_uuid, token)
                .await
                .map_err(|e| anyhow::anyhow!(e.to_string()))?;
        }
        Ok(())
    }
}

/// Resolve the server + its ssh connection.
async fn conn_for(
    deps: &DeployEngineDeps,
    server_uuid: &str,
) -> Result<(rustify_db::repos::Server, ServerConn), DeployError> {
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
    let conn = crate::engine::build_conn(&deps.pool, &server, ct).await;
    Ok((server, conn))
}

/// Install the tunnel, verify it becomes healthy, then persist the flag and
/// swap the server's `ip` for its ssh hostname (stashing the prior `ip`).
pub async fn configure_cloudflared(
    deps: &DeployEngineDeps,
    server_uuid: &str,
    tunnel_token: &str,
) -> Result<(), DeployError> {
    let (server, conn) = conn_for(deps, server_uuid).await?;

    let compose = cloudflared_compose(tunnel_token);
    let script = configure_script(&compose);
    // NB: never log `script`/`compose` — they carry the tunnel token.
    deps.executor
        .exec(&conn, &script, ExecOpts::default())
        .await?;

    // Verify healthy: docker inspect State.Health.Status == healthy, 3x / 5s.
    let mut healthy = false;
    for attempt in 0..HEALTH_ATTEMPTS {
        let out = deps
            .executor
            .exec(&conn, &health_inspect_command(), ExecOpts::default())
            .await?;
        if out.stdout.trim() == "healthy" {
            healthy = true;
            break;
        }
        if attempt + 1 < HEALTH_ATTEMPTS {
            tokio::time::sleep(std::time::Duration::from_secs(5)).await;
        }
    }
    if !healthy {
        return Err(DeployError::Build(
            "cloudflared container did not become healthy".to_string(),
        ));
    }

    // Persist the flag and repoint ssh at the tunnel hostname (keep ip_previous).
    ServerRepo::new(deps.pool.clone())
        .set_cloudflare_tunnel(server.id, true, Some(&server.uuid))
        .await?;
    Ok(())
}

/// Tear the tunnel down and restore the direct IP.
pub async fn disable_cloudflared(
    deps: &DeployEngineDeps,
    server_uuid: &str,
) -> Result<(), DeployError> {
    let (server, conn) = conn_for(deps, server_uuid).await?;
    deps.executor
        .exec(&conn, &disable_script(), ExecOpts::default())
        .await?;
    ServerRepo::new(deps.pool.clone())
        .set_cloudflare_tunnel(server.id, false, None)
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn load_golden(name: &str) -> String {
        let path = format!("{}/tests/golden/{name}", env!("CARGO_MANIFEST_DIR"));
        let raw = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {path}: {e}"));
        let mut lines = raw.lines().peekable();
        while let Some(line) = lines.peek() {
            if line.starts_with('#') {
                lines.next();
            } else {
                break;
            }
        }
        if lines.peek().map(|l| l.trim().is_empty()).unwrap_or(false) {
            lines.next();
        }
        lines.collect::<Vec<_>>().join("\n")
    }

    #[test]
    fn compose_matches_golden() {
        let generated = cloudflared_compose("TESTTOKEN");
        assert_eq!(
            generated.trim(),
            load_golden("cloudflared-compose.yaml").trim()
        );
    }

    #[test]
    fn compose_carries_token_and_host_network() {
        let yaml = cloudflared_compose("secret-tunnel-token");
        assert!(yaml.contains("container_name: rustify-cloudflared"));
        assert!(yaml.contains("image: cloudflare/cloudflared:latest"));
        assert!(yaml.contains("network_mode: host"));
        assert!(yaml.contains("command: tunnel run"));
        assert!(yaml.contains("TUNNEL_TOKEN=secret-tunnel-token"));
        assert!(yaml.contains("TUNNEL_METRICS=127.0.0.1:60123"));
    }

    #[test]
    fn configure_script_hides_token_behind_base64() {
        let compose = cloudflared_compose("VERYSECRET");
        let script = configure_script(&compose);
        // The raw token never appears as a plaintext argument.
        assert!(!script.contains("VERYSECRET"));
        assert!(script.contains("mkdir -p /tmp/cloudflared"));
        assert!(script.contains("| base64 -d | tee docker-compose.yml"));
        assert!(script.contains("docker compose pull"));
        assert!(script.contains("docker rm -f rustify-cloudflared || true"));
        assert!(script.contains("docker compose up --wait --wait-timeout 15 --remove-orphans"));
    }

    #[test]
    fn disable_script_removes_container() {
        assert_eq!(
            disable_script().trim(),
            "docker rm -f rustify-cloudflared || true"
        );
    }

    #[test]
    fn health_inspect_reads_health_status() {
        assert!(health_inspect_command().contains(".State.Health.Status"));
        assert!(health_inspect_command().contains("rustify-cloudflared"));
    }
}
