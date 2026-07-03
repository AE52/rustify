//! Email delivery: Resend HTTP API or SMTP via `lettre`.
//!
//! Port of Coolify's `App\Notifications\Channels\EmailChannel`
//! (coolify/app/Notifications/Channels/EmailChannel.php): when Resend is enabled
//! use its `emails/send` endpoint, otherwise send over SMTP, mapping the
//! `smtp_encryption` string to implicit-TLS / STARTTLS / plaintext.

use lettre::message::header::ContentType;
use lettre::transport::smtp::authentication::Credentials;
use lettre::{AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor};
use serde_json::{Value, json};

/// A fully-resolved email to deliver (secrets already decrypted by the repo).
#[derive(Debug, Clone)]
pub struct EmailDelivery {
    pub resend_enabled: bool,
    pub resend_api_key: Option<String>,
    pub smtp_host: Option<String>,
    pub smtp_port: Option<i32>,
    pub smtp_encryption: Option<String>,
    pub smtp_username: Option<String>,
    pub smtp_password: Option<String>,
    pub from_address: Option<String>,
    pub from_name: Option<String>,
    pub recipients: Vec<String>,
    pub subject: String,
    pub html: String,
}

/// The `From:` header value, `Name <address>`.
pub fn from_header(d: &EmailDelivery) -> String {
    let name = d.from_name.as_deref().unwrap_or("Rustify");
    let addr = d.from_address.as_deref().unwrap_or("noreply@localhost");
    format!("{name} <{addr}>")
}

/// The Resend `emails/send` request body.
pub fn resend_body(d: &EmailDelivery) -> Value {
    json!({
        "from": from_header(d),
        "to": d.recipients,
        "subject": d.subject,
        "html": d.html,
    })
}

/// SMTP transport mode derived from the `smtp_encryption` string.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SmtpMode {
    /// Implicit TLS (typically port 465).
    Tls,
    /// Opportunistic STARTTLS (typically port 587).
    StartTls,
    /// Plaintext.
    Plain,
}

/// Map an `smtp_encryption` string to a transport mode (parity with the
/// `match` in Coolify's `EmailChannel`, which treats `starttls`/`none`/unknown
/// as non-implicit and only `tls` as implicit TLS).
pub fn smtp_mode(encryption: Option<&str>) -> SmtpMode {
    match encryption.unwrap_or("").to_ascii_lowercase().as_str() {
        "tls" | "ssl" => SmtpMode::Tls,
        "starttls" => SmtpMode::StartTls,
        _ => SmtpMode::Plain,
    }
}

/// Deliver the email, choosing Resend or SMTP. Returns a human-readable error
/// string on failure (never contains a secret).
pub async fn deliver(client: &reqwest::Client, d: &EmailDelivery) -> Result<(), String> {
    if d.recipients.is_empty() {
        return Err("no email recipients configured".into());
    }
    if d.resend_enabled {
        let key = d
            .resend_api_key
            .as_deref()
            .filter(|k| !k.is_empty())
            .ok_or("resend enabled but api key missing")?;
        let resp = client
            .post("https://api.resend.com/emails")
            .bearer_auth(key)
            .json(&resend_body(d))
            .send()
            .await
            .map_err(|e| format!("resend request failed: {e}"))?;
        if !resp.status().is_success() {
            return Err(format!("resend returned status {}", resp.status()));
        }
        return Ok(());
    }
    send_smtp(d).await
}

async fn send_smtp(d: &EmailDelivery) -> Result<(), String> {
    let host = d
        .smtp_host
        .as_deref()
        .filter(|h| !h.is_empty())
        .ok_or("no email transport configured (smtp host missing)")?;

    let from = from_header(d)
        .parse::<lettre::message::Mailbox>()
        .map_err(|e| format!("invalid from address: {e}"))?;

    let mut builder = Message::builder().from(from).subject(&d.subject);
    for recipient in &d.recipients {
        let mailbox = recipient
            .parse::<lettre::message::Mailbox>()
            .map_err(|e| format!("invalid recipient: {e}"))?;
        builder = builder.to(mailbox);
    }
    let email = builder
        .header(ContentType::TEXT_HTML)
        .body(d.html.clone())
        .map_err(|e| format!("failed to build email: {e}"))?;

    let transport = match smtp_mode(d.smtp_encryption.as_deref()) {
        SmtpMode::Tls => AsyncSmtpTransport::<Tokio1Executor>::relay(host)
            .map_err(|e| format!("smtp tls setup failed: {e}"))?,
        SmtpMode::StartTls => AsyncSmtpTransport::<Tokio1Executor>::starttls_relay(host)
            .map_err(|e| format!("smtp starttls setup failed: {e}"))?,
        SmtpMode::Plain => AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(host),
    };
    let mut transport = transport;
    if let Some(port) = d.smtp_port {
        transport = transport.port(port as u16);
    }
    if let Some(user) = d.smtp_username.as_deref().filter(|u| !u.is_empty()) {
        let pass = d.smtp_password.clone().unwrap_or_default();
        transport = transport.credentials(Credentials::new(user.to_string(), pass));
    }
    let mailer = transport.build();
    mailer
        .send(email)
        .await
        .map_err(|e| format!("smtp send failed: {e}"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn delivery() -> EmailDelivery {
        EmailDelivery {
            resend_enabled: true,
            resend_api_key: Some("re_key".into()),
            smtp_host: None,
            smtp_port: None,
            smtp_encryption: None,
            smtp_username: None,
            smtp_password: None,
            from_address: Some("ops@acme.io".into()),
            from_name: Some("Acme Ops".into()),
            recipients: vec!["a@acme.io".into(), "b@acme.io".into()],
            subject: "Deploy failed".into(),
            html: "<b>failed</b>".into(),
        }
    }

    #[test]
    fn from_header_formats_name_and_address() {
        assert_eq!(from_header(&delivery()), "Acme Ops <ops@acme.io>");
        let mut d = delivery();
        d.from_name = None;
        d.from_address = None;
        assert_eq!(from_header(&d), "Rustify <noreply@localhost>");
    }

    #[test]
    fn resend_body_shape() {
        let body = resend_body(&delivery());
        assert_eq!(body["from"], "Acme Ops <ops@acme.io>");
        assert_eq!(body["to"][0], "a@acme.io");
        assert_eq!(body["to"][1], "b@acme.io");
        assert_eq!(body["subject"], "Deploy failed");
        assert_eq!(body["html"], "<b>failed</b>");
    }

    #[test]
    fn smtp_mode_mapping() {
        assert_eq!(smtp_mode(Some("tls")), SmtpMode::Tls);
        assert_eq!(smtp_mode(Some("SSL")), SmtpMode::Tls);
        assert_eq!(smtp_mode(Some("starttls")), SmtpMode::StartTls);
        assert_eq!(smtp_mode(Some("none")), SmtpMode::Plain);
        assert_eq!(smtp_mode(None), SmtpMode::Plain);
    }
}
