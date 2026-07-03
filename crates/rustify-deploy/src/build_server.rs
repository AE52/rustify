//! Build-server support: image-name resolution, the registry push command, and
//! the build-target selection + SSH-target-switch flow.
//!
//! Behavioural port of Coolify's `ApplicationDeploymentJob` build-server branch
//! (app/Jobs/ApplicationDeploymentJob.php:347-360, 545-546, 888-925): when an
//! application enables a build server, the image is built and `docker push`ed to
//! a registry on the build server, then the SSH target switches to the deploy
//! server which pulls and runs the pushed image. Build servers are excluded from
//! proxy/destination/deploy-target lists (see `ServerRepo::deploy_targets`).

use rustify_core::exec::{CommandExecutor, ExecError, ExecOpts, ServerConn};

/// The production image reference `name:tag`, defaulting the tag to `latest`.
/// Requires a non-empty registry image name (Coolify: `docker_registry_image_name`).
pub fn registry_image_ref(name: &str, tag: Option<&str>) -> Option<String> {
    let name = name.trim();
    if name.is_empty() {
        return None;
    }
    let tag = tag
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .unwrap_or("latest");
    Some(format!("{name}:{tag}"))
}

/// The `docker push` command for a resolved image reference.
pub fn push_image_command(image_ref: &str) -> String {
    format!("docker push {image_ref}")
}

/// The `docker pull` command run on the deploy server after a build-server push.
pub fn pull_image_command(image_ref: &str) -> String {
    format!("docker pull {image_ref}")
}

/// Which servers a deployment builds and runs on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildTargets {
    /// Server the image is built (and, when `use_build_server`, pushed) on.
    pub build_server_id: i64,
    /// Server the container is pulled and run on.
    pub deploy_server_id: i64,
    /// True when build and deploy happen on different servers (image goes via a
    /// registry). Parity with Coolify's `$use_build_server`.
    pub use_build_server: bool,
}

/// Decide the build/deploy split. `application_build_server_id` is the app's
/// pinned build server (if any); `available_build_server_ids` are the team's
/// usable build servers. Falls back to building on the deploy server when no
/// build server is enabled/available (Coolify: `build_server = server`).
pub fn plan_build_targets(
    deploy_server_id: i64,
    application_build_server_id: Option<i64>,
    available_build_server_ids: &[i64],
) -> BuildTargets {
    // No build server requested → build on the deploy server itself.
    let Some(pinned) = application_build_server_id else {
        return BuildTargets {
            build_server_id: deploy_server_id,
            deploy_server_id,
            use_build_server: false,
        };
    };

    // Requested but none usable → fall back to the deploy server.
    if available_build_server_ids.is_empty() {
        return BuildTargets {
            build_server_id: deploy_server_id,
            deploy_server_id,
            use_build_server: false,
        };
    }

    // Prefer the pinned build server when it is usable; otherwise the first
    // available one.
    let build_server_id = if available_build_server_ids.contains(&pinned) {
        pinned
    } else {
        available_build_server_ids[0]
    };

    BuildTargets {
        build_server_id,
        deploy_server_id,
        use_build_server: build_server_id != deploy_server_id,
    }
}

/// Push a freshly-built image from the build server, then switch the SSH target
/// to the deploy server and pull it there. Only invoked when
/// `BuildTargets::use_build_server` is true. Returns `Err` if the image name is
/// missing (a build server needs a registry image name to push to).
pub async fn push_then_pull<E: CommandExecutor + ?Sized>(
    executor: &E,
    build_conn: &ServerConn,
    deploy_conn: &ServerConn,
    image_ref: &str,
) -> Result<(), ExecError> {
    // Build server: push the image to the registry.
    executor
        .exec(
            build_conn,
            &push_image_command(image_ref),
            ExecOpts::default(),
        )
        .await?;
    // Switch SSH target to the deploy server: pull the pushed image.
    executor
        .exec(
            deploy_conn,
            &pull_image_command(image_ref),
            ExecOpts::default(),
        )
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_image_ref_defaults_tag_to_latest() {
        assert_eq!(
            registry_image_ref("ghcr.io/acme/app", None).as_deref(),
            Some("ghcr.io/acme/app:latest")
        );
        assert_eq!(
            registry_image_ref("ghcr.io/acme/app", Some("v1.2.3")).as_deref(),
            Some("ghcr.io/acme/app:v1.2.3")
        );
    }

    #[test]
    fn registry_image_ref_requires_a_name() {
        assert_eq!(registry_image_ref("", Some("v1")), None);
        assert_eq!(registry_image_ref("   ", None), None);
    }

    #[test]
    fn push_command_is_exact() {
        assert_eq!(
            push_image_command("ghcr.io/acme/app:latest"),
            "docker push ghcr.io/acme/app:latest"
        );
    }

    #[test]
    fn no_build_server_when_unset() {
        let t = plan_build_targets(1, None, &[2, 3]);
        assert_eq!(
            t,
            BuildTargets {
                build_server_id: 1,
                deploy_server_id: 1,
                use_build_server: false,
            }
        );
    }

    #[test]
    fn falls_back_to_deploy_server_when_no_build_servers_usable() {
        let t = plan_build_targets(1, Some(9), &[]);
        assert!(!t.use_build_server);
        assert_eq!(t.build_server_id, 1);
    }

    #[test]
    fn uses_pinned_build_server_when_available() {
        let t = plan_build_targets(1, Some(3), &[2, 3]);
        assert_eq!(
            t,
            BuildTargets {
                build_server_id: 3,
                deploy_server_id: 1,
                use_build_server: true,
            }
        );
    }

    #[test]
    fn falls_back_to_first_available_when_pinned_not_usable() {
        let t = plan_build_targets(1, Some(9), &[2, 3]);
        assert_eq!(t.build_server_id, 2);
        assert!(t.use_build_server);
    }

    #[test]
    fn pinned_build_server_equal_to_deploy_is_not_a_split() {
        // A build server that is also the deploy server needs no registry hop.
        let t = plan_build_targets(2, Some(2), &[2]);
        assert!(!t.use_build_server);
    }
}
