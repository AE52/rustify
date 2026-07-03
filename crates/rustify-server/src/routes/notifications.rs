//! Notification-settings routes (contract C5).
//!
//! One settings row per team. Channel secrets are **write-only**: accepted on
//! `PATCH`, encrypted at rest by [`NotificationsRepo`], and never returned — the
//! read DTO exposes only `*_configured` presence flags. `POST /notifications/test`
//! sends a one-off test message to a single channel.

use axum::Json;
use axum::extract::State;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use utoipa::ToSchema;

use rustify_core::notify::Channel;
use rustify_db::repos::{NotificationSettings, NotificationSettingsPatch, NotificationsRepo};

use crate::app::AppState;
use crate::auth::CurrentTeam;
use crate::error::{ApiError, ApiResult};
use crate::notify;

/// Read view: channel toggles, non-secret config, secret presence flags, and the
/// event matrix. No secret value is ever included.
#[derive(Debug, Serialize, ToSchema)]
pub struct NotificationSettingsDto {
    pub email_enabled: bool,
    pub smtp_host_configured: bool,
    pub smtp_port: Option<i32>,
    pub smtp_encryption: Option<String>,
    pub smtp_username_configured: bool,
    pub smtp_password_configured: bool,
    pub smtp_from_address: Option<String>,
    pub smtp_from_name: Option<String>,
    pub smtp_recipients: Option<String>,
    pub resend_enabled: bool,
    pub resend_api_key_configured: bool,

    pub discord_enabled: bool,
    pub discord_webhook_url_configured: bool,
    pub discord_ping_enabled: bool,

    pub telegram_enabled: bool,
    pub telegram_token_configured: bool,
    pub telegram_chat_id_configured: bool,

    pub slack_enabled: bool,
    pub slack_webhook_url_configured: bool,

    pub pushover_enabled: bool,
    pub pushover_user_key_configured: bool,
    pub pushover_api_token_configured: bool,

    pub webhook_enabled: bool,
    pub webhook_url_configured: bool,

    pub event_matrix: Value,
}

fn present(v: &Option<String>) -> bool {
    v.as_deref().is_some_and(|s| !s.is_empty())
}

impl From<NotificationSettings> for NotificationSettingsDto {
    fn from(s: NotificationSettings) -> Self {
        Self {
            email_enabled: s.email_enabled,
            smtp_host_configured: present(&s.smtp_host),
            smtp_port: s.smtp_port,
            smtp_encryption: s.smtp_encryption,
            smtp_username_configured: present(&s.smtp_username),
            smtp_password_configured: present(&s.smtp_password),
            smtp_from_address: s.smtp_from_address,
            smtp_from_name: s.smtp_from_name,
            smtp_recipients: s.smtp_recipients,
            resend_enabled: s.resend_enabled,
            resend_api_key_configured: present(&s.resend_api_key),
            discord_enabled: s.discord_enabled,
            discord_webhook_url_configured: present(&s.discord_webhook_url),
            discord_ping_enabled: s.discord_ping_enabled,
            telegram_enabled: s.telegram_enabled,
            telegram_token_configured: present(&s.telegram_token),
            telegram_chat_id_configured: present(&s.telegram_chat_id),
            slack_enabled: s.slack_enabled,
            slack_webhook_url_configured: present(&s.slack_webhook_url),
            pushover_enabled: s.pushover_enabled,
            pushover_user_key_configured: present(&s.pushover_user_key),
            pushover_api_token_configured: present(&s.pushover_api_token),
            webhook_enabled: s.webhook_enabled,
            webhook_url_configured: present(&s.webhook_url),
            event_matrix: s.event_matrix,
        }
    }
}

/// Partial update. Secret fields are write-only: `null`/absent leaves them
/// unchanged, `""` clears them, and any other value (re-)encrypts and stores it.
#[derive(Debug, Default, Deserialize, ToSchema)]
pub struct NotificationSettingsUpdate {
    pub email_enabled: Option<bool>,
    pub smtp_host: Option<String>,
    pub smtp_port: Option<i32>,
    pub smtp_encryption: Option<String>,
    pub smtp_username: Option<String>,
    pub smtp_password: Option<String>,
    pub smtp_from_address: Option<String>,
    pub smtp_from_name: Option<String>,
    pub smtp_recipients: Option<String>,
    pub resend_enabled: Option<bool>,
    pub resend_api_key: Option<String>,

    pub discord_enabled: Option<bool>,
    pub discord_webhook_url: Option<String>,
    pub discord_ping_enabled: Option<bool>,

    pub telegram_enabled: Option<bool>,
    pub telegram_token: Option<String>,
    pub telegram_chat_id: Option<String>,

    pub slack_enabled: Option<bool>,
    pub slack_webhook_url: Option<String>,

    pub pushover_enabled: Option<bool>,
    pub pushover_user_key: Option<String>,
    pub pushover_api_token: Option<String>,

    pub webhook_enabled: Option<bool>,
    pub webhook_url: Option<String>,

    pub event_matrix: Option<Value>,
}

impl From<NotificationSettingsUpdate> for NotificationSettingsPatch {
    fn from(u: NotificationSettingsUpdate) -> Self {
        Self {
            email_enabled: u.email_enabled,
            smtp_host: u.smtp_host,
            smtp_port: u.smtp_port,
            smtp_encryption: u.smtp_encryption,
            smtp_username: u.smtp_username,
            smtp_password: u.smtp_password,
            smtp_from_address: u.smtp_from_address,
            smtp_from_name: u.smtp_from_name,
            smtp_recipients: u.smtp_recipients,
            resend_enabled: u.resend_enabled,
            resend_api_key: u.resend_api_key,
            discord_enabled: u.discord_enabled,
            discord_webhook_url: u.discord_webhook_url,
            discord_ping_enabled: u.discord_ping_enabled,
            telegram_enabled: u.telegram_enabled,
            telegram_token: u.telegram_token,
            telegram_chat_id: u.telegram_chat_id,
            slack_enabled: u.slack_enabled,
            slack_webhook_url: u.slack_webhook_url,
            pushover_enabled: u.pushover_enabled,
            pushover_user_key: u.pushover_user_key,
            pushover_api_token: u.pushover_api_token,
            webhook_enabled: u.webhook_enabled,
            webhook_url: u.webhook_url,
            event_matrix: u.event_matrix,
        }
    }
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct TestRequest {
    /// One of `email`, `discord`, `telegram`, `slack`, `pushover`, `webhook`.
    pub channel: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct TestResponse {
    pub sent: bool,
    pub message: String,
}

#[utoipa::path(get, path = "/notifications/settings", operation_id = "get_notification_settings",
    tag = "notifications",
    responses((status = 200, description = "Notification settings", body = NotificationSettingsDto)))]
pub async fn get(
    State(state): State<AppState>,
    team: CurrentTeam,
) -> ApiResult<Json<NotificationSettingsDto>> {
    let settings = NotificationsRepo::new(state.pool.clone())
        .ensure(team.id)
        .await?;
    Ok(Json(settings.into()))
}

#[utoipa::path(patch, path = "/notifications/settings", operation_id = "update_notification_settings",
    tag = "notifications", request_body = NotificationSettingsUpdate,
    responses((status = 200, description = "Updated settings", body = NotificationSettingsDto)))]
pub async fn update(
    State(state): State<AppState>,
    team: CurrentTeam,
    Json(body): Json<NotificationSettingsUpdate>,
) -> ApiResult<Json<NotificationSettingsDto>> {
    let updated = NotificationsRepo::new(state.pool.clone())
        .upsert(team.id, body.into())
        .await?;
    Ok(Json(updated.into()))
}

#[utoipa::path(post, path = "/notifications/test", operation_id = "test_notification",
    tag = "notifications", request_body = TestRequest,
    responses((status = 200, description = "Test delivery result", body = TestResponse)))]
pub async fn test(
    State(state): State<AppState>,
    team: CurrentTeam,
    Json(body): Json<TestRequest>,
) -> ApiResult<Json<TestResponse>> {
    let channel = Channel::parse(&body.channel)
        .ok_or_else(|| ApiError::Validation(format!("unknown channel: {}", body.channel)))?;
    match notify::send_test(&state.pool, team.id, channel).await {
        Ok(()) => Ok(Json(TestResponse {
            sent: true,
            message: "test notification sent".into(),
        })),
        Err(e) => Ok(Json(TestResponse {
            sent: false,
            message: e,
        })),
    }
}
