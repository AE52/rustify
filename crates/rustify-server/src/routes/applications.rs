//! Applications routes (contract C5): CRUD, deploy/stop/restart lifecycle,
//! environment variables, and container logs (over SSH).

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::{Deserialize, Serialize};
use serde_json::json;
use utoipa::ToSchema;

use rustify_core::exec::{CommandExecutor, ExecError, ExecOpts, ServerConn};
use rustify_db::repos::{
    Application, ApplicationPatch, ApplicationRepo, DeploymentRepo, EnvVar, EnvVarRepo, KeyRepo,
    NewApplication, NewDeployment, NewEnvVar, ProjectRepo, Server, ServerRepo,
};

use crate::app::AppState;
use crate::auth::CurrentTeam;
use crate::error::{ApiError, ApiResult};

const BUILD_PACKS: [&str; 5] = [
    "nixpacks",
    "dockerfile",
    "static",
    "docker_image",
    "docker_compose",
];
const ENV_RESOURCE_KIND: &str = "application";

// ----- DTOs ---------------------------------------------------------------

#[derive(Debug, Serialize, ToSchema)]
pub struct ApplicationDto {
    pub uuid: String,
    pub name: String,
    pub fqdn: Option<String>,
    pub environment_uuid: String,
    pub project_uuid: String,
    pub server_uuid: String,
    pub git_repository: String,
    pub git_branch: String,
    pub git_commit_sha: String,
    pub build_pack: String,
    pub static_image: String,
    pub docker_registry_image_name: Option<String>,
    pub docker_registry_image_tag: Option<String>,
    pub dockerfile_location: String,
    pub docker_compose_location: String,
    pub base_directory: String,
    pub publish_directory: Option<String>,
    pub install_command: Option<String>,
    pub build_command: Option<String>,
    pub start_command: Option<String>,
    pub ports_exposes: String,
    pub ports_mappings: Option<String>,
    pub health_check_enabled: bool,
    pub health_check_path: String,
    pub health_check_port: Option<String>,
    pub health_check_host: String,
    pub health_check_method: String,
    pub health_check_return_code: i32,
    pub health_check_interval: i32,
    pub health_check_timeout: i32,
    pub health_check_retries: i32,
    pub health_check_start_period: i32,
    pub limits_memory: String,
    pub limits_cpus: String,
    pub custom_docker_run_options: Option<String>,
    pub status: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct ApplicationCreate {
    pub project_uuid: String,
    pub environment_name: String,
    pub server_uuid: String,
    pub name: String,
    pub git_repository: String,
    pub git_branch: Option<String>,
    pub build_pack: Option<String>,
    pub ports_exposes: Option<String>,
    pub fqdn: Option<String>,
    pub base_directory: Option<String>,
    pub publish_directory: Option<String>,
    pub dockerfile_location: Option<String>,
    pub docker_compose_location: Option<String>,
    pub static_image: Option<String>,
    pub install_command: Option<String>,
    pub build_command: Option<String>,
    pub start_command: Option<String>,
}

#[derive(Debug, Default, Deserialize, ToSchema)]
pub struct ApplicationUpdate {
    pub name: Option<String>,
    pub fqdn: Option<String>,
    pub git_repository: Option<String>,
    pub git_branch: Option<String>,
    pub git_commit_sha: Option<String>,
    pub build_pack: Option<String>,
    pub static_image: Option<String>,
    pub dockerfile_location: Option<String>,
    pub docker_compose_location: Option<String>,
    pub base_directory: Option<String>,
    pub publish_directory: Option<String>,
    pub install_command: Option<String>,
    pub build_command: Option<String>,
    pub start_command: Option<String>,
    pub ports_exposes: Option<String>,
    pub ports_mappings: Option<String>,
    pub health_check_enabled: Option<bool>,
    pub health_check_path: Option<String>,
    pub limits_memory: Option<String>,
    pub limits_cpus: Option<String>,
    pub custom_docker_run_options: Option<String>,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct DeployRequest {
    #[serde(default)]
    pub force_rebuild: bool,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct DeployResponse {
    pub deployment_uuid: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ContainerLogs {
    pub logs: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct EnvVarDto {
    pub uuid: String,
    pub key: String,
    /// Null once a shown-once variable has been persisted.
    pub value: Option<String>,
    pub is_buildtime: bool,
    pub is_literal: bool,
    pub is_shown_once: bool,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

impl EnvVarDto {
    /// Rendered for a list: shown-once values are masked.
    fn masked(v: EnvVar) -> Self {
        let is_shown_once = v.is_shown_once;
        let mut dto = Self::revealed(v);
        if is_shown_once {
            dto.value = None;
        }
        dto
    }

    /// Rendered right after a write: the value is shown once.
    fn revealed(v: EnvVar) -> Self {
        Self {
            uuid: v.uuid,
            key: v.key,
            value: Some(v.value),
            is_buildtime: v.is_buildtime,
            is_literal: v.is_literal,
            is_shown_once: v.is_shown_once,
            created_at: v.created_at,
            updated_at: v.updated_at,
        }
    }
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct EnvVarCreate {
    pub key: String,
    pub value: String,
    #[serde(default)]
    pub is_buildtime: bool,
    #[serde(default)]
    pub is_literal: bool,
    #[serde(default)]
    pub is_shown_once: bool,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct EnvVarUpdate {
    pub key: Option<String>,
    pub value: Option<String>,
    pub is_buildtime: Option<bool>,
    pub is_literal: Option<bool>,
    pub is_shown_once: Option<bool>,
}

#[derive(Debug, Deserialize)]
pub struct LogsQuery {
    pub lines: Option<i64>,
}

// ----- Resolution helpers -------------------------------------------------

/// An application plus the related rows needed to render/act on it.
struct AppContext {
    app: Application,
    environment_uuid: String,
    project_uuid: String,
    server: Server,
}

/// Resolve an application by uuid and enforce team ownership (via its
/// environment → project → team chain).
async fn resolve(state: &AppState, team: &CurrentTeam, uuid: &str) -> ApiResult<AppContext> {
    let app = ApplicationRepo::new(state.pool.clone())
        .get_by_uuid(uuid)
        .await?
        .ok_or(ApiError::NotFound)?;

    let projects = ProjectRepo::new(state.pool.clone());
    let environment = projects
        .environment_by_id(app.environment_id)
        .await?
        .ok_or(ApiError::NotFound)?;
    let project = projects
        .get_by_id(environment.project_id)
        .await?
        .ok_or(ApiError::NotFound)?;
    if project.team_id != team.id {
        return Err(ApiError::NotFound);
    }

    let servers = ServerRepo::new(state.pool.clone());
    let destination = servers
        .destination_by_id(app.destination_id)
        .await?
        .ok_or(ApiError::NotFound)?;
    let server = servers
        .get_by_id(destination.server_id)
        .await?
        .ok_or(ApiError::NotFound)?;

    Ok(AppContext {
        app,
        environment_uuid: environment.uuid,
        project_uuid: project.uuid,
        server,
    })
}

fn to_dto(ctx: AppContext) -> ApplicationDto {
    let a = ctx.app;
    ApplicationDto {
        uuid: a.uuid,
        name: a.name,
        fqdn: a.fqdn,
        environment_uuid: ctx.environment_uuid,
        project_uuid: ctx.project_uuid,
        server_uuid: ctx.server.uuid,
        git_repository: a.git_repository,
        git_branch: a.git_branch,
        git_commit_sha: a.git_commit_sha,
        build_pack: a.build_pack,
        static_image: a.static_image,
        docker_registry_image_name: a.docker_registry_image_name,
        docker_registry_image_tag: a.docker_registry_image_tag,
        dockerfile_location: a.dockerfile_location,
        docker_compose_location: a.docker_compose_location,
        base_directory: a.base_directory,
        publish_directory: a.publish_directory,
        install_command: a.install_command,
        build_command: a.build_command,
        start_command: a.start_command,
        ports_exposes: a.ports_exposes,
        ports_mappings: a.ports_mappings,
        health_check_enabled: a.health_check_enabled,
        health_check_path: a.health_check_path,
        health_check_port: a.health_check_port,
        health_check_host: a.health_check_host,
        health_check_method: a.health_check_method,
        health_check_return_code: a.health_check_return_code,
        health_check_interval: a.health_check_interval,
        health_check_timeout: a.health_check_timeout,
        health_check_retries: a.health_check_retries,
        health_check_start_period: a.health_check_start_period,
        limits_memory: a.limits_memory,
        limits_cpus: a.limits_cpus,
        custom_docker_run_options: a.custom_docker_run_options,
        status: a.status,
        created_at: a.created_at,
        updated_at: a.updated_at,
    }
}

/// Reject a git URL that is not an `https://`, `git@`, or `file://` remote
/// (contract C5). `file://` supports local bare repositories (used by the e2e
/// harness, which deploys from bare repos seeded onto the target host).
fn validate_git(url: &str) -> ApiResult<()> {
    if url.starts_with("https://") || url.starts_with("git@") || url.starts_with("file://") {
        Ok(())
    } else {
        Err(ApiError::Validation(
            "git_repository must start with https://, git@, or file://".into(),
        ))
    }
}

/// Reject an unknown build pack.
fn validate_build_pack(bp: &str) -> ApiResult<()> {
    if BUILD_PACKS.contains(&bp) {
        Ok(())
    } else {
        Err(ApiError::Validation(format!(
            "build_pack must be one of {BUILD_PACKS:?}"
        )))
    }
}

// ----- CRUD ---------------------------------------------------------------

#[utoipa::path(get, path = "/applications", operation_id = "list_applications", tag = "applications",
    responses((status = 200, description = "List of applications", body = [ApplicationDto])))]
pub async fn list(
    State(state): State<AppState>,
    team: CurrentTeam,
) -> ApiResult<Json<Vec<ApplicationDto>>> {
    // List every application the team can see, across all its projects.
    let projects = ProjectRepo::new(state.pool.clone()).list(team.id).await?;
    let project_repo = ProjectRepo::new(state.pool.clone());
    let app_repo = ApplicationRepo::new(state.pool.clone());
    let mut out = Vec::new();
    for project in projects {
        for env in project_repo.environments(project.id).await? {
            for app in app_repo.list_by_environment(env.id).await? {
                let ctx = resolve(&state, &team, &app.uuid).await?;
                out.push(to_dto(ctx));
            }
        }
    }
    Ok(Json(out))
}

#[utoipa::path(post, path = "/applications", operation_id = "create_application", tag = "applications", request_body = ApplicationCreate,
    responses(
        (status = 201, description = "Application created", body = ApplicationDto),
        (status = 422, description = "Validation error", body = crate::error::ApiErrorBody),
    ))]
pub async fn create(
    State(state): State<AppState>,
    team: CurrentTeam,
    Json(body): Json<ApplicationCreate>,
) -> ApiResult<Response> {
    validate_git(&body.git_repository)?;
    let build_pack = body.build_pack.clone().unwrap_or_else(|| "nixpacks".into());
    validate_build_pack(&build_pack)?;

    let projects = ProjectRepo::new(state.pool.clone());
    let project = projects
        .get_by_uuid(&body.project_uuid)
        .await?
        .filter(|p| p.team_id == team.id)
        .ok_or_else(|| ApiError::Validation("unknown project_uuid".into()))?;
    let environment = projects
        .environment_by_name(project.id, &body.environment_name)
        .await?
        .ok_or_else(|| ApiError::Validation("unknown environment_name".into()))?;

    let servers = ServerRepo::new(state.pool.clone());
    let server = servers
        .get_by_uuid(&body.server_uuid)
        .await?
        .filter(|s| s.team_id == team.id)
        .ok_or_else(|| ApiError::Validation("unknown server_uuid".into()))?;
    let destination = servers
        .default_destination(server.id)
        .await?
        .ok_or_else(|| ApiError::Validation("server has no destination".into()))?;

    let app = ApplicationRepo::new(state.pool.clone())
        .create(NewApplication {
            environment_id: environment.id,
            destination_id: destination.id,
            name: body.name.clone(),
            git_repository: body.git_repository.clone(),
            git_branch: body.git_branch.clone().unwrap_or_else(|| "main".into()),
            build_pack,
            ports_exposes: body.ports_exposes.clone().unwrap_or_else(|| "80".into()),
            fqdn: body.fqdn.clone(),
        })
        .await?;

    // Apply any optional build-config fields the create body carried.
    let patch = ApplicationPatch {
        base_directory: body.base_directory,
        publish_directory: body.publish_directory,
        dockerfile_location: body.dockerfile_location,
        docker_compose_location: body.docker_compose_location,
        static_image: body.static_image,
        install_command: body.install_command,
        build_command: body.build_command,
        start_command: body.start_command,
        ..Default::default()
    };
    if has_patch(&patch) {
        ApplicationRepo::new(state.pool.clone())
            .update(&app.uuid, &patch)
            .await?;
    }

    let ctx = resolve(&state, &team, &app.uuid).await?;
    Ok((StatusCode::CREATED, Json(to_dto(ctx))).into_response())
}

fn has_patch(p: &ApplicationPatch) -> bool {
    p.base_directory.is_some()
        || p.publish_directory.is_some()
        || p.dockerfile_location.is_some()
        || p.docker_compose_location.is_some()
        || p.static_image.is_some()
        || p.install_command.is_some()
        || p.build_command.is_some()
        || p.start_command.is_some()
}

#[utoipa::path(get, path = "/applications/{uuid}", operation_id = "get_application", tag = "applications",
    params(("uuid" = String, Path, description = "Application uuid")),
    responses(
        (status = 200, description = "The application", body = ApplicationDto),
        (status = 404, description = "Not found", body = crate::error::ApiErrorBody),
    ))]
pub async fn get(
    State(state): State<AppState>,
    team: CurrentTeam,
    Path(uuid): Path<String>,
) -> ApiResult<Json<ApplicationDto>> {
    let ctx = resolve(&state, &team, &uuid).await?;
    Ok(Json(to_dto(ctx)))
}

#[utoipa::path(patch, path = "/applications/{uuid}", operation_id = "update_application", tag = "applications",
    params(("uuid" = String, Path, description = "Application uuid")),
    request_body = ApplicationUpdate,
    responses(
        (status = 200, description = "Updated application", body = ApplicationDto),
        (status = 404, description = "Not found", body = crate::error::ApiErrorBody),
        (status = 422, description = "Validation error", body = crate::error::ApiErrorBody),
    ))]
pub async fn update(
    State(state): State<AppState>,
    team: CurrentTeam,
    Path(uuid): Path<String>,
    Json(body): Json<ApplicationUpdate>,
) -> ApiResult<Json<ApplicationDto>> {
    resolve(&state, &team, &uuid).await?;
    if let Some(url) = &body.git_repository {
        validate_git(url)?;
    }
    if let Some(bp) = &body.build_pack {
        validate_build_pack(bp)?;
    }
    let patch = ApplicationPatch {
        name: body.name,
        fqdn: body.fqdn,
        git_repository: body.git_repository,
        git_branch: body.git_branch,
        git_commit_sha: body.git_commit_sha,
        build_pack: body.build_pack,
        static_image: body.static_image,
        dockerfile_location: body.dockerfile_location,
        docker_compose_location: body.docker_compose_location,
        base_directory: body.base_directory,
        publish_directory: body.publish_directory,
        install_command: body.install_command,
        build_command: body.build_command,
        start_command: body.start_command,
        ports_exposes: body.ports_exposes,
        ports_mappings: body.ports_mappings,
        health_check_enabled: body.health_check_enabled,
        health_check_path: body.health_check_path,
        limits_memory: body.limits_memory,
        limits_cpus: body.limits_cpus,
        custom_docker_run_options: body.custom_docker_run_options,
    };
    ApplicationRepo::new(state.pool.clone())
        .update(&uuid, &patch)
        .await?
        .ok_or(ApiError::NotFound)?;
    let ctx = resolve(&state, &team, &uuid).await?;
    Ok(Json(to_dto(ctx)))
}

#[utoipa::path(delete, path = "/applications/{uuid}", operation_id = "delete_application", tag = "applications",
    params(("uuid" = String, Path, description = "Application uuid")),
    responses(
        (status = 204, description = "Deleted"),
        (status = 404, description = "Not found", body = crate::error::ApiErrorBody),
    ))]
pub async fn delete(
    State(state): State<AppState>,
    team: CurrentTeam,
    Path(uuid): Path<String>,
) -> ApiResult<StatusCode> {
    resolve(&state, &team, &uuid).await?;
    if ApplicationRepo::new(state.pool.clone())
        .delete(&uuid)
        .await?
    {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError::NotFound)
    }
}

// ----- Lifecycle ----------------------------------------------------------

#[utoipa::path(post, path = "/applications/{uuid}/deploy", tag = "applications",
    params(("uuid" = String, Path, description = "Application uuid")),
    request_body = DeployRequest,
    responses((status = 202, description = "Deployment queued", body = DeployResponse)))]
pub async fn deploy(
    State(state): State<AppState>,
    team: CurrentTeam,
    Path(uuid): Path<String>,
    body: Option<Json<DeployRequest>>,
) -> ApiResult<Response> {
    let ctx = resolve(&state, &team, &uuid).await?;
    let force_rebuild = body.map(|b| b.0.force_rebuild).unwrap_or(false);

    let deployment = DeploymentRepo::new(state.pool.clone())
        .create_queued(NewDeployment {
            application_id: ctx.app.id,
            server_id: ctx.server.id,
            commit_sha: Some(ctx.app.git_commit_sha.clone()),
            commit_message: None,
            force_rebuild,
            rollback: false,
            config_snapshot: None,
        })
        .await?;

    state
        .queue
        .enqueue(
            "deploy",
            json!({ "deployment_uuid": deployment.uuid }),
            None,
        )
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    Ok((
        StatusCode::ACCEPTED,
        Json(DeployResponse {
            deployment_uuid: deployment.uuid,
        }),
    )
        .into_response())
}

async fn lifecycle(
    state: &AppState,
    team: &CurrentTeam,
    uuid: &str,
    kind: &str,
) -> ApiResult<Response> {
    let ctx = resolve(state, team, uuid).await?;
    state
        .queue
        .enqueue(kind, json!({ "application_uuid": ctx.app.uuid }), None)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    Ok((StatusCode::ACCEPTED, Json(json!({ "status": "accepted" }))).into_response())
}

#[utoipa::path(post, path = "/applications/{uuid}/stop", tag = "applications",
    params(("uuid" = String, Path, description = "Application uuid")),
    responses((status = 202, description = "Stop enqueued")))]
pub async fn stop(
    State(state): State<AppState>,
    team: CurrentTeam,
    Path(uuid): Path<String>,
) -> ApiResult<Response> {
    lifecycle(&state, &team, &uuid, "app_stop").await
}

#[utoipa::path(post, path = "/applications/{uuid}/restart", tag = "applications",
    params(("uuid" = String, Path, description = "Application uuid")),
    responses((status = 202, description = "Restart enqueued")))]
pub async fn restart(
    State(state): State<AppState>,
    team: CurrentTeam,
    Path(uuid): Path<String>,
) -> ApiResult<Response> {
    lifecycle(&state, &team, &uuid, "app_restart").await
}

// ----- Container logs -----------------------------------------------------

#[utoipa::path(get, path = "/applications/{uuid}/logs", tag = "applications",
    params(
        ("uuid" = String, Path, description = "Application uuid"),
        ("lines" = Option<i64>, Query, description = "Tail this many lines (default 100)"),
    ),
    responses((status = 200, description = "Container logs", body = ContainerLogs)))]
pub async fn logs(
    State(state): State<AppState>,
    team: CurrentTeam,
    Path(uuid): Path<String>,
    Query(q): Query<LogsQuery>,
) -> ApiResult<Json<ContainerLogs>> {
    let ctx = resolve(&state, &team, &uuid).await?;
    let lines = q.lines.unwrap_or(100).clamp(1, 10_000);

    // Materialize the server's key and run `docker logs` over SSH.
    let keys = KeyRepo::new(state.pool.clone());
    let key = keys
        .get_by_id(ctx.server.private_key_id)
        .await?
        .ok_or_else(|| ApiError::Internal("server key missing".into()))?;
    let pem = keys.decrypt_private_key(ctx.server.private_key_id).await?;
    let key_path = rustify_ssh::keys::materialize(&key.uuid, &pem, &state.config.ssh_key_dir)
        .map_err(|e| ApiError::Internal(format!("key materialization failed: {e}")))?;

    let connection_timeout_secs = ServerRepo::new(state.pool.clone())
        .settings(ctx.server.id)
        .await?
        .map(|s| s.connection_timeout as u32)
        .unwrap_or(10);

    let conn = ServerConn {
        uuid: ctx.server.uuid.clone(),
        host: ctx.server.ip.clone(),
        port: ctx.server.port as u16,
        user: ctx.server.ssh_user.clone(),
        key_path,
        connection_timeout_secs,
    };

    let script = format!(
        "cid=$(docker ps -aq --filter \"label=rustify.applicationUuid={app}\" | head -n1)\n\
         if [ -z \"$cid\" ]; then echo \"no container found for application {app}\"; \
         else docker logs --tail {lines} \"$cid\" 2>&1; fi",
        app = ctx.app.uuid,
        lines = lines,
    );

    let executor = rustify_ssh::SshExecutor::new(state.config.ssh_mux_dir.clone());
    let logs = match executor.exec(&conn, &script, ExecOpts::default()).await {
        Ok(out) => combine(out.stdout, out.stderr),
        Err(ExecError::NonZero { stdout, stderr, .. }) => combine(stdout, stderr),
        Err(other) => other.to_string(),
    };
    Ok(Json(ContainerLogs { logs }))
}

fn combine(stdout: String, stderr: String) -> String {
    match (stdout.is_empty(), stderr.is_empty()) {
        (false, false) => format!("{stdout}\n{stderr}"),
        (false, true) => stdout,
        (true, false) => stderr,
        (true, true) => String::new(),
    }
}

// ----- Environment variables ----------------------------------------------

#[utoipa::path(get, path = "/applications/{uuid}/envs", tag = "applications",
    params(("uuid" = String, Path, description = "Application uuid")),
    responses((status = 200, description = "Environment variables", body = [EnvVarDto])))]
pub async fn list_envs(
    State(state): State<AppState>,
    team: CurrentTeam,
    Path(uuid): Path<String>,
) -> ApiResult<Json<Vec<EnvVarDto>>> {
    let ctx = resolve(&state, &team, &uuid).await?;
    let vars = EnvVarRepo::new(state.pool.clone())
        .list(ENV_RESOURCE_KIND, ctx.app.id)
        .await?;
    Ok(Json(vars.into_iter().map(EnvVarDto::masked).collect()))
}

#[utoipa::path(post, path = "/applications/{uuid}/envs", tag = "applications",
    params(("uuid" = String, Path, description = "Application uuid")),
    request_body = EnvVarCreate,
    responses((status = 201, description = "Env var upserted", body = EnvVarDto)))]
pub async fn create_env(
    State(state): State<AppState>,
    team: CurrentTeam,
    Path(uuid): Path<String>,
    Json(body): Json<EnvVarCreate>,
) -> ApiResult<Response> {
    let ctx = resolve(&state, &team, &uuid).await?;
    let var = EnvVarRepo::new(state.pool.clone())
        .upsert(NewEnvVar {
            resource_kind: ENV_RESOURCE_KIND.into(),
            resource_id: ctx.app.id,
            key: body.key,
            value: body.value,
            is_buildtime: body.is_buildtime,
            is_literal: body.is_literal,
            is_shown_once: body.is_shown_once,
        })
        .await?;
    Ok((StatusCode::CREATED, Json(EnvVarDto::revealed(var))).into_response())
}

#[utoipa::path(patch, path = "/applications/{uuid}/envs/{env_uuid}", tag = "applications",
    params(
        ("uuid" = String, Path, description = "Application uuid"),
        ("env_uuid" = String, Path, description = "Env var uuid"),
    ),
    request_body = EnvVarUpdate,
    responses(
        (status = 200, description = "Updated env var", body = EnvVarDto),
        (status = 404, description = "Not found", body = crate::error::ApiErrorBody),
    ))]
pub async fn update_env(
    State(state): State<AppState>,
    team: CurrentTeam,
    Path((uuid, env_uuid)): Path<(String, String)>,
    Json(body): Json<EnvVarUpdate>,
) -> ApiResult<Json<EnvVarDto>> {
    let ctx = resolve(&state, &team, &uuid).await?;
    let repo = EnvVarRepo::new(state.pool.clone());
    let existing = repo
        .list(ENV_RESOURCE_KIND, ctx.app.id)
        .await?
        .into_iter()
        .find(|v| v.uuid == env_uuid)
        .ok_or(ApiError::NotFound)?;

    let new_key = body.key.clone().unwrap_or_else(|| existing.key.clone());
    let updated = repo
        .upsert(NewEnvVar {
            resource_kind: ENV_RESOURCE_KIND.into(),
            resource_id: ctx.app.id,
            key: new_key.clone(),
            value: body.value.unwrap_or(existing.value),
            is_buildtime: body.is_buildtime.unwrap_or(existing.is_buildtime),
            is_literal: body.is_literal.unwrap_or(existing.is_literal),
            is_shown_once: body.is_shown_once.unwrap_or(existing.is_shown_once),
        })
        .await?;
    // A key rename creates a new row; drop the old one.
    if new_key != existing.key {
        repo.delete(&existing.uuid).await?;
    }
    Ok(Json(EnvVarDto::revealed(updated)))
}

#[utoipa::path(delete, path = "/applications/{uuid}/envs/{env_uuid}", tag = "applications",
    params(
        ("uuid" = String, Path, description = "Application uuid"),
        ("env_uuid" = String, Path, description = "Env var uuid"),
    ),
    responses(
        (status = 204, description = "Deleted"),
        (status = 404, description = "Not found", body = crate::error::ApiErrorBody),
    ))]
pub async fn delete_env(
    State(state): State<AppState>,
    team: CurrentTeam,
    Path((uuid, env_uuid)): Path<(String, String)>,
) -> ApiResult<StatusCode> {
    let ctx = resolve(&state, &team, &uuid).await?;
    let repo = EnvVarRepo::new(state.pool.clone());
    let owned = repo
        .list(ENV_RESOURCE_KIND, ctx.app.id)
        .await?
        .into_iter()
        .any(|v| v.uuid == env_uuid);
    if !owned {
        return Err(ApiError::NotFound);
    }
    if repo.delete(&env_uuid).await? {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError::NotFound)
    }
}

#[cfg(test)]
mod tests {
    use super::validate_git;

    #[test]
    fn accepts_supported_git_schemes() {
        assert!(validate_git("https://github.com/x/y.git").is_ok());
        assert!(validate_git("git@github.com:x/y.git").is_ok());
        assert!(validate_git("file:///srv/git/nixpacks-node.git").is_ok());
    }

    #[test]
    fn rejects_unsupported_git_schemes() {
        assert!(validate_git("http://insecure/x.git").is_err());
        assert!(validate_git("ssh://host/x.git").is_err());
        assert!(validate_git("/local/path").is_err());
    }
}
