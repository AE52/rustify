#![forbid(unsafe_code)]

//! rustify-server: the HTTP/WS API for Rustify.
//!
//! Produces the contract C5 REST surface (utoipa-documented, served with
//! Swagger UI), the contract C4 WebSocket fan-out, session + bearer
//! authentication, and the embedded web SPA. `main.rs` wires the pool,
//! migrations, seed, event bus, job workers and axum server together.

pub mod app;
pub mod auth;
pub mod aws;
pub mod embed;
pub mod error;
pub mod hetzner;
pub mod notify;
pub mod routes;
pub mod terminal;
pub mod ws;

pub use app::{ApiDoc, AppState, Config, build_router};
pub use error::{ApiError, ApiErrorBody, ApiResult};
