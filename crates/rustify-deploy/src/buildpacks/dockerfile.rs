//! Dockerfile buildpack: build the user's Dockerfile directly.
//!
//! Parity with Coolify's `deploy_dockerfile_buildpack`: the build context is the
//! cloned workdir and `-f` points at the app's `dockerfile_location`, with the
//! build-time env sourced.

use rustify_docker::BuildCommand;

use super::BuildCtx;

/// `docker build` for the user Dockerfile at `dockerfile_location` (a path
/// relative to the repo root, e.g. `/Dockerfile`).
pub fn docker_build(ctx: &BuildCtx, dockerfile_location: &str) -> BuildCommand {
    let rel = dockerfile_location.trim_start_matches('/');
    BuildCommand {
        context: ctx.workdir.clone(),
        dockerfile: Some(format!("{}/{}", ctx.workdir, rel)),
        image: ctx.image.clone(),
        build_args: vec![],
        no_cache: ctx.no_cache,
        pull: false,
        target: None,
        buildkit: true,
        env_file: Some(ctx.build_env_path.clone()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dockerfile_path_is_joined_without_double_slash() {
        let ctx = BuildCtx {
            deployment_uuid: "d".into(),
            workdir: "/artifacts/d".into(),
            image: "app:sha".into(),
            build_env_path: "/artifacts/build-time.env".into(),
            no_cache: false,
        };
        let rendered = docker_build(&ctx, "/Dockerfile").render();
        assert!(rendered.contains("-f /artifacts/d/Dockerfile"));
        assert!(!rendered.contains("//Dockerfile"));
        assert!(rendered.contains("source /artifacts/build-time.env"));
    }

    #[test]
    fn nested_dockerfile_location() {
        let ctx = BuildCtx {
            deployment_uuid: "d".into(),
            workdir: "/artifacts/d".into(),
            image: "app:sha".into(),
            build_env_path: "/artifacts/build-time.env".into(),
            no_cache: true,
        };
        let rendered = docker_build(&ctx, "docker/prod.Dockerfile").render();
        assert!(rendered.contains("-f /artifacts/d/docker/prod.Dockerfile"));
        assert!(rendered.contains("--no-cache"));
    }
}
