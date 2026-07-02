//! Authentication routes: login (sets session cookie), logout, and the
//! current-user probe.

use axum::Json;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use rustify_db::repos::{SettingsRepo, TeamRepo, UserRepo, users};

use crate::app::AppState;
use crate::auth::{
    CurrentUser, SESSION_COOKIE, SESSION_TTL_DAYS, clear_cookie, generate_token, read_cookie,
    session_cookie,
};
use crate::error::{ApiError, ApiResult};

/// A user as returned by the API (contract C5). `id` is the external uuid;
/// `team_uuid` is included so the web client can address the `team:<uuid>` WS
/// channel without a second round-trip.
#[derive(Debug, Serialize, ToSchema)]
pub struct UserDto {
    pub id: String,
    pub email: String,
    pub name: String,
    pub team_uuid: String,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct LoginRequest {
    #[schema(format = "email")]
    pub email: String,
    pub password: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct LoginResponse {
    pub user: UserDto,
}

async fn user_dto(state: &AppState, user: &rustify_db::repos::User) -> ApiResult<UserDto> {
    let team = TeamRepo::new(state.pool.clone())
        .get_by_id(user.team_id)
        .await?
        .ok_or_else(|| ApiError::Internal("user has no team".into()))?;
    Ok(UserDto {
        id: user.uuid.clone(),
        email: user.email.clone(),
        name: user.name.clone(),
        team_uuid: team.uuid,
    })
}

#[utoipa::path(
    post,
    path = "/auth/login",
    tag = "auth",
    request_body = LoginRequest,
    responses(
        (status = 200, description = "Logged in; session cookie set", body = LoginResponse),
        (status = 401, description = "Invalid credentials", body = crate::error::ApiErrorBody),
    )
)]
pub async fn login(
    State(state): State<AppState>,
    Json(body): Json<LoginRequest>,
) -> ApiResult<Response> {
    let user = UserRepo::new(state.pool.clone())
        .find_by_email(&body.email)
        .await?
        .ok_or(ApiError::Unauthorized)?;

    if !users::verify_password(&body.password, &user.password_hash) {
        return Err(ApiError::Unauthorized);
    }

    // Opaque session token = the sessions.id primary key.
    let token = generate_token();
    let expires_at = chrono::Utc::now() + chrono::Duration::days(SESSION_TTL_DAYS);
    SettingsRepo::new(state.pool.clone())
        .create_session(&token, user.id, expires_at)
        .await?;

    let dto = user_dto(&state, &user).await?;
    let cookie = session_cookie(&token, state.config.cookie_secure);
    Ok((
        StatusCode::OK,
        [(header::SET_COOKIE, cookie)],
        Json(LoginResponse { user: dto }),
    )
        .into_response())
}

#[utoipa::path(
    post,
    path = "/auth/logout",
    tag = "auth",
    responses((status = 204, description = "Logged out"))
)]
pub async fn logout(State(state): State<AppState>, headers: HeaderMap) -> ApiResult<Response> {
    if let Some(token) = read_cookie(&headers, SESSION_COOKIE) {
        SettingsRepo::new(state.pool.clone())
            .delete_session(&token)
            .await?;
    }
    let cookie = clear_cookie(state.config.cookie_secure);
    Ok((StatusCode::NO_CONTENT, [(header::SET_COOKIE, cookie)]).into_response())
}

#[utoipa::path(
    get,
    path = "/auth/me",
    tag = "auth",
    responses(
        (status = 200, description = "The authenticated user", body = UserDto),
        (status = 401, description = "Not authenticated", body = crate::error::ApiErrorBody),
    )
)]
pub async fn me(State(state): State<AppState>, user: CurrentUser) -> ApiResult<Json<UserDto>> {
    let team = TeamRepo::new(state.pool.clone())
        .get_by_id(user.team_id)
        .await?
        .ok_or_else(|| ApiError::Internal("user has no team".into()))?;
    Ok(Json(UserDto {
        id: user.uuid,
        email: user.email,
        name: user.name,
        team_uuid: team.uuid,
    }))
}
