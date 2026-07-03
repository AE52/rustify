//! Buildpack strategies: pure generators for the shell each pack runs inside
//! the helper container. The engine owns execution and the nixpacks
//! plan→env→build ordering; these modules only produce command/Dockerfile
//! text, so every pack shares one tested code path per artifact.
//!
//! Per-pack behaviour follows the brief's table, itself derived from Coolify's
//! `ApplicationDeploymentJob` buildpack methods (nixpacks: :560-600 &
//! `generate_nixpacks_confs`; dockerfile: `deploy_dockerfile_buildpack`;
//! static: `deploy_static_buildpack`; docker image: `deploy_dockerimage_buildpack`).

pub mod compose;
pub mod docker_image;
pub mod dockerfile;
pub mod nixpacks;
pub mod railpack;
pub mod static_site;

/// The supported buildpacks (Contract C2 `BuildPack`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pack {
    Nixpacks,
    Dockerfile,
    Static,
    DockerImage,
    DockerCompose,
    Railpack,
}

impl Pack {
    /// Parse the `applications.build_pack` string tolerantly: case-insensitive,
    /// ignoring `_`/`-`/spaces, so both `docker_image` (Contract C2 serde) and
    /// Coolify's `dockerimage` map to the same variant. Unknown packs default
    /// to nixpacks (Coolify's default).
    pub fn parse(raw: &str) -> Self {
        let norm: String = raw
            .chars()
            .filter(|c| !matches!(c, '_' | '-' | ' '))
            .flat_map(char::to_lowercase)
            .collect();
        match norm.as_str() {
            "dockerfile" => Pack::Dockerfile,
            "static" => Pack::Static,
            "dockerimage" => Pack::DockerImage,
            "dockercompose" => Pack::DockerCompose,
            "railpack" => Pack::Railpack,
            _ => Pack::Nixpacks,
        }
    }
}

/// Paths and flags shared by the build generators.
#[derive(Debug, Clone)]
pub struct BuildCtx {
    /// Helper container name (also the deployment uuid).
    pub deployment_uuid: String,
    /// Build context / working directory inside the helper (`/artifacts/<uuid>`).
    pub workdir: String,
    /// Target image tag (`<app_uuid>:<sha>`).
    pub image: String,
    /// Path to the sourced build-time env file inside the helper.
    pub build_env_path: String,
    /// Force a `--no-cache` rebuild.
    pub no_cache: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_is_tolerant() {
        assert_eq!(Pack::parse("nixpacks"), Pack::Nixpacks);
        assert_eq!(Pack::parse("dockerfile"), Pack::Dockerfile);
        assert_eq!(Pack::parse("static"), Pack::Static);
        assert_eq!(Pack::parse("docker_image"), Pack::DockerImage);
        assert_eq!(Pack::parse("dockerimage"), Pack::DockerImage);
        assert_eq!(Pack::parse("docker_compose"), Pack::DockerCompose);
        assert_eq!(Pack::parse("dockercompose"), Pack::DockerCompose);
        assert_eq!(Pack::parse("DockerCompose"), Pack::DockerCompose);
        assert_eq!(Pack::parse("railpack"), Pack::Railpack);
        assert_eq!(Pack::parse("Railpack"), Pack::Railpack);
        assert_eq!(Pack::parse("weird"), Pack::Nixpacks);
    }
}
