//! Pure naming + FQDN helpers for PR preview deployments.
//!
//! Ports the naming Coolify derives for a preview environment: a `pr-{pr}-{sha}`
//! image tag, a `{uuid}-pr-{pr}` container, an `{uuid}-{pr}` dedicated network,
//! and the templated preview FQDN from `ApplicationPreview::generate_preview_fqdn`
//! (`app/Models/ApplicationPreview.php`). Deterministic and I/O-free so the
//! image-tag / FQDN expansion can be golden-tested.

/// Preview image tag: `pr-{pr}-{commit}` (Coolify `pr-{$pull_request_id}-{$commit}`).
pub fn preview_image_tag(pull_request_id: i64, commit: &str) -> String {
    format!("pr-{pull_request_id}-{commit}")
}

/// Deterministic preview container name `{app_uuid}-pr-{pr}` so the cleanup path
/// can find it with a `docker ps --filter name`.
pub fn preview_container_name(app_uuid: &str, pull_request_id: i64) -> String {
    format!("{app_uuid}-pr-{pull_request_id}")
}

/// Dedicated preview network `{app_uuid}-{pr}` (the `-{pr}` suffixed network the
/// cleanup handler disconnects the proxy from and removes).
pub fn preview_network(app_uuid: &str, pull_request_id: i64) -> String {
    format!("{app_uuid}-{pull_request_id}")
}

/// Expand the preview URL template against the application's FQDN.
///
/// Parity with `ApplicationPreview::generate_preview_fqdn`: take the first FQDN
/// (comma-separated list → first), pull it apart into scheme/host/port/path,
/// substitute `{{random}}` (a cuid2), `{{domain}}` (host) and `{{pr_id}}` in the
/// template, then reassemble `scheme://<expanded>[:port][path]`. Returns `None`
/// when the application has no FQDN.
pub fn expand_preview_fqdn(
    app_fqdn: Option<&str>,
    template: &str,
    pull_request_id: i64,
    random: &str,
) -> Option<String> {
    let raw = app_fqdn?.trim();
    if raw.is_empty() {
        return None;
    }
    let first = raw.split(',').next().unwrap_or(raw).trim();
    let parts = parse_url(first);

    let expanded = template
        .replace("{{random}}", random)
        .replace("{{domain}}", &parts.host)
        .replace("{{pr_id}}", &pull_request_id.to_string());

    let port = parts.port.map(|p| format!(":{p}")).unwrap_or_default();
    let path = match parts.path.as_deref() {
        Some(p) if p != "/" && !p.is_empty() => p.to_string(),
        _ => String::new(),
    };
    Some(format!("{}://{}{}{}", parts.scheme, expanded, port, path))
}

struct UrlParts {
    scheme: String,
    host: String,
    port: Option<u16>,
    path: Option<String>,
}

/// Minimal `scheme://host[:port][/path]` splitter (no external URL crate).
/// A missing scheme defaults to `http`.
fn parse_url(url: &str) -> UrlParts {
    let (scheme, rest) = match url.split_once("://") {
        Some((s, r)) => (s.to_string(), r),
        None => ("http".to_string(), url),
    };
    // Split host[:port] from the path.
    let (authority, path) = match rest.split_once('/') {
        Some((a, p)) => (a, Some(format!("/{p}"))),
        None => (rest, None),
    };
    let (host, port) = match authority.rsplit_once(':') {
        Some((h, p)) => (h.to_string(), p.parse::<u16>().ok()),
        None => (authority.to_string(), None),
    };
    UrlParts {
        scheme,
        host,
        port,
        path,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn image_tag_and_names_are_golden() {
        assert_eq!(preview_image_tag(42, "abc123"), "pr-42-abc123");
        assert_eq!(preview_container_name("app-uuid", 42), "app-uuid-pr-42");
        assert_eq!(preview_network("app-uuid", 42), "app-uuid-42");
    }

    #[test]
    fn fqdn_default_template_expands() {
        // default template {{pr_id}}.{{domain}}
        let got = expand_preview_fqdn(
            Some("https://app.example.com"),
            "{{pr_id}}.{{domain}}",
            7,
            "r",
        )
        .unwrap();
        assert_eq!(got, "https://7.app.example.com");
    }

    #[test]
    fn fqdn_preserves_scheme_port_and_path() {
        let got = expand_preview_fqdn(
            Some("https://app.example.com:8443/base"),
            "{{pr_id}}.{{domain}}",
            9,
            "rand",
        )
        .unwrap();
        assert_eq!(got, "https://9.app.example.com:8443/base");
    }

    #[test]
    fn fqdn_random_placeholder_and_first_of_list() {
        let got = expand_preview_fqdn(
            Some("http://a.example.com,https://b.example.com"),
            "{{random}}-{{pr_id}}.{{domain}}",
            3,
            "cuidx",
        )
        .unwrap();
        assert_eq!(got, "http://cuidx-3.a.example.com");
    }

    #[test]
    fn fqdn_none_when_no_app_fqdn() {
        assert!(expand_preview_fqdn(None, "{{pr_id}}.{{domain}}", 1, "r").is_none());
        assert!(expand_preview_fqdn(Some("  "), "{{pr_id}}.{{domain}}", 1, "r").is_none());
    }
}
