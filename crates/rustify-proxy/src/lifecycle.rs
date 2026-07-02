//! Shell scripts that start and stop the Traefik proxy on a server.
//!
//! Ports of Coolify's `StartProxy` (app/Actions/Proxy/StartProxy.php:50-87,
//! standalone branch) and `StopProxy` (app/Actions/Proxy/StopProxy.php:26-36),
//! adapted to Rustify naming per Contract C7 (`rustify-proxy`, `/data/rustify/proxy`,
//! network `rustify`).

use crate::config::{PROXY_CONTAINER, PROXY_DIR, PROXY_NETWORK};

/// Heredoc delimiter unlikely to collide with compose content.
const HEREDOC: &str = "RUSTIFY_PROXY_EOF";

/// Build the start script: create dirs, write the compose file, ensure the
/// network exists (idempotent guard), pull, bring the proxy up, and connect it
/// to the network idempotently. `compose_yaml` is the output of
/// [`crate::config::generate_proxy_compose`].
pub fn start_script(compose_yaml: &str) -> String {
    let compose_path = format!("{PROXY_DIR}/docker-compose.yml");
    format!(
        "set -e\n\
         mkdir -p {PROXY_DIR}/dynamic\n\
         cd {PROXY_DIR}\n\
         cat > {compose_path} <<'{HEREDOC}'\n\
         {compose_yaml}\n\
         {HEREDOC}\n\
         docker network create --attachable {PROXY_NETWORK} || true\n\
         docker compose -f {compose_path} pull\n\
         docker compose -f {compose_path} up -d --wait --remove-orphans\n\
         docker network connect {PROXY_NETWORK} {PROXY_CONTAINER} >/dev/null 2>&1 || true\n"
    )
}

/// Build the stop script: stop and force-remove the proxy container, then wait
/// for it to disappear.
pub fn stop_script() -> String {
    format!(
        "docker stop -t=30 {PROXY_CONTAINER} 2>/dev/null || true\n\
         docker rm -f {PROXY_CONTAINER} 2>/dev/null || true\n\
         for i in $(seq 1 10); do\n\
         \x20   if ! docker ps -a --format \"{{{{.Names}}}}\" | grep -q \"^{PROXY_CONTAINER}$\"; then\n\
         \x20       break\n\
         \x20   fi\n\
         \x20   sleep 1\n\
         done\n"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::generate_proxy_compose;

    #[test]
    fn start_script_contains_network_create_guard() {
        let script = start_script(&generate_proxy_compose(&[]));
        assert!(script.contains("docker network create --attachable rustify || true"));
    }

    #[test]
    fn start_script_writes_compose_via_heredoc() {
        let compose = generate_proxy_compose(&[]);
        let script = start_script(&compose);
        assert!(script.contains(&format!(
            "cat > {PROXY_DIR}/docker-compose.yml <<'{HEREDOC}'"
        )));
        // The compose body is embedded verbatim between the heredoc markers.
        assert!(script.contains("image: traefik:v3.6"));
        assert!(script.contains("mkdir -p /data/rustify/proxy/dynamic"));
    }

    #[test]
    fn start_script_brings_proxy_up_and_connects_network() {
        let script = start_script(&generate_proxy_compose(&[]));
        assert!(script.contains(
            "docker compose -f /data/rustify/proxy/docker-compose.yml up -d --wait --remove-orphans"
        ));
        assert!(
            script.contains("docker network connect rustify rustify-proxy >/dev/null 2>&1 || true")
        );
    }

    #[test]
    fn stop_script_stops_and_removes_container() {
        let script = stop_script();
        assert!(script.contains("docker stop -t=30 rustify-proxy 2>/dev/null || true"));
        assert!(script.contains("docker rm -f rustify-proxy 2>/dev/null || true"));
        assert!(script.contains("grep -q \"^rustify-proxy$\""));
    }
}
