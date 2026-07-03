//! Application state, configuration, the OpenAPI document, and router wiring.

use std::path::PathBuf;

use axum::Router;
use axum::routing::{get, patch, post};
use sqlx::PgPool;
use tokio::sync::broadcast;
use utoipa::OpenApi;
use utoipa_swagger_ui::SwaggerUi;

use rustify_core::WsEvent;
use rustify_jobs::JobQueue;

use crate::routes::{
    applications, auth, backups, databases, deployments, github_apps, health, keys, notifications,
    projects, s3_storages, scheduled_tasks, servers, service_templates, services, settings, tokens,
    webhooks,
};
use crate::{embed, ws};

/// Runtime configuration derived from the environment.
#[derive(Clone, Debug)]
pub struct Config {
    /// Emit the `Secure` cookie attribute (disabled for local HTTP/tests).
    pub cookie_secure: bool,
    /// Directory where server SSH keys are materialized `0600` on demand.
    pub ssh_key_dir: PathBuf,
    /// Directory for SSH ControlMaster mux sockets.
    pub ssh_mux_dir: PathBuf,
}

impl Config {
    /// Build configuration from environment variables, applying defaults.
    ///
    /// The data dir holds SSH mux sockets and materialised keys. It defaults to
    /// `$RUSTIFY_DATA_DIR`, else `$HOME/.rustify` (writable on dev machines),
    /// else a temp dir. The release image sets `RUSTIFY_DATA_DIR=/data/rustify`.
    pub fn from_env() -> Self {
        let base = std::env::var("RUSTIFY_DATA_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|_| {
                std::env::var("HOME")
                    .map(|h| PathBuf::from(h).join(".rustify"))
                    .unwrap_or_else(|_| std::env::temp_dir().join("rustify"))
            });
        Self {
            cookie_secure: std::env::var("RUSTIFY_COOKIE_SECURE")
                .map(|v| v != "false" && v != "0")
                .unwrap_or(true),
            ssh_key_dir: base.join("ssh").join("keys"),
            ssh_mux_dir: base.join("ssh").join("mux"),
        }
    }

    /// Test defaults: insecure cookies (HTTP) and a per-process temp key dir.
    pub fn for_test() -> Self {
        let base = std::env::temp_dir().join(format!("rustify-test-{}", std::process::id()));
        Self {
            cookie_secure: false,
            ssh_key_dir: base.join("keys"),
            ssh_mux_dir: base.join("mux"),
        }
    }
}

/// Shared state handed to every handler (contract F: `{pool, queue, events,
/// config}`). Repositories are cheap `Clone` handles constructed from `pool`
/// on demand.
#[derive(Clone)]
pub struct AppState {
    pub pool: PgPool,
    pub queue: JobQueue,
    pub events: broadcast::Sender<WsEvent>,
    pub config: Config,
}

/// The generated OpenAPI document (contract C5 surface), served at
/// `/api/v1/openapi.json` with Swagger UI at `/docs`.
#[derive(OpenApi)]
#[openapi(
    info(title = "Rustify API", version = "0.1.0"),
    servers((url = "/api/v1")),
    paths(
        health::health,
        auth::login,
        auth::logout,
        auth::me,
        keys::list,
        keys::create,
        keys::generate,
        keys::get,
        keys::update,
        keys::delete,
        servers::list,
        servers::create,
        servers::get,
        servers::update,
        servers::delete,
        servers::validate,
        servers::get_proxy,
        servers::update_proxy,
        servers::proxy_start,
        servers::proxy_stop,
        servers::proxy_restart,
        projects::list,
        projects::create,
        projects::get,
        projects::update,
        projects::delete,
        projects::list_environments,
        projects::create_environment,
        applications::list,
        applications::create,
        applications::get,
        applications::update,
        applications::delete,
        applications::deploy,
        applications::stop,
        applications::restart,
        applications::logs,
        applications::list_envs,
        applications::create_env,
        applications::update_env,
        applications::delete_env,
        deployments::list,
        deployments::get,
        deployments::cancel,
        github_apps::list,
        github_apps::create,
        github_apps::get,
        github_apps::update,
        github_apps::delete,
        github_apps::repositories,
        github_apps::branches,
        databases::list,
        databases::create,
        databases::get,
        databases::update,
        databases::delete,
        databases::start,
        databases::stop,
        databases::restart,
        backups::list,
        backups::create,
        backups::get,
        backups::update,
        backups::delete,
        backups::trigger,
        backups::executions,
        s3_storages::list,
        s3_storages::create,
        s3_storages::get,
        s3_storages::update,
        s3_storages::delete,
        s3_storages::test,
        service_templates::list,
        service_templates::get,
        services::list,
        services::create,
        services::get,
        services::update,
        services::delete,
        services::deploy,
        services::stop,
        services::restart,
        scheduled_tasks::list_for_application,
        scheduled_tasks::create_for_application,
        scheduled_tasks::list_for_service,
        scheduled_tasks::create_for_service,
        scheduled_tasks::get,
        scheduled_tasks::update,
        scheduled_tasks::delete,
        scheduled_tasks::trigger,
        scheduled_tasks::executions,
        settings::get,
        settings::update,
        notifications::get,
        notifications::update,
        notifications::test,
        tokens::list,
        tokens::create,
        tokens::delete,
    ),
    components(schemas(
        crate::error::ApiErrorBody,
        health::Health,
        auth::UserDto,
        auth::LoginRequest,
        auth::LoginResponse,
        keys::PrivateKeyDto,
        keys::PrivateKeyCreate,
        keys::PrivateKeyGenerate,
        keys::PrivateKeyUpdate,
        servers::ServerDto,
        servers::ServerCreate,
        servers::ServerUpdate,
        servers::ProxyConfig,
        servers::ProxyConfigUpdate,
        servers::ValidateResponse,
        projects::ProjectDto,
        projects::EnvironmentDto,
        projects::ProjectCreate,
        projects::ProjectUpdate,
        projects::EnvironmentCreate,
        applications::ApplicationDto,
        applications::ApplicationCreate,
        applications::ApplicationUpdate,
        applications::DeployRequest,
        applications::DeployResponse,
        applications::ContainerLogs,
        applications::EnvVarDto,
        applications::EnvVarCreate,
        applications::EnvVarUpdate,
        deployments::DeploymentDto,
        deployments::LogLineDto,
        deployments::DeploymentDetailDto,
        github_apps::GithubAppDto,
        github_apps::GithubAppCreate,
        github_apps::GithubAppUpdate,
        github_apps::RepositoriesResponse,
        github_apps::BranchesResponse,
        databases::DatabaseDto,
        databases::DatabaseCreate,
        databases::DatabaseUpdate,
        backups::BackupDto,
        backups::BackupCreate,
        backups::BackupUpdate,
        backups::ExecutionDto,
        s3_storages::S3StorageDto,
        s3_storages::S3StorageCreate,
        s3_storages::S3StorageUpdate,
        s3_storages::S3TestResponse,
        service_templates::ServiceTemplateDto,
        service_templates::ServiceTemplateDetailDto,
        services::ServiceDto,
        services::ServiceApplicationDto,
        services::ServiceCreate,
        services::ServiceUpdate,
        scheduled_tasks::ScheduledTaskDto,
        scheduled_tasks::ScheduledTaskExecutionDto,
        scheduled_tasks::ScheduledTaskCreate,
        scheduled_tasks::ScheduledTaskUpdate,
        settings::InstanceSettingsDto,
        settings::InstanceSettingsUpdate,
        notifications::NotificationSettingsDto,
        notifications::NotificationSettingsUpdate,
        notifications::TestRequest,
        notifications::TestResponse,
        tokens::ApiTokenDto,
        tokens::ApiTokenCreate,
        tokens::ApiTokenCreated,
    )),
    tags(
        (name = "health", description = "Liveness"),
        (name = "auth", description = "Authentication"),
        (name = "private-keys", description = "SSH private keys"),
        (name = "servers", description = "Servers and proxy"),
        (name = "projects", description = "Projects and environments"),
        (name = "applications", description = "Applications, deploys, env vars, logs"),
        (name = "deployments", description = "Deployments"),
        (name = "github-apps", description = "GitHub App sources"),
        (name = "databases", description = "Standalone databases"),
        (name = "backups", description = "Scheduled database backups"),
        (name = "s3-storages", description = "S3-compatible backup storage"),
        (name = "service-templates", description = "One-click service catalog"),
        (name = "services", description = "One-click services"),
        (name = "scheduled-tasks", description = "User scheduled tasks and executions"),
        (name = "settings", description = "Instance settings"),
        (name = "notifications", description = "Notification channels and settings"),
        (name = "api-tokens", description = "API tokens"),
    )
)]
pub struct ApiDoc;

/// The `/api/v1` route table (contract C5). Paths are explicit (not nested) so
/// they never collide with the Swagger `/api/v1/openapi.json` route.
fn api_router() -> Router<AppState> {
    Router::new()
        .route("/api/v1/health", get(health::health))
        // auth
        .route("/api/v1/auth/login", post(auth::login))
        .route("/api/v1/auth/logout", post(auth::logout))
        .route("/api/v1/auth/me", get(auth::me))
        // private keys
        .route("/api/v1/private-keys", get(keys::list).post(keys::create))
        .route("/api/v1/private-keys/generate", post(keys::generate))
        .route(
            "/api/v1/private-keys/{uuid}",
            get(keys::get).patch(keys::update).delete(keys::delete),
        )
        // servers
        .route("/api/v1/servers", get(servers::list).post(servers::create))
        .route(
            "/api/v1/servers/{uuid}",
            get(servers::get)
                .patch(servers::update)
                .delete(servers::delete),
        )
        .route("/api/v1/servers/{uuid}/validate", post(servers::validate))
        .route(
            "/api/v1/servers/{uuid}/proxy",
            get(servers::get_proxy).patch(servers::update_proxy),
        )
        .route(
            "/api/v1/servers/{uuid}/proxy/start",
            post(servers::proxy_start),
        )
        .route(
            "/api/v1/servers/{uuid}/proxy/stop",
            post(servers::proxy_stop),
        )
        .route(
            "/api/v1/servers/{uuid}/proxy/restart",
            post(servers::proxy_restart),
        )
        // projects
        .route(
            "/api/v1/projects",
            get(projects::list).post(projects::create),
        )
        .route(
            "/api/v1/projects/{uuid}",
            get(projects::get)
                .patch(projects::update)
                .delete(projects::delete),
        )
        .route(
            "/api/v1/projects/{uuid}/environments",
            get(projects::list_environments).post(projects::create_environment),
        )
        // applications
        .route(
            "/api/v1/applications",
            get(applications::list).post(applications::create),
        )
        .route(
            "/api/v1/applications/{uuid}",
            get(applications::get)
                .patch(applications::update)
                .delete(applications::delete),
        )
        .route(
            "/api/v1/applications/{uuid}/deploy",
            post(applications::deploy),
        )
        .route("/api/v1/applications/{uuid}/stop", post(applications::stop))
        .route(
            "/api/v1/applications/{uuid}/restart",
            post(applications::restart),
        )
        .route("/api/v1/applications/{uuid}/logs", get(applications::logs))
        .route(
            "/api/v1/applications/{uuid}/envs",
            get(applications::list_envs).post(applications::create_env),
        )
        .route(
            "/api/v1/applications/{uuid}/envs/{env_uuid}",
            patch(applications::update_env).delete(applications::delete_env),
        )
        // databases
        .route(
            "/api/v1/databases",
            get(databases::list).post(databases::create),
        )
        .route(
            "/api/v1/databases/{uuid}",
            get(databases::get)
                .patch(databases::update)
                .delete(databases::delete),
        )
        .route("/api/v1/databases/{uuid}/start", post(databases::start))
        .route("/api/v1/databases/{uuid}/stop", post(databases::stop))
        .route("/api/v1/databases/{uuid}/restart", post(databases::restart))
        .route(
            "/api/v1/databases/{uuid}/backups",
            get(backups::list).post(backups::create),
        )
        // backups
        .route(
            "/api/v1/backups/{uuid}",
            get(backups::get)
                .patch(backups::update)
                .delete(backups::delete),
        )
        .route("/api/v1/backups/{uuid}/trigger", post(backups::trigger))
        .route(
            "/api/v1/backups/{uuid}/executions",
            get(backups::executions),
        )
        // s3 storages
        .route(
            "/api/v1/s3-storages",
            get(s3_storages::list).post(s3_storages::create),
        )
        .route(
            "/api/v1/s3-storages/{uuid}",
            get(s3_storages::get)
                .patch(s3_storages::update)
                .delete(s3_storages::delete),
        )
        .route("/api/v1/s3-storages/{uuid}/test", post(s3_storages::test))
        // github apps
        .route(
            "/api/v1/github-apps",
            get(github_apps::list).post(github_apps::create),
        )
        .route(
            "/api/v1/github-apps/{uuid}",
            get(github_apps::get)
                .patch(github_apps::update)
                .delete(github_apps::delete),
        )
        .route(
            "/api/v1/github-apps/{uuid}/repositories",
            get(github_apps::repositories),
        )
        .route(
            "/api/v1/github-apps/{uuid}/repositories/{owner}/{repo}/branches",
            get(github_apps::branches),
        )
        // github app-manifest web flow (not part of the /api/v1 OpenAPI surface)
        .route(
            "/webhooks/source/github/redirect",
            get(github_apps::redirect),
        )
        .route("/webhooks/source/github/install", get(github_apps::install))
        // git-source webhook receivers (unauthenticated; provider-signed)
        .route("/webhooks/source/github/events", post(webhooks::github_app))
        .route(
            "/webhooks/source/github/events/manual",
            post(webhooks::github_manual),
        )
        .route(
            "/webhooks/source/gitlab/events/manual",
            post(webhooks::gitlab_manual),
        )
        .route(
            "/webhooks/source/gitea/events/manual",
            post(webhooks::gitea_manual),
        )
        .route(
            "/webhooks/source/bitbucket/events/manual",
            post(webhooks::bitbucket_manual),
        )
        // deployments
        .route("/api/v1/deployments", get(deployments::list))
        .route("/api/v1/deployments/{uuid}", get(deployments::get))
        .route(
            "/api/v1/deployments/{uuid}/cancel",
            post(deployments::cancel),
        )
        // service templates (catalog)
        .route("/api/v1/service-templates", get(service_templates::list))
        .route(
            "/api/v1/service-templates/{key}",
            get(service_templates::get),
        )
        // services
        .route(
            "/api/v1/services",
            get(services::list).post(services::create),
        )
        .route(
            "/api/v1/services/{uuid}",
            get(services::get)
                .patch(services::update)
                .delete(services::delete),
        )
        .route("/api/v1/services/{uuid}/deploy", post(services::deploy))
        .route("/api/v1/services/{uuid}/stop", post(services::stop))
        .route("/api/v1/services/{uuid}/restart", post(services::restart))
        // scheduled tasks
        .route(
            "/api/v1/applications/{uuid}/scheduled-tasks",
            get(scheduled_tasks::list_for_application)
                .post(scheduled_tasks::create_for_application),
        )
        .route(
            "/api/v1/services/{uuid}/scheduled-tasks",
            get(scheduled_tasks::list_for_service).post(scheduled_tasks::create_for_service),
        )
        .route(
            "/api/v1/scheduled-tasks/{uuid}",
            get(scheduled_tasks::get)
                .patch(scheduled_tasks::update)
                .delete(scheduled_tasks::delete),
        )
        .route(
            "/api/v1/scheduled-tasks/{uuid}/trigger",
            post(scheduled_tasks::trigger),
        )
        .route(
            "/api/v1/scheduled-tasks/{uuid}/executions",
            get(scheduled_tasks::executions),
        )
        // settings
        .route(
            "/api/v1/settings",
            get(settings::get).patch(settings::update),
        )
        // notifications
        .route(
            "/api/v1/notifications/settings",
            get(notifications::get).patch(notifications::update),
        )
        .route("/api/v1/notifications/test", post(notifications::test))
        // api tokens
        .route("/api/v1/api-tokens", get(tokens::list).post(tokens::create))
        .route(
            "/api/v1/api-tokens/{uuid}",
            axum::routing::delete(tokens::delete),
        )
}

/// Build the full application router: API + WS + OpenAPI/Swagger + SPA.
pub fn build_router(state: AppState) -> Router {
    Router::new()
        .merge(api_router())
        .route("/ws", get(ws::ws_handler))
        .merge(SwaggerUi::new("/docs").url("/api/v1/openapi.json", ApiDoc::openapi()))
        .fallback(embed::static_handler)
        .with_state(state)
}
