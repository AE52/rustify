//! Parsers for `docker inspect` and `docker ps` output that the status layer
//! consumes. Reads the `rustify.*` labels defined in Contract C7.

use serde::Deserialize;

/// Health of a container as reported by `docker inspect`'s `.State.Health.Status`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContainerHealth {
    Healthy,
    Unhealthy,
    Starting,
    /// No healthcheck configured on the container.
    None,
}

/// A rustify-managed container as read from `docker ps --format json`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManagedContainer {
    pub id: String,
    pub name: String,
    pub image: String,
    pub state: String,
    pub application_id: Option<String>,
    pub application_uuid: Option<String>,
    pub deployment_uuid: Option<String>,
}

/// Parse the health status out of `docker inspect <container>` JSON (an array of
/// one inspect object). Returns [`ContainerHealth::None`] when no healthcheck is
/// configured or the JSON cannot be parsed.
pub fn parse_health(inspect_json: &str) -> ContainerHealth {
    let value: serde_json::Value = match serde_json::from_str(inspect_json) {
        Ok(v) => v,
        Err(_) => return ContainerHealth::None,
    };
    // `docker inspect` yields an array; a single object is also tolerated.
    let object = value.get(0).unwrap_or(&value);
    let status = object
        .get("State")
        .and_then(|s| s.get("Health"))
        .and_then(|h| h.get("Status"))
        .and_then(|s| s.as_str());
    match status {
        Some("healthy") => ContainerHealth::Healthy,
        Some("unhealthy") => ContainerHealth::Unhealthy,
        Some("starting") => ContainerHealth::Starting,
        _ => ContainerHealth::None,
    }
}

#[derive(Deserialize)]
struct PsLine {
    #[serde(rename = "ID", default)]
    id: String,
    #[serde(rename = "Names", default)]
    names: String,
    #[serde(rename = "Image", default)]
    image: String,
    #[serde(rename = "State", default)]
    state: String,
    #[serde(rename = "Labels", default)]
    labels: String,
}

/// Look up a single label value from Docker's comma-joined `k=v,k2=v2` label string.
fn label_value<'a>(labels: &'a str, key: &str) -> Option<&'a str> {
    labels.split(',').find_map(|pair| {
        let (k, v) = pair.split_once('=')?;
        (k == key).then_some(v)
    })
}

/// Parse `docker ps --format json` (one JSON object per line, JSONL). Only
/// containers carrying `rustify.managed=true` are returned.
pub fn parse_containers(ps_json: &str) -> Vec<ManagedContainer> {
    ps_json
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .filter_map(|line| serde_json::from_str::<PsLine>(line).ok())
        .filter(|line| label_value(&line.labels, "rustify.managed") == Some("true"))
        .map(|line| ManagedContainer {
            id: line.id.clone(),
            name: line.names.clone(),
            image: line.image.clone(),
            state: line.state.clone(),
            application_id: label_value(&line.labels, "rustify.applicationId").map(str::to_string),
            application_uuid: label_value(&line.labels, "rustify.applicationUuid")
                .map(str::to_string),
            deployment_uuid: label_value(&line.labels, "rustify.deploymentId").map(str::to_string),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> String {
        let path = format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"));
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {path}: {e}"))
    }

    #[test]
    fn parse_health_healthy() {
        assert_eq!(
            parse_health(&fixture("inspect-healthy.json")),
            ContainerHealth::Healthy
        );
    }

    #[test]
    fn parse_health_starting() {
        assert_eq!(
            parse_health(&fixture("inspect-starting.json")),
            ContainerHealth::Starting
        );
    }

    #[test]
    fn parse_health_unhealthy() {
        assert_eq!(
            parse_health(&fixture("inspect-unhealthy.json")),
            ContainerHealth::Unhealthy
        );
    }

    #[test]
    fn parse_health_none_when_no_healthcheck() {
        assert_eq!(
            parse_health(&fixture("inspect-nohealth.json")),
            ContainerHealth::None
        );
    }

    #[test]
    fn parse_health_none_on_garbage() {
        assert_eq!(parse_health("not json"), ContainerHealth::None);
    }

    #[test]
    fn parse_containers_reads_rustify_labels() {
        let containers = parse_containers(&fixture("ps.jsonl"));
        assert_eq!(containers.len(), 2);
        let first = &containers[0];
        assert_eq!(first.name, "app-uuid-abc123");
        assert_eq!(first.image, "app-uuid:commitsha");
        assert_eq!(first.state, "running");
        assert_eq!(first.application_id.as_deref(), Some("42"));
        assert_eq!(first.application_uuid.as_deref(), Some("app-uuid"));
        assert_eq!(first.deployment_uuid.as_deref(), Some("dep-uuid"));
    }

    #[test]
    fn parse_containers_skips_unmanaged() {
        // The fixture also contains a non-rustify container (coolify-proxy) that
        // must be filtered out.
        let containers = parse_containers(&fixture("ps.jsonl"));
        assert!(containers.iter().all(|c| c.name != "some-other-container"));
    }
}
