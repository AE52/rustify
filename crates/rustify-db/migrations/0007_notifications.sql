-- Notification channels (Phase 3 wave-1, track p3-notifications). Clean-slate
-- simplification of Coolify's six per-channel `*NotificationSettings` models
-- (app/Models/{Email,Discord,Telegram,Slack,Pushover,Webhook}NotificationSettings.php)
-- + the `HasNotificationSettings` trait: one settings row per team, all channel
-- secrets AES-GCM encrypted (rustify_core::crypto) in *_enc columns, and one
-- JSONB `event_matrix` of shape { "<event>": { "<channel>": bool } } replacing
-- the per-(event,channel) boolean columns Coolify carries on each model.
CREATE TABLE notification_settings (
  id BIGSERIAL PRIMARY KEY,
  uuid TEXT UNIQUE NOT NULL,
  team_id BIGINT UNIQUE NOT NULL REFERENCES teams(id) ON DELETE CASCADE,

  -- Email (SMTP or Resend).
  email_enabled BOOLEAN NOT NULL DEFAULT false,
  smtp_host_enc BYTEA,
  smtp_port INT,
  smtp_encryption TEXT,
  smtp_username_enc BYTEA,
  smtp_password_enc BYTEA,
  smtp_from_address TEXT,
  smtp_from_name TEXT,
  smtp_recipients TEXT,
  resend_enabled BOOLEAN NOT NULL DEFAULT false,
  resend_api_key_enc BYTEA,

  -- Discord.
  discord_enabled BOOLEAN NOT NULL DEFAULT false,
  discord_webhook_url_enc BYTEA,
  discord_ping_enabled BOOLEAN NOT NULL DEFAULT false,

  -- Telegram.
  telegram_enabled BOOLEAN NOT NULL DEFAULT false,
  telegram_token_enc BYTEA,
  telegram_chat_id_enc BYTEA,

  -- Slack (or Mattermost-compatible webhook).
  slack_enabled BOOLEAN NOT NULL DEFAULT false,
  slack_webhook_url_enc BYTEA,

  -- Pushover.
  pushover_enabled BOOLEAN NOT NULL DEFAULT false,
  pushover_user_key_enc BYTEA,
  pushover_api_token_enc BYTEA,

  -- Generic webhook.
  webhook_enabled BOOLEAN NOT NULL DEFAULT false,
  webhook_url_enc BYTEA,

  -- { "<event>": { "<channel>": bool } }
  event_matrix JSONB NOT NULL DEFAULT '{}',

  created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
