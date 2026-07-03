//! Notification event/channel model and the routing predicate shared by the
//! server's notification subscriber and settings routes.
//!
//! Clean-slate simplification of Coolify's per-channel `*NotificationSettings`
//! models + the `HasNotificationSettings` trait
//! (coolify/app/Traits/HasNotificationSettings.php): instead of one settings
//! model per channel with a `{event}_{channel}_notifications` boolean column per
//! pair, Rustify keeps a single settings row per team plus one JSONB
//! `event_matrix` of shape `{ "<event>": { "<channel>": bool } }`.
//!
//! [`should_send`] ports `HasNotificationSettings::isNotificationTypeEnabled` +
//! `getEnabledChannels`: an event is delivered to a channel when the event is in
//! the always-send set, or the matrix opts that (event, channel) pair in; and
//! `general` is never sent over email (parity with `getEnabledChannels`, which
//! removes the email channel for the `general` event).

use serde_json::Value;

/// A notification event kind. The string form (`as_str`) is the stable key used
/// in the JSONB `event_matrix`, in the WS→notification mapping, and on the wire
/// — it matches Coolify's event slugs (e.g. `deployment_failure`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NotifEvent {
    DeploymentSuccess,
    DeploymentFailure,
    StatusChange,
    BackupSuccess,
    BackupFailure,
    ScheduledTaskSuccess,
    ScheduledTaskFailure,
    DockerCleanupSuccess,
    DockerCleanupFailure,
    ServerDiskUsage,
    ServerReachable,
    ServerUnreachable,
    ServerPatch,
    TraefikOutdated,
    // Always-send events (see [`NotifEvent::always_send`]).
    General,
    Test,
    SslCertificateRenewal,
    ApiTokenExpiring,
    ServerForceEnabled,
    ServerForceDisabled,
}

impl NotifEvent {
    /// Stable slug used as the `event_matrix` key and on the wire.
    pub fn as_str(self) -> &'static str {
        match self {
            NotifEvent::DeploymentSuccess => "deployment_success",
            NotifEvent::DeploymentFailure => "deployment_failure",
            NotifEvent::StatusChange => "status_change",
            NotifEvent::BackupSuccess => "backup_success",
            NotifEvent::BackupFailure => "backup_failure",
            NotifEvent::ScheduledTaskSuccess => "scheduled_task_success",
            NotifEvent::ScheduledTaskFailure => "scheduled_task_failure",
            NotifEvent::DockerCleanupSuccess => "docker_cleanup_success",
            NotifEvent::DockerCleanupFailure => "docker_cleanup_failure",
            NotifEvent::ServerDiskUsage => "server_disk_usage",
            NotifEvent::ServerReachable => "server_reachable",
            NotifEvent::ServerUnreachable => "server_unreachable",
            NotifEvent::ServerPatch => "server_patch",
            NotifEvent::TraefikOutdated => "traefik_outdated",
            NotifEvent::General => "general",
            NotifEvent::Test => "test",
            NotifEvent::SslCertificateRenewal => "ssl_certificate_renewal",
            NotifEvent::ApiTokenExpiring => "api_token_expiring",
            NotifEvent::ServerForceEnabled => "server_force_enabled",
            NotifEvent::ServerForceDisabled => "server_force_disabled",
        }
    }

    /// Every configurable (matrix-driven) event, in a stable order. Excludes the
    /// always-send events, which have no matrix toggle.
    pub const CONFIGURABLE: [NotifEvent; 14] = [
        NotifEvent::DeploymentSuccess,
        NotifEvent::DeploymentFailure,
        NotifEvent::StatusChange,
        NotifEvent::BackupSuccess,
        NotifEvent::BackupFailure,
        NotifEvent::ScheduledTaskSuccess,
        NotifEvent::ScheduledTaskFailure,
        NotifEvent::DockerCleanupSuccess,
        NotifEvent::DockerCleanupFailure,
        NotifEvent::ServerDiskUsage,
        NotifEvent::ServerReachable,
        NotifEvent::ServerUnreachable,
        NotifEvent::ServerPatch,
        NotifEvent::TraefikOutdated,
    ];

    /// Events that are always delivered to every enabled channel regardless of
    /// the matrix (parity with `HasNotificationSettings::$alwaysSendEvents`).
    pub fn is_always_send(self) -> bool {
        matches!(
            self,
            NotifEvent::General
                | NotifEvent::Test
                | NotifEvent::SslCertificateRenewal
                | NotifEvent::ApiTokenExpiring
                | NotifEvent::ServerForceEnabled
                | NotifEvent::ServerForceDisabled
        )
    }
}

/// A delivery channel. The string form is the inner key of the JSONB
/// `event_matrix` and the value accepted by the `POST /notifications/test` body.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Channel {
    Email,
    Discord,
    Telegram,
    Slack,
    Pushover,
    Webhook,
}

impl Channel {
    /// Every channel, in the order `getEnabledChannels` iterates them.
    pub const ALL: [Channel; 6] = [
        Channel::Email,
        Channel::Discord,
        Channel::Telegram,
        Channel::Slack,
        Channel::Pushover,
        Channel::Webhook,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Channel::Email => "email",
            Channel::Discord => "discord",
            Channel::Telegram => "telegram",
            Channel::Slack => "slack",
            Channel::Pushover => "pushover",
            Channel::Webhook => "webhook",
        }
    }

    /// Parse a channel slug (used by the test-notification route).
    pub fn parse(s: &str) -> Option<Channel> {
        Channel::ALL.into_iter().find(|c| c.as_str() == s)
    }
}

/// Whether `event` should be delivered over `channel` given the team's JSONB
/// `event_matrix` (shape `{ event: { channel: bool } }`).
///
/// Ports `isNotificationTypeEnabled` + the `general`→no-email carve-out of
/// `getEnabledChannels`: `general` is never emailed; always-send events go to
/// every channel; otherwise the (event, channel) pair must be `true` in the
/// matrix. This predicate is orthogonal to whether the channel is *configured*
/// (has credentials + its `*_enabled` flag) — the caller checks that separately.
pub fn should_send(event: NotifEvent, channel: Channel, matrix: &Value) -> bool {
    // `general` is never emailed (parity with getEnabledChannels).
    if channel == Channel::Email && event == NotifEvent::General {
        return false;
    }
    if event.is_always_send() {
        return true;
    }
    matrix
        .get(event.as_str())
        .and_then(|c| c.get(channel.as_str()))
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

/// The sane-default `event_matrix` provisioned for a new team: the critical
/// failure/unreachable events opt in on every channel, so enabling any channel
/// immediately surfaces problems. Non-critical events default off.
pub fn default_event_matrix() -> Value {
    let critical = [
        NotifEvent::DeploymentFailure,
        NotifEvent::BackupFailure,
        NotifEvent::ScheduledTaskFailure,
        NotifEvent::ServerUnreachable,
    ];
    let mut matrix = serde_json::Map::new();
    for event in critical {
        let mut channels = serde_json::Map::new();
        for channel in Channel::ALL {
            channels.insert(channel.as_str().to_string(), Value::Bool(true));
        }
        matrix.insert(event.as_str().to_string(), Value::Object(channels));
    }
    Value::Object(matrix)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn always_send_events_ignore_the_matrix() {
        let empty = json!({});
        // Every always-send event reaches every channel except the general/email
        // carve-out — even with an empty matrix.
        for event in [
            NotifEvent::Test,
            NotifEvent::SslCertificateRenewal,
            NotifEvent::ApiTokenExpiring,
            NotifEvent::ServerForceEnabled,
            NotifEvent::ServerForceDisabled,
        ] {
            for channel in Channel::ALL {
                assert!(
                    should_send(event, channel, &empty),
                    "{} should always send over {}",
                    event.as_str(),
                    channel.as_str()
                );
            }
        }
    }

    #[test]
    fn general_is_never_emailed_but_reaches_other_channels() {
        let empty = json!({});
        assert!(!should_send(NotifEvent::General, Channel::Email, &empty));
        for channel in [
            Channel::Discord,
            Channel::Telegram,
            Channel::Slack,
            Channel::Pushover,
            Channel::Webhook,
        ] {
            assert!(should_send(NotifEvent::General, channel, &empty));
        }
    }

    #[test]
    fn matrix_toggles_a_configurable_event() {
        let matrix = json!({
            "deployment_failure": { "discord": true, "email": false }
        });
        assert!(should_send(
            NotifEvent::DeploymentFailure,
            Channel::Discord,
            &matrix
        ));
        assert!(!should_send(
            NotifEvent::DeploymentFailure,
            Channel::Email,
            &matrix
        ));
        // A pair absent from the matrix defaults off.
        assert!(!should_send(
            NotifEvent::DeploymentFailure,
            Channel::Slack,
            &matrix
        ));
        // An event absent from the matrix defaults off.
        assert!(!should_send(
            NotifEvent::DeploymentSuccess,
            Channel::Discord,
            &matrix
        ));
    }

    #[test]
    fn malformed_matrix_values_default_off() {
        let matrix = json!({ "deployment_failure": { "discord": "yes" } });
        assert!(!should_send(
            NotifEvent::DeploymentFailure,
            Channel::Discord,
            &matrix
        ));
        let matrix = json!({ "deployment_failure": 3 });
        assert!(!should_send(
            NotifEvent::DeploymentFailure,
            Channel::Discord,
            &matrix
        ));
    }

    #[test]
    fn default_matrix_opts_critical_events_into_all_channels() {
        let matrix = default_event_matrix();
        for event in [
            NotifEvent::DeploymentFailure,
            NotifEvent::BackupFailure,
            NotifEvent::ScheduledTaskFailure,
            NotifEvent::ServerUnreachable,
        ] {
            for channel in Channel::ALL {
                assert!(
                    should_send(event, channel, &matrix),
                    "default: {} over {}",
                    event.as_str(),
                    channel.as_str()
                );
            }
        }
        // Success/non-critical events stay off by default.
        assert!(!should_send(
            NotifEvent::DeploymentSuccess,
            Channel::Discord,
            &matrix
        ));
    }

    #[test]
    fn channel_parse_roundtrips() {
        for channel in Channel::ALL {
            assert_eq!(Channel::parse(channel.as_str()), Some(channel));
        }
        assert_eq!(Channel::parse("sms"), None);
    }
}
