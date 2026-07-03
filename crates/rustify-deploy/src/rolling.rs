//! Zero-downtime rolling update with a health gate.
//!
//! Port of Coolify's `rolling_update` + `health_check`
//! (ApplicationDeploymentJob.php:1904-2030), standalone (non-swarm) branch:
//!
//! 1. Disqualify rolling when it cannot be safe (host port mappings, custom
//!    `--ip`) — then stop the old container *before* starting the new one.
//! 2. Otherwise start the new container alongside the old, then poll its
//!    Docker healthcheck: wait `start_period` seconds, then up to `retries`
//!    attempts, checking `docker inspect .State.Health.Status` every
//!    `interval` seconds. `healthy` ⇒ stop+remove the old containers; anything
//!    else after the budget ⇒ dump `docker logs -n 100`, remove the new
//!    container and fail (the old one keeps serving traffic).

use rustify_docker::ContainerHealth;

use crate::DeployError;
use crate::engine::Engine;

/// Whether a rolling update is safe for this application, or why not.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Eligibility {
    Eligible,
    Disqualified(String),
}

/// Rolling-update disqualifiers (ApplicationDeploymentJob.php:1919-1938),
/// reduced to Phase 1 features: host port mappings and custom `--ip`/`--ip6`
/// in the docker run options both force a stop-then-start recreate.
pub fn eligibility(
    ports_mappings: Option<&str>,
    custom_docker_run_options: Option<&str>,
) -> Eligibility {
    if ports_mappings
        .map(|p| !p.trim().is_empty())
        .unwrap_or(false)
    {
        return Eligibility::Disqualified("application has ports mapped to the host system".into());
    }
    if let Some(opts) = custom_docker_run_options
        && (opts.contains("--ip") || opts.contains("--ip6"))
    {
        return Eligibility::Disqualified("a custom IP address is set".into());
    }
    Eligibility::Eligible
}

/// `docker inspect` health command for a container (parity with the pinned
/// `--format='{{json .State.Health.Status}}'`).
pub fn inspect_health_command(container_name: &str) -> String {
    format!("docker inspect --format='{{{{json .State.Health.Status}}}}' {container_name}")
}

/// Parse the `--format='{{json .State.Health.Status}}'` output, which is a
/// JSON string like `"healthy"` (Coolify strips the quotes before comparing).
pub fn parse_health_status(raw: &str) -> ContainerHealth {
    match raw.trim().trim_matches('"') {
        "healthy" => ContainerHealth::Healthy,
        "unhealthy" => ContainerHealth::Unhealthy,
        "starting" => ContainerHealth::Starting,
        _ => ContainerHealth::None,
    }
}

/// Run the rolling update for the deployment held by `engine`. The new
/// container name is `engine.container_name()` and the compose file has already
/// been written to the app config dir.
pub async fn rolling_update(engine: &mut Engine) -> Result<(), DeployError> {
    engine.check_cancel().await?;
    let container_name = engine.container_name().to_string();
    let app = engine.application();
    let elig = eligibility(
        app.ports_mappings.as_deref(),
        app.custom_docker_run_options.as_deref(),
    );

    match elig {
        Eligibility::Disqualified(reason) => {
            engine
                .info(&format!(
                    "Rolling update not supported ({reason}); recreating container."
                ))
                .await;
            // Stop the old container first, then start the new one. The old
            // container is already gone, so `--remove-orphans` is safe here.
            engine.stop_other_containers(None).await?;
            engine.compose_up(true).await?;
            engine.info("Container recreated.").await;
            Ok(())
        }
        Eligibility::Eligible => {
            engine.info("Rolling update started.").await;
            // Start the new (uniquely-named) container ALONGSIDE the old one: no
            // `--remove-orphans`, so the old container keeps serving traffic
            // until the health gate passes. `stop_other_containers` removes the
            // old container only after a healthy result.
            engine.compose_up(false).await?;
            match health_check(engine, &container_name).await {
                Ok(()) => {
                    // Healthy: remove every managed container for this app
                    // except the one we just started.
                    engine.stop_other_containers(Some(&container_name)).await?;
                    engine.info("Rolling update completed.").await;
                    Ok(())
                }
                Err(e) => {
                    engine.query_container_logs(&container_name).await;
                    engine.remove_container(&container_name).await;
                    Err(e)
                }
            }
        }
    }
}

/// Poll the new container's healthcheck. When healthchecking is disabled the
/// container is assumed healthy immediately (Coolify's `isHealthcheckDisabled`
/// short-circuit). Sleeps use the app's configured `start_period`/`interval`,
/// so tests set them to `0` for instant polls.
async fn health_check(engine: &mut Engine, container_name: &str) -> Result<(), DeployError> {
    let app = engine.application();
    if !app.health_check_enabled {
        engine
            .info("Healthcheck disabled; assuming the new container is healthy.")
            .await;
        return Ok(());
    }
    let start_period = app.health_check_start_period.max(0) as u64;
    let interval = app.health_check_interval.max(0) as u64;
    let retries = app.health_check_retries.max(1);

    engine
        .info(&format!(
            "Waiting {start_period}s (start period) before healthchecking the new container."
        ))
        .await;
    sleep_secs(start_period).await;

    let cmd = inspect_health_command(container_name);
    for attempt in 1..=retries {
        engine.check_cancel().await?;
        let out = engine.exec_step(&cmd, true, true).await?;
        let status = parse_health_status(&out.stdout);
        engine
            .info(&format!(
                "Healthcheck attempt {attempt}/{retries}: {status:?}"
            ))
            .await;
        match status {
            ContainerHealth::Healthy => {
                engine.info("New container is healthy.").await;
                return Ok(());
            }
            ContainerHealth::Unhealthy => {
                engine.error("New container is unhealthy.").await;
                return Err(DeployError::Unhealthy);
            }
            _ => sleep_secs(interval).await,
        }
    }
    engine
        .error("New container did not become healthy within the retry budget.")
        .await;
    Err(DeployError::Unhealthy)
}

async fn sleep_secs(secs: u64) {
    if secs > 0 {
        tokio::time::sleep(std::time::Duration::from_secs(secs)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn eligible_when_no_disqualifiers() {
        assert_eq!(eligibility(None, None), Eligibility::Eligible);
        assert_eq!(
            eligibility(Some(""), Some("--memory=1g")),
            Eligibility::Eligible
        );
    }

    #[test]
    fn disqualified_by_port_mappings() {
        assert!(matches!(
            eligibility(Some("8080:80"), None),
            Eligibility::Disqualified(_)
        ));
    }

    #[test]
    fn disqualified_by_custom_ip() {
        assert!(matches!(
            eligibility(None, Some("--ip 10.0.0.5")),
            Eligibility::Disqualified(_)
        ));
        assert!(matches!(
            eligibility(None, Some("--ip6 fd00::1")),
            Eligibility::Disqualified(_)
        ));
    }

    #[test]
    fn inspect_command_shape() {
        assert_eq!(
            inspect_health_command("app-uuid-abc123"),
            "docker inspect --format='{{json .State.Health.Status}}' app-uuid-abc123"
        );
    }

    #[test]
    fn parse_status_strips_quotes() {
        assert_eq!(
            parse_health_status("\"healthy\"\n"),
            ContainerHealth::Healthy
        );
        assert_eq!(
            parse_health_status("\"unhealthy\""),
            ContainerHealth::Unhealthy
        );
        assert_eq!(parse_health_status("starting"), ContainerHealth::Starting);
        assert_eq!(parse_health_status("\"\""), ContainerHealth::None);
    }
}
