//! Pure webhook logic shared by the git-source webhook routes: constant-time
//! signature verification, event→action classification, `[skip ci]`/`[skip cd]`
//! detection, and fork gating.
//!
//! Behaviour parity with Coolify's `app/Http/Controllers/Webhook/*` and
//! `Concerns/DetectsSkipDeployCommits`. Everything here is deterministic and
//! I/O-free so it can be unit tested without a running server.

use hmac::{Hmac, Mac};
use sha2::Sha256;

/// The git providers Rustify accepts webhooks from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Provider {
    Github,
    Gitea,
    Gitlab,
    Bitbucket,
}

/// How a pull-request / merge-request action maps onto a preview lifecycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrAction {
    /// Create/refresh the preview and (re)deploy it.
    Open,
    /// Tear the preview down.
    Close,
    /// Not a preview-affecting action.
    Ignore,
}

/// Classify a provider's PR/MR `action` string (Github/Gitea `pull_request`,
/// Gitlab `merge_request`, Bitbucket `x-event-key`).
///
/// Ports the per-provider action sets from the Coolify webhook controllers.
pub fn classify_pr_action(provider: Provider, action: &str) -> PrAction {
    let a = action.to_ascii_lowercase();
    match provider {
        Provider::Github => match a.as_str() {
            "opened" | "synchronize" | "reopened" => PrAction::Open,
            "closed" => PrAction::Close,
            _ => PrAction::Ignore,
        },
        Provider::Gitea => match a.as_str() {
            "opened" | "synchronized" | "reopened" => PrAction::Open,
            "closed" => PrAction::Close,
            _ => PrAction::Ignore,
        },
        Provider::Gitlab => match a.as_str() {
            "open" | "opened" | "synchronize" | "reopened" | "reopen" | "update" => PrAction::Open,
            "close" | "closed" | "merge" => PrAction::Close,
            _ => PrAction::Ignore,
        },
        Provider::Bitbucket => match a.as_str() {
            // Bitbucket ships the action in the `x-event-key` header.
            "pullrequest:created" | "pullrequest:updated" => PrAction::Open,
            "pullrequest:rejected" | "pullrequest:fulfilled" => PrAction::Close,
            _ => PrAction::Ignore,
        },
    }
}

/// Constant-time HMAC-SHA256 verification of a raw request body against the
/// hex-encoded `signature` (already stripped of any `sha256=` prefix).
///
/// Parity with `hash_equals(Str::after($sig,'sha256='), hash_hmac('sha256', body, secret))`.
pub fn verify_hmac_sha256(secret: &[u8], body: &[u8], signature_hex: &str) -> bool {
    let Ok(mut mac) = <Hmac<Sha256> as Mac>::new_from_slice(secret) else {
        return false;
    };
    mac.update(body);
    let Some(provided) = decode_hex(signature_hex.trim()) else {
        return false;
    };
    // `verify_slice` performs the length check and a constant-time comparison.
    mac.verify_slice(&provided).is_ok()
}

/// Constant-time plaintext equality (GitLab's `X-Gitlab-Token` is a shared
/// secret compared verbatim, not an HMAC).
pub fn verify_plaintext_token(secret: &[u8], provided: &[u8]) -> bool {
    if secret.len() != provided.len() {
        return false;
    }
    let mut diff: u8 = 0;
    for (a, b) in secret.iter().zip(provided.iter()) {
        diff |= a ^ b;
    }
    diff == 0
}

/// Strip a leading `sha256=` prefix (case-insensitive) from a signature header.
pub fn strip_sha256_prefix(header: &str) -> &str {
    header
        .strip_prefix("sha256=")
        .or_else(|| header.strip_prefix("SHA256="))
        .unwrap_or(header)
}

/// `refs/heads/<branch>` → `<branch>` (leaves other refs untouched).
pub fn strip_refs_heads(git_ref: &str) -> &str {
    git_ref.strip_prefix("refs/heads/").unwrap_or(git_ref)
}

/// True if any non-empty message contains `[skip ci]` or `[skip cd]`
/// (case-insensitive). Used for PR title + latest-commit signals where any one
/// marker triggers the skip (Coolify `shouldSkipDeployAny`).
pub fn should_skip_deploy_any<S: AsRef<str>>(messages: &[Option<S>]) -> bool {
    messages.iter().flatten().any(|m| contains_skip(m.as_ref()))
}

/// True if there is at least one non-empty message and *every* message carries a
/// skip marker (Coolify `shouldSkipDeploy`, used for push events).
pub fn should_skip_deploy_all<S: AsRef<str>>(messages: &[Option<S>]) -> bool {
    let present: Vec<&str> = messages
        .iter()
        .flatten()
        .map(|m| m.as_ref())
        .filter(|m| !m.trim().is_empty())
        .collect();
    !present.is_empty() && present.iter().all(|m| contains_skip(m))
}

fn contains_skip(message: &str) -> bool {
    let lower = message.to_ascii_lowercase();
    lower.contains("[skip ci]") || lower.contains("[skip cd]")
}

/// Fork detection: a PR is a fork PR when head and base repository ids differ
/// (Coolify `isForkPullRequest`, canonical signal). Ids absent on both sides ⇒
/// treated as same-repo (`false`).
pub fn is_fork_pull_request(head_repo_id: Option<i64>, base_repo_id: Option<i64>) -> bool {
    match (head_repo_id, base_repo_id) {
        (Some(h), Some(b)) => h != b,
        _ => false,
    }
}

/// Open-action fork gate (Coolify `ProcessGithubPullRequestWebhook::handleOpenAction`).
///
/// When public previews are enabled anyone may trigger. Otherwise fork PRs are
/// always rejected, and same-repo PRs require a trusted `author_association`
/// (`OWNER`/`MEMBER`/`COLLABORATOR`).
pub fn fork_gate_allows(
    public_enabled: bool,
    is_fork: bool,
    author_association: Option<&str>,
) -> bool {
    if public_enabled {
        return true;
    }
    if is_fork {
        return false;
    }
    matches!(
        author_association
            .map(|s| s.to_ascii_uppercase())
            .as_deref(),
        Some("OWNER") | Some("MEMBER") | Some("COLLABORATOR")
    )
}

/// Canonicalise an `owner/repo` path from a git repository URL/spec so it can be
/// compared case-insensitively to a webhook payload's `full_name`. Parity with
/// Coolify's `canonicalManualWebhookRepository` (https/ssh/scp/plain forms with
/// a `.git` suffix and query/fragment stripped).
pub fn canonical_repository(git_repository: &str) -> Option<String> {
    let repo = git_repository.trim();
    if repo.is_empty() {
        return None;
    }
    let path = if let Some((scheme_and_host, rest)) = split_scheme(repo) {
        let _ = scheme_and_host;
        rest
    } else if let Some(after) = repo.strip_prefix("git@").and_then(|r| r.split_once(':')) {
        // git@host:owner/repo(.git) — strip an optional scp-style numeric port.
        let p = after.1;
        strip_leading_port(p).to_string()
    } else {
        repo.to_string()
    };
    Some(normalize_repo_path(&path)).filter(|p| !p.is_empty())
}

/// Normalise a webhook payload `full_name` to `owner/repo`, rejecting anything
/// that is not a `segment(/segment)+` path (Coolify `manualWebhookRepositoryFullName`).
pub fn normalize_full_name(full_name: &str) -> Option<String> {
    let trimmed = full_name.trim().trim_matches('/');
    if trimmed.is_empty() {
        return None;
    }
    let valid = trimmed.split('/').all(|seg| {
        !seg.is_empty()
            && seg
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-'))
    }) && trimmed.contains('/');
    if !valid {
        return None;
    }
    Some(normalize_repo_path(trimmed))
}

fn split_scheme(repo: &str) -> Option<(&str, String)> {
    let (scheme, rest) = repo.split_once("://")?;
    // rest = host[/path]; keep only the path.
    let path = rest.split_once('/').map(|(_, p)| p).unwrap_or("");
    Some((scheme, path.to_string()))
}

fn strip_leading_port(path: &str) -> &str {
    // "2222/owner/repo" → "owner/repo".
    if let Some((head, tail)) = path.split_once('/')
        && !head.is_empty()
        && head.chars().all(|c| c.is_ascii_digit())
    {
        return tail;
    }
    path
}

fn normalize_repo_path(path: &str) -> String {
    let mut p = path.trim();
    // strip query/fragment
    if let Some((head, _)) = p.split_once(['?', '#']) {
        p = head;
    }
    let p = p.trim_matches('/');
    p.strip_suffix(".git").unwrap_or(p).to_string()
}

fn decode_hex(s: &str) -> Option<Vec<u8>> {
    if s.len() % 2 != 0 {
        return None;
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).ok())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn github_hmac_valid_and_invalid() {
        let secret = b"topsecret";
        let body = br#"{"zen":"hi"}"#;
        // reference digest computed with the same primitive
        let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(secret).unwrap();
        mac.update(body);
        let good = to_hex(&mac.finalize().into_bytes());
        assert!(verify_hmac_sha256(secret, body, &good));
        assert!(verify_hmac_sha256(
            secret,
            body,
            &format!("sha256={good}").replace("sha256=", "")
        ));
        assert!(!verify_hmac_sha256(secret, body, "deadbeef"));
        assert!(!verify_hmac_sha256(secret, body, "not-hex"));
        assert!(!verify_hmac_sha256(b"other", body, &good));
    }

    #[test]
    fn gitlab_plaintext_token() {
        assert!(verify_plaintext_token(b"abc123", b"abc123"));
        assert!(!verify_plaintext_token(b"abc123", b"abc124"));
        assert!(!verify_plaintext_token(b"abc123", b"abc1234"));
    }

    #[test]
    fn strips_sha256_prefix_and_refs_heads() {
        assert_eq!(strip_sha256_prefix("sha256=abcd"), "abcd");
        assert_eq!(strip_sha256_prefix("abcd"), "abcd");
        assert_eq!(strip_refs_heads("refs/heads/main"), "main");
        assert_eq!(strip_refs_heads("main"), "main");
    }

    #[test]
    fn pr_action_sets_per_provider() {
        use PrAction::*;
        assert_eq!(classify_pr_action(Provider::Github, "opened"), Open);
        assert_eq!(classify_pr_action(Provider::Github, "synchronize"), Open);
        assert_eq!(classify_pr_action(Provider::Github, "reopened"), Open);
        assert_eq!(classify_pr_action(Provider::Github, "closed"), Close);
        assert_eq!(classify_pr_action(Provider::Github, "assigned"), Ignore);

        assert_eq!(classify_pr_action(Provider::Gitea, "synchronized"), Open);
        assert_eq!(classify_pr_action(Provider::Gitea, "closed"), Close);

        assert_eq!(classify_pr_action(Provider::Gitlab, "open"), Open);
        assert_eq!(classify_pr_action(Provider::Gitlab, "update"), Open);
        assert_eq!(classify_pr_action(Provider::Gitlab, "reopen"), Open);
        assert_eq!(classify_pr_action(Provider::Gitlab, "merge"), Close);
        assert_eq!(classify_pr_action(Provider::Gitlab, "close"), Close);

        assert_eq!(
            classify_pr_action(Provider::Bitbucket, "pullrequest:created"),
            Open
        );
        assert_eq!(
            classify_pr_action(Provider::Bitbucket, "pullrequest:updated"),
            Open
        );
        assert_eq!(
            classify_pr_action(Provider::Bitbucket, "pullrequest:rejected"),
            Close
        );
        assert_eq!(
            classify_pr_action(Provider::Bitbucket, "pullrequest:fulfilled"),
            Close
        );
    }

    #[test]
    fn skip_ci_detection() {
        let all = [Some("fix [skip ci]"), Some("chore [SKIP CD]")];
        assert!(should_skip_deploy_all(&all));
        let mixed = [Some("fix [skip ci]"), Some("real work")];
        assert!(!should_skip_deploy_all(&mixed));
        assert!(should_skip_deploy_any(&mixed));
        let none: [Option<&str>; 0] = [];
        assert!(!should_skip_deploy_all(&none));
        assert!(!should_skip_deploy_any(&[Some("normal title")]));
    }

    #[test]
    fn fork_detection_and_gate() {
        assert!(is_fork_pull_request(Some(1), Some(2)));
        assert!(!is_fork_pull_request(Some(1), Some(1)));
        assert!(!is_fork_pull_request(None, Some(1)));

        // public enabled ⇒ always allowed
        assert!(fork_gate_allows(true, true, None));
        // fork + not public ⇒ rejected
        assert!(!fork_gate_allows(false, true, Some("OWNER")));
        // same-repo trusted association ⇒ allowed
        assert!(fork_gate_allows(false, false, Some("MEMBER")));
        assert!(fork_gate_allows(false, false, Some("collaborator")));
        // same-repo untrusted association ⇒ rejected
        assert!(!fork_gate_allows(false, false, Some("CONTRIBUTOR")));
        assert!(!fork_gate_allows(false, false, None));
    }

    #[test]
    fn canonical_repository_forms() {
        assert_eq!(
            canonical_repository("https://github.com/owner/repo.git").as_deref(),
            Some("owner/repo")
        );
        assert_eq!(
            canonical_repository("git@github.com:owner/repo.git").as_deref(),
            Some("owner/repo")
        );
        assert_eq!(
            canonical_repository("git@github.com:2222/owner/repo").as_deref(),
            Some("owner/repo")
        );
        assert_eq!(
            canonical_repository("owner/repo").as_deref(),
            Some("owner/repo")
        );
    }

    #[test]
    fn normalize_full_name_rejects_garbage() {
        assert_eq!(
            normalize_full_name("owner/repo").as_deref(),
            Some("owner/repo")
        );
        assert_eq!(
            normalize_full_name("/owner/repo/").as_deref(),
            Some("owner/repo")
        );
        assert_eq!(
            normalize_full_name("owner/repo.git").as_deref(),
            Some("owner/repo")
        );
        assert!(normalize_full_name("noslash").is_none());
        assert!(normalize_full_name("owner/re po").is_none());
        assert!(normalize_full_name("owner/re$po").is_none());
        assert!(normalize_full_name("").is_none());
    }

    fn to_hex(bytes: &[u8]) -> String {
        use std::fmt::Write as _;
        let mut s = String::with_capacity(bytes.len() * 2);
        for b in bytes {
            let _ = write!(s, "{b:02x}");
        }
        s
    }
}
