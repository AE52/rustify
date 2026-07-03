//! GitHub PR-status comment: a single editable issue comment reflecting the
//! preview deployment's lifecycle.
//!
//! Port of Coolify's `ApplicationPullRequestUpdateJob`: on the first status a
//! comment is created (`POST /repos/{owner}/{repo}/issues/{pr}/comments`), later
//! statuses edit it (`PATCH /repos/{owner}/{repo}/issues/comments/{id}`, falling
//! back to create on `Not Found`), and PR close deletes it. Public repos are
//! skipped by the caller (`is_public_repository()` short-circuit).

use chrono::Utc;
use serde_json::json;

use crate::github::{self, GithubAppRow};

/// The preview lifecycle states that produce a comment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrCommentState {
    Queued,
    InProgress,
    Finished,
    Error,
    Closed,
}

/// Build the markdown comment body for a status. `fqdn` adds an `[Open Preview]`
/// link on success. Parity with the `match ($this->status)` block in
/// ApplicationPullRequestUpdateJob (trimmed to the states rustify emits).
pub fn comment_body(state: PrCommentState, app_name: &str, fqdn: Option<&str>) -> String {
    let head = match state {
        PrCommentState::Queued => {
            format!("The preview deployment for **{app_name}** is queued. \u{23f3}\n\n")
        }
        PrCommentState::InProgress => {
            format!("The preview deployment for **{app_name}** is in progress. \u{1f7e1}\n\n")
        }
        PrCommentState::Finished => {
            let link = fqdn
                .map(|f| format!("[Open Preview]({f}) | "))
                .unwrap_or_default();
            format!("The preview deployment for **{app_name}** is ready. \u{1f7e2}\n\n{link}")
        }
        PrCommentState::Error => {
            format!("The preview deployment for **{app_name}** failed. \u{1f534}\n\n")
        }
        PrCommentState::Closed => String::new(),
    };
    format!("{head}Last updated at: {} UTC", Utc::now().to_rfc3339())
}

/// `{api_url}/repos/{owner/repo}/issues/{pr}/comments` (create).
pub fn create_comment_url(api_url: &str, git_repository: &str, pull_request_id: i32) -> String {
    format!(
        "{}/repos/{}/issues/{pull_request_id}/comments",
        api_url.trim_end_matches('/'),
        git_repository.trim_matches('/'),
    )
}

/// `{api_url}/repos/{owner/repo}/issues/comments/{id}` (patch/delete).
pub fn edit_comment_url(api_url: &str, git_repository: &str, comment_id: i64) -> String {
    format!(
        "{}/repos/{}/issues/comments/{comment_id}",
        api_url.trim_end_matches('/'),
        git_repository.trim_matches('/'),
    )
}

/// GitHub App-authenticated request builder (v3 accept + api-version + bearer).
fn authed(
    client: &reqwest::Client,
    method: reqwest::Method,
    url: &str,
    token: &str,
) -> reqwest::RequestBuilder {
    client
        .request(method, url)
        .header("Accept", "application/vnd.github.v3+json")
        .header("X-GitHub-Api-Version", "2022-11-28")
        .header("User-Agent", "rustify")
        .header("Authorization", format!("Bearer {token}"))
}

/// Create or update the PR comment for `state`, returning the (possibly new)
/// comment id. Mints an installation token from `app`. Errors are surfaced so
/// the caller can log-and-continue (a failed comment never fails a deploy).
#[allow(clippy::too_many_arguments)]
pub async fn upsert(
    client: &reqwest::Client,
    app: &GithubAppRow,
    git_repository: &str,
    pull_request_id: i32,
    existing_comment_id: Option<i64>,
    state: PrCommentState,
    app_name: &str,
    fqdn: Option<&str>,
) -> Result<Option<i64>, String> {
    let token = github::installation_token(client, app, Utc::now())
        .await
        .map_err(|e| e.to_string())?;
    let body = comment_body(state, app_name, fqdn);

    if let Some(id) = existing_comment_id {
        let url = edit_comment_url(&app.api_url, git_repository, id);
        let resp = authed(client, reqwest::Method::PATCH, &url, &token)
            .json(&json!({ "body": body }))
            .send()
            .await
            .map_err(|e| e.to_string())?;
        if resp.status().as_u16() != 404 {
            return Ok(Some(id));
        }
        // fall through to create on Not Found
    }

    let url = create_comment_url(&app.api_url, git_repository, pull_request_id);
    let resp = authed(client, reqwest::Method::POST, &url, &token)
        .json(&json!({ "body": body }))
        .send()
        .await
        .map_err(|e| e.to_string())?;
    let data: serde_json::Value = resp.json().await.map_err(|e| e.to_string())?;
    Ok(data.get("id").and_then(|v| v.as_i64()))
}

/// Delete the PR comment (PR closed).
pub async fn delete(
    client: &reqwest::Client,
    app: &GithubAppRow,
    git_repository: &str,
    comment_id: i64,
) -> Result<(), String> {
    let token = github::installation_token(client, app, Utc::now())
        .await
        .map_err(|e| e.to_string())?;
    let url = edit_comment_url(&app.api_url, git_repository, comment_id);
    authed(client, reqwest::Method::DELETE, &url, &token)
        .send()
        .await
        .map_err(|e| e.to_string())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urls_are_golden() {
        assert_eq!(
            create_comment_url("https://api.github.com", "owner/repo", 7),
            "https://api.github.com/repos/owner/repo/issues/7/comments"
        );
        assert_eq!(
            edit_comment_url("https://api.github.com/", "owner/repo", 123),
            "https://api.github.com/repos/owner/repo/issues/comments/123"
        );
    }

    #[test]
    fn body_reflects_state() {
        assert!(comment_body(PrCommentState::Queued, "app", None).contains("queued"));
        assert!(comment_body(PrCommentState::InProgress, "app", None).contains("in progress"));
        let done = comment_body(
            PrCommentState::Finished,
            "app",
            Some("https://7.example.com"),
        );
        assert!(done.contains("ready"));
        assert!(done.contains("[Open Preview](https://7.example.com)"));
        assert!(comment_body(PrCommentState::Error, "app", None).contains("failed"));
        assert!(comment_body(PrCommentState::Closed, "app", None).starts_with("Last updated"));
    }
}
