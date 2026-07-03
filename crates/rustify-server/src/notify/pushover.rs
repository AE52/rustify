//! Pushover Messages API payload builder.
//!
//! Port of Coolify's `App\Notifications\Dto\PushoverMessage::toPayload`
//! (coolify/app/Notifications/Dto/PushoverMessage.php): a `POST` to
//! `https://api.pushover.net/1/messages.json` with `{token, user, title,
//! message, html:1}`, the title prefixed by a level icon.

use serde_json::{Value, json};

use super::{Level, NotifPayload};

/// The Pushover messages endpoint.
pub const URL: &str = "https://api.pushover.net/1/messages.json";

/// Level icon prepended to the title (parity with `PushoverMessage::getLevelIcon`).
pub fn icon(level: Level) -> &'static str {
    match level {
        Level::Info => "\u{2139}\u{fe0f}",    // ℹ️
        Level::Error => "\u{274c}",           // ❌
        Level::Success => "\u{2705}",         // ✅
        Level::Warning => "\u{26a0}\u{fe0f}", // ⚠️
    }
}

/// Build the Pushover message JSON body.
pub fn build(token: &str, user: &str, payload: &NotifPayload) -> Value {
    json!({
        "token": token,
        "user": user,
        "title": format!("{} {}", icon(payload.level), payload.title),
        "message": payload.description,
        "html": 1,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn body_shape_with_icon_and_html_flag() {
        let payload = NotifPayload::new("test", Level::Error, "Deploy failed", "web failed");
        let body = build("tok", "usr", &payload);
        assert_eq!(body["token"], "tok");
        assert_eq!(body["user"], "usr");
        assert_eq!(
            body["title"],
            format!("{} Deploy failed", icon(Level::Error))
        );
        assert_eq!(body["message"], "web failed");
        assert_eq!(body["html"], json!(1));
    }

    #[test]
    fn each_level_has_a_distinct_icon() {
        let icons = [
            icon(Level::Info),
            icon(Level::Error),
            icon(Level::Success),
            icon(Level::Warning),
        ];
        for (i, a) in icons.iter().enumerate() {
            for b in &icons[i + 1..] {
                assert_ne!(a, b);
            }
        }
    }
}
