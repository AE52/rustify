//! Railpack buildpack: the plan→buildx split.
//!
//! Parity with Coolify's `build_railpack_image` (ApplicationDeploymentJob.php:2839):
//! `railpack prepare` emits `/artifacts/railpack-plan.json`, then
//! `docker buildx build` (via [`rustify_docker::RailpackBuildCommand`]) with the
//! Railpack BuildKit frontend consumes that plan. Because Railpack's schema
//! disallows top-level `variables`, every build-time variable travels the
//! `--env` (prepare) → `--secret id=,env=` (buildx) channel: this module owns
//! that fan-out (`plan_env`) plus the pure command/Dockerfile text.
//!
//! Coolify refs: env fan-out :2540-2656, prepare :2805-2828, apt-package force
//! :2615-2630, install-cmd env :2598-2600, static image :2910-2940,
//! secret-stripped plan logging :2859-2871.

use rustify_docker::railpack::{RailpackBuildCommand, builder_create_command, prune_command};

pub use rustify_core::railpack::{RAILPACK_BUILDER, RAILPACK_FRONTEND, RAILPACK_VERSION};

/// The generated plan path inside the helper (`--plan-out` / `-f`).
pub const PLAN_PATH: &str = "/artifacts/railpack-plan.json";
/// The `RAILPACK_DEPLOY_APT_PACKAGES` variable that always gains `curl`+`wget`.
pub const APT_PACKAGES_KEY: &str = "RAILPACK_DEPLOY_APT_PACKAGES";
/// The install-command variable Railpack reads instead of a CLI flag.
pub const INSTALL_CMD_KEY: &str = "RAILPACK_INSTALL_CMD";
/// Deployment-id label stamped on the generated static image (parity with
/// Coolify's `coolify.deploymentId`, :2916).
pub const DEPLOYMENT_ID_LABEL: &str = "rustify.deploymentId";

/// Re-export the idempotent builder-create + prune helpers so the engine calls
/// them through the buildpack module (single import site).
pub fn builder_create() -> String {
    builder_create_command()
}

/// Reclaim the dedicated builder's cache after the build.
pub fn prune() -> String {
    prune_command()
}

/// POSIX single-quote a shell value (Coolify `escapeShellValue`,
/// bootstrap/helpers/docker.php:140).
fn sh_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

/// The three env outputs Railpack needs from one resolved build-time env map.
///
/// * `prepare_env` — `'K=V'` tokens for `railpack prepare --env`.
/// * `buildx_env_prefix` — `(K, V)` pairs for the `env K=V …` process prefix.
/// * `secret_ids` — keys for `--secret id=K,env=K` flags.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RailpackEnv {
    pub prepare_env: Vec<String>,
    pub buildx_env_prefix: Vec<(String, String)>,
    pub secret_ids: Vec<String>,
}

/// Force `curl` and `wget` into `RAILPACK_DEPLOY_APT_PACKAGES` (deploy image
/// needs them for healthchecks), preserving any user-declared packages and
/// de-duplicating. Port of `merge_railpack_deploy_apt_packages` (:2615).
pub fn merge_deploy_apt_packages(vars: &mut Vec<(String, String)>) {
    let existing = vars
        .iter()
        .find(|(k, _)| k == APT_PACKAGES_KEY)
        .map(|(_, v)| v.clone())
        .unwrap_or_default();
    let mut packages: Vec<String> = existing
        .split_whitespace()
        .map(str::to_string)
        .collect::<Vec<_>>();
    for pkg in ["curl", "wget"] {
        if !packages.iter().any(|p| p == pkg) {
            packages.push(pkg.to_string());
        }
    }
    let merged = packages.join(" ");
    if let Some(slot) = vars.iter_mut().find(|(k, _)| k == APT_PACKAGES_KEY) {
        slot.1 = merged;
    } else {
        vars.push((APT_PACKAGES_KEY.to_string(), merged));
    }
}

/// Inject `install_command` as `RAILPACK_INSTALL_CMD` (Railpack has no
/// `--install-cmd` flag; it reads this env instead — :2598).
pub fn inject_install_command(vars: &mut Vec<(String, String)>, install_command: Option<&str>) {
    if let Some(cmd) = install_command.filter(|c| !c.is_empty()) {
        if let Some(slot) = vars.iter_mut().find(|(k, _)| k == INSTALL_CMD_KEY) {
            slot.1 = cmd.to_string();
        } else {
            vars.push((INSTALL_CMD_KEY.to_string(), cmd.to_string()));
        }
    }
}

/// Fan a resolved (sorted) build-time env map into the three Railpack outputs.
/// Empty/`""`-valued entries are dropped (Coolify skips them, :2591) so they
/// never become empty secrets.
pub fn plan_env(vars: &[(String, String)]) -> RailpackEnv {
    let mut kept: Vec<(String, String)> = vars
        .iter()
        .filter(|(_, v)| !v.is_empty())
        .cloned()
        .collect();
    kept.sort_by(|a, b| a.0.cmp(&b.0));
    RailpackEnv {
        prepare_env: kept
            .iter()
            .map(|(k, v)| sh_quote(&format!("{k}={v}")))
            .collect(),
        buildx_env_prefix: kept.clone(),
        secret_ids: kept.into_iter().map(|(k, _)| k).collect(),
    }
}

/// `railpack prepare [--build-cmd C] [--start-cmd C] {--env 'K=V' …}
/// [--config-file P] --plan-out <plan> <workdir>` (:2805-2828).
pub fn prepare_command(
    workdir: &str,
    build_command: Option<&str>,
    start_command: Option<&str>,
    prepare_env: &[String],
    config_file: Option<&str>,
) -> String {
    let mut cmd = String::from("railpack prepare");
    if let Some(b) = build_command.filter(|c| !c.is_empty()) {
        cmd.push_str(" --build-cmd ");
        cmd.push_str(&sh_quote(b));
    }
    if let Some(s) = start_command.filter(|c| !c.is_empty()) {
        cmd.push_str(" --start-cmd ");
        cmd.push_str(&sh_quote(s));
    }
    for env in prepare_env {
        cmd.push_str(" --env ");
        cmd.push_str(env);
    }
    if let Some(cf) = config_file.filter(|c| !c.is_empty()) {
        cmd.push_str(" --config-file ");
        cmd.push_str(&sh_quote(cf));
    }
    cmd.push_str(&format!(" --plan-out {PLAN_PATH} {workdir}"));
    cmd
}

/// Assemble the buildx build for the given plan (delegates rendering to
/// [`RailpackBuildCommand`]).
#[allow(clippy::too_many_arguments)]
pub fn buildx_command(
    env: &RailpackEnv,
    add_hosts: Vec<(String, String)>,
    no_cache: bool,
    cache_key: Option<String>,
    secrets_hash: Option<String>,
    image: &str,
    workdir: &str,
) -> RailpackBuildCommand {
    RailpackBuildCommand {
        env_prefix: env.buildx_env_prefix.clone(),
        add_hosts,
        no_cache,
        cache_key,
        secrets_hash,
        secret_ids: env.secret_ids.clone(),
        plan_file: PLAN_PATH.to_string(),
        image: image.to_string(),
        workdir: workdir.to_string(),
    }
}

/// Strip the `secrets` array from a plan before it is logged, so build-time
/// variable *names* never leak (Coolify :2864-2868). Returns `None` when the
/// plan is not a JSON object — callers must then log nothing rather than the
/// raw plan (which could contain secret material).
pub fn strip_plan_secrets(plan_json: &str) -> Option<String> {
    let mut value: serde_json::Value = serde_json::from_str(plan_json).ok()?;
    let obj = value.as_object_mut()?;
    obj.remove("secrets");
    serde_json::to_string_pretty(&value).ok()
}

/// nginx production Dockerfile for the static Railpack variant: copy the built
/// assets *out of* the build image (`COPY --from`) into the nginx web root.
/// Port of `build_railpack_static_image` (:2910-2918).
pub fn nginx_dockerfile_from_build(
    static_image: &str,
    build_image: &str,
    publish_directory: &str,
    deployment_uuid: &str,
) -> String {
    let publish = publish_directory.trim_matches('/');
    let publish = if publish.is_empty() {
        String::new()
    } else {
        format!("/{publish}")
    };
    format!(
        "FROM {static_image}\n\
         WORKDIR /usr/share/nginx/html/\n\
         LABEL {DEPLOYMENT_ID_LABEL}={deployment_uuid}\n\
         COPY --from={build_image} /app{publish} .\n\
         COPY ./nginx.conf /etc/nginx/conf.d/default.conf\n",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prepare_command_golden() {
        // Cites ApplicationDeploymentJob.php:2805-2828 (railpack_prepare_command):
        // build-cmd, start-cmd, --env fan-out, then --plan-out <plan> <workdir>.
        let env = plan_env(&[("B".into(), "2".into()), ("A".into(), "1".into())]);
        let cmd = prepare_command(
            "/artifacts/dep1",
            Some("npm run build"),
            Some("node server.js"),
            &env.prepare_env,
            None,
        );
        assert_eq!(
            cmd,
            "railpack prepare --build-cmd 'npm run build' --start-cmd 'node server.js' \
             --env 'A=1' --env 'B=2' --plan-out /artifacts/railpack-plan.json /artifacts/dep1"
        );
    }

    #[test]
    fn prepare_command_minimal_and_with_config_file() {
        let cmd = prepare_command(
            "/w",
            None,
            None,
            &[],
            Some(".coolify/railpack.generated.json"),
        );
        assert_eq!(
            cmd,
            "railpack prepare --config-file '.coolify/railpack.generated.json' \
             --plan-out /artifacts/railpack-plan.json /w"
        );
        let bare = prepare_command("/w", None, None, &[], None);
        assert_eq!(
            bare,
            "railpack prepare --plan-out /artifacts/railpack-plan.json /w"
        );
    }

    #[test]
    fn plan_env_produces_three_sorted_outputs() {
        let env = plan_env(&[
            ("Z".into(), "last".into()),
            ("A".into(), "first".into()),
            ("EMPTY".into(), "".into()),
        ]);
        // Empty-valued vars are dropped from all three outputs.
        assert_eq!(env.prepare_env, vec!["'A=first'", "'Z=last'"]);
        assert_eq!(
            env.buildx_env_prefix,
            vec![
                ("A".to_string(), "first".to_string()),
                ("Z".to_string(), "last".to_string())
            ]
        );
        assert_eq!(env.secret_ids, vec!["A".to_string(), "Z".to_string()]);
    }

    #[test]
    fn apt_packages_force_curl_wget_and_dedup() {
        let mut vars = vec![(APT_PACKAGES_KEY.to_string(), "git curl".to_string())];
        merge_deploy_apt_packages(&mut vars);
        let v = &vars.iter().find(|(k, _)| k == APT_PACKAGES_KEY).unwrap().1;
        assert_eq!(v, "git curl wget", "curl kept once, wget appended");

        let mut empty: Vec<(String, String)> = vec![];
        merge_deploy_apt_packages(&mut empty);
        assert_eq!(
            empty,
            vec![(APT_PACKAGES_KEY.to_string(), "curl wget".to_string())]
        );
    }

    #[test]
    fn install_command_injected_as_env_not_flag() {
        let mut vars = vec![];
        inject_install_command(&mut vars, Some("npm ci"));
        assert_eq!(
            vars,
            vec![(INSTALL_CMD_KEY.to_string(), "npm ci".to_string())]
        );
        // A prepare command built from these vars must NOT carry --install-cmd.
        let env = plan_env(&vars);
        let cmd = prepare_command("/w", None, None, &env.prepare_env, None);
        assert!(
            !cmd.contains("--install-cmd"),
            "install cmd is an env, not a flag"
        );
        assert!(cmd.contains("--env 'RAILPACK_INSTALL_CMD=npm ci'"));

        // None / empty is a no-op.
        let mut none = vec![];
        inject_install_command(&mut none, None);
        inject_install_command(&mut none, Some(""));
        assert!(none.is_empty());
    }

    #[test]
    fn force_rebuild_env_flows_into_buildx_no_cache() {
        let env = plan_env(&[("K".into(), "v".into())]);
        let bc = buildx_command(
            &env,
            vec![],
            true, // force rebuild
            Some("app-uuid".into()),
            Some("hash".into()),
            "app:sha",
            "/w",
        );
        let out = bc.render();
        assert!(out.contains("--no-cache"));
        assert!(!out.contains("cache-key="));
        assert!(!out.contains("secrets-hash="));
        // But the secret channel is still wired (build needs the value).
        assert!(out.contains("--secret 'id=K,env=K'"));
        assert!(out.starts_with("env 'K=v' docker buildx build"));
    }

    #[test]
    fn secrets_hash_only_present_when_vars_and_not_force_rebuild() {
        let env = plan_env(&[("K".into(), "v".into())]);
        let with_hash = buildx_command(
            &env,
            vec![],
            false,
            Some("app-uuid".into()),
            Some("abc123".into()),
            "app:sha",
            "/w",
        )
        .render();
        assert!(with_hash.contains("--build-arg cache-key='app-uuid'"));
        assert!(with_hash.contains("--build-arg secrets-hash=abc123"));
    }

    #[test]
    fn strip_plan_secrets_removes_secrets_key() {
        let plan = r#"{"steps":[{"name":"build"}],"secrets":["TOKEN","API_KEY"]}"#;
        let stripped = strip_plan_secrets(plan).unwrap();
        assert!(!stripped.contains("secrets"));
        assert!(!stripped.contains("TOKEN"));
        assert!(stripped.contains("build"));
    }

    #[test]
    fn strip_plan_secrets_returns_none_for_non_object() {
        assert!(strip_plan_secrets("not json").is_none());
        assert!(strip_plan_secrets("[1,2,3]").is_none());
    }

    #[test]
    fn nginx_static_copies_from_build_image() {
        let df = nginx_dockerfile_from_build("nginx:alpine", "app-uuid:sha-build", "/dist", "dep1");
        assert!(df.contains("FROM nginx:alpine"));
        assert!(df.contains("COPY --from=app-uuid:sha-build /app/dist ."));
        assert!(df.contains("COPY ./nginx.conf /etc/nginx/conf.d/default.conf"));

        let root = nginx_dockerfile_from_build("nginx:alpine", "b", "/", "dep1");
        assert!(
            root.contains("COPY --from=b /app ."),
            "empty publish dir copies /app"
        );
    }
}
