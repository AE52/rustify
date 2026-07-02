//! Docker-image buildpack: deploy a pre-built registry image, no build.
//!
//! Parity with Coolify's `deploy_dockerimage_buildpack`: there is no git repo
//! and no build stage — the configured `docker_registry_image_name:tag` is
//! pulled and run directly.

/// The fully-qualified image reference from name + tag (tag defaults to `latest`).
pub fn registry_image(name: &str, tag: Option<&str>) -> String {
    let tag = tag.filter(|t| !t.is_empty()).unwrap_or("latest");
    format!("{name}:{tag}")
}

/// `docker pull <image>`.
pub fn pull_command(image: &str) -> String {
    format!("docker pull {image}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn image_ref_defaults_tag_to_latest() {
        assert_eq!(
            registry_image("ghcr.io/o/app", None),
            "ghcr.io/o/app:latest"
        );
        assert_eq!(registry_image("redis", Some("")), "redis:latest");
        assert_eq!(registry_image("redis", Some("7-alpine")), "redis:7-alpine");
    }

    #[test]
    fn pull_command_shape() {
        assert_eq!(pull_command("redis:7"), "docker pull redis:7");
    }
}
