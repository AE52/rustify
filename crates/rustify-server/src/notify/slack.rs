//! Slack / Mattermost webhook payload builder.
//!
//! Port of Coolify's `SendMessageToSlackJob`
//! (coolify/app/Jobs/SendMessageToSlackJob.php): genuine Slack webhooks
//! (`https://hooks.slack.com/...`) get a Block Kit payload; any other host gets
//! the Mattermost-compatible `attachments` fallback. Colors are the hex strings
//! from `App\Notifications\Dto\SlackMessage`.

use serde_json::{Value, json};

use super::{Level, NotifPayload};

/// Slack/Mattermost attachment color as a hex string.
pub fn color(level: Level) -> &'static str {
    match level {
        Level::Success => "#00ff00",
        Level::Warning => "#ffa500",
        Level::Error => "#ff0000",
        Level::Info => "#0099ff",
    }
}

/// Whether `url` is a genuine Slack incoming webhook (`https` + `hooks.slack.com`).
pub fn is_slack_webhook(url: &str) -> bool {
    match reqwest::Url::parse(url) {
        Ok(u) => u.scheme() == "https" && u.host_str() == Some("hooks.slack.com"),
        Err(_) => false,
    }
}

/// Build the JSON body, choosing Block Kit for Slack and `attachments` for
/// Mattermost based on the destination host.
pub fn build(url: &str, payload: &NotifPayload) -> Value {
    if is_slack_webhook(url) {
        block_kit(payload)
    } else {
        mattermost(payload)
    }
}

fn block_kit(payload: &NotifPayload) -> Value {
    json!({
        "text": payload.title,
        "blocks": [{
            "type": "section",
            "text": { "type": "plain_text", "text": "Rustify Notification" }
        }],
        "attachments": [{
            "color": color(payload.level),
            "blocks": [
                {
                    "type": "header",
                    "text": { "type": "plain_text", "text": payload.title }
                },
                {
                    "type": "section",
                    "text": { "type": "mrkdwn", "text": payload.description }
                }
            ]
        }]
    })
}

fn mattermost(payload: &NotifPayload) -> Value {
    json!({
        "username": "Rustify",
        "attachments": [{
            "title": payload.title,
            "color": color(payload.level),
            "text": payload.description,
            "footer": "Rustify"
        }]
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn payload() -> NotifPayload {
        NotifPayload::new("test", Level::Error, "Deploy failed", "web failed")
    }

    #[test]
    fn detects_slack_host() {
        assert!(is_slack_webhook("https://hooks.slack.com/services/T/B/x"));
        assert!(!is_slack_webhook("http://hooks.slack.com/x")); // must be https
        assert!(!is_slack_webhook("https://mattermost.example.com/hooks/x"));
        assert!(!is_slack_webhook("not a url"));
    }

    #[test]
    fn slack_host_gets_block_kit() {
        let body = build("https://hooks.slack.com/services/x", &payload());
        assert_eq!(body["text"], "Deploy failed");
        assert_eq!(body["blocks"][0]["type"], "section");
        assert_eq!(body["attachments"][0]["color"], "#ff0000");
        assert_eq!(body["attachments"][0]["blocks"][0]["type"], "header");
        assert_eq!(
            body["attachments"][0]["blocks"][0]["text"]["text"],
            "Deploy failed"
        );
        assert_eq!(
            body["attachments"][0]["blocks"][1]["text"]["type"],
            "mrkdwn"
        );
        // Mattermost keys must be absent.
        assert!(body.get("username").is_none());
    }

    #[test]
    fn other_host_gets_mattermost_attachments() {
        let body = build("https://chat.example.com/hooks/abc", &payload());
        assert_eq!(body["username"], "Rustify");
        assert_eq!(body["attachments"][0]["title"], "Deploy failed");
        assert_eq!(body["attachments"][0]["color"], "#ff0000");
        assert_eq!(body["attachments"][0]["text"], "web failed");
        assert_eq!(body["attachments"][0]["footer"], "Rustify");
        // Block Kit keys must be absent.
        assert!(body.get("blocks").is_none());
    }

    #[test]
    fn colors_match_coolify_palette() {
        assert_eq!(color(Level::Success), "#00ff00");
        assert_eq!(color(Level::Warning), "#ffa500");
        assert_eq!(color(Level::Error), "#ff0000");
        assert_eq!(color(Level::Info), "#0099ff");
    }
}
