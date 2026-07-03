//! The API error type and its `{code, message}` envelope (contract C5).

use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use rustify_db::DbError;
use serde::Serialize;
use utoipa::ToSchema;

/// Error envelope returned by every failing endpoint (contract C5).
#[derive(Debug, Serialize, ToSchema)]
pub struct ApiErrorBody {
    /// Stable machine-readable error code.
    pub code: String,
    /// Human-readable message.
    pub message: String,
}

/// All the failure modes the HTTP layer can surface, each mapped to a status
/// code and a stable `code` string.
#[derive(Debug)]
pub enum ApiError {
    /// Missing/invalid session cookie or bearer token.
    Unauthorized,
    /// Authenticated but lacks the required team role/permission (HTTP 403).
    Forbidden(String),
    /// The addressed resource does not exist (or is not in the caller's team).
    NotFound,
    /// The request body/params failed validation (HTTP 422).
    Validation(String),
    /// The request conflicts with current state (HTTP 409).
    Conflict(String),
    /// An unexpected internal error (HTTP 500).
    Internal(String),
}

impl ApiError {
    fn parts(&self) -> (StatusCode, &'static str, String) {
        match self {
            ApiError::Unauthorized => (
                StatusCode::UNAUTHORIZED,
                "unauthorized",
                "authentication required".to_string(),
            ),
            ApiError::Forbidden(m) => (StatusCode::FORBIDDEN, "forbidden", m.clone()),
            ApiError::NotFound => (
                StatusCode::NOT_FOUND,
                "not_found",
                "resource not found".to_string(),
            ),
            ApiError::Validation(m) => (
                StatusCode::UNPROCESSABLE_ENTITY,
                "validation_error",
                m.clone(),
            ),
            ApiError::Conflict(m) => (StatusCode::CONFLICT, "conflict", m.clone()),
            ApiError::Internal(m) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal_error",
                m.clone(),
            ),
        }
    }
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let (_, code, message) = self.parts();
        write!(f, "{code}: {message}")
    }
}

impl std::error::Error for ApiError {}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, code, message) = self.parts();
        if status == StatusCode::INTERNAL_SERVER_ERROR {
            tracing::error!(error = %message, "request failed with internal error");
        }
        (
            status,
            Json(ApiErrorBody {
                code: code.to_string(),
                message,
            }),
        )
            .into_response()
    }
}

impl From<DbError> for ApiError {
    fn from(err: DbError) -> Self {
        match &err {
            DbError::NotFound => ApiError::NotFound,
            DbError::Invalid(m) => ApiError::Conflict(m.clone()),
            DbError::Sqlx(e) => {
                if e.as_database_error()
                    .is_some_and(|d| d.is_unique_violation())
                {
                    ApiError::Conflict("resource already exists".to_string())
                } else {
                    ApiError::Internal(err.to_string())
                }
            }
            _ => ApiError::Internal(err.to_string()),
        }
    }
}

/// Result alias for handlers.
pub type ApiResult<T> = Result<T, ApiError>;
