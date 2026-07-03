//! Generic webhook payload builder + SSRF guard.
//!
//! Port of Coolify's `SendWebhookJob` + `App\Rules\SafeWebhookUrl`
//! (coolify/app/Jobs/SendWebhookJob.php, app/Rules/SafeWebhookUrl.php): POST a
//! JSON body describing the event, but only after validating the destination is
//! not a loopback/link-local/private/internal host. The task tightens Coolify's
//! rule (which permits private IPs for self-hosting) to also drop private ranges.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

use serde_json::{Map, Value};

use super::NotifPayload;

/// Build the webhook JSON body: the event slug, a title/message, the level, and
/// any structured `extra` fields the caller attached.
pub fn build(event_slug: &str, payload: &NotifPayload) -> Value {
    let mut body = Map::new();
    body.insert("event".into(), Value::String(event_slug.to_string()));
    body.insert("title".into(), Value::String(payload.title.clone()));
    body.insert("message".into(), Value::String(payload.description.clone()));
    body.insert(
        "level".into(),
        Value::String(payload.level.as_str().to_string()),
    );
    for (k, v) in &payload.extra {
        body.insert(k.clone(), v.clone());
    }
    Value::Object(body)
}

/// Whether `raw` is safe to POST to: an http(s) URL whose host is not a
/// loopback, link-local, private, unspecified, or obviously-internal target.
/// Bare-hostname (non-IP-literal) destinations are allowed — they are resolved
/// by the HTTP client at send time, and this guard blocks the direct-literal
/// SSRF vectors (`127.0.0.1`, `::1`, `169.254.169.254`, `10.x`, `localhost`).
pub fn is_safe_url(raw: &str) -> bool {
    let Ok(url) = reqwest::Url::parse(raw) else {
        return false;
    };
    if !matches!(url.scheme(), "http" | "https") {
        return false;
    }
    let Some(host) = url.host_str() else {
        return false;
    };
    // `host_str` keeps IPv6 brackets; strip them for parsing.
    let host = host.trim_start_matches('[').trim_end_matches(']');
    let lower = host.to_ascii_lowercase();
    if lower == "localhost" || lower.ends_with(".internal") || lower.ends_with(".local") {
        return false;
    }
    match host.parse::<IpAddr>() {
        Ok(ip) => is_public_ip(ip),
        // A DNS name — allow it (cannot resolve here without a DNS lookup).
        Err(_) => true,
    }
}

fn is_public_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => is_public_v4(v4),
        IpAddr::V6(v6) => is_public_v6(v6),
    }
}

fn is_public_v4(ip: Ipv4Addr) -> bool {
    !(ip.is_loopback()          // 127.0.0.0/8
        || ip.is_private()      // 10/8, 172.16/12, 192.168/16
        || ip.is_link_local()   // 169.254.0.0/16 (incl. cloud metadata)
        || ip.is_unspecified()  // 0.0.0.0
        || ip.is_broadcast()
        || ip.is_documentation())
}

fn is_public_v6(ip: Ipv6Addr) -> bool {
    if ip.is_loopback() || ip.is_unspecified() {
        return false;
    }
    let seg0 = ip.segments()[0];
    // fe80::/10 link-local and fc00::/7 unique-local (private).
    if (seg0 & 0xffc0) == 0xfe80 || (seg0 & 0xfe00) == 0xfc00 {
        return false;
    }
    // IPv4-mapped (::ffff:a.b.c.d) — apply the v4 rules to the embedded address.
    if let Some(v4) = ip.to_ipv4_mapped() {
        return is_public_v4(v4);
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::notify::Level;
    use serde_json::json;

    #[test]
    fn body_includes_event_and_extra_fields() {
        let mut payload = NotifPayload::new("x", Level::Error, "Deploy failed", "web failed");
        payload
            .extra
            .insert("application_uuid".into(), json!("app123"));
        let body = build("deployment_failure", &payload);
        assert_eq!(body["event"], "deployment_failure");
        assert_eq!(body["title"], "Deploy failed");
        assert_eq!(body["message"], "web failed");
        assert_eq!(body["level"], "error");
        assert_eq!(body["application_uuid"], "app123");
    }

    #[test]
    fn blocks_loopback_and_internal_hosts() {
        for bad in [
            "http://localhost/hook",
            "http://127.0.0.1/hook",
            "https://127.0.0.5:9000/hook",
            "http://0.0.0.0/hook",
            "http://[::1]/hook",
            "http://169.254.169.254/latest/meta-data",
            "http://10.0.0.5/hook",
            "http://192.168.1.10/hook",
            "http://172.16.4.2/hook",
            "https://foo.internal/hook",
            "https://db.local/hook",
            "http://[fd00::1]/hook",
            "http://[fe80::1]/hook",
            "http://[::ffff:127.0.0.1]/hook",
            "ftp://example.com/hook",
            "not-a-url",
        ] {
            assert!(!is_safe_url(bad), "{bad} must be rejected");
        }
    }

    #[test]
    fn allows_public_hosts_and_ips() {
        for good in [
            "https://example.com/hook",
            "https://hooks.slack.com/services/x",
            "http://8.8.8.8/hook",
            "https://[2606:4700:4700::1111]/hook",
        ] {
            assert!(is_safe_url(good), "{good} must be allowed");
        }
    }
}
