//! Container label generation per Contract C7.
//!
//! Every managed container carries the `rustify.*` bookkeeping labels plus, when
//! the app has an FQDN, a Traefik http router, an https (`-secure`) router with
//! the `letsencrypt` cert resolver, and a loadbalancer service port. Derived
//! from Coolify's `fqdnLabelsForTraefik` (bootstrap/helpers/docker.php:499-585),
//! simplified to the pinned C7 label set.

use crate::compose::AppComposeInput;

/// Parsed FQDN pieces: (host, scheme).
fn parse_host(fqdn: &str) -> String {
    // Strip scheme.
    let without_scheme = fqdn.split_once("://").map(|(_, r)| r).unwrap_or(fqdn);
    // Host ends at the first `/`, `:` (port) or `?`.
    without_scheme
        .split(['/', ':', '?'])
        .next()
        .unwrap_or(without_scheme)
        .to_string()
}

/// The port Traefik load-balances to: first exposed port, defaulting to 80.
fn service_port(app: &AppComposeInput) -> String {
    app.ports_exposes
        .first()
        .cloned()
        .unwrap_or_else(|| "80".to_string())
}

/// Full label set for a managed container.
pub fn traefik_labels(app: &AppComposeInput) -> Vec<String> {
    let uuid = &app.application_uuid;
    let mut labels = vec![
        "rustify.managed=true".to_string(),
        format!("rustify.applicationId={}", app.application_id),
        format!("rustify.applicationUuid={}", uuid),
        format!("rustify.pullRequestId={}", app.pull_request_id),
        format!("rustify.deploymentId={}", app.deployment_uuid),
        "traefik.enable=true".to_string(),
    ];

    if let Some(fqdn) = &app.fqdn {
        let host = parse_host(fqdn);
        let port = service_port(app);
        // http router
        labels.push(format!("traefik.http.routers.{uuid}.rule=Host(`{host}`)"));
        labels.push(format!("traefik.http.routers.{uuid}.entrypoints=http"));
        // https router
        labels.push(format!(
            "traefik.http.routers.{uuid}-secure.rule=Host(`{host}`)"
        ));
        labels.push(format!(
            "traefik.http.routers.{uuid}-secure.entrypoints=https"
        ));
        labels.push(format!(
            "traefik.http.routers.{uuid}-secure.tls.certresolver=letsencrypt"
        ));
        // shared service
        labels.push(format!(
            "traefik.http.services.{uuid}.loadbalancer.server.port={port}"
        ));
    }

    labels
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compose::AppComposeInput;

    fn app_with_fqdn(fqdn: Option<&str>) -> AppComposeInput {
        AppComposeInput {
            application_id: 42,
            application_uuid: "app-uuid".to_string(),
            pull_request_id: 0,
            deployment_uuid: "dep-uuid".to_string(),
            container_name: "app-uuid-abc123".to_string(),
            service_name: "app-uuid-abc123".to_string(),
            image: "app-uuid:sha".to_string(),
            network: "rustify".to_string(),
            ports_exposes: vec!["3000".to_string()],
            ports_mappings: vec![],
            fqdn: fqdn.map(|s| s.to_string()),
            health: None,
            limits_memory: "0".to_string(),
            limits_cpus: "0".to_string(),
            volumes: vec![],
            env_file: None,
            restart: "unless-stopped".to_string(),
        }
    }

    #[test]
    fn parse_host_strips_scheme_and_path() {
        assert_eq!(parse_host("https://x.example.com"), "x.example.com");
        assert_eq!(parse_host("http://x.example.com/foo"), "x.example.com");
        assert_eq!(
            parse_host("https://x.example.com:8443/foo"),
            "x.example.com"
        );
    }

    #[test]
    fn bookkeeping_labels_always_present() {
        let labels = traefik_labels(&app_with_fqdn(None));
        assert_eq!(
            labels,
            vec![
                "rustify.managed=true",
                "rustify.applicationId=42",
                "rustify.applicationUuid=app-uuid",
                "rustify.pullRequestId=0",
                "rustify.deploymentId=dep-uuid",
                "traefik.enable=true",
            ]
        );
    }

    #[test]
    fn fqdn_produces_http_and_https_routers_matching_golden() {
        let labels = traefik_labels(&app_with_fqdn(Some("https://x.example.com")));
        let golden = crate::test_support::load_golden("labels-https.txt");
        assert_eq!(labels.join("\n").trim(), golden.trim());
    }

    #[test]
    fn https_router_has_certresolver_and_port() {
        let labels = traefik_labels(&app_with_fqdn(Some("https://x.example.com")));
        assert!(labels.contains(
            &"traefik.http.routers.app-uuid-secure.tls.certresolver=letsencrypt".to_string()
        ));
        assert!(
            labels.contains(
                &"traefik.http.services.app-uuid.loadbalancer.server.port=3000".to_string()
            )
        );
        assert!(labels.contains(&"traefik.http.routers.app-uuid.entrypoints=http".to_string()));
        assert!(
            labels.contains(&"traefik.http.routers.app-uuid-secure.entrypoints=https".to_string())
        );
    }
}
