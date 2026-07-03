//! Caddy (caddy-docker-proxy) reverse-proxy compose generation.
//!
//! Port of Coolify's `generateDefaultProxyConfiguration` CADDY branch
//! (bootstrap/helpers/proxy.php:360-393), adapted to Rustify naming per Contract
//! C7: proxy container `rustify-proxy`, config dir `/data/rustify/proxy`, default
//! network `rustify`. Unlike Traefik, Caddy exposes no `8080` dashboard port.

use serde::Serialize;
use std::collections::BTreeMap;

use crate::config::{PROXY_CONTAINER, PROXY_DIR, PROXY_NETWORK};

#[derive(Serialize)]
struct CaddyCompose {
    name: String,
    networks: BTreeMap<String, ExternalNetwork>,
    services: BTreeMap<String, CaddyService>,
}

#[derive(Serialize)]
struct ExternalNetwork {
    external: bool,
}

#[derive(Serialize)]
struct CaddyService {
    container_name: String,
    image: String,
    restart: String,
    extra_hosts: Vec<String>,
    environment: Vec<String>,
    networks: Vec<String>,
    ports: Vec<String>,
    labels: Vec<String>,
    volumes: Vec<String>,
}

/// Generate the Caddy proxy `docker-compose.yml`.
pub fn generate_caddy_proxy_compose() -> String {
    let service = CaddyService {
        container_name: PROXY_CONTAINER.to_string(),
        image: "lucaslorentz/caddy-docker-proxy:2.8-alpine".to_string(),
        restart: "unless-stopped".to_string(),
        extra_hosts: vec!["host.docker.internal:host-gateway".to_string()],
        environment: vec![
            "CADDY_DOCKER_POLLING_INTERVAL=5s".to_string(),
            "CADDY_DOCKER_CADDYFILE_PATH=/dynamic/Caddyfile".to_string(),
        ],
        networks: vec![PROXY_NETWORK.to_string()],
        ports: vec![
            "80:80".to_string(),
            "443:443".to_string(),
            "443:443/udp".to_string(),
        ],
        labels: vec![
            "rustify.managed=true".to_string(),
            "rustify.proxy=true".to_string(),
        ],
        volumes: vec![
            "/var/run/docker.sock:/var/run/docker.sock:ro".to_string(),
            format!("{PROXY_DIR}/dynamic:/dynamic"),
            format!("{PROXY_DIR}/config:/config"),
            format!("{PROXY_DIR}/data:/data"),
        ],
    };

    let mut networks = BTreeMap::new();
    networks.insert(
        PROXY_NETWORK.to_string(),
        ExternalNetwork { external: true },
    );
    let mut services = BTreeMap::new();
    services.insert("caddy".to_string(), service);

    let compose = CaddyCompose {
        name: PROXY_CONTAINER.to_string(),
        networks,
        services,
    };
    serde_yaml::to_string(&compose).unwrap_or_default()
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
    fn caddy_compose_matches_golden() {
        let generated = generate_caddy_proxy_compose();
        assert_eq!(generated.trim(), load_golden("caddy-compose.yaml").trim());
    }

    #[test]
    fn caddy_compose_uses_rustify_naming_and_no_dashboard_port() {
        let yaml = generate_caddy_proxy_compose();
        assert!(yaml.contains("container_name: rustify-proxy"));
        assert!(yaml.contains("image: lucaslorentz/caddy-docker-proxy:2.8-alpine"));
        assert!(yaml.contains("rustify.proxy=true"));
        assert!(yaml.contains(&format!("{PROXY_DIR}/dynamic:/dynamic")));
        // Caddy exposes 80/443/443udp but never the 8080 dashboard port.
        assert!(yaml.contains("- 80:80"));
        assert!(yaml.contains("- 443:443/udp"));
        assert!(!yaml.contains("8080"));
    }
}
