//! The `docker buildx` invocation for the Railpack build pack.
//!
//! Railpack's BuildKit frontend needs full BuildKit (mergeop), so Coolify runs
//! it through a `docker-container` driver builder rather than the classic
//! `docker build`. This module is the single place that
//! `docker buildx build --builder coolify-railpack …` is rendered, ported from
//! `ApplicationDeploymentJob::railpack_build_command`
//! (app/Jobs/ApplicationDeploymentJob.php:2658-2686), plus the idempotent
//! builder-create and prune helpers (:2675, cleanup parity).

use rustify_core::railpack::{RAILPACK_BUILDER, RAILPACK_FRONTEND};

/// POSIX single-quote a shell value, matching Coolify's `escapeShellValue`
/// (bootstrap/helpers/docker.php:140).
fn sh_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

/// Idempotent builder bootstrap: create the `docker-container` builder if it
/// does not already exist, swallowing the "already exists" error.
/// Port of ApplicationDeploymentJob.php:2675.
pub fn builder_create_command() -> String {
    format!(
        "docker buildx create --name {RAILPACK_BUILDER} --driver docker-container 2>/dev/null || true"
    )
}

/// Post-build cache reclamation for the dedicated builder.
pub fn prune_command() -> String {
    format!("docker buildx prune --builder {RAILPACK_BUILDER} -af")
}

/// A fully described `docker buildx build` for a Railpack plan.
///
/// `env_prefix` renders as a process-level `env K=V …` ahead of the command
/// (Coolify's `railpack_build_environment_prefix`, :2632); `secret_ids` become
/// `--secret id=K,env=K` flags (`railpack_build_secret_flags`, :2645) so the
/// frontend can read each value from `/run/secrets/<K>`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RailpackBuildCommand {
    /// Process env prefix (build-time variable KEY=VALUE pairs).
    pub env_prefix: Vec<(String, String)>,
    /// `--add-host name:ip` entries (server internal hostnames).
    pub add_hosts: Vec<(String, String)>,
    /// Force a `--no-cache` rebuild (mutually exclusive with the cache args).
    pub no_cache: bool,
    /// `--build-arg cache-key='<value>'` when not `no_cache` (the app uuid).
    pub cache_key: Option<String>,
    /// `--build-arg secrets-hash=<value>` cache buster (present iff vars exist).
    pub secrets_hash: Option<String>,
    /// Build-time variable keys that become `--secret id=K,env=K` flags.
    pub secret_ids: Vec<String>,
    /// Path to the generated plan (`/artifacts/railpack-plan.json`).
    pub plan_file: String,
    /// Target image tag.
    pub image: String,
    /// Build context / workdir.
    pub workdir: String,
}

impl RailpackBuildCommand {
    /// Render the full shell command. Deterministic given deterministic input,
    /// so golden files are stable.
    pub fn render(&self) -> String {
        let mut parts: Vec<String> = Vec::new();

        // Process env prefix: `env 'K=V' 'K2=V2'` (never rendered when empty).
        if !self.env_prefix.is_empty() {
            parts.push("env".to_string());
            for (k, v) in &self.env_prefix {
                parts.push(sh_quote(&format!("{k}={v}")));
            }
        }

        parts.push("docker".to_string());
        parts.push("buildx".to_string());
        parts.push("build".to_string());
        parts.push("--builder".to_string());
        parts.push(RAILPACK_BUILDER.to_string());

        for (name, ip) in &self.add_hosts {
            parts.push("--add-host".to_string());
            parts.push(format!("{name}:{ip}"));
        }

        parts.push("--network".to_string());
        parts.push("host".to_string());
        parts.push("--build-arg".to_string());
        parts.push(format!("BUILDKIT_SYNTAX=\"{RAILPACK_FRONTEND}\""));

        // Cache args: --no-cache OR cache-key (+ optional secrets-hash).
        if self.no_cache {
            parts.push("--no-cache".to_string());
        } else if let Some(key) = &self.cache_key {
            parts.push("--build-arg".to_string());
            parts.push(format!("cache-key={}", sh_quote(key)));
        }
        if !self.no_cache
            && let Some(hash) = &self.secrets_hash
        {
            parts.push("--build-arg".to_string());
            parts.push(format!("secrets-hash={hash}"));
        }

        for id in &self.secret_ids {
            parts.push("--secret".to_string());
            parts.push(sh_quote(&format!("id={id},env={id}")));
        }

        parts.push("-f".to_string());
        parts.push(self.plan_file.clone());
        parts.push("--progress".to_string());
        parts.push("plain".to_string());
        parts.push("--load".to_string());
        parts.push("-t".to_string());
        parts.push(self.image.clone());
        parts.push(self.workdir.clone());

        parts.join(" ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn buildx_command_matches_golden() {
        let cmd = RailpackBuildCommand {
            env_prefix: vec![
                ("A".to_string(), "1".to_string()),
                ("B".to_string(), "2".to_string()),
            ],
            add_hosts: vec![("db".to_string(), "10.0.0.5".to_string())],
            no_cache: false,
            cache_key: Some("app-uuid".to_string()),
            secrets_hash: Some("deadbeef".to_string()),
            secret_ids: vec!["A".to_string(), "B".to_string()],
            plan_file: "/artifacts/railpack-plan.json".to_string(),
            image: "app-uuid:commitsha".to_string(),
            workdir: "/artifacts/dep1".to_string(),
        };
        assert_eq!(
            cmd.render().trim(),
            crate::test_support::load_golden("build-railpack.txt").trim()
        );
    }

    #[test]
    fn force_rebuild_uses_no_cache_and_drops_cache_args() {
        let cmd = RailpackBuildCommand {
            env_prefix: vec![],
            add_hosts: vec![],
            no_cache: true,
            cache_key: Some("app-uuid".to_string()),
            secrets_hash: Some("deadbeef".to_string()),
            secret_ids: vec![],
            plan_file: "/artifacts/railpack-plan.json".to_string(),
            image: "app:sha".to_string(),
            workdir: "/artifacts/d".to_string(),
        };
        let out = cmd.render();
        assert!(out.contains("--no-cache"));
        assert!(!out.contains("cache-key="), "no_cache drops cache-key");
        assert!(
            !out.contains("secrets-hash="),
            "no_cache drops secrets-hash"
        );
        // Still uses buildx with the pinned frontend + plan file.
        assert!(out.contains("docker buildx build --builder coolify-railpack"));
        assert!(out.contains(
            "--build-arg BUILDKIT_SYNTAX=\"ghcr.io/railwayapp/railpack-frontend:v0.23.0\""
        ));
        assert!(out.contains("-f /artifacts/railpack-plan.json --progress plain --load"));
    }

    #[test]
    fn no_vars_renders_no_env_prefix_no_secrets_no_hash() {
        let cmd = RailpackBuildCommand {
            env_prefix: vec![],
            add_hosts: vec![],
            no_cache: false,
            cache_key: Some("app-uuid".to_string()),
            secrets_hash: None,
            secret_ids: vec![],
            plan_file: "/artifacts/railpack-plan.json".to_string(),
            image: "app:sha".to_string(),
            workdir: "/artifacts/d".to_string(),
        };
        let out = cmd.render();
        assert!(
            out.starts_with("docker buildx build"),
            "no env prefix: {out}"
        );
        assert!(!out.contains("--secret"));
        assert!(!out.contains("secrets-hash="));
        assert!(out.contains("--build-arg cache-key='app-uuid'"));
    }

    #[test]
    fn builder_create_is_idempotent() {
        assert_eq!(
            builder_create_command(),
            "docker buildx create --name coolify-railpack --driver docker-container 2>/dev/null || true"
        );
    }

    #[test]
    fn prune_targets_the_builder() {
        assert_eq!(
            prune_command(),
            "docker buildx prune --builder coolify-railpack -af"
        );
    }

    #[test]
    fn sh_quote_escapes_embedded_single_quotes() {
        assert_eq!(sh_quote("a'b"), "'a'\\''b'");
    }
}
