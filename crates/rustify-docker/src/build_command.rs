//! The single place every `docker build` invocation is born.
//!
//! `BuildCommand` is the pinned struct from the track brief; `render()` is the
//! only function that turns it into a shell string, so all build-pack variants
//! (nixpacks, dockerfile, static, ...) share one code path.

/// A fully described `docker build` invocation.
///
/// Field semantics mirror the `docker build` CLI. `env_file` wraps the rendered
/// command in a sourced-env prelude (parity with Coolify's
/// `wrap_build_command_with_env_export`, ApplicationDeploymentJob.php:3512) so
/// build args can shell-interpolate build-time variables.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildCommand {
    pub context: String,
    pub dockerfile: Option<String>,
    pub image: String,
    pub build_args: Vec<(String, String)>,
    pub no_cache: bool,
    pub pull: bool,
    pub target: Option<String>,
    pub buildkit: bool,
    /// Path to a build-time `.env` file that is sourced before the build runs.
    pub env_file: Option<String>,
}

impl BuildCommand {
    /// Render the full shell command. Deterministic: flags always appear in a
    /// fixed order so golden files are stable.
    pub fn render(&self) -> String {
        let mut parts: Vec<String> = Vec::new();
        parts.push("docker".to_string());
        parts.push("build".to_string());
        if self.no_cache {
            parts.push("--no-cache".to_string());
        }
        if self.pull {
            parts.push("--pull".to_string());
        }
        if let Some(dockerfile) = &self.dockerfile {
            parts.push("-f".to_string());
            parts.push(dockerfile.clone());
        }
        if let Some(target) = &self.target {
            parts.push("--target".to_string());
            parts.push(target.clone());
        }
        for (key, value) in &self.build_args {
            parts.push("--build-arg".to_string());
            parts.push(format!("{key}={value}"));
        }
        parts.push("-t".to_string());
        parts.push(self.image.clone());
        parts.push(self.context.clone());

        let mut command = parts.join(" ");
        if self.buildkit {
            command = format!("DOCKER_BUILDKIT=1 {command}");
        }
        if let Some(env_file) = &self.env_file {
            command = format!("set -a && source {env_file} && set +a && {command}");
        }
        command
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nixpacks_image_matches_golden() {
        // Plain image build (nixpacks generates the Dockerfile in the context).
        let cmd = BuildCommand {
            context: "/artifacts/dep1".to_string(),
            dockerfile: Some("/artifacts/dep1/.nixpacks/Dockerfile".to_string()),
            image: "app-uuid:commitsha".to_string(),
            build_args: vec![],
            no_cache: false,
            pull: false,
            target: None,
            buildkit: false,
            env_file: None,
        };
        assert_eq!(
            cmd.render().trim(),
            crate::test_support::load_golden("build-nixpacks.txt").trim()
        );
    }

    #[test]
    fn dockerfile_with_build_args_matches_golden() {
        let cmd = BuildCommand {
            context: "/artifacts/dep1".to_string(),
            dockerfile: Some("/artifacts/dep1/Dockerfile".to_string()),
            image: "app-uuid:commitsha".to_string(),
            build_args: vec![
                ("NODE_ENV".to_string(), "production".to_string()),
                ("VERSION".to_string(), "1.2.3".to_string()),
            ],
            no_cache: false,
            pull: false,
            target: None,
            buildkit: false,
            env_file: None,
        };
        assert_eq!(
            cmd.render().trim(),
            crate::test_support::load_golden("build-dockerfile-args.txt").trim()
        );
    }

    #[test]
    fn no_cache_and_pull_matches_golden() {
        let cmd = BuildCommand {
            context: ".".to_string(),
            dockerfile: None,
            image: "app-uuid:commitsha".to_string(),
            build_args: vec![],
            no_cache: true,
            pull: true,
            target: None,
            buildkit: false,
            env_file: None,
        };
        assert_eq!(
            cmd.render().trim(),
            crate::test_support::load_golden("build-no-cache.txt").trim()
        );
    }

    #[test]
    fn buildkit_matches_golden() {
        let cmd = BuildCommand {
            context: "/artifacts/dep1".to_string(),
            dockerfile: Some("/artifacts/dep1/.nixpacks/Dockerfile".to_string()),
            image: "app-uuid:commitsha-build".to_string(),
            build_args: vec![],
            no_cache: false,
            pull: false,
            target: Some("runtime".to_string()),
            buildkit: true,
            env_file: None,
        };
        assert_eq!(
            cmd.render().trim(),
            crate::test_support::load_golden("build-buildkit.txt").trim()
        );
    }

    #[test]
    fn env_file_wraps_with_sourced_prelude() {
        let cmd = BuildCommand {
            context: "/artifacts/dep1".to_string(),
            dockerfile: None,
            image: "app-uuid:abc123".to_string(),
            build_args: vec![],
            no_cache: false,
            pull: false,
            target: None,
            buildkit: true,
            env_file: Some("/artifacts/dep1/.env-buildtime".to_string()),
        };
        assert_eq!(
            cmd.render(),
            "set -a && source /artifacts/dep1/.env-buildtime && set +a && DOCKER_BUILDKIT=1 docker build -t app-uuid:abc123 /artifacts/dep1"
        );
    }
}
