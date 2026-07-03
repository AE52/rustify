//! Single-service `docker-compose.yml` generation for a managed application.
//!
//! `AppComposeInput` is a DB-free plain struct carrying exactly the fields the
//! `applications` row (Contract C6) exposes to the compose layer. `generate_compose`
//! renders it to YAML via `serde_yaml`, wiring in the Traefik labels from
//! [`crate::labels::traefik_labels`].

use crate::labels::{ProxyKind, labels_for};
use serde::Serialize;
use std::collections::BTreeMap;

/// HTTP healthcheck description. Rendered into the curl||wget fallback chain
/// exactly as Coolify emits it (ApplicationDeploymentJob.php:3424).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HealthCheck {
    pub host: String,
    pub port: u16,
    pub path: String,
    pub interval_secs: u32,
    pub timeout_secs: u32,
    pub retries: u32,
    pub start_period_secs: u32,
}

impl HealthCheck {
    /// The exact command string used as the healthcheck test.
    ///
    /// `curl -f http://H:P/path || wget -qO- http://H:P/path || exit 1`
    pub fn test_command(&self) -> String {
        let HealthCheck {
            host, port, path, ..
        } = self;
        format!(
            "curl -f http://{host}:{port}{path} || wget -qO- http://{host}:{port}{path} || exit 1"
        )
    }
}

/// DB-free description of a single application to be rendered into a compose file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppComposeInput {
    pub application_id: i64,
    pub application_uuid: String,
    /// PR number for a preview container; `0` for a production container. Emitted
    /// as the `rustify.pullRequestId` label so cleanup/status can target previews.
    pub pull_request_id: i64,
    pub deployment_uuid: String,
    /// `{app_uuid}-{6char}` per Contract C7.
    pub container_name: String,
    /// Compose service key.
    pub service_name: String,
    pub image: String,
    /// Destination network (default `rustify`).
    pub network: String,
    /// Internal ports the container exposes (e.g. `["3000"]`).
    pub ports_exposes: Vec<String>,
    /// Host published port mappings (e.g. `["8080:3000"]`); usually empty.
    pub ports_mappings: Vec<String>,
    /// Full URL incl. scheme, e.g. `https://x.example.com`.
    pub fqdn: Option<String>,
    pub health: Option<HealthCheck>,
    /// Memory limit; `"0"` (unlimited) omits the key.
    pub limits_memory: String,
    /// CPU limit; `"0"` (unlimited) omits the key.
    pub limits_cpus: String,
    /// Bind/volume mounts (`host:container` strings).
    pub volumes: Vec<String>,
    /// Path to a runtime `.env` file, referenced via `env_file`.
    pub env_file: Option<String>,
    /// Restart policy (default `unless-stopped`).
    pub restart: String,
}

#[derive(Serialize)]
struct Compose {
    services: BTreeMap<String, Service>,
    networks: BTreeMap<String, Network>,
}

#[derive(Serialize)]
struct Service {
    container_name: String,
    image: String,
    restart: String,
    networks: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    expose: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    ports: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    env_file: Option<Vec<String>>,
    labels: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    healthcheck: Option<Healthcheck>,
    #[serde(skip_serializing_if = "Option::is_none")]
    mem_limit: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    cpus: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    volumes: Vec<String>,
}

#[derive(Serialize)]
struct Healthcheck {
    test: Vec<String>,
    interval: String,
    timeout: String,
    retries: u32,
    start_period: String,
}

#[derive(Serialize)]
struct Network {
    external: bool,
}

/// Render a single-service compose file for `app`, emitting Traefik container
/// labels. Deterministic output.
pub fn generate_compose(app: &AppComposeInput) -> String {
    generate_compose_for_proxy(app, ProxyKind::Traefik)
}

/// Render a single-service compose file for `app` under the given reverse proxy,
/// selecting the matching container label set (Traefik vs Caddy). Deterministic.
pub fn generate_compose_for_proxy(app: &AppComposeInput, kind: ProxyKind) -> String {
    let healthcheck = app.health.as_ref().map(|h| Healthcheck {
        test: vec!["CMD-SHELL".to_string(), h.test_command()],
        interval: format!("{}s", h.interval_secs),
        timeout: format!("{}s", h.timeout_secs),
        retries: h.retries,
        start_period: format!("{}s", h.start_period_secs),
    });

    let mem_limit = (app.limits_memory != "0").then(|| app.limits_memory.clone());
    let cpus = (app.limits_cpus != "0").then(|| app.limits_cpus.clone());
    let env_file = app.env_file.as_ref().map(|f| vec![f.clone()]);

    let service = Service {
        container_name: app.container_name.clone(),
        image: app.image.clone(),
        restart: app.restart.clone(),
        networks: vec![app.network.clone()],
        expose: app.ports_exposes.clone(),
        ports: app.ports_mappings.clone(),
        env_file,
        labels: labels_for(app, kind),
        healthcheck,
        mem_limit,
        cpus,
        volumes: app.volumes.clone(),
    };

    let mut services = BTreeMap::new();
    services.insert(app.service_name.clone(), service);
    let mut networks = BTreeMap::new();
    networks.insert(app.network.clone(), Network { external: true });

    let compose = Compose { services, networks };
    // serde_yaml::to_string on an owned Serialize value cannot fail here.
    serde_yaml::to_string(&compose).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn full_app() -> AppComposeInput {
        AppComposeInput {
            application_id: 42,
            application_uuid: "app-uuid".to_string(),
            pull_request_id: 0,
            deployment_uuid: "dep-uuid".to_string(),
            container_name: "app-uuid-abc123".to_string(),
            service_name: "app-uuid-abc123".to_string(),
            image: "app-uuid:commitsha".to_string(),
            network: "rustify".to_string(),
            ports_exposes: vec!["3000".to_string()],
            ports_mappings: vec![],
            fqdn: Some("https://x.example.com".to_string()),
            health: Some(HealthCheck {
                host: "localhost".to_string(),
                port: 3000,
                path: "/health".to_string(),
                interval_secs: 5,
                timeout_secs: 5,
                retries: 10,
                start_period_secs: 5,
            }),
            limits_memory: "512m".to_string(),
            limits_cpus: "1.5".to_string(),
            volumes: vec!["/data/rustify/applications/app-uuid/storage:/app/storage".to_string()],
            env_file: Some(".env".to_string()),
            restart: "unless-stopped".to_string(),
        }
    }

    #[test]
    fn healthcheck_command_is_exact() {
        let h = HealthCheck {
            host: "localhost".to_string(),
            port: 3000,
            path: "/health".to_string(),
            interval_secs: 5,
            timeout_secs: 5,
            retries: 10,
            start_period_secs: 5,
        };
        assert_eq!(
            h.test_command(),
            "curl -f http://localhost:3000/health || wget -qO- http://localhost:3000/health || exit 1"
        );
    }

    #[test]
    fn full_app_matches_golden() {
        let generated = generate_compose(&full_app());
        let golden = crate::test_support::load_golden("compose-full.yaml");
        assert_eq!(generated.trim(), golden.trim());
    }

    #[test]
    fn minimal_app_matches_golden() {
        let app = AppComposeInput {
            application_id: 1,
            application_uuid: "minimal".to_string(),
            pull_request_id: 0,
            deployment_uuid: "dep".to_string(),
            container_name: "minimal-xyz789".to_string(),
            service_name: "minimal-xyz789".to_string(),
            image: "minimal:latest".to_string(),
            network: "rustify".to_string(),
            ports_exposes: vec!["80".to_string()],
            ports_mappings: vec![],
            fqdn: None,
            health: None,
            limits_memory: "0".to_string(),
            limits_cpus: "0".to_string(),
            volumes: vec![],
            env_file: None,
            restart: "unless-stopped".to_string(),
        };
        let generated = generate_compose(&app);
        let golden = crate::test_support::load_golden("compose-minimal.yaml");
        assert_eq!(generated.trim(), golden.trim());
    }

    #[test]
    fn unlimited_resources_are_omitted() {
        let mut app = full_app();
        app.limits_memory = "0".to_string();
        app.limits_cpus = "0".to_string();
        let generated = generate_compose(&app);
        assert!(!generated.contains("mem_limit"));
        assert!(!generated.contains("cpus"));
    }

    #[test]
    fn healthcheck_fallback_chain_present_in_compose() {
        let generated = generate_compose(&full_app());
        assert!(generated.contains(
            "curl -f http://localhost:3000/health || wget -qO- http://localhost:3000/health || exit 1"
        ));
    }
}
