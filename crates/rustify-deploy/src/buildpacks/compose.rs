//! Docker-Compose buildpack: run the user's own compose file, no rolling update.
//!
//! Parity with Coolify's `deploy_docker_compose_buildpack`: after cloning, the
//! user compose is brought up in place with `docker compose up -d --build`,
//! with the runtime env file supplied via `--env-file`. Rustify Phase 1 runs
//! the compose inside the helper container (which shares the host docker
//! socket), so no per-service label/network override rewriting is performed
//! beyond what the user's compose declares.

/// `docker compose ... up -d --build` for the user compose located at
/// `compose_location` (repo-relative), run from `workdir`, sourcing `env_file`.
pub fn up_command(workdir: &str, compose_location: &str, env_file: &str) -> String {
    let rel = compose_location.trim_start_matches('/');
    format!(
        "cd {workdir} && docker compose --env-file {env_file} -f {rel} up -d --build --remove-orphans"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn up_command_shape() {
        let cmd = up_command(
            "/artifacts/d",
            "/docker-compose.yaml",
            "/artifacts/runtime.env",
        );
        assert_eq!(
            cmd,
            "cd /artifacts/d && docker compose --env-file /artifacts/runtime.env -f docker-compose.yaml up -d --build --remove-orphans"
        );
    }
}
