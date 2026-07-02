//! Shared harness for the server integration tests: a fixed encryption key, an
//! `AppState` over the test pool, a seeded admin user, and request helpers over
//! `tower::ServiceExt::oneshot`.
#![allow(dead_code)]

use std::sync::Once;

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use base64::Engine as _;
use serde_json::Value;
use sqlx::PgPool;
use tokio::sync::broadcast;
use tower::ServiceExt;

use rustify_db::repos::{TeamRepo, UserRepo};
use rustify_jobs::JobQueue;
use rustify_server::app::{AppState, Config};

static KEY_INIT: Once = Once::new();

/// Install a fixed 32-byte base64 `RUSTIFY_SECRET_KEY` for `rustify_core::crypto`.
pub fn init_secret_key() {
    KEY_INIT.call_once(|| {
        let key = base64::engine::general_purpose::STANDARD.encode([9u8; 32]);
        // SAFETY: set exactly once via `Once`, with a constant value, before any
        // crypto call in the test binary; nothing else mutates the environment.
        unsafe {
            std::env::set_var("RUSTIFY_SECRET_KEY", key);
        }
    });
}

/// Build an `AppState` (and keep the event sender) over the test pool.
pub fn state(pool: PgPool) -> AppState {
    init_secret_key();
    let (events, _rx) = broadcast::channel(1024);
    AppState {
        pool: pool.clone(),
        queue: JobQueue::new(pool),
        events,
        config: Config::for_test(),
    }
}

pub const ADMIN_EMAIL: &str = "admin@test.local";
pub const ADMIN_PASSWORD: &str = "correct horse battery";

/// Seed a team and admin user; returns `(team_id, user_uuid)`.
pub async fn seed_user(pool: &PgPool) -> (i64, String) {
    let team = TeamRepo::new(pool.clone()).create("root").await.unwrap();
    let user = UserRepo::new(pool.clone())
        .create(team.id, ADMIN_EMAIL, "Admin", ADMIN_PASSWORD)
        .await
        .unwrap();
    (team.id, user.uuid)
}

/// Perform a request against the router, returning the status and parsed JSON
/// body (or `Value::Null` for empty bodies).
pub async fn send(app: &Router, req: Request<Body>) -> (StatusCode, Value) {
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let json = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    (status, json)
}

/// Same as [`send`], but also returns the response headers (for `Set-Cookie`).
pub async fn send_full(
    app: &Router,
    req: Request<Body>,
) -> (StatusCode, axum::http::HeaderMap, Value) {
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let headers = resp.headers().clone();
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let json = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    (status, headers, json)
}

/// A request builder for the tests.
pub struct Req {
    method: &'static str,
    path: String,
    cookie: Option<String>,
    bearer: Option<String>,
    body: Option<Value>,
}

impl Req {
    pub fn new(method: &'static str, path: impl Into<String>) -> Self {
        Self {
            method,
            path: path.into(),
            cookie: None,
            bearer: None,
            body: None,
        }
    }
    pub fn get(path: impl Into<String>) -> Self {
        Self::new("GET", path)
    }
    pub fn post(path: impl Into<String>) -> Self {
        Self::new("POST", path)
    }
    pub fn patch(path: impl Into<String>) -> Self {
        Self::new("PATCH", path)
    }
    pub fn delete(path: impl Into<String>) -> Self {
        Self::new("DELETE", path)
    }
    pub fn cookie(mut self, cookie: impl Into<String>) -> Self {
        self.cookie = Some(cookie.into());
        self
    }
    pub fn bearer(mut self, token: impl Into<String>) -> Self {
        self.bearer = Some(token.into());
        self
    }
    pub fn json(mut self, body: Value) -> Self {
        self.body = Some(body);
        self
    }
    pub fn build(self) -> Request<Body> {
        let mut builder = Request::builder().method(self.method).uri(self.path);
        if let Some(cookie) = self.cookie {
            builder = builder.header(header::COOKIE, cookie);
        }
        if let Some(token) = self.bearer {
            builder = builder.header(header::AUTHORIZATION, format!("Bearer {token}"));
        }
        match self.body {
            Some(body) => builder
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(serde_json::to_vec(&body).unwrap()))
                .unwrap(),
            None => builder.body(Body::empty()).unwrap(),
        }
    }
}

/// Log in over the API and return the `rustify_session=<token>` cookie value to
/// replay on subsequent requests.
pub async fn login(app: &Router) -> String {
    let (status, headers, _) = send_full(
        app,
        Req::post("/api/v1/auth/login")
            .json(serde_json::json!({ "email": ADMIN_EMAIL, "password": ADMIN_PASSWORD }))
            .build(),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "login should succeed");
    let raw = headers
        .get(header::SET_COOKIE)
        .expect("login sets a session cookie")
        .to_str()
        .unwrap();
    // Return just the `name=value` portion.
    raw.split(';').next().unwrap().to_string()
}
