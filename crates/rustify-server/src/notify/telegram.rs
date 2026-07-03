//! Telegram Bot API payload builder.
//!
//! Port of Coolify's `SendMessageToTelegramJob`
//! (coolify/app/Jobs/SendMessageToTelegramJob.php): a `POST` to
//! `https://api.telegram.org/bot{token}/sendMessage` with a `{chat_id, text}`
//! body. The clean-slate version drops the inline-keyboard/thread-id extras.

use serde_json::{Value, json};

use super::NotifPayload;

/// The Bot API `sendMessage` endpoint for `token`.
pub fn url(token: &str) -> String {
    format!("https://api.telegram.org/bot{token}/sendMessage")
}

/// Render the message text: title, then description on the next line if present.
pub fn text(payload: &NotifPayload) -> String {
    if payload.description.is_empty() {
        payload.title.clone()
    } else {
        format!("{}\n{}", payload.title, payload.description)
    }
}

/// Build the `sendMessage` JSON body.
pub fn build(chat_id: &str, payload: &NotifPayload) -> Value {
    json!({ "chat_id": chat_id, "text": text(payload) })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::notify::Level;

    #[test]
    fn body_shape() {
        let payload = NotifPayload::new("test", Level::Error, "Deploy failed", "app web failed");
        let body = build("-1001", &payload);
        assert_eq!(body["chat_id"], "-1001");
        assert_eq!(body["text"], "Deploy failed\napp web failed");
    }

    #[test]
    fn text_omits_blank_description() {
        let payload = NotifPayload::new("test", Level::Info, "Just a title", "");
        assert_eq!(build("42", &payload)["text"], "Just a title");
    }

    #[test]
    fn url_embeds_token() {
        assert_eq!(
            url("123:ABC"),
            "https://api.telegram.org/bot123:ABC/sendMessage"
        );
    }
}
