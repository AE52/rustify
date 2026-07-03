//! Decoupled notification delivery.
//!
//! A single [`subscriber`] task rides the existing `broadcast::Sender<WsEvent>`
//! event bus (contract C4) and, for terminal resource events, resolves the
//! owning team and fans the notification out to every configured channel whose
//! `event_matrix` opts the (event, channel) pair in — so the deploy/backup/task
//! handlers stay untouched. Per-channel payloads live in the sibling modules
//! (`discord`, `telegram`, `slack`, `pushover`, `webhook`, `email`); routing is
//! [`rustify_core::notify::should_send`].

pub mod discord;
pub mod email;
pub mod pushover;
pub mod slack;
pub mod telegram;
pub mod webhook;

use std::sync::Arc;

use serde_json::{Map, Value};
use sqlx::PgPool;
use tokio::sync::broadcast;

use rustify_core::WsEvent;
use rustify_core::notify::{Channel, NotifEvent, should_send};
use rustify_db::repos::{NotificationSettings, NotificationsRepo};

use self::email::EmailDelivery;

/// Severity of a notification; drives per-channel color/icon choices.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    Success,
    Warning,
    Error,
    Info,
}

impl Level {
    pub fn as_str(self) -> &'static str {
        match self {
            Level::Success => "success",
            Level::Warning => "warning",
            Level::Error => "error",
            Level::Info => "info",
        }
    }
}

/// A channel-agnostic notification, rendered per channel by the sender modules.
#[derive(Debug, Clone)]
pub struct NotifPayload {
    /// Event slug (matches [`NotifEvent::as_str`]); emitted in the webhook body.
    pub event_slug: &'static str,
    pub level: Level,
    pub title: String,
    pub description: String,
    /// Whether the event warrants a Discord `@here` ping (when pinging is on).
    pub critical: bool,
    /// Discord embed fields (`name`, `value`).
    pub fields: Vec<(String, String)>,
    /// Structured fields merged into the generic webhook body.
    pub extra: Map<String, Value>,
}

impl NotifPayload {
    pub fn new(
        event_slug: &'static str,
        level: Level,
        title: impl Into<String>,
        description: impl Into<String>,
    ) -> Self {
        Self {
            event_slug,
            level,
            title: title.into(),
            description: description.into(),
            critical: false,
            fields: Vec::new(),
            extra: Map::new(),
        }
    }

    /// Mark as critical (enables the Discord `@here` ping).
    pub fn critical(mut self) -> Self {
        self.critical = true;
        self
    }

    /// Add a display field (shown in the Discord embed).
    pub fn field(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.fields.push((name.into(), value.into()));
        self
    }

    /// Add a structured field to the webhook body.
    pub fn extra(mut self, key: impl Into<String>, value: Value) -> Self {
        self.extra.insert(key.into(), value);
        self
    }
}

/// The delivery seam: real HTTP/SMTP in production, a recording fake in tests.
#[async_trait::async_trait]
pub trait Sender: Send + Sync {
    /// POST a JSON body to a webhook-style channel endpoint.
    async fn post_json(&self, channel: Channel, url: &str, body: Value) -> Result<(), String>;
    /// Deliver an email (Resend or SMTP).
    async fn send_email(&self, delivery: EmailDelivery) -> Result<(), String>;
}

/// Production [`Sender`]: a shared `reqwest` client for webhooks + Resend, and
/// `lettre` for SMTP (inside [`email::deliver`]).
pub struct ReqwestSender {
    client: reqwest::Client,
}

impl ReqwestSender {
    pub fn new() -> Self {
        // Disable redirect-following: `webhook::is_safe_url` validates the
        // *initial* target's resolved IPs, but reqwest would otherwise follow up
        // to 10 redirects to an unvalidated (possibly internal) host. With
        // `Policy::none()` a 30x is returned verbatim and never chased (SSRF
        // defense-in-depth alongside the resolve-and-block URL guard).
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .unwrap_or_default();
        Self { client }
    }
}

impl Default for ReqwestSender {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl Sender for ReqwestSender {
    async fn post_json(&self, _channel: Channel, url: &str, body: Value) -> Result<(), String> {
        let resp = self
            .client
            .post(url)
            .json(&body)
            .send()
            .await
            .map_err(|e| e.to_string())?;
        if resp.status().is_success() {
            Ok(())
        } else {
            Err(format!("endpoint returned status {}", resp.status()))
        }
    }

    async fn send_email(&self, delivery: EmailDelivery) -> Result<(), String> {
        email::deliver(&self.client, &delivery).await
    }
}

/// Trim to `None` for empty/whitespace-only secrets.
fn nonempty(v: &Option<String>) -> Option<String> {
    v.as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(String::from)
}

/// Spawn a fire-and-forget POST; failures are logged, never propagated.
fn spawn_post<S: Sender + 'static>(sender: Arc<S>, channel: Channel, url: String, body: Value) {
    tokio::spawn(async move {
        if let Err(e) = sender.post_json(channel, &url, body).await {
            tracing::warn!(channel = channel.as_str(), error = %e, "notification delivery failed");
        }
    });
}

/// Build an [`EmailDelivery`] from the team's settings + payload, or `None` when
/// no transport/recipients are configured.
fn build_email_delivery(
    settings: &NotificationSettings,
    payload: &NotifPayload,
) -> Option<EmailDelivery> {
    let recipients: Vec<String> = settings
        .smtp_recipients
        .as_deref()
        .unwrap_or("")
        .split([',', ';', ' ', '\n'])
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(String::from)
        .collect();
    if recipients.is_empty() {
        return None;
    }
    Some(EmailDelivery {
        resend_enabled: settings.resend_enabled,
        resend_api_key: nonempty(&settings.resend_api_key),
        smtp_host: nonempty(&settings.smtp_host),
        smtp_port: settings.smtp_port,
        smtp_encryption: settings.smtp_encryption.clone(),
        smtp_username: nonempty(&settings.smtp_username),
        smtp_password: settings.smtp_password.clone(),
        from_address: nonempty(&settings.smtp_from_address),
        from_name: nonempty(&settings.smtp_from_name),
        recipients,
        subject: payload.title.clone(),
        html: format!("<p>{}</p>", html_escape(&payload.description)),
    })
}

/// Minimal HTML escaping for the email body.
fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// Load a team's settings and deliver `event`/`payload` to every configured
/// channel the matrix opts in. Email is synchronous; the other channels are
/// fire-and-forget. Failures are logged, never panicked. A missing settings row
/// or load error is a silent no-op.
pub async fn notify(pool: &PgPool, team_id: i64, event: NotifEvent, payload: NotifPayload) {
    notify_with(
        Arc::new(ReqwestSender::new()),
        pool,
        team_id,
        event,
        payload,
    )
    .await;
}

/// [`notify`] over an injected [`Sender`] (the test seam).
pub async fn notify_with<S: Sender + 'static>(
    sender: Arc<S>,
    pool: &PgPool,
    team_id: i64,
    event: NotifEvent,
    payload: NotifPayload,
) {
    let settings = match NotificationsRepo::new(pool.clone()).get(team_id).await {
        Ok(Some(s)) => s,
        Ok(None) => return,
        Err(e) => {
            tracing::warn!(team_id, error = %e, "notify: failed to load settings");
            return;
        }
    };
    let matrix = &settings.event_matrix;

    // Email — synchronous (parity with Coolify's transactional path).
    if settings.email_enabled
        && should_send(event, Channel::Email, matrix)
        && let Some(delivery) = build_email_delivery(&settings, &payload)
        && let Err(e) = sender.send_email(delivery).await
    {
        tracing::warn!(team_id, error = %e, "notify: email delivery failed");
    }

    // Discord.
    if settings.discord_enabled
        && should_send(event, Channel::Discord, matrix)
        && let Some(url) = nonempty(&settings.discord_webhook_url)
    {
        let body = discord::build(&payload, settings.discord_ping_enabled);
        spawn_post(sender.clone(), Channel::Discord, url, body);
    }

    // Telegram.
    if settings.telegram_enabled
        && should_send(event, Channel::Telegram, matrix)
        && let Some(token) = nonempty(&settings.telegram_token)
        && let Some(chat_id) = nonempty(&settings.telegram_chat_id)
    {
        let body = telegram::build(&chat_id, &payload);
        spawn_post(
            sender.clone(),
            Channel::Telegram,
            telegram::url(&token),
            body,
        );
    }

    // Slack / Mattermost.
    if settings.slack_enabled
        && should_send(event, Channel::Slack, matrix)
        && let Some(url) = nonempty(&settings.slack_webhook_url)
    {
        let body = slack::build(&url, &payload);
        spawn_post(sender.clone(), Channel::Slack, url, body);
    }

    // Pushover.
    if settings.pushover_enabled
        && should_send(event, Channel::Pushover, matrix)
        && let Some(user) = nonempty(&settings.pushover_user_key)
        && let Some(token) = nonempty(&settings.pushover_api_token)
    {
        let body = pushover::build(&token, &user, &payload);
        spawn_post(
            sender.clone(),
            Channel::Pushover,
            pushover::URL.to_string(),
            body,
        );
    }

    // Generic webhook (SSRF-guarded).
    if settings.webhook_enabled
        && should_send(event, Channel::Webhook, matrix)
        && let Some(url) = nonempty(&settings.webhook_url)
    {
        if webhook::is_safe_url(&url) {
            let body = webhook::build(payload.event_slug, &payload);
            spawn_post(sender.clone(), Channel::Webhook, url, body);
        } else {
            tracing::warn!(team_id, "notify: dropped webhook to unsafe/blocked url");
        }
    }
}

/// Send a one-off test notification to a single channel (the `test` event is
/// always-send, so it ignores the matrix — but the channel must be configured).
/// Returns a human-readable error on misconfiguration or delivery failure.
pub async fn send_test(pool: &PgPool, team_id: i64, channel: Channel) -> Result<(), String> {
    let settings = NotificationsRepo::new(pool.clone())
        .ensure(team_id)
        .await
        .map_err(|e| e.to_string())?;
    let payload = NotifPayload::new(
        NotifEvent::Test.as_str(),
        Level::Info,
        "Rustify test notification",
        "If you can read this, the channel is configured correctly.",
    );
    let sender = ReqwestSender::new();
    match channel {
        Channel::Email => {
            if !settings.email_enabled {
                return Err("email channel is not enabled".into());
            }
            let delivery = build_email_delivery(&settings, &payload)
                .ok_or("no email recipients configured")?;
            sender.send_email(delivery).await
        }
        Channel::Discord => {
            let url = enabled_url(
                settings.discord_enabled,
                &settings.discord_webhook_url,
                "discord",
            )?;
            sender
                .post_json(
                    channel,
                    &url,
                    discord::build(&payload, settings.discord_ping_enabled),
                )
                .await
        }
        Channel::Telegram => {
            if !settings.telegram_enabled {
                return Err("telegram channel is not enabled".into());
            }
            let token = nonempty(&settings.telegram_token).ok_or("telegram token missing")?;
            let chat_id = nonempty(&settings.telegram_chat_id).ok_or("telegram chat id missing")?;
            sender
                .post_json(
                    channel,
                    &telegram::url(&token),
                    telegram::build(&chat_id, &payload),
                )
                .await
        }
        Channel::Slack => {
            let url = enabled_url(settings.slack_enabled, &settings.slack_webhook_url, "slack")?;
            sender
                .post_json(channel, &url, slack::build(&url, &payload))
                .await
        }
        Channel::Pushover => {
            if !settings.pushover_enabled {
                return Err("pushover channel is not enabled".into());
            }
            let user = nonempty(&settings.pushover_user_key).ok_or("pushover user key missing")?;
            let token =
                nonempty(&settings.pushover_api_token).ok_or("pushover api token missing")?;
            sender
                .post_json(
                    channel,
                    pushover::URL,
                    pushover::build(&token, &user, &payload),
                )
                .await
        }
        Channel::Webhook => {
            let url = enabled_url(settings.webhook_enabled, &settings.webhook_url, "webhook")?;
            if !webhook::is_safe_url(&url) {
                return Err("webhook url points to a blocked host".into());
            }
            sender
                .post_json(channel, &url, webhook::build(payload.event_slug, &payload))
                .await
        }
    }
}

fn enabled_url(enabled: bool, url: &Option<String>, name: &str) -> Result<String, String> {
    if !enabled {
        return Err(format!("{name} channel is not enabled"));
    }
    nonempty(url).ok_or_else(|| format!("{name} url is not configured"))
}

// ----- WS event → notification mapping -----------------------------------

/// The resource a WS event addresses, used to resolve the owning team.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Resource {
    Deployment(String),
    Backup(String),
    ScheduledTask(String),
    Server(String),
}

/// A WS event mapped to a notification.
#[derive(Debug, Clone)]
pub struct Mapped {
    pub resource: Resource,
    pub event: NotifEvent,
    pub payload: NotifPayload,
}

/// Map a terminal [`WsEvent`] to a notification, or `None` for events that don't
/// warrant one (in-progress/running states, log lines, other event kinds).
pub fn map_ws_event(ev: &WsEvent) -> Option<Mapped> {
    let data = &ev.data;
    let s = |k: &str| {
        data.get(k)
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string()
    };
    match ev.event.as_str() {
        "deployment_status_changed" => {
            let uuid = s("uuid");
            let (event, level, title) = match s("status").as_str() {
                "finished" => (
                    NotifEvent::DeploymentSuccess,
                    Level::Success,
                    "Deployment succeeded",
                ),
                "failed" => (
                    NotifEvent::DeploymentFailure,
                    Level::Error,
                    "Deployment failed",
                ),
                _ => return None,
            };
            let mut payload =
                NotifPayload::new(event.as_str(), level, title, format!("Deployment {uuid}"))
                    .field("Deployment", uuid.clone())
                    .extra("deployment_uuid", Value::String(uuid.clone()));
            if event == NotifEvent::DeploymentFailure {
                payload = payload.critical();
            }
            Some(Mapped {
                resource: Resource::Deployment(uuid),
                event,
                payload,
            })
        }
        "backup_status_changed" => {
            let backup_uuid = s("backup_uuid");
            let (event, level, title) = match s("status").as_str() {
                "success" => (
                    NotifEvent::BackupSuccess,
                    Level::Success,
                    "Backup succeeded",
                ),
                "failed" => (NotifEvent::BackupFailure, Level::Error, "Backup failed"),
                _ => return None,
            };
            let mut payload = NotifPayload::new(
                event.as_str(),
                level,
                title,
                format!("Backup {backup_uuid}"),
            )
            .field("Backup", backup_uuid.clone())
            .extra("backup_uuid", Value::String(backup_uuid.clone()))
            .extra("execution_uuid", Value::String(s("execution_uuid")));
            if event == NotifEvent::BackupFailure {
                payload = payload.critical();
            }
            Some(Mapped {
                resource: Resource::Backup(backup_uuid),
                event,
                payload,
            })
        }
        "scheduled_task_status_changed" => {
            let uuid = s("uuid");
            let (event, level, title) = match s("status").as_str() {
                "success" => (
                    NotifEvent::ScheduledTaskSuccess,
                    Level::Success,
                    "Scheduled task succeeded",
                ),
                "failed" => (
                    NotifEvent::ScheduledTaskFailure,
                    Level::Error,
                    "Scheduled task failed",
                ),
                _ => return None,
            };
            let mut payload = NotifPayload::new(
                event.as_str(),
                level,
                title,
                format!("Scheduled task {uuid}"),
            )
            .field("Task", uuid.clone())
            .extra("scheduled_task_uuid", Value::String(uuid.clone()))
            .extra("execution_uuid", Value::String(s("execution_uuid")));
            if event == NotifEvent::ScheduledTaskFailure {
                payload = payload.critical();
            }
            Some(Mapped {
                resource: Resource::ScheduledTask(uuid),
                event,
                payload,
            })
        }
        "server_reachability_changed" => {
            let uuid = s("uuid");
            let reachable = data
                .get("reachable")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let (event, level, title) = if reachable {
                (
                    NotifEvent::ServerReachable,
                    Level::Success,
                    "Server reachable",
                )
            } else {
                (
                    NotifEvent::ServerUnreachable,
                    Level::Error,
                    "Server unreachable",
                )
            };
            let mut payload =
                NotifPayload::new(event.as_str(), level, title, format!("Server {uuid}"))
                    .field("Server", uuid.clone())
                    .extra("server_uuid", Value::String(uuid.clone()));
            if event == NotifEvent::ServerUnreachable {
                payload = payload.critical();
            }
            Some(Mapped {
                resource: Resource::Server(uuid),
                event,
                payload,
            })
        }
        _ => None,
    }
}

/// Resolve the team that owns a resource, or `None` if it can't be found.
pub async fn resolve_team(pool: &PgPool, resource: &Resource) -> Option<i64> {
    let result: Result<Option<i64>, sqlx::Error> = match resource {
        Resource::Deployment(uuid) => {
            sqlx::query_scalar(
                "SELECT p.team_id FROM deployments d
               JOIN applications a ON a.id = d.application_id
               JOIN environments e ON e.id = a.environment_id
               JOIN projects p ON p.id = e.project_id
             WHERE d.uuid = $1",
            )
            .bind(uuid)
            .fetch_optional(pool)
            .await
        }
        Resource::Backup(uuid) => {
            sqlx::query_scalar(
                "SELECT p.team_id FROM scheduled_database_backups b
               JOIN standalone_databases sd ON sd.id = b.database_id
               JOIN environments e ON e.id = sd.environment_id
               JOIN projects p ON p.id = e.project_id
             WHERE b.uuid = $1",
            )
            .bind(uuid)
            .fetch_optional(pool)
            .await
        }
        Resource::ScheduledTask(uuid) => {
            sqlx::query_scalar(
                "SELECT COALESCE(st.team_id, pa.team_id, ps.team_id) FROM scheduled_tasks st
               LEFT JOIN applications a ON a.id = st.application_id
               LEFT JOIN environments ea ON ea.id = a.environment_id
               LEFT JOIN projects pa ON pa.id = ea.project_id
               LEFT JOIN services s ON s.id = st.service_id
               LEFT JOIN environments es ON es.id = s.environment_id
               LEFT JOIN projects ps ON ps.id = es.project_id
             WHERE st.uuid = $1",
            )
            .bind(uuid)
            .fetch_optional(pool)
            .await
        }
        Resource::Server(uuid) => {
            sqlx::query_scalar("SELECT team_id FROM servers WHERE uuid = $1")
                .bind(uuid)
                .fetch_optional(pool)
                .await
        }
    };
    match result {
        Ok(team) => team,
        Err(e) => {
            tracing::warn!(error = %e, "notify: failed to resolve team for resource");
            None
        }
    }
}

/// Handle one WS event: map it, resolve the team, and notify. No-op for events
/// that don't map or whose team can't be resolved.
pub async fn handle_event<S: Sender + 'static>(sender: Arc<S>, pool: &PgPool, ev: &WsEvent) {
    let Some(mapped) = map_ws_event(ev) else {
        return;
    };
    let Some(team_id) = resolve_team(pool, &mapped.resource).await else {
        return;
    };
    notify_with(sender, pool, team_id, mapped.event, mapped.payload).await;
}

/// The long-lived subscriber task: consumes the WS event bus and delivers
/// notifications. Spawned once from `main.rs`; exits when the bus is closed.
pub async fn subscriber(pool: PgPool, mut rx: broadcast::Receiver<WsEvent>) {
    let sender = Arc::new(ReqwestSender::new());
    loop {
        match rx.recv().await {
            Ok(ev) => handle_event(sender.clone(), &pool, &ev).await,
            Err(broadcast::error::RecvError::Lagged(n)) => {
                tracing::warn!(skipped = n, "notify subscriber lagged");
            }
            Err(broadcast::error::RecvError::Closed) => break,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustify_core::deployment::DeploymentStatus;

    #[test]
    fn maps_failed_deployment_to_deployment_failure() {
        let ev = WsEvent::deployment_status_changed("dep1", DeploymentStatus::Failed);
        let mapped = map_ws_event(&ev).expect("failed deploy should map");
        assert_eq!(mapped.event, NotifEvent::DeploymentFailure);
        assert_eq!(mapped.resource, Resource::Deployment("dep1".into()));
        assert!(mapped.payload.critical);
        assert_eq!(mapped.payload.level, Level::Error);
        assert_eq!(mapped.payload.extra["deployment_uuid"], "dep1");
    }

    #[test]
    fn maps_finished_deployment_to_success() {
        let ev = WsEvent::deployment_status_changed("dep2", DeploymentStatus::Finished);
        let mapped = map_ws_event(&ev).unwrap();
        assert_eq!(mapped.event, NotifEvent::DeploymentSuccess);
        assert!(!mapped.payload.critical);
    }

    #[test]
    fn ignores_non_terminal_deployment_states() {
        for st in [
            DeploymentStatus::InProgress,
            DeploymentStatus::Queued,
            DeploymentStatus::Cancelled,
        ] {
            let ev = WsEvent::deployment_status_changed("d", st);
            assert!(map_ws_event(&ev).is_none(), "{st:?} should not notify");
        }
    }

    #[test]
    fn maps_backup_and_task_and_server_events() {
        let ev = WsEvent::backup_status_changed("b1", "ex1", "failed");
        let m = map_ws_event(&ev).unwrap();
        assert_eq!(m.event, NotifEvent::BackupFailure);
        assert_eq!(m.resource, Resource::Backup("b1".into()));

        let ev = WsEvent::scheduled_task_status_changed("t1", "ex1", "success");
        let m = map_ws_event(&ev).unwrap();
        assert_eq!(m.event, NotifEvent::ScheduledTaskSuccess);
        assert_eq!(m.resource, Resource::ScheduledTask("t1".into()));

        let ev = WsEvent::server_reachability_changed("s1", false, false);
        let m = map_ws_event(&ev).unwrap();
        assert_eq!(m.event, NotifEvent::ServerUnreachable);
        assert_eq!(m.resource, Resource::Server("s1".into()));
    }

    #[test]
    fn ignores_unrelated_events() {
        let ev = WsEvent::application_status_changed("a1", "running");
        assert!(map_ws_event(&ev).is_none());
    }

    /// The webhook client must NOT follow redirects: a 30x from a (validated)
    /// public host must not be chased to a second, unvalidated host. Each
    /// connection replies `302` + `Connection: close`, so a followed redirect
    /// would open a *second* TCP connection; `Policy::none()` keeps it at one.
    #[tokio::test]
    async fn webhook_client_does_not_follow_redirects() {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicUsize, Ordering};
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let connections = Arc::new(AtomicUsize::new(0));
        let seen = connections.clone();

        let server = tokio::spawn(async move {
            loop {
                let accept =
                    tokio::time::timeout(std::time::Duration::from_millis(400), listener.accept())
                        .await;
                let Ok(Ok((mut sock, _))) = accept else { break };
                seen.fetch_add(1, Ordering::SeqCst);
                let mut buf = [0u8; 2048];
                let _ = sock.read(&mut buf).await;
                // Redirect back to this same listener: a followed redirect would
                // (because of `Connection: close`) open a second connection here.
                let resp = format!(
                    "HTTP/1.1 302 Found\r\nLocation: http://{addr}/next\r\n\
                     Connection: close\r\nContent-Length: 0\r\n\r\n"
                );
                let _ = sock.write_all(resp.as_bytes()).await;
                let _ = sock.flush().await;
            }
        });

        let sender = ReqwestSender::new();
        let url = format!("http://{addr}/start");
        let _ = sender
            .post_json(Channel::Webhook, &url, serde_json::json!({ "x": 1 }))
            .await;
        // Allow time for any (erroneous) redirect follow to reconnect.
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        server.abort();

        assert_eq!(
            connections.load(Ordering::SeqCst),
            1,
            "the webhook client must not follow the redirect"
        );
    }
}
