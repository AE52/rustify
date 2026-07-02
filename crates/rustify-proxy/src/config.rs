//! Traefik proxy compose generation and custom-command survival.
//!
//! Port of Coolify's `generateDefaultProxyConfiguration` Traefik branch
//! (bootstrap/helpers/proxy.php:266-359) and `extractCustomProxyCommands`
//! (:170-224), adapted to Rustify naming per Contract C7: proxy container
//! `rustify-proxy`, config dir `/data/rustify/proxy`, default network `rustify`.

use serde::Serialize;
use std::collections::BTreeMap;

/// Proxy container name (Contract C7).
pub const PROXY_CONTAINER: &str = "rustify-proxy";
/// Proxy config directory on the server (Contract C7).
pub const PROXY_DIR: &str = "/data/rustify/proxy";
/// Default destination network (Contract C7).
pub const PROXY_NETWORK: &str = "rustify";

/// The default Traefik command flags Rustify always generates, in order.
/// Mirrors proxy.php:304-351 (standalone, non-dev branch).
fn default_commands() -> Vec<String> {
    [
        "--ping=true",
        "--ping.entrypoint=http",
        "--api.dashboard=true",
        "--entrypoints.http.address=:80",
        "--entrypoints.https.address=:443",
        "--entrypoints.http.http.encodequerysemicolons=true",
        "--entryPoints.http.http2.maxConcurrentStreams=250",
        "--entrypoints.https.http.encodequerysemicolons=true",
        "--entryPoints.https.http2.maxConcurrentStreams=250",
        "--entrypoints.https.http3",
        "--providers.file.directory=/traefik/dynamic/",
        "--providers.file.watch=true",
        "--certificatesresolvers.letsencrypt.acme.httpchallenge=true",
        "--certificatesresolvers.letsencrypt.acme.httpchallenge.entrypoint=http",
        "--certificatesresolvers.letsencrypt.acme.storage=/traefik/acme.json",
        "--api.insecure=false",
        "--providers.docker=true",
        "--providers.docker.exposedbydefault=false",
    ]
    .into_iter()
    .map(String::from)
    .collect()
}

/// Default command prefixes used to distinguish Rustify-generated flags from
/// user-supplied ones. Same list as Coolify proxy.php:188-204.
const DEFAULT_COMMAND_PREFIXES: &[&str] = &[
    "--ping=",
    "--api.",
    "--entrypoints.http.address=",
    "--entrypoints.https.address=",
    "--entrypoints.http.http.encodequerysemicolons=",
    "--entryPoints.http.http2.maxConcurrentStreams=",
    "--entrypoints.https.http.encodequerysemicolons=",
    "--entryPoints.https.http2.maxConcurrentStreams=",
    "--entrypoints.https.http3",
    "--providers.file.",
    "--certificatesresolvers.",
    "--providers.docker",
    "--providers.swarm",
    "--log.level=",
    "--accesslog.",
];

#[derive(Serialize)]
struct ProxyCompose {
    name: String,
    networks: BTreeMap<String, ExternalNetwork>,
    services: BTreeMap<String, TraefikService>,
}

#[derive(Serialize)]
struct ExternalNetwork {
    external: bool,
}

#[derive(Serialize)]
struct TraefikService {
    container_name: String,
    image: String,
    restart: String,
    extra_hosts: Vec<String>,
    networks: Vec<String>,
    ports: Vec<String>,
    healthcheck: ProxyHealthcheck,
    volumes: Vec<String>,
    command: Vec<String>,
    labels: Vec<String>,
}

#[derive(Serialize)]
struct ProxyHealthcheck {
    test: String,
    interval: String,
    timeout: String,
    retries: u32,
}

/// Generate the Traefik proxy `docker-compose.yml`. `custom_commands` are
/// appended verbatim after the default command flags (parity with proxy.php:355-358).
pub fn generate_proxy_compose(custom_commands: &[String]) -> String {
    let mut command = default_commands();
    // Append survivors, but never duplicate an exact default flag. Coolify's own
    // prefix list (used by `extract_custom_commands`) fails to match one of its
    // default flags (`--ping.entrypoint=http`), so a naive round-trip would
    // re-append it; this exact-match guard keeps regeneration idempotent while
    // preserving the pinned prefix list verbatim.
    for custom in custom_commands {
        if !command.contains(custom) {
            command.push(custom.clone());
        }
    }

    let service = TraefikService {
        container_name: PROXY_CONTAINER.to_string(),
        image: "traefik:v3.6".to_string(),
        restart: "unless-stopped".to_string(),
        extra_hosts: vec!["host.docker.internal:host-gateway".to_string()],
        networks: vec![PROXY_NETWORK.to_string()],
        ports: vec![
            "80:80".to_string(),
            "443:443".to_string(),
            "443:443/udp".to_string(),
            "8080:8080".to_string(),
        ],
        healthcheck: ProxyHealthcheck {
            test: "wget -qO- http://localhost:80/ping || exit 1".to_string(),
            interval: "4s".to_string(),
            timeout: "2s".to_string(),
            retries: 5,
        },
        volumes: vec![
            "/var/run/docker.sock:/var/run/docker.sock:ro".to_string(),
            format!("{PROXY_DIR}:/traefik"),
        ],
        command,
        labels: vec![
            "traefik.enable=true".to_string(),
            "traefik.http.routers.traefik.entrypoints=http".to_string(),
            "traefik.http.routers.traefik.service=api@internal".to_string(),
            "traefik.http.services.traefik.loadbalancer.server.port=8080".to_string(),
            "rustify.managed=true".to_string(),
            "rustify.proxy=true".to_string(),
        ],
    };

    let mut networks = BTreeMap::new();
    networks.insert(
        PROXY_NETWORK.to_string(),
        ExternalNetwork { external: true },
    );
    let mut services = BTreeMap::new();
    services.insert("traefik".to_string(), service);

    let compose = ProxyCompose {
        name: PROXY_CONTAINER.to_string(),
        networks,
        services,
    };
    serde_yaml::to_string(&compose).unwrap_or_default()
}

/// Extract user-supplied (non-default) Traefik command flags from an existing
/// proxy compose YAML, so they survive regeneration. Returns an empty vec if the
/// YAML cannot be parsed or has no traefik command list (parity with proxy.php:170-224).
pub fn extract_custom_commands(existing_yaml: &str) -> Vec<String> {
    if existing_yaml.trim().is_empty() {
        return Vec::new();
    }
    let value: serde_yaml::Value = match serde_yaml::from_str(existing_yaml) {
        Ok(v) => v,
        Err(_) => return Vec::new(),
    };
    let commands = value
        .get("services")
        .and_then(|s| s.get("traefik"))
        .and_then(|t| t.get("command"))
        .and_then(|c| c.as_sequence());
    let Some(commands) = commands else {
        return Vec::new();
    };
    commands
        .iter()
        .filter_map(|c| c.as_str())
        .filter(|command| {
            !DEFAULT_COMMAND_PREFIXES
                .iter()
                .any(|prefix| command.starts_with(prefix))
        })
        .map(String::from)
        .collect()
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
    fn default_compose_matches_golden() {
        let generated = generate_proxy_compose(&[]);
        assert_eq!(generated.trim(), load_golden("proxy-compose.yaml").trim());
    }

    #[test]
    fn compose_uses_rustify_naming() {
        let yaml = generate_proxy_compose(&[]);
        assert!(yaml.contains("container_name: rustify-proxy"));
        assert!(yaml.contains(&format!("{PROXY_DIR}:/traefik")));
        assert!(yaml.contains("image: traefik:v3.6"));
        assert!(yaml.contains("rustify.proxy=true"));
    }

    #[test]
    fn custom_commands_appended_after_defaults() {
        let custom = vec!["--entryPoints.http.forwardedHeaders.trustedIPs=10.0.0.0/8".to_string()];
        let yaml = generate_proxy_compose(&custom);
        assert!(yaml.contains("--entryPoints.http.forwardedHeaders.trustedIPs=10.0.0.0/8"));
        // The last command line should be the custom one.
        let last_command = yaml
            .lines()
            .rfind(|l| l.trim_start().starts_with("- --"))
            .unwrap()
            .trim();
        assert_eq!(
            last_command,
            "- --entryPoints.http.forwardedHeaders.trustedIPs=10.0.0.0/8"
        );
    }

    #[test]
    fn custom_command_survives_regeneration_without_default_duplication() {
        // A generated config carrying an injected default flag (--log.level=error)
        // plus a genuinely custom flag.
        let custom = vec!["--entryPoints.http.forwardedHeaders.trustedIPs=10.0.0.0/8".to_string()];
        let mut yaml = generate_proxy_compose(&custom);
        // Simulate a user/tool having added a default-prefixed flag too.
        yaml = yaml.replace(
            "    - --providers.docker.exposedbydefault=false\n",
            "    - --providers.docker.exposedbydefault=false\n    - --log.level=error\n",
        );

        let survivors = extract_custom_commands(&yaml);
        // The genuinely custom flag survives; the default-prefixed --log.level=
        // does not.
        assert!(
            survivors
                .contains(&"--entryPoints.http.forwardedHeaders.trustedIPs=10.0.0.0/8".to_string())
        );
        assert!(!survivors.iter().any(|c| c.starts_with("--log.level=")));

        // Regenerating with the survivors duplicates neither the default flags
        // nor the custom one.
        let regenerated = generate_proxy_compose(&survivors);
        assert_eq!(regenerated.matches("--ping=true").count(), 1);
        assert_eq!(regenerated.matches("--ping.entrypoint=http").count(), 1);
        assert_eq!(
            regenerated
                .matches("--entryPoints.http.forwardedHeaders.trustedIPs=10.0.0.0/8")
                .count(),
            1
        );
        assert_eq!(regenerated.matches("--log.level=error").count(), 0);
    }

    #[test]
    fn extract_returns_empty_on_garbage() {
        assert!(extract_custom_commands("not: [valid").is_empty());
        assert!(extract_custom_commands("").is_empty());
        assert!(extract_custom_commands("services: {}").is_empty());
    }
}
