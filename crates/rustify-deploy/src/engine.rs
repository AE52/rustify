//! The deployment engine: a behavioural port of Coolify's
//! `ApplicationDeploymentJob::handle` (app/Jobs/ApplicationDeploymentJob.php).
//!
//! [`run_deployment`] drives the ten-step flow from the brief. Every remote
//! command goes through [`Engine::exec_step`], which checks for cancellation
//! *before* running and streams each output line to the DB + event bus. The
//! build helper container is torn down on every exit path (success, failure,
//! cancel) by the explicit `cleanup` in [`run_deployment`].

use async_trait::async_trait;
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use chrono::Utc;
use serde_json::Value;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use rustify_core::events::WsEvent;
use rustify_core::{
    DeploymentStatus, ExecEvent, ExecOpts, ExecOutput, LogLine, ServerConn, ids, redact,
};
use rustify_db::repos::{
    Application, ApplicationRepo, Deployment, DeploymentRepo, EnvVar, EnvVarRepo, GithubAppRepo,
    KeyRepo, Server, ServerRepo,
};
use rustify_docker::{AppComposeInput, HealthCheck, generate_compose, parse_containers};
use rustify_jobs::JobHandler;

use crate::buildpacks::{self, BuildCtx, Pack};
use crate::github::{self, GithubAppRow};
use crate::{DeployEngineDeps, DeployError, admission, envfile, git, rolling};

/// Coolify build-helper image (Contract C7 / brief step 2).
const HELPER_IMAGE: &str = "ghcr.io/coollabsio/coolify-helper:latest";
/// Build-time env path inside the helper (ApplicationDeploymentJob.php:46).
const BUILD_TIME_ENV: &str = "/artifacts/build-time.env";
/// Runtime env path inside the helper (used by the compose buildpack).
const RUNTIME_ENV_HELPER: &str = "/artifacts/runtime.env";
/// The env-var `resource_kind` discriminator for applications.
const APP_RESOURCE_KIND: &str = "application";

/// The resolved git source for a deployment (parity with Coolify's
/// `deploymentType()` + `customRepository()`), computed once in [`Engine::prepare`].
///
/// - [`GitSource::Public`] — a public HTTPS / `git@` / `file://` clone (Phase 1).
/// - [`GitSource::GithubApp`] — a private repo behind a GitHub App installation
///   token (minted lazily at clone time).
/// - [`GitSource::DeployKey`] — a private repo reached over SSH with a raw
///   deploy key materialised into the build helper.
enum GitSource {
    Public {
        url: String,
    },
    GithubApp {
        scheme: String,
        host: String,
        /// `owner/repo` (Coolify `customRepository`).
        repo: String,
        app: GithubAppRow,
    },
    DeployKey {
        repository: String,
        port: i32,
        /// The base64-encoded PEM materialised into the helper (a secret).
        b64_key: String,
    },
}

/// [`rustify_jobs::JobHandler`] for kind `"deploy"`, payload `{"deployment_uuid": ".."}`.
pub struct DeployJobHandler {
    deps: DeployEngineDeps,
    shutdown: CancellationToken,
}

impl DeployJobHandler {
    pub fn new(deps: DeployEngineDeps, shutdown: CancellationToken) -> Self {
        Self { deps, shutdown }
    }
}

#[async_trait]
impl JobHandler for DeployJobHandler {
    async fn run(&self, payload: Value) -> anyhow::Result<()> {
        let uuid = payload
            .get("deployment_uuid")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("deploy job payload missing deployment_uuid"))?;
        run_deployment(&self.deps, &self.shutdown, uuid)
            .await
            .map_err(|e| anyhow::anyhow!(e.to_string()))?;
        Ok(())
    }
}

/// Run one deployment to a terminal state. Deployment-level failures
/// (build/unhealthy/cancel) are recorded in the DB and return `Ok(())`; only
/// infrastructure errors (missing rows, DB failure) return `Err` so the job
/// queue may retry.
pub async fn run_deployment(
    deps: &DeployEngineDeps,
    shutdown: &CancellationToken,
    uuid: &str,
) -> Result<(), DeployError> {
    let mut engine = Engine::prepare(deps.clone(), shutdown.clone(), uuid).await?;

    // Step 1: claim the deployment.
    if !engine.claim().await? {
        tracing::info!(
            deployment = uuid,
            "claim lost or already terminal; skipping"
        );
        return Ok(());
    }

    // Steps 2-8 (streamed, cancellable). The helper is torn down afterwards no
    // matter how these exit.
    let result = engine.run_steps().await;

    // Step 9: helper cleanup, always.
    engine.cleanup().await;

    // Step 10: terminal transition + queue drain.
    match result {
        Ok(()) => {
            engine.set_status(DeploymentStatus::Finished).await?;
            engine.mark_application_status("running").await;
            engine.info("Deployment finished successfully.").await;
        }
        Err(DeployError::Cancelled) => {
            engine.set_status(DeploymentStatus::Cancelled).await?;
            engine.info("Deployment cancelled.").await;
        }
        Err(e) => {
            engine.error(&format!("Deployment failed: {e}")).await;
            engine.set_status(DeploymentStatus::Failed).await?;
        }
    }
    // Whatever the outcome, a slot may have freed up on this server.
    let _ = admission::queue_next(&engine.deps, engine.server.id).await;
    Ok(())
}

/// All resolved context for a single deployment plus the streaming log cursor.
pub struct Engine {
    deps: DeployEngineDeps,
    shutdown: CancellationToken,
    repo: DeploymentRepo,
    app_repo: ApplicationRepo,
    deployment: Deployment,
    application: Application,
    server: Server,
    conn: ServerConn,
    network: String,
    /// Unique per-deploy container name `<app_uuid>-<6 char>` (Contract C7).
    container_name: String,
    /// Build working dir inside the helper: `/artifacts/<deployment_uuid>`.
    workdir: String,
    /// App config dir on the server: `/data/rustify/applications/<app_uuid>`.
    app_config_dir: String,
    /// All env vars for the application (decrypted).
    env_vars: Vec<EnvVar>,
    /// Non-empty env values to redact from every log line.
    secrets: Vec<String>,
    /// Monotonic log-line order cursor.
    order: i64,
    /// Command batch counter.
    batch: i32,
    /// Per-command timeout (server `dynamic_timeout`).
    timeout_secs: u32,
    /// Resolved git source (public / GitHub App / deploy key).
    git_source: GitSource,
    /// HTTP client for GitHub installation-token exchange.
    http: reqwest::Client,
    /// Memoised url-encoded GitHub installation token for this deploy, so the
    /// ls-remote and clone steps reuse one mint. `None` until first needed.
    gh_enc_token: Option<String>,
    /// Whether the deploy key has already been materialised into the helper.
    deploy_key_ready: bool,
}

impl Engine {
    /// Load and resolve all context for `uuid` without mutating deployment state.
    pub async fn prepare(
        deps: DeployEngineDeps,
        shutdown: CancellationToken,
        uuid: &str,
    ) -> Result<Self, DeployError> {
        let repo = DeploymentRepo::new(deps.pool.clone());
        let app_repo = ApplicationRepo::new(deps.pool.clone());
        let server_repo = ServerRepo::new(deps.pool.clone());
        let env_repo = EnvVarRepo::new(deps.pool.clone());

        let deployment = repo
            .get_by_uuid(uuid)
            .await?
            .ok_or_else(|| DeployError::NotFound(uuid.to_string()))?;

        let application =
            load_application(&app_repo, &deps.pool, deployment.application_id).await?;
        let server = load_server(&server_repo, &deps.pool, deployment.server_id).await?;

        let network = destination_network(&deps.pool, application.destination_id).await?;
        let settings = server_repo.settings(server.id).await?;
        let timeout_secs = settings
            .as_ref()
            .map(|s| s.dynamic_timeout.max(1) as u32)
            .unwrap_or(3600);
        let connection_timeout = settings
            .as_ref()
            .map(|s| s.connection_timeout.max(1) as u32)
            .unwrap_or(10);

        let env_vars = env_repo.list(APP_RESOURCE_KIND, application.id).await?;
        let secrets: Vec<String> = env_vars
            .iter()
            .map(|e| e.value.clone())
            .filter(|v| !v.is_empty())
            .collect();

        let conn = build_conn(&deps.pool, &server, connection_timeout).await;

        let git_source = resolve_git_source(&deps.pool, &application).await?;

        let container_name = format!("{}-{}", application.uuid, short_id());
        let workdir = format!("/artifacts/{}", deployment.uuid);
        let app_config_dir = format!("/data/rustify/applications/{}", application.uuid);

        Ok(Self {
            deps,
            shutdown,
            repo,
            app_repo,
            deployment,
            application,
            server,
            conn,
            network,
            container_name,
            workdir,
            app_config_dir,
            env_vars,
            secrets,
            order: 0,
            batch: 1,
            timeout_secs,
            git_source,
            http: reqwest::Client::new(),
            gh_enc_token: None,
            deploy_key_ready: false,
        })
    }

    // ---- accessors used by sibling modules (rolling) ------------------------

    pub(crate) fn container_name(&self) -> &str {
        &self.container_name
    }

    pub(crate) fn application(&self) -> &Application {
        &self.application
    }

    // ---- step 1: claim ------------------------------------------------------

    async fn claim(&mut self) -> Result<bool, DeployError> {
        if self.set_status(DeploymentStatus::InProgress).await? {
            self.info("Deployment started.").await;
            return Ok(true);
        }
        // May have been pre-claimed by admission control (`next_queuable`),
        // which enqueued this very job after moving it to in_progress.
        if let Some(current) = self.repo.get_by_uuid(&self.deployment.uuid).await?
            && current.status == DeploymentStatus::InProgress
        {
            self.info("Deployment started.").await;
            return Ok(true);
        }
        Ok(false)
    }

    // ---- steps 2-8 ----------------------------------------------------------

    async fn run_steps(&mut self) -> Result<(), DeployError> {
        let pack = Pack::parse(&self.application.build_pack);

        // Step 2: bring up the build helper container.
        self.helper_up().await?;

        match pack {
            Pack::DockerImage => {
                // No git, no build — pull and run a registry image.
                let image = self.registry_image()?;
                self.info(&format!("Pulling image {image}")).await;
                self.exec_step(
                    &buildpacks::docker_image::pull_command(&image),
                    false,
                    false,
                )
                .await?;
                self.write_config(&image).await?; // step 7
                rolling::rolling_update(self).await?; // step 8
            }
            Pack::DockerCompose => {
                let sha = self.resolve_commit().await?; // step 3
                self.persist_commit(&sha).await?;
                self.clone_repo(&sha).await?; // step 5
                self.deploy_compose(&sha).await?; // steps 6-8 (compose owns its own run)
            }
            _ => {
                let sha = self.resolve_commit().await?; // step 3
                self.persist_commit(&sha).await?;
                let image = format!("{}:{}", self.application.uuid, sha);
                // Step 4: skip build if the image already exists.
                if self.skip_build(&image).await? {
                    self.info("Image already exists for this commit; skipping build.")
                        .await;
                } else {
                    self.clone_repo(&sha).await?; // step 5
                    self.build(pack, &image, &sha).await?; // step 6
                }
                self.write_config(&image).await?; // step 7
                rolling::rolling_update(self).await?; // step 8
            }
        }
        Ok(())
    }

    // ---- step 2 -------------------------------------------------------------

    async fn helper_up(&mut self) -> Result<(), DeployError> {
        self.next_batch();
        self.info(&format!("Preparing build helper ({HELPER_IMAGE})."))
            .await;
        // A `file://` repository lives on the server's filesystem, not inside
        // the helper. Bind-mount its path read-only at the same location so the
        // helper's `git ls-remote`/`git clone` can reach it.
        let repo_mount = file_repo_mount(&self.application.git_repository)
            .map(|m| format!(" {m}"))
            .unwrap_or_default();
        let run = format!(
            "docker run -d --rm --name {dep} --network {net} \
             -v /var/run/docker.sock:/var/run/docker.sock{repo_mount} {image}",
            dep = self.deployment.uuid,
            net = self.network,
            image = HELPER_IMAGE
        );
        self.exec_step(&run, true, false).await?;
        // Ensure the artifacts workdir exists inside the helper.
        let mkdir = self.in_helper(&format!("mkdir -p {}", self.workdir));
        self.exec_step(&mkdir, true, true).await?;
        Ok(())
    }

    // ---- step 3 -------------------------------------------------------------

    async fn resolve_commit(&mut self) -> Result<String, DeployError> {
        self.next_batch();
        self.info("Resolving commit from the git remote.").await;
        self.prepare_git_auth().await?;
        let cmd = self.ls_remote_cmd().await?;
        let out = self.exec_step(&cmd, true, true).await?;
        let sha = git::parse_commit_sha(&out.stdout)
            .or_else(|| {
                let pinned = &self.application.git_commit_sha;
                (pinned != "HEAD" && !pinned.is_empty()).then(|| pinned.clone())
            })
            .ok_or_else(|| DeployError::Build("could not resolve a commit sha".into()))?;
        self.info(&format!("Resolved commit {sha}.")).await;
        Ok(sha)
    }

    async fn persist_commit(&mut self, sha: &str) -> Result<(), DeployError> {
        sqlx::query("UPDATE deployments SET commit_sha = $2 WHERE id = $1")
            .bind(self.deployment.id)
            .bind(sha)
            .execute(&self.deps.pool)
            .await?;
        self.app_repo
            .set_commit_sha(self.application.id, sha)
            .await?;
        self.deployment.commit_sha = Some(sha.to_string());
        Ok(())
    }

    // ---- step 4 -------------------------------------------------------------

    async fn skip_build(&mut self, image: &str) -> Result<bool, DeployError> {
        if self.deployment.force_rebuild {
            return Ok(false);
        }
        let out = self
            .exec_step(&format!("docker images -q {image}"), true, true)
            .await?;
        Ok(!out.stdout.trim().is_empty())
    }

    // ---- step 5 -------------------------------------------------------------

    async fn clone_repo(&mut self, sha: &str) -> Result<(), DeployError> {
        self.next_batch();
        self.info(&format!(
            "Cloning {}:{} into the helper.",
            self.application.git_repository, self.application.git_branch
        ))
        .await;
        self.prepare_git_auth().await?;
        let clone = self.clone_cmd().await?;
        self.exec_step(&clone, true, false).await?;

        let msg_cmd = self.in_helper(&git::commit_message_command(&self.workdir));
        let out = self.exec_step(&msg_cmd, true, true).await?;
        let message = out.stdout.lines().next().unwrap_or("").trim().to_string();
        if !message.is_empty() {
            sqlx::query("UPDATE deployments SET commit_message = $2 WHERE id = $1")
                .bind(self.deployment.id)
                .bind(&message)
                .execute(&self.deps.pool)
                .await?;
        }
        let _ = sha;
        Ok(())
    }

    // ---- private-repo git auth (GitHub App token / deploy key) --------------

    /// Materialise any credentials the resolved [`GitSource`] needs before a git
    /// command runs, and register the secret material for log redaction. For a
    /// deploy key this writes the 0600 key into the helper (once); for a GitHub
    /// App it mints (and caches) the installation token. Never logs the secret.
    async fn prepare_git_auth(&mut self) -> Result<(), DeployError> {
        enum Action {
            None,
            DeployKey(String),
            GithubApp(GithubAppRow),
        }
        let action = match &self.git_source {
            GitSource::Public { .. } => Action::None,
            GitSource::DeployKey { b64_key, .. } if !self.deploy_key_ready => {
                Action::DeployKey(b64_key.clone())
            }
            GitSource::DeployKey { .. } => Action::None,
            GitSource::GithubApp { app, .. } if self.gh_enc_token.is_none() => {
                Action::GithubApp(app.clone())
            }
            GitSource::GithubApp { .. } => Action::None,
        };

        match action {
            Action::None => {}
            Action::DeployKey(b64) => {
                // Redact the base64 key from every subsequent log line.
                self.secrets.push(b64.clone());
                let key_path = git::deploy_key_path(&self.deployment.uuid);
                for cmd in git::deploy_key_materialise_commands(&key_path, &b64) {
                    let wrapped = self.in_helper_bash(&cmd);
                    self.exec_step(&wrapped, true, false).await?;
                }
                self.deploy_key_ready = true;
            }
            Action::GithubApp(app) => {
                let now = Utc::now();
                // Optional clock-skew guard (github.php:17-29); best-effort.
                let _ = github::check_clock_skew(&self.http, &app.api_url, now).await;
                let token = github::installation_token(&self.http, &app, now)
                    .await
                    .map_err(|e| DeployError::Build(format!("github app token: {e}")))?;
                let enc = git::urlencode(&token);
                // Redact both the raw token and its url-encoded form.
                self.secrets.push(token);
                self.secrets.push(enc.clone());
                self.gh_enc_token = Some(enc);
            }
        }
        Ok(())
    }

    /// Build the (helper-wrapped) `git ls-remote` command for the resolved source.
    async fn ls_remote_cmd(&mut self) -> Result<String, DeployError> {
        let branch = self.application.git_branch.clone();
        Ok(match &self.git_source {
            GitSource::Public { url } => self.in_helper(&git::ls_remote_command(url, &branch)),
            GitSource::DeployKey {
                repository, port, ..
            } => {
                let key_path = git::deploy_key_path(&self.deployment.uuid);
                let cmd = git::deploy_key_ls_remote_command(repository, *port, &key_path, &branch);
                self.in_helper_bash(&cmd)
            }
            GitSource::GithubApp {
                scheme, host, repo, ..
            } => {
                let enc = self
                    .gh_enc_token
                    .clone()
                    .ok_or_else(|| DeployError::Build("github token not prepared".into()))?;
                let cmd = git::github_app_ls_remote_command(scheme, host, repo, &branch, &enc);
                self.in_helper_bash(&cmd)
            }
        })
    }

    /// Build the (helper-wrapped) `git clone` command for the resolved source.
    /// Shallow single-branch, into the deployment workdir.
    async fn clone_cmd(&mut self) -> Result<String, DeployError> {
        let branch = self.application.git_branch.clone();
        let base = self.workdir.clone();
        Ok(match &self.git_source {
            GitSource::Public { url } => self.in_helper(&git::clone_command(url, &branch, &base)),
            GitSource::DeployKey {
                repository, port, ..
            } => {
                let key_path = git::deploy_key_path(&self.deployment.uuid);
                let cmd = git::deploy_key_clone_command(
                    repository, *port, &key_path, &branch, true, &base,
                );
                self.in_helper_bash(&cmd)
            }
            GitSource::GithubApp {
                scheme, host, repo, ..
            } => {
                let enc = self
                    .gh_enc_token
                    .clone()
                    .ok_or_else(|| DeployError::Build("github token not prepared".into()))?;
                let cmd =
                    git::github_app_clone_command(scheme, host, repo, &branch, &enc, true, &base);
                self.in_helper_bash(&cmd)
            }
        })
    }

    /// `docker exec <helper> bash -c '<cmd>'` (single-quote escaped, parity with
    /// Coolify's `executeInDocker`). Used for git commands that embed their own
    /// single/double quotes (`-c 'url...'`, `GIT_SSH_COMMAND="ssh ..."`).
    fn in_helper_bash(&self, cmd: &str) -> String {
        let escaped = cmd.replace('\'', "'\\''");
        format!("docker exec {} bash -c '{}'", self.deployment.uuid, escaped)
    }

    // ---- step 6 -------------------------------------------------------------

    async fn build(&mut self, pack: Pack, image: &str, sha: &str) -> Result<(), DeployError> {
        self.next_batch();
        let ctx = BuildCtx {
            deployment_uuid: self.deployment.uuid.clone(),
            workdir: self.workdir.clone(),
            image: image.to_string(),
            build_env_path: BUILD_TIME_ENV.to_string(),
            no_cache: self.deployment.force_rebuild,
        };

        match pack {
            Pack::Nixpacks => {
                self.info("Generating nixpacks configuration.").await;
                let plan = self
                    .exec_step(
                        &self.in_helper(&buildpacks::nixpacks::plan_command(&self.workdir)),
                        true,
                        true,
                    )
                    .await?;
                let nix_vars = envfile::parse_nixpacks_variables(&plan.stdout);
                self.write_build_env(sha, nix_vars, vec![]).await?;
                self.info("Building image with nixpacks.").await;
                self.exec_step(
                    &self.in_helper(&buildpacks::nixpacks::generate_command(&ctx)),
                    false,
                    false,
                )
                .await?;
                self.exec_step(
                    &self.in_helper(&buildpacks::nixpacks::docker_build(&ctx).render()),
                    false,
                    false,
                )
                .await?;
            }
            Pack::Dockerfile => {
                self.write_build_env(sha, vec![], vec![]).await?;
                self.info("Building image from the application Dockerfile.")
                    .await;
                let bc = buildpacks::dockerfile::docker_build(
                    &ctx,
                    &self.application.dockerfile_location,
                );
                self.exec_step(&self.in_helper(&bc.render()), false, false)
                    .await?;
            }
            Pack::Static => {
                self.write_build_env(sha, vec![], vec![]).await?;
                let publish = self.application.publish_directory.as_deref().unwrap_or("/");
                let df = buildpacks::static_site::nginx_dockerfile(
                    &self.application.static_image,
                    publish,
                );
                let df_path = format!(
                    "{}/{}",
                    self.workdir,
                    buildpacks::static_site::NGINX_DOCKERFILE
                );
                self.info("Generating nginx Dockerfile for static site.")
                    .await;
                self.exec_step(
                    &write_text_in_helper(&self.deployment.uuid, &df_path, &df),
                    true,
                    false,
                )
                .await?;
                let bc = buildpacks::static_site::docker_build(&ctx);
                self.exec_step(&self.in_helper(&bc.render()), false, false)
                    .await?;
            }
            // DockerImage / DockerCompose are handled outside build().
            Pack::DockerImage | Pack::DockerCompose => {}
        }
        Ok(())
    }

    // ---- step 7 -------------------------------------------------------------

    /// Write the runtime `.env` and generated compose file into the app config
    /// dir on the server (via the executor's `upload`).
    async fn write_config(&mut self, image: &str) -> Result<(), DeployError> {
        self.next_batch();
        self.info("Writing runtime configuration.").await;
        // Ensure the destination directory exists.
        self.exec_step(&format!("mkdir -p {}", self.app_config_dir), true, true)
            .await?;

        let runtime_env = self.runtime_env_vars();
        let env_body = envfile::render_runtime_env(&runtime_env);
        let compose = generate_compose(&self.compose_input(image));

        self.upload_text(".env", &env_body).await?;
        self.upload_text("docker-compose.yml", &compose).await?;
        Ok(())
    }

    // ---- compose buildpack (steps 6-8) --------------------------------------

    async fn deploy_compose(&mut self, sha: &str) -> Result<(), DeployError> {
        self.next_batch();
        // Build-time env (with SERVICE_ layer) + runtime env inside the helper.
        self.write_build_env(sha, vec![], self.service_vars())
            .await?;
        let runtime = envfile::render_runtime_env(&self.runtime_env_vars());
        self.exec_step(
            &envfile::write_file_in_helper(&self.deployment.uuid, RUNTIME_ENV_HELPER, &runtime),
            true,
            false,
        )
        .await?;
        self.info("Bringing up docker compose stack.").await;
        let cmd = buildpacks::compose::up_command(
            &self.workdir,
            &self.application.docker_compose_location,
            RUNTIME_ENV_HELPER,
        );
        self.exec_step(&self.in_helper(&cmd), false, false).await?;
        Ok(())
    }

    // ---- step 8 helpers (called by rolling) ---------------------------------

    pub(crate) async fn compose_up(&mut self) -> Result<(), DeployError> {
        let script = format!(
            "cd {dir} && docker compose -f docker-compose.yml up -d --remove-orphans",
            dir = self.app_config_dir
        );
        self.exec_step(&script, false, false).await?;
        Ok(())
    }

    /// Stop+remove every managed container for this application, optionally
    /// excluding `except` (the freshly started container in a rolling update).
    pub(crate) async fn stop_other_containers(
        &mut self,
        except: Option<&str>,
    ) -> Result<(), DeployError> {
        let ps = format!(
            "docker ps -a --filter label=rustify.applicationUuid={} --format '{{{{json .}}}}'",
            self.application.uuid
        );
        let out = self.exec_step(&ps, true, true).await?;
        let names: Vec<String> = parse_containers(&out.stdout)
            .into_iter()
            .map(|c| c.name)
            .filter(|n| Some(n.as_str()) != except && !n.is_empty())
            .collect();
        if names.is_empty() {
            return Ok(());
        }
        self.info(&format!("Stopping {} old container(s).", names.len()))
            .await;
        let script = names
            .iter()
            .map(|n| {
                format!("docker stop {n} >/dev/null 2>&1 || true; docker rm -f {n} >/dev/null 2>&1 || true")
            })
            .collect::<Vec<_>>()
            .join("\n");
        self.exec_step(&script, true, true).await?;
        Ok(())
    }

    pub(crate) async fn remove_container(&mut self, name: &str) {
        let _ = self
            .exec_step(&format!("docker rm -f {name}"), true, true)
            .await;
    }

    pub(crate) async fn query_container_logs(&mut self, name: &str) {
        self.info("Fetching container logs for the failed container.")
            .await;
        let _ = self
            .exec_step(&format!("docker logs -n 100 {name}"), false, true)
            .await;
    }

    // ---- step 9 -------------------------------------------------------------

    /// Tear down the build helper container. Always runs (success/fail/cancel);
    /// never checks cancellation and ignores errors.
    async fn cleanup(&mut self) {
        let script = format!(
            "docker rm -f {} >/dev/null 2>&1 || true",
            self.deployment.uuid
        );
        let _ = self
            .deps
            .executor
            .exec(&self.conn, &script, self.exec_opts())
            .await;
        self.info_hidden("Removed build helper container.").await;
    }

    // ---- step 10 helpers ----------------------------------------------------

    async fn mark_application_status(&mut self, status: &str) {
        if let Err(e) = self.app_repo.set_status(self.application.id, status).await {
            tracing::warn!(error = %e, "failed to update application status");
        }
        let _ = self.deps.events.send(WsEvent::application_status_changed(
            &self.application.uuid,
            status,
        ));
    }

    // ---- env-file helpers ---------------------------------------------------

    /// Write `/artifacts/build-time.env` inside the helper, applying the
    /// precedence layers (nixpacks < RUSTIFY_* < SERVICE_* < user build-time).
    async fn write_build_env(
        &mut self,
        sha: &str,
        nixpacks: Vec<(String, String)>,
        service: Vec<(String, String)>,
    ) -> Result<(), DeployError> {
        let layers = envfile::BuildEnvLayers {
            nixpacks,
            rustify: self.rustify_vars(sha),
            service,
            user_buildtime: self
                .env_vars
                .iter()
                .filter(|e| e.is_buildtime)
                .map(|e| (e.key.clone(), e.value.clone()))
                .collect(),
        };
        let body = envfile::render_build_env(&layers);
        let script = envfile::write_file_in_helper(&self.deployment.uuid, BUILD_TIME_ENV, &body);
        self.exec_step(&script, true, false).await?;
        Ok(())
    }

    /// Rustify-generated `RUSTIFY_*` build variables (analogue of Coolify's
    /// `COOLIFY_*`), including `SOURCE_COMMIT` used by build tooling.
    fn rustify_vars(&self, sha: &str) -> Vec<(String, String)> {
        let mut vars = vec![
            ("SOURCE_COMMIT".to_string(), sha.to_string()),
            (
                "RUSTIFY_BRANCH".to_string(),
                self.application.git_branch.clone(),
            ),
            (
                "RUSTIFY_CONTAINER_NAME".to_string(),
                self.container_name.clone(),
            ),
        ];
        if let Some(fqdn) = &self.application.fqdn {
            vars.push(("RUSTIFY_FQDN".to_string(), fqdn.clone()));
            vars.push(("RUSTIFY_URL".to_string(), fqdn.clone()));
        }
        vars
    }

    /// `SERVICE_*` variables for the docker-compose buildpack. Phase 1 emits the
    /// service FQDN/URL only; per-service name generation is out of scope.
    fn service_vars(&self) -> Vec<(String, String)> {
        let mut vars = Vec::new();
        if let Some(fqdn) = &self.application.fqdn {
            vars.push(("SERVICE_FQDN".to_string(), fqdn.clone()));
            vars.push(("SERVICE_URL".to_string(), fqdn.clone()));
        }
        vars
    }

    /// All application env vars as (key, value) for the runtime `.env`.
    fn runtime_env_vars(&self) -> Vec<(String, String)> {
        self.env_vars
            .iter()
            .map(|e| (e.key.clone(), e.value.clone()))
            .collect()
    }

    fn registry_image(&self) -> Result<String, DeployError> {
        let name = self
            .application
            .docker_registry_image_name
            .as_deref()
            .filter(|n| !n.is_empty())
            .ok_or_else(|| {
                DeployError::Build("docker image buildpack requires a registry image name".into())
            })?;
        Ok(buildpacks::docker_image::registry_image(
            name,
            self.application.docker_registry_image_tag.as_deref(),
        ))
    }

    /// Build the compose input from the application row + resolved image.
    fn compose_input(&self, image: &str) -> AppComposeInput {
        let ports_exposes: Vec<String> = self
            .application
            .ports_exposes
            .split([',', ' '])
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .collect();
        let ports_mappings: Vec<String> = self
            .application
            .ports_mappings
            .as_deref()
            .unwrap_or("")
            .split([',', ' '])
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .collect();

        let health = self.application.health_check_enabled.then(|| {
            let port = self
                .application
                .health_check_port
                .as_deref()
                .and_then(|p| p.trim().parse::<u16>().ok())
                .or_else(|| ports_exposes.first().and_then(|p| p.parse().ok()))
                .unwrap_or(80);
            HealthCheck {
                host: self.application.health_check_host.clone(),
                port,
                path: self.application.health_check_path.clone(),
                interval_secs: self.application.health_check_interval.max(1) as u32,
                timeout_secs: self.application.health_check_timeout.max(1) as u32,
                retries: self.application.health_check_retries.max(1) as u32,
                start_period_secs: self.application.health_check_start_period.max(0) as u32,
            }
        });

        AppComposeInput {
            application_id: self.application.id,
            application_uuid: self.application.uuid.clone(),
            deployment_uuid: self.deployment.uuid.clone(),
            container_name: self.container_name.clone(),
            service_name: self.container_name.clone(),
            image: image.to_string(),
            network: self.network.clone(),
            ports_exposes,
            ports_mappings,
            fqdn: self.application.fqdn.clone(),
            health,
            limits_memory: self.application.limits_memory.clone(),
            limits_cpus: self.application.limits_cpus.clone(),
            volumes: vec![],
            env_file: Some(".env".to_string()),
            restart: "unless-stopped".to_string(),
        }
    }

    /// Write `content` to a scratch file and upload it to `<app_config_dir>/<name>`.
    async fn upload_text(&self, name: &str, content: &str) -> Result<(), DeployError> {
        let dir = std::env::temp_dir()
            .join("rustify-deploy")
            .join(&self.deployment.uuid);
        std::fs::create_dir_all(&dir)
            .map_err(|e| DeployError::Missing(format!("scratch dir: {e}")))?;
        let local = dir.join(name);
        std::fs::write(&local, content)
            .map_err(|e| DeployError::Missing(format!("write {name}: {e}")))?;
        let remote = format!("{}/{}", self.app_config_dir, name);
        self.deps
            .executor
            .upload(&self.conn, &local, &remote)
            .await?;
        Ok(())
    }

    // ---- command execution + logging ----------------------------------------

    /// `docker exec <helper> sh -c "<cmd>"`.
    fn in_helper(&self, cmd: &str) -> String {
        format!("docker exec {} sh -c \"{}\"", self.deployment.uuid, cmd)
    }

    fn exec_opts(&self) -> ExecOpts {
        ExecOpts {
            timeout_secs: Some(self.timeout_secs),
            disable_mux: false,
        }
    }

    /// True if the deployment has been asked to cancel, via either the process
    /// shutdown token or a DB cancel request.
    pub(crate) async fn check_cancel(&self) -> Result<(), DeployError> {
        if self.shutdown.is_cancelled() {
            return Err(DeployError::Cancelled);
        }
        if self.repo.cancel_requested(self.deployment.id).await? {
            return Err(DeployError::Cancelled);
        }
        Ok(())
    }

    /// Run a remote command, streaming its output to the log/event bus. Checks
    /// cancellation *before* dispatching. When `allow_failure` is false, a
    /// non-zero exit becomes [`DeployError::Build`].
    pub(crate) async fn exec_step(
        &mut self,
        script: &str,
        hidden: bool,
        allow_failure: bool,
    ) -> Result<ExecOutput, DeployError> {
        self.check_cancel().await?;

        let (tx, mut rx) = mpsc::channel::<ExecEvent>(256);
        let executor = self.deps.executor.clone();
        let conn = self.conn.clone();
        let opts = self.exec_opts();
        let owned = script.to_string();
        let exec_fut = async move { executor.exec_streaming(&conn, &owned, opts, tx).await };
        tokio::pin!(exec_fut);

        // Poll for cancellation every 500ms *while the command runs* so a
        // long-running step (e.g. a build) is aborted promptly instead of only
        // between steps. `biased` polls this first so heavy output never starves
        // the cancel check; on cancel we drop `exec_fut` (abandoning the local
        // command) and return — the caller's `cleanup` force-removes the helper,
        // which stops any remote work.
        let mut cancel_poll = tokio::time::interval(std::time::Duration::from_millis(500));
        cancel_poll.tick().await; // consume the immediate first tick
        let mut result: Option<Result<ExecOutput, rustify_core::ExecError>> = None;
        loop {
            tokio::select! {
                biased;
                _ = cancel_poll.tick(), if result.is_none() => { self.check_cancel().await?; }
                evt = rx.recv() => match evt {
                    Some(ExecEvent::Stdout(line)) => self.emit("stdout", &line, hidden).await,
                    Some(ExecEvent::Stderr(line)) => self.emit("stderr", &line, hidden).await,
                    None => if result.is_some() { break; },
                },
                r = &mut exec_fut, if result.is_none() => { result = Some(r); }
            }
        }

        // The loop only breaks once `result` is `Some` (the exec future has
        // resolved). Guard the invariant with a graceful error instead of a
        // panic: a panic here would unwind past the caller's helper-cleanup
        // step, leaking the build container.
        let output = match result {
            Some(r) => r?,
            None => {
                return Err(DeployError::Exec(rustify_core::ExecError::Io(
                    "exec stream closed before the command resolved".into(),
                )));
            }
        };
        if !allow_failure && output.exit_code != 0 {
            return Err(DeployError::Build(format!(
                "command exited {}: {}",
                output.exit_code,
                output.stderr.trim()
            )));
        }
        Ok(output)
    }

    fn next_batch(&mut self) {
        self.batch += 1;
    }

    /// Set the deployment status (Contract C2, enforced in SQL) and broadcast
    /// the change when it actually moved.
    async fn set_status(&mut self, status: DeploymentStatus) -> Result<bool, DeployError> {
        let moved = self.repo.transition(self.deployment.id, status).await?;
        if moved {
            self.deployment.status = status;
            let _ = self.deps.events.send(WsEvent::deployment_status_changed(
                &self.deployment.uuid,
                status,
            ));
        }
        Ok(moved)
    }

    /// Append one redacted log line and broadcast it.
    async fn emit(&mut self, kind: &str, content: &str, hidden: bool) {
        let refs: Vec<&str> = self.secrets.iter().map(String::as_str).collect();
        let content = redact(content, &refs);
        let line = LogLine {
            order: self.order,
            kind: kind.to_string(),
            content,
            hidden,
            batch: self.batch,
            timestamp: Utc::now(),
        };
        self.order += 1;
        if let Err(e) = self
            .repo
            .append_logs(self.deployment.id, std::slice::from_ref(&line))
            .await
        {
            tracing::warn!(error = %e, "failed to append deployment log");
        }
        let _ = self.deps.events.send(WsEvent::deployment_log_appended(
            &self.deployment.uuid,
            &line,
        ));
    }

    pub(crate) async fn info(&mut self, msg: &str) {
        self.emit("info", msg, false).await;
    }

    async fn info_hidden(&mut self, msg: &str) {
        self.emit("info", msg, true).await;
    }

    pub(crate) async fn error(&mut self, msg: &str) {
        self.emit("stderr", msg, false).await;
    }
}

/// A short 6-char container-name suffix (Contract C7), drawn from a fresh CUID2.
fn short_id() -> String {
    ids::new_uuid().chars().take(6).collect()
}

/// Plaintext heredoc file writer for non-secret files (Dockerfiles), so the
/// content is auditable in recorded scripts / logs.
fn write_text_in_helper(deployment_uuid: &str, path: &str, content: &str) -> String {
    format!(
        "docker exec {deployment_uuid} sh -c 'cat > {path} <<'\"'\"'RUSTIFY_EOF'\"'\"'\n{content}\nRUSTIFY_EOF'"
    )
}

/// Load an application by numeric id (repos expose only uuid lookups).
async fn load_application(
    repo: &ApplicationRepo,
    pool: &sqlx::PgPool,
    id: i64,
) -> Result<Application, DeployError> {
    let uuid: Option<String> = sqlx::query_scalar("SELECT uuid FROM applications WHERE id = $1")
        .bind(id)
        .fetch_optional(pool)
        .await?;
    let uuid = uuid.ok_or_else(|| DeployError::Missing(format!("application {id}")))?;
    repo.get_by_uuid(&uuid)
        .await?
        .ok_or_else(|| DeployError::Missing(format!("application {uuid}")))
}

/// Resolve the git source for an application (parity with Coolify's
/// `deploymentType()`): a raw deploy key takes precedence, then a GitHub App
/// source, otherwise a public clone. The private-key PEM / installation-token
/// material is loaded here (decrypted) but never logged.
async fn resolve_git_source(
    pool: &sqlx::PgPool,
    application: &Application,
) -> Result<GitSource, DeployError> {
    // A real deploy key (private_key_id) always wins (deploymentType == 'deploy_key').
    if let Some(pk_id) = application.private_key_id {
        let pem = KeyRepo::new(pool.clone())
            .decrypt_private_key(pk_id)
            .await
            .map_err(|e| DeployError::Missing(format!("deploy key {pk_id}: {e}")))?;
        return Ok(GitSource::DeployKey {
            repository: application.git_repository.clone(),
            port: 22,
            b64_key: BASE64.encode(pem.as_bytes()),
        });
    }

    // A GitHub App source (deploymentType == 'source').
    if application.source_type.as_deref() == Some("github_app")
        && let Some(src_id) = application.source_id
    {
        let gh = GithubAppRepo::new(pool.clone())
            .get_by_id(src_id)
            .await?
            .ok_or_else(|| DeployError::Missing(format!("github app {src_id}")))?;

        if gh.is_public {
            // Public GitHub-App repo: unauthenticated {html_url}/{repo}.git clone.
            let url = format!(
                "{}/{}.git",
                gh.html_url.trim_end_matches('/'),
                application.git_repository
            );
            return Ok(GitSource::Public { url });
        }

        let pk_id = gh.private_key_id.ok_or_else(|| {
            DeployError::Missing("github app has no private key configured".into())
        })?;
        let pem = KeyRepo::new(pool.clone())
            .decrypt_private_key(pk_id)
            .await
            .map_err(|e| DeployError::Missing(format!("github app key: {e}")))?;
        let (scheme, host) = parse_scheme_host(&gh.html_url);
        return Ok(GitSource::GithubApp {
            scheme,
            host,
            repo: application.git_repository.clone(),
            app: GithubAppRow {
                id: gh.id,
                app_id: gh.app_id.unwrap_or(0),
                installation_id: gh.installation_id.unwrap_or(0),
                api_url: gh.api_url.clone(),
                private_key_pem: pem,
            },
        });
    }

    Ok(GitSource::Public {
        url: application.git_repository.clone(),
    })
}

/// Split an `html_url` (e.g. `https://github.com`) into `(scheme, host)`.
fn parse_scheme_host(html_url: &str) -> (String, String) {
    match html_url.split_once("://") {
        Some((scheme, rest)) => {
            let host = rest.split('/').next().unwrap_or(rest).to_string();
            (scheme.to_string(), host)
        }
        None => (
            "https".to_string(),
            html_url.trim_end_matches('/').to_string(),
        ),
    }
}

async fn load_server(
    repo: &ServerRepo,
    pool: &sqlx::PgPool,
    id: i64,
) -> Result<Server, DeployError> {
    let uuid: Option<String> = sqlx::query_scalar("SELECT uuid FROM servers WHERE id = $1")
        .bind(id)
        .fetch_optional(pool)
        .await?;
    let uuid = uuid.ok_or_else(|| DeployError::Missing(format!("server {id}")))?;
    repo.get_by_uuid(&uuid)
        .await?
        .ok_or_else(|| DeployError::Missing(format!("server {uuid}")))
}

async fn destination_network(
    pool: &sqlx::PgPool,
    destination_id: i64,
) -> Result<String, DeployError> {
    let net: Option<String> = sqlx::query_scalar("SELECT network FROM destinations WHERE id = $1")
        .bind(destination_id)
        .fetch_optional(pool)
        .await?;
    Ok(net.unwrap_or_else(|| "rustify".to_string()))
}

/// Build the SSH connection parameters, best-effort materialising the private
/// key to a 0600 file. If the key cannot be decrypted (e.g. in tests using a
/// fake executor that ignores it) the path still points at the conventional
/// location and a real connect would surface the missing key.
pub(crate) async fn build_conn(
    pool: &sqlx::PgPool,
    server: &Server,
    connection_timeout: u32,
) -> ServerConn {
    let key_dir = std::env::var("RUSTIFY_SSH_KEY_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| std::env::temp_dir().join("rustify-keys"));
    let key_path = key_dir.join(&server.uuid);

    let key_repo = KeyRepo::new(pool.clone());
    if let Ok(material) = key_repo.decrypt_private_key(server.private_key_id).await {
        materialise_key(&key_dir, &key_path, &material);
    }

    ServerConn {
        uuid: server.uuid.clone(),
        host: server.ip.clone(),
        port: server.port as u16,
        user: server.ssh_user.clone(),
        key_path,
        connection_timeout_secs: connection_timeout,
    }
}

/// Bind-mount argument that makes a `file://` git repository readable inside
/// the build helper. Returns `None` for remote URLs (https/git@), which the
/// helper reaches over the network. The repo path is mounted read-only at the
/// same absolute path so the unchanged `file://<path>` URL resolves.
fn file_repo_mount(git_repository: &str) -> Option<String> {
    let path = git_repository.strip_prefix("file://")?;
    // Only mount a sane absolute path; never inject shell metacharacters.
    if !path.starts_with('/') || path.contains(['"', '\'', ' ', ';', '&', '|', '$', '`', '\n']) {
        return None;
    }
    Some(format!("-v {path}:{path}:ro"))
}

fn materialise_key(dir: &std::path::Path, path: &std::path::Path, material: &str) {
    if std::fs::create_dir_all(dir).is_err() {
        return;
    }
    if std::fs::write(path, material).is_err() {
        return;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    }
}

#[cfg(test)]
mod tests {
    use super::file_repo_mount;

    #[test]
    fn mounts_file_repo_read_only_at_same_path() {
        assert_eq!(
            file_repo_mount("file:///srv/git/app.git").as_deref(),
            Some("-v /srv/git/app.git:/srv/git/app.git:ro")
        );
    }

    #[test]
    fn no_mount_for_remote_urls() {
        assert_eq!(file_repo_mount("https://github.com/x/y.git"), None);
        assert_eq!(file_repo_mount("git@github.com:x/y.git"), None);
    }

    #[test]
    fn rejects_unsafe_or_relative_file_paths() {
        assert_eq!(file_repo_mount("file://relative/x.git"), None);
        assert_eq!(file_repo_mount("file:///srv/git/$(rm -rf).git"), None);
        assert_eq!(file_repo_mount("file:///srv/a b.git"), None);
    }
}
