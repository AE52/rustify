//! Private-keys routes (contract C5). Private key material is write-only; the
//! public key is derived on write via `ssh-key` and stored alongside.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::{Deserialize, Serialize};
use ssh_key::private::PrivateKey as SshPrivateKey;
use ssh_key::{Algorithm, LineEnding};
use utoipa::ToSchema;

use rustify_db::repos::{KeyRepo, PrivateKey};

use crate::app::AppState;
use crate::auth::CurrentTeam;
use crate::error::{ApiError, ApiResult};

/// A private key as returned by the API (private material elided).
#[derive(Debug, Serialize, ToSchema)]
pub struct PrivateKeyDto {
    pub uuid: String,
    pub name: String,
    pub public_key: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

impl From<PrivateKey> for PrivateKeyDto {
    fn from(k: PrivateKey) -> Self {
        Self {
            uuid: k.uuid,
            name: k.name,
            public_key: k.public_key,
            created_at: k.created_at,
            updated_at: k.updated_at,
        }
    }
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct PrivateKeyCreate {
    pub name: String,
    /// PEM (OpenSSH) private key; write-only.
    pub private_key: String,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct PrivateKeyGenerate {
    pub name: String,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct PrivateKeyUpdate {
    pub name: Option<String>,
    /// Replacement PEM private key; write-only.
    pub private_key: Option<String>,
}

/// Derive the OpenSSH public key from a PEM private key, rejecting invalid input.
fn derive_public_key(pem: &str) -> ApiResult<String> {
    let key = SshPrivateKey::from_openssh(pem)
        .map_err(|e| ApiError::Validation(format!("invalid private key: {e}")))?;
    key.public_key()
        .to_openssh()
        .map_err(|e| ApiError::Internal(format!("failed to encode public key: {e}")))
}

/// Fetch a key by uuid, enforcing team ownership.
async fn owned(state: &AppState, team: &CurrentTeam, uuid: &str) -> ApiResult<PrivateKey> {
    let key = KeyRepo::new(state.pool.clone())
        .get_by_uuid(uuid)
        .await?
        .ok_or(ApiError::NotFound)?;
    if key.team_id != team.id {
        return Err(ApiError::NotFound);
    }
    Ok(key)
}

#[utoipa::path(
    get,
    path = "/private-keys",
    operation_id = "list_keys",
    tag = "private-keys",
    responses((status = 200, description = "List of private keys", body = [PrivateKeyDto]))
)]
pub async fn list(
    State(state): State<AppState>,
    team: CurrentTeam,
) -> ApiResult<Json<Vec<PrivateKeyDto>>> {
    let keys = KeyRepo::new(state.pool.clone()).list(team.id).await?;
    Ok(Json(keys.into_iter().map(PrivateKeyDto::from).collect()))
}

#[utoipa::path(
    post,
    path = "/private-keys",
    operation_id = "create_key",
    tag = "private-keys",
    request_body = PrivateKeyCreate,
    responses(
        (status = 201, description = "Key created", body = PrivateKeyDto),
        (status = 422, description = "Invalid key material", body = crate::error::ApiErrorBody),
    )
)]
pub async fn create(
    State(state): State<AppState>,
    team: CurrentTeam,
    Json(body): Json<PrivateKeyCreate>,
) -> ApiResult<Response> {
    let public_key = derive_public_key(&body.private_key)?;
    let key = KeyRepo::new(state.pool.clone())
        .create(team.id, &body.name, &body.private_key, &public_key)
        .await?;
    Ok((StatusCode::CREATED, Json(PrivateKeyDto::from(key))).into_response())
}

#[utoipa::path(
    post,
    path = "/private-keys/generate",
    tag = "private-keys",
    request_body = PrivateKeyGenerate,
    responses((status = 201, description = "ed25519 key generated", body = PrivateKeyDto))
)]
pub async fn generate(
    State(state): State<AppState>,
    team: CurrentTeam,
    Json(body): Json<PrivateKeyGenerate>,
) -> ApiResult<Response> {
    let mut rng = rand::rngs::OsRng;
    let generated = SshPrivateKey::random(&mut rng, Algorithm::Ed25519)
        .map_err(|e| ApiError::Internal(format!("key generation failed: {e}")))?;
    let pem = generated
        .to_openssh(LineEnding::LF)
        .map_err(|e| ApiError::Internal(format!("key encoding failed: {e}")))?;
    let public_key = generated
        .public_key()
        .to_openssh()
        .map_err(|e| ApiError::Internal(format!("public key encoding failed: {e}")))?;

    let key = KeyRepo::new(state.pool.clone())
        .create(team.id, &body.name, &pem, &public_key)
        .await?;
    Ok((StatusCode::CREATED, Json(PrivateKeyDto::from(key))).into_response())
}

#[utoipa::path(
    get,
    path = "/private-keys/{uuid}",
    operation_id = "get_key",
    tag = "private-keys",
    params(("uuid" = String, Path, description = "Private key uuid")),
    responses(
        (status = 200, description = "The key", body = PrivateKeyDto),
        (status = 404, description = "Not found", body = crate::error::ApiErrorBody),
    )
)]
pub async fn get(
    State(state): State<AppState>,
    team: CurrentTeam,
    Path(uuid): Path<String>,
) -> ApiResult<Json<PrivateKeyDto>> {
    let key = owned(&state, &team, &uuid).await?;
    Ok(Json(PrivateKeyDto::from(key)))
}

#[utoipa::path(
    patch,
    path = "/private-keys/{uuid}",
    operation_id = "update_key",
    tag = "private-keys",
    params(("uuid" = String, Path, description = "Private key uuid")),
    request_body = PrivateKeyUpdate,
    responses(
        (status = 200, description = "Updated key", body = PrivateKeyDto),
        (status = 404, description = "Not found", body = crate::error::ApiErrorBody),
    )
)]
pub async fn update(
    State(state): State<AppState>,
    team: CurrentTeam,
    Path(uuid): Path<String>,
    Json(body): Json<PrivateKeyUpdate>,
) -> ApiResult<Json<PrivateKeyDto>> {
    owned(&state, &team, &uuid).await?;
    let material = match &body.private_key {
        Some(pem) => Some((pem.as_str(), derive_public_key(pem)?)),
        None => None,
    };
    let material_ref = material.as_ref().map(|(p, k)| (*p, k.as_str()));
    let key = KeyRepo::new(state.pool.clone())
        .update(&uuid, body.name.as_deref(), material_ref)
        .await?
        .ok_or(ApiError::NotFound)?;
    Ok(Json(PrivateKeyDto::from(key)))
}

#[utoipa::path(
    delete,
    path = "/private-keys/{uuid}",
    operation_id = "delete_key",
    tag = "private-keys",
    params(("uuid" = String, Path, description = "Private key uuid")),
    responses(
        (status = 204, description = "Deleted"),
        (status = 404, description = "Not found", body = crate::error::ApiErrorBody),
    )
)]
pub async fn delete(
    State(state): State<AppState>,
    team: CurrentTeam,
    Path(uuid): Path<String>,
) -> ApiResult<StatusCode> {
    owned(&state, &team, &uuid).await?;
    let deleted = KeyRepo::new(state.pool.clone()).delete(&uuid).await?;
    if deleted {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError::NotFound)
    }
}
