//! GitHub App installation access tokens.
//!
//! Behaviour parity with Coolify's `generateGithubToken(..., 'installation')`
//! (bootstrap/helpers/github.php:46-62): mint an App JWT, POST it to
//! `/app/installations/{id}/access_tokens`, and return the short-lived
//! installation token. Tokens live ~1h; we cache them in-memory per
//! installation id until shortly before expiry so a deployment does not mint a
//! fresh token for every git step.
//!
//! The minted token is a secret: it is never logged here, and the engine adds
//! it (and its url-encoded form) to the deployment's redaction set before any
//! command that embeds it runs.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use chrono::{DateTime, Duration, Utc};

use rustify_core::github_jwt;

/// The GitHub-App fields needed to mint an installation token. `private_key_pem`
/// is the decrypted RSA PEM; callers must not log it.
#[derive(Debug, Clone)]
pub struct GithubAppRow {
    pub id: i64,
    pub app_id: i64,
    pub installation_id: i64,
    pub api_url: String,
    pub private_key_pem: String,
}

/// Failures minting an installation token.
#[derive(Debug, thiserror::Error)]
pub enum GithubError {
    #[error("github app is missing app_id/installation_id/private key")]
    MissingCredentials,
    #[error("jwt: {0}")]
    Jwt(String),
    #[error("http request failed: {0}")]
    Http(String),
    #[error("github api error ({status}): {message}")]
    Api { status: u16, message: String },
    #[error("clock skew with github is too large: {0}s")]
    ClockSkew(i64),
}

/// The access-tokens endpoint URL (github.php:56). Trailing slashes on the
/// configured `api_url` are trimmed so the path is well-formed.
pub fn access_token_url(api_url: &str, installation_id: i64) -> String {
    format!(
        "{}/app/installations/{installation_id}/access_tokens",
        api_url.trim_end_matches('/')
    )
}

/// Request headers for the access-tokens POST (github.php:53-54). The JWT is a
/// bearer credential; the preview `Accept` matches Coolify exactly.
pub fn access_token_headers(jwt: &str) -> Vec<(&'static str, String)> {
    vec![
        ("Authorization", format!("Bearer {jwt}")),
        (
            "Accept",
            "application/vnd.github.machine-man-preview+json".to_string(),
        ),
    ]
}

type Cache = Mutex<HashMap<i64, (String, DateTime<Utc>)>>;

fn cache() -> &'static Cache {
    static CACHE: OnceLock<Cache> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Look up a still-valid cached token for `installation_id` relative to `now`.
fn cached(installation_id: i64, now: DateTime<Utc>) -> Option<String> {
    let guard = cache().lock().ok()?;
    let (token, expiry) = guard.get(&installation_id)?;
    (*expiry > now).then(|| token.clone())
}

fn store(installation_id: i64, token: &str, expiry: DateTime<Utc>) {
    if let Ok(mut guard) = cache().lock() {
        guard.insert(installation_id, (token.to_string(), expiry));
    }
}

/// Optional clock-skew guard (github.php:17-29): compare the local clock to
/// GitHub's `Date` header via `/zen`. A skew over 50s is fatal because it makes
/// the JWT `iat`/`exp` invalid. Network/parse failures are tolerated (returns
/// `Ok`) so a transient `/zen` blip never blocks a deploy.
pub async fn check_clock_skew(
    client: &reqwest::Client,
    api_url: &str,
    now: DateTime<Utc>,
) -> Result<(), GithubError> {
    let url = format!("{}/zen", api_url.trim_end_matches('/'));
    let resp = match client.get(&url).send().await {
        Ok(r) => r,
        Err(_) => return Ok(()),
    };
    let Some(date) = resp.headers().get("date").and_then(|v| v.to_str().ok()) else {
        return Ok(());
    };
    let Ok(github_time) = DateTime::parse_from_rfc2822(date) else {
        return Ok(());
    };
    let diff = (now - github_time.with_timezone(&Utc)).num_seconds().abs();
    if diff > 50 {
        return Err(GithubError::ClockSkew(diff));
    }
    Ok(())
}

/// Mint (or reuse a cached) installation access token for `app`, relative to
/// `now`. On a cache miss the App JWT is signed and exchanged with GitHub; the
/// returned token is cached until one minute before its stated expiry.
pub async fn installation_token(
    client: &reqwest::Client,
    app: &GithubAppRow,
    now: DateTime<Utc>,
) -> Result<String, GithubError> {
    if app.app_id == 0 || app.installation_id == 0 || app.private_key_pem.is_empty() {
        return Err(GithubError::MissingCredentials);
    }
    if let Some(token) = cached(app.installation_id, now) {
        return Ok(token);
    }

    let jwt = github_jwt::app_jwt(app.app_id, &app.private_key_pem, now)
        .map_err(|e| GithubError::Jwt(e.to_string()))?;

    let mut req = client.post(access_token_url(&app.api_url, app.installation_id));
    for (name, value) in access_token_headers(&jwt) {
        req = req.header(name, value);
    }
    let resp = req
        .send()
        .await
        .map_err(|e| GithubError::Http(e.to_string()))?;

    let status = resp.status();
    let body: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| GithubError::Http(e.to_string()))?;
    if !status.is_success() {
        let message = body
            .get("message")
            .and_then(|m| m.as_str())
            .unwrap_or("no error message found");
        let message = if message == "Not Found" {
            "Repository not found. Is it moved or deleted?".to_string()
        } else {
            message.to_string()
        };
        return Err(GithubError::Api {
            status: status.as_u16(),
            message,
        });
    }

    let token = body
        .get("token")
        .and_then(|t| t.as_str())
        .ok_or_else(|| GithubError::Api {
            status: status.as_u16(),
            message: "installation token missing from response".into(),
        })?
        .to_string();

    let expiry = body
        .get("expires_at")
        .and_then(|e| e.as_str())
        .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
        .map(|t| t.with_timezone(&Utc) - Duration::seconds(60))
        .unwrap_or_else(|| now + Duration::minutes(55));
    store(app.installation_id, &token, expiry);

    Ok(token)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn access_token_url_is_golden() {
        assert_eq!(
            access_token_url("https://api.github.com", 42),
            "https://api.github.com/app/installations/42/access_tokens"
        );
        // trailing slash on api_url is trimmed
        assert_eq!(
            access_token_url("https://ghe.example.com/api/v3/", 7),
            "https://ghe.example.com/api/v3/app/installations/7/access_tokens"
        );
    }

    #[test]
    fn access_token_headers_are_golden() {
        let h = access_token_headers("JWT123");
        assert_eq!(h[0], ("Authorization", "Bearer JWT123".to_string()));
        assert_eq!(
            h[1],
            (
                "Accept",
                "application/vnd.github.machine-man-preview+json".to_string()
            )
        );
    }

    #[test]
    fn cache_hits_before_expiry_and_misses_after() {
        let now = Utc::now();
        let iid = 999_001;
        assert!(cached(iid, now).is_none(), "empty cache misses");
        store(iid, "tok", now + Duration::minutes(30));
        assert_eq!(cached(iid, now).as_deref(), Some("tok"));
        // once past expiry the cache no longer serves the token
        assert!(cached(iid, now + Duration::minutes(31)).is_none());
    }

    #[tokio::test]
    async fn missing_credentials_short_circuits() {
        let client = reqwest::Client::new();
        let app = GithubAppRow {
            id: 1,
            app_id: 0,
            installation_id: 0,
            api_url: "https://api.github.com".into(),
            private_key_pem: String::new(),
        };
        let err = installation_token(&client, &app, Utc::now())
            .await
            .unwrap_err();
        assert!(matches!(err, GithubError::MissingCredentials));
    }
}
