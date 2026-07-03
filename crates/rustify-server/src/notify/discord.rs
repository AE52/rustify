//! Discord webhook payload builder.
//!
//! Port of Coolify's `App\Notifications\Dto\DiscordMessage::toPayload`
//! (coolify/app/Notifications/Dto/DiscordMessage.php) and `SendMessageToDiscordJob`:
//! a single embed with title/description/decimal color/fields/footer, plus a
//! top-level `content: "@here"` when the message is critical and pinging is on.
//! Colors match the Coolify hex→decimal values.

use serde_json::{Value, json};

use super::{Level, NotifPayload};

/// Discord embed color as the decimal integer Discord expects.
pub fn color(level: Level) -> i64 {
    match level {
        Level::Success => 0xa1_ff_a5,
        Level::Warning => 0xff_a7_43,
        Level::Error => 0xff_70_5f,
        Level::Info => 0x4f_54_5c,
    }
}

/// Build the Discord webhook JSON body. `ping` adds the `@here` content when the
/// payload is critical (parity with `DiscordChannel`, which suppresses the ping
/// unless `discord_ping_enabled`).
pub fn build(payload: &NotifPayload, ping: bool) -> Value {
    let fields: Vec<Value> = payload
        .fields
        .iter()
        .map(|(name, value)| json!({ "name": name, "value": value, "inline": false }))
        .collect();
    let mut body = json!({
        "embeds": [{
            "title": payload.title,
            "description": payload.description,
            "color": color(payload.level),
            "fields": fields,
            "footer": { "text": "Rustify" },
        }]
    });
    if ping && payload.critical {
        body["content"] = json!("@here");
    }
    body
}

#[cfg(test)]
mod tests {
    use super::*;

    fn payload() -> NotifPayload {
        NotifPayload::new(
            "test",
            Level::Error,
            "Deployment failed",
            "Deployment failed for web",
        )
        .critical()
        .field("Project", "acme")
    }

    #[test]
    fn embed_shape_and_error_color() {
        let body = build(&payload(), true);
        let embed = &body["embeds"][0];
        assert_eq!(embed["title"], "Deployment failed");
        assert_eq!(embed["description"], "Deployment failed for web");
        assert_eq!(embed["color"], json!(0xff_70_5f));
        assert_eq!(embed["footer"]["text"], "Rustify");
        assert_eq!(embed["fields"][0]["name"], "Project");
        assert_eq!(embed["fields"][0]["value"], "acme");
        assert_eq!(embed["fields"][0]["inline"], json!(false));
        // Critical + ping enabled => @here content.
        assert_eq!(body["content"], "@here");
    }

    #[test]
    fn ping_suppressed_when_disabled_or_not_critical() {
        assert!(build(&payload(), false).get("content").is_none());
        let non_critical = NotifPayload::new("test", Level::Success, "ok", "done");
        assert!(build(&non_critical, true).get("content").is_none());
    }

    #[test]
    fn colors_match_coolify_palette() {
        assert_eq!(color(Level::Success), 0xa1_ff_a5);
        assert_eq!(color(Level::Warning), 0xff_a7_43);
        assert_eq!(color(Level::Error), 0xff_70_5f);
        assert_eq!(color(Level::Info), 0x4f_54_5c);
    }
}
