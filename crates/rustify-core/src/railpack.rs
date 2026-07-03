//! Railpack build-pack constants (parity with Coolify's
//! `config('constants.coolify.railpack_version')` and the pinned
//! `ghcr.io/railwayapp/railpack-frontend` frontend image used by
//! `ApplicationDeploymentJob::railpack_build_command`, :2673).

/// Pinned Railpack version (config/constants.php:8 `railpack_version`).
pub const RAILPACK_VERSION: &str = "0.23.0";

/// BuildKit frontend image passed as `--build-arg BUILDKIT_SYNTAX`
/// (ApplicationDeploymentJob.php:2673, :2678).
pub const RAILPACK_FRONTEND: &str = "ghcr.io/railwayapp/railpack-frontend:v0.23.0";

/// The `docker buildx` builder name Coolify creates/uses for Railpack builds
/// (ApplicationDeploymentJob.php:2675-2676).
pub const RAILPACK_BUILDER: &str = "coolify-railpack";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frontend_embeds_the_pinned_version() {
        // The frontend tag must stay in lock-step with the version constant so
        // the plan frontend and the CLI never drift.
        assert!(RAILPACK_FRONTEND.ends_with(&format!("v{RAILPACK_VERSION}")));
        assert_eq!(RAILPACK_BUILDER, "coolify-railpack");
    }
}
