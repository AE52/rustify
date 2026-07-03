//! S3 storage routes: CRUD + a connectivity `test`.
//!
//! Access key + secret are write-only: accepted on create/update, encrypted at
//! rest, and never returned. The `test` endpoint performs a lightweight
//! credential/endpoint validation (a live `mc` round-trip requires a server
//! executor, which an S3 storage is not bound to) and records `is_usable`.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use rustify_db::repos::{NewS3Storage, S3Storage, S3StoragePatch, S3StorageRepo};

use crate::app::AppState;
use crate::auth::{CurrentTeam, RequireAdmin};
use crate::error::{ApiError, ApiResult};

#[derive(Debug, Serialize, ToSchema)]
pub struct S3StorageDto {
    pub uuid: String,
    pub name: String,
    pub region: String,
    pub endpoint: Option<String>,
    pub bucket: String,
    pub path: String,
    pub use_path_style: bool,
    pub is_usable: bool,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

impl From<S3Storage> for S3StorageDto {
    fn from(s: S3Storage) -> Self {
        Self {
            uuid: s.uuid,
            name: s.name,
            region: s.region,
            endpoint: s.endpoint,
            bucket: s.bucket,
            path: s.path,
            use_path_style: s.use_path_style,
            is_usable: s.is_usable,
            created_at: s.created_at,
            updated_at: s.updated_at,
        }
    }
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct S3StorageCreate {
    pub name: String,
    pub region: Option<String>,
    pub endpoint: Option<String>,
    pub bucket: String,
    pub key: String,
    pub secret: String,
    pub path: Option<String>,
    pub use_path_style: Option<bool>,
}

#[derive(Debug, Default, Deserialize, ToSchema)]
pub struct S3StorageUpdate {
    pub name: Option<String>,
    pub region: Option<String>,
    pub endpoint: Option<String>,
    pub bucket: Option<String>,
    pub key: Option<String>,
    pub secret: Option<String>,
    pub path: Option<String>,
    pub use_path_style: Option<bool>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct S3TestResponse {
    pub usable: bool,
    pub message: String,
}

/// Fetch a storage and enforce team ownership.
async fn owned(state: &AppState, team: &CurrentTeam, uuid: &str) -> ApiResult<S3Storage> {
    let s3 = S3StorageRepo::new(state.pool.clone())
        .get_by_uuid(uuid)
        .await?
        .ok_or(ApiError::NotFound)?;
    if s3.team_id != team.id {
        return Err(ApiError::NotFound);
    }
    Ok(s3)
}

#[utoipa::path(get, path = "/s3-storages", operation_id = "list_s3_storages", tag = "s3-storages",
    responses((status = 200, description = "List of S3 storages", body = [S3StorageDto])))]
pub async fn list(
    State(state): State<AppState>,
    team: CurrentTeam,
) -> ApiResult<Json<Vec<S3StorageDto>>> {
    let rows = S3StorageRepo::new(state.pool.clone())
        .list_by_team(team.id)
        .await?;
    Ok(Json(rows.into_iter().map(Into::into).collect()))
}

#[utoipa::path(post, path = "/s3-storages", operation_id = "create_s3_storage", tag = "s3-storages",
    request_body = S3StorageCreate,
    responses((status = 201, description = "Created", body = S3StorageDto)))]
pub async fn create(
    State(state): State<AppState>,
    team: CurrentTeam,
    _guard: RequireAdmin,
    Json(body): Json<S3StorageCreate>,
) -> ApiResult<Response> {
    if body.bucket.trim().is_empty() {
        return Err(ApiError::Validation("bucket is required".into()));
    }
    let s3 = S3StorageRepo::new(state.pool.clone())
        .create(NewS3Storage {
            team_id: team.id,
            name: body.name,
            region: body.region.unwrap_or_else(|| "us-east-1".into()),
            endpoint: body.endpoint,
            bucket: body.bucket,
            key: body.key,
            secret: body.secret,
            path: body.path.unwrap_or_else(|| "/".into()),
            use_path_style: body.use_path_style.unwrap_or(true),
        })
        .await?;
    Ok((StatusCode::CREATED, Json(S3StorageDto::from(s3))).into_response())
}

#[utoipa::path(get, path = "/s3-storages/{uuid}", operation_id = "get_s3_storage", tag = "s3-storages",
    params(("uuid" = String, Path, description = "S3 storage uuid")),
    responses((status = 200, description = "The storage", body = S3StorageDto),
        (status = 404, description = "Not found", body = crate::error::ApiErrorBody)))]
pub async fn get(
    State(state): State<AppState>,
    team: CurrentTeam,
    Path(uuid): Path<String>,
) -> ApiResult<Json<S3StorageDto>> {
    Ok(Json(owned(&state, &team, &uuid).await?.into()))
}

#[utoipa::path(patch, path = "/s3-storages/{uuid}", operation_id = "update_s3_storage", tag = "s3-storages",
    params(("uuid" = String, Path, description = "S3 storage uuid")),
    request_body = S3StorageUpdate,
    responses((status = 200, description = "Updated", body = S3StorageDto),
        (status = 404, description = "Not found", body = crate::error::ApiErrorBody)))]
pub async fn update(
    State(state): State<AppState>,
    team: CurrentTeam,
    _guard: RequireAdmin,
    Path(uuid): Path<String>,
    Json(body): Json<S3StorageUpdate>,
) -> ApiResult<Json<S3StorageDto>> {
    owned(&state, &team, &uuid).await?;
    let updated = S3StorageRepo::new(state.pool.clone())
        .update(
            &uuid,
            &S3StoragePatch {
                name: body.name,
                region: body.region,
                endpoint: body.endpoint,
                bucket: body.bucket,
                key: body.key,
                secret: body.secret,
                path: body.path,
                use_path_style: body.use_path_style,
            },
        )
        .await?
        .ok_or(ApiError::NotFound)?;
    Ok(Json(updated.into()))
}

#[utoipa::path(delete, path = "/s3-storages/{uuid}", operation_id = "delete_s3_storage", tag = "s3-storages",
    params(("uuid" = String, Path, description = "S3 storage uuid")),
    responses((status = 204, description = "Deleted"),
        (status = 404, description = "Not found", body = crate::error::ApiErrorBody)))]
pub async fn delete(
    State(state): State<AppState>,
    team: CurrentTeam,
    _guard: RequireAdmin,
    Path(uuid): Path<String>,
) -> ApiResult<StatusCode> {
    owned(&state, &team, &uuid).await?;
    if S3StorageRepo::new(state.pool.clone()).delete(&uuid).await? {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError::NotFound)
    }
}

#[utoipa::path(post, path = "/s3-storages/{uuid}/test", operation_id = "test_s3_storage", tag = "s3-storages",
    params(("uuid" = String, Path, description = "S3 storage uuid")),
    responses((status = 200, description = "Connectivity result", body = S3TestResponse)))]
pub async fn test(
    State(state): State<AppState>,
    team: CurrentTeam,
    Path(uuid): Path<String>,
) -> ApiResult<Json<S3TestResponse>> {
    let s3 = owned(&state, &team, &uuid).await?;
    let repo = S3StorageRepo::new(state.pool.clone());
    let (usable, message) = validate(&s3, &repo).await;
    repo.set_usable(s3.id, usable).await?;
    Ok(Json(S3TestResponse { usable, message }))
}

/// Lightweight validation: endpoint is an http(s) URL, bucket is set, and the
/// stored credentials decrypt to non-empty values.
async fn validate(s3: &S3Storage, repo: &S3StorageRepo) -> (bool, String) {
    let Some(endpoint) = s3.endpoint.as_deref().filter(|e| !e.trim().is_empty()) else {
        return (false, "endpoint is required".into());
    };
    if !(endpoint.starts_with("http://") || endpoint.starts_with("https://")) {
        return (false, "endpoint must be an http(s) URL".into());
    }
    if s3.bucket.trim().is_empty() {
        return (false, "bucket is required".into());
    }
    match repo.decrypt_credentials(s3.id).await {
        Ok(c) if !c.key.is_empty() && !c.secret.is_empty() => {
            (true, "configuration looks valid".into())
        }
        Ok(_) => (false, "access key and secret are required".into()),
        Err(_) => (false, "stored credentials could not be read".into()),
    }
}
