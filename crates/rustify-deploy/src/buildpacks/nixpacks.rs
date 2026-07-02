//! Nixpacks buildpack generators.
//!
//! Coolify's nixpacks flow (ApplicationDeploymentJob.php `generate_nixpacks_confs`
//! → `build_image`) is a plan→build split: `nixpacks plan` yields the config
//! (whose `variables` seed the lowest env-precedence layer), then nixpacks
//! generates a Dockerfile which is built with the sourced build-time env. The
//! brief pins the generate command shape and defers the actual `docker build`
//! to [`rustify_docker::BuildCommand`].

use rustify_docker::BuildCommand;

use super::BuildCtx;

/// `nixpacks plan <workdir>` — emits the plan JSON we parse for build variables.
pub fn plan_command(workdir: &str) -> String {
    format!("nixpacks plan {workdir}")
}

/// `nixpacks build ... -o <workdir>` — generates `.nixpacks/Dockerfile` in the
/// context. `--no-error-without-start` mirrors the brief's pinned invocation.
pub fn generate_command(ctx: &BuildCtx) -> String {
    format!(
        "nixpacks build {workdir} --name {image} --no-error-without-start -o {workdir}",
        workdir = ctx.workdir,
        image = ctx.image
    )
}

/// The `docker build` on the nixpacks-generated Dockerfile, with the build-time
/// env sourced (BuildKit on, matching Coolify).
pub fn docker_build(ctx: &BuildCtx) -> BuildCommand {
    BuildCommand {
        context: ctx.workdir.clone(),
        dockerfile: Some(format!("{}/.nixpacks/Dockerfile", ctx.workdir)),
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

    fn ctx() -> BuildCtx {
        BuildCtx {
            deployment_uuid: "dep1".into(),
            workdir: "/artifacts/dep1".into(),
            image: "app:sha".into(),
            build_env_path: "/artifacts/build-time.env".into(),
            no_cache: false,
        }
    }

    #[test]
    fn generate_command_matches_brief() {
        assert_eq!(
            generate_command(&ctx()),
            "nixpacks build /artifacts/dep1 --name app:sha --no-error-without-start -o /artifacts/dep1"
        );
    }

    #[test]
    fn docker_build_sources_env_and_uses_generated_dockerfile() {
        let rendered = docker_build(&ctx()).render();
        assert!(rendered.contains("source /artifacts/build-time.env"));
        assert!(rendered.contains("-f /artifacts/dep1/.nixpacks/Dockerfile"));
        assert!(rendered.contains("-t app:sha"));
    }
}
