//! Per-team notification settings (migration `0007_notifications.sql`).
//!
//! Clean-slate port of Coolify's six `*NotificationSettings` models: one row per
//! team, every channel secret AES-256-GCM encrypted (`rustify_core::crypto`)
//! into a `*_enc` column and decrypted only on explicit read (never logged),
//! plus a single JSONB `event_matrix` (`{ event: { channel: bool } }`) replacing
//! the per-(event,channel) boolean columns Coolify keeps on each model.

use chrono::{DateTime, Utc};
use serde_json::Value;
use sqlx::PgPool;

use rustify_core::{crypto, ids, notify};

use crate::{DbError, DbResult};

/// A fully-decrypted settings row. Deliberately **not** `Serialize`: the HTTP
/// layer builds its own masked DTO so plaintext secrets never reach a response
/// or a log line.
#[derive(Debug, Clone)]
pub struct NotificationSettings {
    pub id: i64,
    pub uuid: String,
    pub team_id: i64,

    pub email_enabled: bool,
    pub smtp_host: Option<String>,
    pub smtp_port: Option<i32>,
    pub smtp_encryption: Option<String>,
    pub smtp_username: Option<String>,
    pub smtp_password: Option<String>,
    pub smtp_from_address: Option<String>,
    pub smtp_from_name: Option<String>,
    pub smtp_recipients: Option<String>,
    pub resend_enabled: bool,
    pub resend_api_key: Option<String>,

    pub discord_enabled: bool,
    pub discord_webhook_url: Option<String>,
    pub discord_ping_enabled: bool,

    pub telegram_enabled: bool,
    pub telegram_token: Option<String>,
    pub telegram_chat_id: Option<String>,

    pub slack_enabled: bool,
    pub slack_webhook_url: Option<String>,

    pub pushover_enabled: bool,
    pub pushover_user_key: Option<String>,
    pub pushover_api_token: Option<String>,

    pub webhook_enabled: bool,
    pub webhook_url: Option<String>,

    pub event_matrix: Value,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Partial update for the settings row. `None` leaves a field unchanged; for a
/// secret/text field `Some("")` clears it (stores `NULL`) and `Some(v)`
/// (re-)encrypts and stores `v`.
#[derive(Debug, Clone, Default)]
pub struct NotificationSettingsPatch {
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

/// Raw row with the encrypted blobs still sealed; mapped into
/// [`NotificationSettings`] by decrypting each `*_enc` column.
#[derive(sqlx::FromRow)]
struct Raw {
    id: i64,
    uuid: String,
    team_id: i64,
    email_enabled: bool,
    smtp_host_enc: Option<Vec<u8>>,
    smtp_port: Option<i32>,
    smtp_encryption: Option<String>,
    smtp_username_enc: Option<Vec<u8>>,
    smtp_password_enc: Option<Vec<u8>>,
    smtp_from_address: Option<String>,
    smtp_from_name: Option<String>,
    smtp_recipients: Option<String>,
    resend_enabled: bool,
    resend_api_key_enc: Option<Vec<u8>>,
    discord_enabled: bool,
    discord_webhook_url_enc: Option<Vec<u8>>,
    discord_ping_enabled: bool,
    telegram_enabled: bool,
    telegram_token_enc: Option<Vec<u8>>,
    telegram_chat_id_enc: Option<Vec<u8>>,
    slack_enabled: bool,
    slack_webhook_url_enc: Option<Vec<u8>>,
    pushover_enabled: bool,
    pushover_user_key_enc: Option<Vec<u8>>,
    pushover_api_token_enc: Option<Vec<u8>>,
    webhook_enabled: bool,
    webhook_url_enc: Option<Vec<u8>>,
    event_matrix: Value,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

/// Decrypt an optional sealed blob to its plaintext UTF-8 string.
fn dec(blob: Option<Vec<u8>>) -> DbResult<Option<String>> {
    match blob {
        None => Ok(None),
        Some(b) => {
            let plain = crypto::decrypt(&b)?;
            Ok(Some(String::from_utf8(plain).map_err(|_| DbError::Utf8)?))
        }
    }
}

/// Seal an optional plaintext secret for storage.
fn enc(plain: &Option<String>) -> Option<Vec<u8>> {
    plain.as_ref().map(|s| crypto::encrypt(s.as_bytes()))
}

/// PATCH-merge a secret/text field: `None` keeps `cur`, `Some("")` clears it,
/// `Some(v)` replaces it.
fn merge(patch: Option<String>, cur: Option<String>) -> Option<String> {
    match patch {
        None => cur,
        Some(s) if s.is_empty() => None,
        Some(s) => Some(s),
    }
}

impl Raw {
    fn decrypt(self) -> DbResult<NotificationSettings> {
        Ok(NotificationSettings {
            id: self.id,
            uuid: self.uuid,
            team_id: self.team_id,
            email_enabled: self.email_enabled,
            smtp_host: dec(self.smtp_host_enc)?,
            smtp_port: self.smtp_port,
            smtp_encryption: self.smtp_encryption,
            smtp_username: dec(self.smtp_username_enc)?,
            smtp_password: dec(self.smtp_password_enc)?,
            smtp_from_address: self.smtp_from_address,
            smtp_from_name: self.smtp_from_name,
            smtp_recipients: self.smtp_recipients,
            resend_enabled: self.resend_enabled,
            resend_api_key: dec(self.resend_api_key_enc)?,
            discord_enabled: self.discord_enabled,
            discord_webhook_url: dec(self.discord_webhook_url_enc)?,
            discord_ping_enabled: self.discord_ping_enabled,
            telegram_enabled: self.telegram_enabled,
            telegram_token: dec(self.telegram_token_enc)?,
            telegram_chat_id: dec(self.telegram_chat_id_enc)?,
            slack_enabled: self.slack_enabled,
            slack_webhook_url: dec(self.slack_webhook_url_enc)?,
            pushover_enabled: self.pushover_enabled,
            pushover_user_key: dec(self.pushover_user_key_enc)?,
            pushover_api_token: dec(self.pushover_api_token_enc)?,
            webhook_enabled: self.webhook_enabled,
            webhook_url: dec(self.webhook_url_enc)?,
            event_matrix: self.event_matrix,
            created_at: self.created_at,
            updated_at: self.updated_at,
        })
    }
}

const COLS: &str = "id, uuid, team_id, email_enabled, smtp_host_enc, smtp_port, smtp_encryption, \
    smtp_username_enc, smtp_password_enc, smtp_from_address, smtp_from_name, smtp_recipients, \
    resend_enabled, resend_api_key_enc, discord_enabled, discord_webhook_url_enc, \
    discord_ping_enabled, telegram_enabled, telegram_token_enc, telegram_chat_id_enc, \
    slack_enabled, slack_webhook_url_enc, pushover_enabled, pushover_user_key_enc, \
    pushover_api_token_enc, webhook_enabled, webhook_url_enc, event_matrix, created_at, updated_at";

#[derive(Clone)]
pub struct NotificationsRepo {
    pool: PgPool,
}

impl NotificationsRepo {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Fetch a team's decrypted settings, if the row exists.
    pub async fn get(&self, team_id: i64) -> DbResult<Option<NotificationSettings>> {
        let raw = sqlx::query_as::<_, Raw>(&format!(
            "SELECT {COLS} FROM notification_settings WHERE team_id = $1"
        ))
        .bind(team_id)
        .fetch_optional(&self.pool)
        .await?;
        raw.map(Raw::decrypt).transpose()
    }

    /// Return the team's settings, provisioning a default row (with the
    /// sane-default event matrix) if none exists yet. Idempotent.
    pub async fn ensure(&self, team_id: i64) -> DbResult<NotificationSettings> {
        sqlx::query(
            "INSERT INTO notification_settings (uuid, team_id, event_matrix)
             VALUES ($1, $2, $3) ON CONFLICT (team_id) DO NOTHING",
        )
        .bind(ids::new_uuid())
        .bind(team_id)
        .bind(notify::default_event_matrix())
        .execute(&self.pool)
        .await?;
        self.get(team_id).await?.ok_or(DbError::NotFound)
    }

    /// Apply a partial update, encrypting any provided secrets. Reads the
    /// current row (provisioning one if missing), merges the patch in memory,
    /// then writes every column back. Returns the updated decrypted settings.
    pub async fn upsert(
        &self,
        team_id: i64,
        patch: NotificationSettingsPatch,
    ) -> DbResult<NotificationSettings> {
        let cur = self.ensure(team_id).await?;

        let email_enabled = patch.email_enabled.unwrap_or(cur.email_enabled);
        let smtp_host = merge(patch.smtp_host, cur.smtp_host);
        let smtp_port = patch.smtp_port.or(cur.smtp_port);
        let smtp_encryption = merge(patch.smtp_encryption, cur.smtp_encryption);
        let smtp_username = merge(patch.smtp_username, cur.smtp_username);
        let smtp_password = merge(patch.smtp_password, cur.smtp_password);
        let smtp_from_address = merge(patch.smtp_from_address, cur.smtp_from_address);
        let smtp_from_name = merge(patch.smtp_from_name, cur.smtp_from_name);
        let smtp_recipients = merge(patch.smtp_recipients, cur.smtp_recipients);
        let resend_enabled = patch.resend_enabled.unwrap_or(cur.resend_enabled);
        let resend_api_key = merge(patch.resend_api_key, cur.resend_api_key);

        let discord_enabled = patch.discord_enabled.unwrap_or(cur.discord_enabled);
        let discord_webhook_url = merge(patch.discord_webhook_url, cur.discord_webhook_url);
        let discord_ping_enabled = patch
            .discord_ping_enabled
            .unwrap_or(cur.discord_ping_enabled);

        let telegram_enabled = patch.telegram_enabled.unwrap_or(cur.telegram_enabled);
        let telegram_token = merge(patch.telegram_token, cur.telegram_token);
        let telegram_chat_id = merge(patch.telegram_chat_id, cur.telegram_chat_id);

        let slack_enabled = patch.slack_enabled.unwrap_or(cur.slack_enabled);
        let slack_webhook_url = merge(patch.slack_webhook_url, cur.slack_webhook_url);

        let pushover_enabled = patch.pushover_enabled.unwrap_or(cur.pushover_enabled);
        let pushover_user_key = merge(patch.pushover_user_key, cur.pushover_user_key);
        let pushover_api_token = merge(patch.pushover_api_token, cur.pushover_api_token);

        let webhook_enabled = patch.webhook_enabled.unwrap_or(cur.webhook_enabled);
        let webhook_url = merge(patch.webhook_url, cur.webhook_url);

        let event_matrix = patch.event_matrix.unwrap_or(cur.event_matrix);

        let raw = sqlx::query_as::<_, Raw>(&format!(
            "UPDATE notification_settings SET
                email_enabled = $2, smtp_host_enc = $3, smtp_port = $4, smtp_encryption = $5,
                smtp_username_enc = $6, smtp_password_enc = $7, smtp_from_address = $8,
                smtp_from_name = $9, smtp_recipients = $10, resend_enabled = $11,
                resend_api_key_enc = $12, discord_enabled = $13, discord_webhook_url_enc = $14,
                discord_ping_enabled = $15, telegram_enabled = $16, telegram_token_enc = $17,
                telegram_chat_id_enc = $18, slack_enabled = $19, slack_webhook_url_enc = $20,
                pushover_enabled = $21, pushover_user_key_enc = $22, pushover_api_token_enc = $23,
                webhook_enabled = $24, webhook_url_enc = $25, event_matrix = $26, updated_at = now()
             WHERE team_id = $1
             RETURNING {COLS}"
        ))
        .bind(team_id)
        .bind(email_enabled)
        .bind(enc(&smtp_host))
        .bind(smtp_port)
        .bind(&smtp_encryption)
        .bind(enc(&smtp_username))
        .bind(enc(&smtp_password))
        .bind(&smtp_from_address)
        .bind(&smtp_from_name)
        .bind(&smtp_recipients)
        .bind(resend_enabled)
        .bind(enc(&resend_api_key))
        .bind(discord_enabled)
        .bind(enc(&discord_webhook_url))
        .bind(discord_ping_enabled)
        .bind(telegram_enabled)
        .bind(enc(&telegram_token))
        .bind(enc(&telegram_chat_id))
        .bind(slack_enabled)
        .bind(enc(&slack_webhook_url))
        .bind(pushover_enabled)
        .bind(enc(&pushover_user_key))
        .bind(enc(&pushover_api_token))
        .bind(webhook_enabled)
        .bind(enc(&webhook_url))
        .bind(event_matrix)
        .fetch_one(&self.pool)
        .await?;
        raw.decrypt()
    }
}
