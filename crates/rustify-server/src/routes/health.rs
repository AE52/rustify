//! `GET /health` — the only route besides `/auth/login` that needs no auth.

use axum::Json;
use serde::Serialize;
use utoipa::ToSchema;

#[derive(Debug, Serialize, ToSchema)]
pub struct Health {
    /// Always `"ok"`.
    pub status: String,
}

#[utoipa::path(
    get,
    path = "/health",
    tag = "health",
    responses((status = 200, description = "Service is up", body = Health))
)]
pub async fn health() -> Json<Health> {
    Json(Health {
        status: "ok".to_string(),
    })
}
