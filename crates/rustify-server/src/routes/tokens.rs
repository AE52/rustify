//! API-token routes (contract C5). The plaintext token is shown exactly once
//! on creation; only its sha256 hash is stored.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use rustify_db::repos::SettingsRepo;

use crate::app::AppState;
use crate::auth::{CurrentTeam, generate_token, sha256_hex};
use crate::error::{ApiError, ApiResult};

#[derive(Debug, Serialize, ToSchema)]
pub struct ApiTokenDto {
    pub uuid: String,
    pub name: String,
    pub abilities: Vec<String>,
    pub last_used_at: Option<chrono::DateTime<chrono::Utc>>,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct ApiTokenCreate {
    pub name: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ApiTokenCreated {
    pub uuid: String,
    pub name: String,
    /// The plaintext token — shown exactly once.
    pub token: String,
}

#[utoipa::path(get, path = "/api-tokens", operation_id = "list_tokens", tag = "api-tokens",
    responses((status = 200, description = "List of API tokens", body = [ApiTokenDto])))]
pub async fn list(
    State(state): State<AppState>,
    team: CurrentTeam,
) -> ApiResult<Json<Vec<ApiTokenDto>>> {
    let tokens = SettingsRepo::new(state.pool.clone())
        .list_api_tokens(team.id)
        .await?;
    Ok(Json(
        tokens
            .into_iter()
            .map(|t| ApiTokenDto {
                uuid: t.uuid,
                name: t.name,
                abilities: t.abilities,
                last_used_at: t.last_used_at,
                created_at: t.created_at,
            })
            .collect(),
    ))
}

#[utoipa::path(post, path = "/api-tokens", operation_id = "create_token", tag = "api-tokens",
    request_body = ApiTokenCreate,
    responses((status = 201, description = "Token created (shown once)", body = ApiTokenCreated)))]
pub async fn create(
    State(state): State<AppState>,
    team: CurrentTeam,
    Json(body): Json<ApiTokenCreate>,
) -> ApiResult<Response> {
    let raw = generate_token();
    let hash = sha256_hex(&raw);
    let token = SettingsRepo::new(state.pool.clone())
        .create_api_token(team.id, &body.name, &hash)
        .await?;
    Ok((
        StatusCode::CREATED,
        Json(ApiTokenCreated {
            uuid: token.uuid,
            name: token.name,
            token: raw,
        }),
    )
        .into_response())
}

#[utoipa::path(delete, path = "/api-tokens/{uuid}", operation_id = "delete_token", tag = "api-tokens",
    params(("uuid" = String, Path, description = "API token uuid")),
    responses(
        (status = 204, description = "Deleted"),
        (status = 404, description = "Not found", body = crate::error::ApiErrorBody),
    ))]
pub async fn delete(
    State(state): State<AppState>,
    team: CurrentTeam,
    Path(uuid): Path<String>,
) -> ApiResult<StatusCode> {
    let repo = SettingsRepo::new(state.pool.clone());
    // Enforce team ownership before deleting.
    let owned = repo
        .list_api_tokens(team.id)
        .await?
        .into_iter()
        .any(|t| t.uuid == uuid);
    if !owned {
        return Err(ApiError::NotFound);
    }
    if repo.delete_api_token(&uuid).await? {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError::NotFound)
    }
}
