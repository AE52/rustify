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
    applications, auth, databases, deployments, health, keys, projects, servers, settings, tokens,
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
        databases::list,
        databases::create,
        databases::get,
        databases::update,
        databases::delete,
        databases::start,
        databases::stop,
        databases::restart,
        settings::get,
        settings::update,
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
        databases::DatabaseDto,
        databases::DatabaseCreate,
        databases::DatabaseUpdate,
        settings::InstanceSettingsDto,
        settings::InstanceSettingsUpdate,
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
        (name = "databases", description = "Standalone databases"),
        (name = "settings", description = "Instance settings"),
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
        // deployments
        .route("/api/v1/deployments", get(deployments::list))
        .route("/api/v1/deployments/{uuid}", get(deployments::get))
        .route(
            "/api/v1/deployments/{uuid}/cancel",
            post(deployments::cancel),
        )
        // settings
        .route(
            "/api/v1/settings",
            get(settings::get).patch(settings::update),
        )
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
