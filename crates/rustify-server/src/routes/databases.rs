//! Standalone-databases routes: CRUD + start/stop/restart lifecycle.
//!
//! Credentials are generated server-side on create ([`DatabaseEngine::
//! default_credentials`]), encrypted at rest, and never returned in responses.
//! Lifecycle endpoints enqueue `database_start` / `database_stop` jobs handled
//! by `rustify-deploy`.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::{Deserialize, Serialize};
use serde_json::json;
use utoipa::ToSchema;

use rustify_core::DatabaseEngine;
use rustify_db::repos::{
    DatabasePatch, DatabaseRepo, NewDatabase, ProjectRepo, Server, ServerRepo, StandaloneDatabase,
};

use crate::app::AppState;
use crate::auth::CurrentTeam;
use crate::error::{ApiError, ApiResult};

// ----- DTOs ---------------------------------------------------------------

#[derive(Debug, Serialize, ToSchema)]
pub struct DatabaseDto {
    pub uuid: String,
    pub name: String,
    pub description: Option<String>,
    pub engine: String,
    pub image: String,
    pub status: String,
    pub environment_uuid: String,
    pub project_uuid: String,
    pub server_uuid: String,
    pub is_public: bool,
    pub public_port: Option<i32>,
    pub public_port_timeout: i32,
    pub ports_mappings: Option<String>,
    pub limits_memory: String,
    pub limits_cpus: String,
    pub health_check_enabled: bool,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct DatabaseCreate {
    pub project_uuid: String,
    pub environment_name: String,
    pub server_uuid: String,
    /// One of `postgresql`, `mysql`, `mariadb`, `mongodb`, `redis`, `keydb`,
    /// `dragonfly`, `clickhouse`.
    pub engine: String,
    pub name: String,
    /// Overrides the engine's default image when set.
    pub image: Option<String>,
    #[serde(default)]
    pub is_public: bool,
    pub public_port: Option<i32>,
}

#[derive(Debug, Default, Deserialize, ToSchema)]
pub struct DatabaseUpdate {
    pub name: Option<String>,
    pub description: Option<String>,
    pub image: Option<String>,
    pub is_public: Option<bool>,
    pub public_port: Option<i32>,
    pub public_port_timeout: Option<i32>,
    pub ports_mappings: Option<String>,
    pub limits_memory: Option<String>,
    pub limits_cpus: Option<String>,
    pub health_check_enabled: Option<bool>,
}

// ----- Resolution helpers -------------------------------------------------

struct DbContext {
    db: StandaloneDatabase,
    environment_uuid: String,
    project_uuid: String,
    server: Server,
}

/// Resolve a database by uuid and enforce team ownership via its
/// environment → project → team chain.
async fn resolve(state: &AppState, team: &CurrentTeam, uuid: &str) -> ApiResult<DbContext> {
    let db = DatabaseRepo::new(state.pool.clone())
        .get_by_uuid(uuid)
        .await?
        .ok_or(ApiError::NotFound)?;

    let projects = ProjectRepo::new(state.pool.clone());
    let environment = projects
        .environment_by_id(db.environment_id)
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
        .destination_by_id(db.destination_id)
        .await?
        .ok_or(ApiError::NotFound)?;
    let server = servers
        .get_by_id(destination.server_id)
        .await?
        .ok_or(ApiError::NotFound)?;

    Ok(DbContext {
        db,
        environment_uuid: environment.uuid,
        project_uuid: project.uuid,
        server,
    })
}

fn to_dto(ctx: DbContext) -> DatabaseDto {
    let d = ctx.db;
    DatabaseDto {
        uuid: d.uuid,
        name: d.name,
        description: d.description,
        engine: d.engine,
        image: d.image,
        status: d.status,
        environment_uuid: ctx.environment_uuid,
        project_uuid: ctx.project_uuid,
        server_uuid: ctx.server.uuid,
        is_public: d.is_public,
        public_port: d.public_port,
        public_port_timeout: d.public_port_timeout,
        ports_mappings: d.ports_mappings,
        limits_memory: d.limits_memory,
        limits_cpus: d.limits_cpus,
        health_check_enabled: d.health_check_enabled,
        created_at: d.created_at,
        updated_at: d.updated_at,
    }
}

// ----- CRUD ---------------------------------------------------------------

#[utoipa::path(get, path = "/databases", operation_id = "list_databases", tag = "databases",
    responses((status = 200, description = "List of databases", body = [DatabaseDto])))]
pub async fn list(
    State(state): State<AppState>,
    team: CurrentTeam,
) -> ApiResult<Json<Vec<DatabaseDto>>> {
    let project_repo = ProjectRepo::new(state.pool.clone());
    let db_repo = DatabaseRepo::new(state.pool.clone());
    let mut out = Vec::new();
    for project in project_repo.list(team.id).await? {
        for env in project_repo.environments(project.id).await? {
            for db in db_repo.list_by_environment(env.id).await? {
                let ctx = resolve(&state, &team, &db.uuid).await?;
                out.push(to_dto(ctx));
            }
        }
    }
    Ok(Json(out))
}

#[utoipa::path(post, path = "/databases", operation_id = "create_database", tag = "databases",
    request_body = DatabaseCreate,
    responses(
        (status = 201, description = "Database created", body = DatabaseDto),
        (status = 422, description = "Validation error", body = crate::error::ApiErrorBody),
    ))]
pub async fn create(
    State(state): State<AppState>,
    team: CurrentTeam,
    Json(body): Json<DatabaseCreate>,
) -> ApiResult<Response> {
    let engine = DatabaseEngine::parse(&body.engine)
        .ok_or_else(|| ApiError::Validation(format!("unknown engine {}", body.engine)))?;

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

    let image = body
        .image
        .clone()
        .unwrap_or_else(|| engine.descriptor().default_image.to_string());

    let db = DatabaseRepo::new(state.pool.clone())
        .create(NewDatabase {
            environment_id: environment.id,
            destination_id: destination.id,
            name: body.name.clone(),
            engine: engine.as_str().to_string(),
            image,
            credentials: engine.default_credentials(),
            is_public: body.is_public,
            public_port: body.public_port,
        })
        .await?;

    let ctx = resolve(&state, &team, &db.uuid).await?;
    Ok((StatusCode::CREATED, Json(to_dto(ctx))).into_response())
}

#[utoipa::path(get, path = "/databases/{uuid}", operation_id = "get_database", tag = "databases",
    params(("uuid" = String, Path, description = "Database uuid")),
    responses(
        (status = 200, description = "The database", body = DatabaseDto),
        (status = 404, description = "Not found", body = crate::error::ApiErrorBody),
    ))]
pub async fn get(
    State(state): State<AppState>,
    team: CurrentTeam,
    Path(uuid): Path<String>,
) -> ApiResult<Json<DatabaseDto>> {
    let ctx = resolve(&state, &team, &uuid).await?;
    Ok(Json(to_dto(ctx)))
}

#[utoipa::path(patch, path = "/databases/{uuid}", operation_id = "update_database", tag = "databases",
    params(("uuid" = String, Path, description = "Database uuid")),
    request_body = DatabaseUpdate,
    responses(
        (status = 200, description = "Updated database", body = DatabaseDto),
        (status = 404, description = "Not found", body = crate::error::ApiErrorBody),
    ))]
pub async fn update(
    State(state): State<AppState>,
    team: CurrentTeam,
    Path(uuid): Path<String>,
    Json(body): Json<DatabaseUpdate>,
) -> ApiResult<Json<DatabaseDto>> {
    resolve(&state, &team, &uuid).await?;
    let patch = DatabasePatch {
        name: body.name,
        description: body.description,
        image: body.image,
        is_public: body.is_public,
        public_port: body.public_port,
        public_port_timeout: body.public_port_timeout,
        ports_mappings: body.ports_mappings,
        limits_memory: body.limits_memory,
        limits_cpus: body.limits_cpus,
        health_check_enabled: body.health_check_enabled,
    };
    DatabaseRepo::new(state.pool.clone())
        .update(&uuid, &patch)
        .await?
        .ok_or(ApiError::NotFound)?;
    let ctx = resolve(&state, &team, &uuid).await?;
    Ok(Json(to_dto(ctx)))
}

#[utoipa::path(delete, path = "/databases/{uuid}", operation_id = "delete_database", tag = "databases",
    params(("uuid" = String, Path, description = "Database uuid")),
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
    if DatabaseRepo::new(state.pool.clone()).delete(&uuid).await? {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError::NotFound)
    }
}

// ----- Lifecycle ----------------------------------------------------------

async fn lifecycle(
    state: &AppState,
    team: &CurrentTeam,
    uuid: &str,
    kind: &str,
) -> ApiResult<Response> {
    let ctx = resolve(state, team, uuid).await?;
    state
        .queue
        .enqueue(kind, json!({ "database_uuid": ctx.db.uuid }), None)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    Ok((StatusCode::ACCEPTED, Json(json!({ "status": "accepted" }))).into_response())
}

#[utoipa::path(post, path = "/databases/{uuid}/start", operation_id = "start_database", tag = "databases",
    params(("uuid" = String, Path, description = "Database uuid")),
    responses((status = 202, description = "Start enqueued")))]
pub async fn start(
    State(state): State<AppState>,
    team: CurrentTeam,
    Path(uuid): Path<String>,
) -> ApiResult<Response> {
    lifecycle(&state, &team, &uuid, "database_start").await
}

#[utoipa::path(post, path = "/databases/{uuid}/stop", operation_id = "stop_database", tag = "databases",
    params(("uuid" = String, Path, description = "Database uuid")),
    responses((status = 202, description = "Stop enqueued")))]
pub async fn stop(
    State(state): State<AppState>,
    team: CurrentTeam,
    Path(uuid): Path<String>,
) -> ApiResult<Response> {
    lifecycle(&state, &team, &uuid, "database_stop").await
}

#[utoipa::path(post, path = "/databases/{uuid}/restart", operation_id = "restart_database", tag = "databases",
    params(("uuid" = String, Path, description = "Database uuid")),
    responses((status = 202, description = "Restart enqueued")))]
pub async fn restart(
    State(state): State<AppState>,
    team: CurrentTeam,
    Path(uuid): Path<String>,
) -> ApiResult<Response> {
    // A start job rewrites the compose and recreates the container, so restart
    // reuses it (StartPostgresql.php stops + removes the old container first).
    lifecycle(&state, &team, &uuid, "database_start").await
}
