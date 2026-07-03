//! Git-source webhook endpoints (coolify routes/webhooks.php + the
//! `Webhook/{Github,Gitlab,Gitea,Bitbucket}` controllers).
//!
//! These endpoints are **unauthenticated** (a git host cannot present a rustify
//! API token) — trust comes from verifying the provider signature over the raw
//! request body. Two GitHub modes: App mode (`/events`, keyed on the App's
//! `app_id`) and manual mode (`/events/manual`, keyed on the application's own
//! per-provider secret); Gitlab/Gitea/Bitbucket are manual only.
//!
//! Supported events: `push` → a normal deploy for the matching branch;
//! `pull_request`/`merge_request` → a preview deploy (open) or teardown (close).

use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};

use rustify_core::webhook::{
    self, PrAction, Provider, canonical_repository, classify_pr_action, normalize_full_name,
    should_skip_deploy_all, should_skip_deploy_any, strip_refs_heads, strip_sha256_prefix,
    verify_hmac_sha256, verify_plaintext_token,
};
use rustify_db::repos::{
    Application, ApplicationRepo, DeploymentRepo, GithubAppRepo, NewDeployment, PreviewRepo,
};

use crate::app::AppState;

/// How many pending `deploy` jobs constitute a full queue (429 back-pressure).
const DEPLOY_QUEUE_LIMIT: i64 = 200;

fn header(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get(name)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string())
}

fn queue_full_response() -> Response {
    (
        StatusCode::TOO_MANY_REQUESTS,
        [("Retry-After", "60")],
        "Deployment queue is full.",
    )
        .into_response()
}

/// True when the pending `deploy` backlog exceeds [`DEPLOY_QUEUE_LIMIT`].
async fn deploy_queue_full(state: &AppState) -> bool {
    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM jobs WHERE kind = 'deploy' AND locked_at IS NULL")
            .fetch_one(&state.pool)
            .await
            .unwrap_or(0);
    count >= DEPLOY_QUEUE_LIMIT
}

/// Enqueue a production (`pr = 0`) deploy for `app` at `commit`.
async fn enqueue_push_deploy(
    state: &AppState,
    app: &Application,
    commit: Option<&str>,
) -> Result<String, String> {
    let server_id = ApplicationRepo::new(state.pool.clone())
        .server_id(app.id)
        .await
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "no destination server".to_string())?;
    let dep = DeploymentRepo::new(state.pool.clone())
        .create_queued(NewDeployment {
            application_id: app.id,
            server_id,
            commit_sha: commit.map(str::to_string),
            force_rebuild: false,
            ..Default::default()
        })
        .await
        .map_err(|e| e.to_string())?;
    state
        .queue
        .enqueue("deploy", json!({ "deployment_uuid": dep.uuid }), None)
        .await
        .map_err(|e| e.to_string())?;
    Ok(dep.uuid)
}

/// Enqueue a preview deploy for `app` on `pr` at `commit`, upserting the
/// preview row first.
async fn enqueue_preview_deploy(
    state: &AppState,
    app: &Application,
    pr: i32,
    html_url: Option<&str>,
    commit: Option<&str>,
    git_type: &str,
) -> Result<String, String> {
    let server_id = ApplicationRepo::new(state.pool.clone())
        .server_id(app.id)
        .await
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "no destination server".to_string())?;
    PreviewRepo::new(state.pool.clone())
        .upsert(app.id, pr, html_url, Some(git_type))
        .await
        .map_err(|e| e.to_string())?;
    let dep = DeploymentRepo::new(state.pool.clone())
        .create_queued(NewDeployment {
            application_id: app.id,
            server_id,
            commit_sha: commit.map(str::to_string),
            force_rebuild: false,
            pull_request_id: pr,
            git_type: Some(git_type.to_string()),
            ..Default::default()
        })
        .await
        .map_err(|e| e.to_string())?;
    state
        .queue
        .enqueue("deploy", json!({ "deployment_uuid": dep.uuid }), None)
        .await
        .map_err(|e| e.to_string())?;
    Ok(dep.uuid)
}

/// Enqueue teardown of a PR preview.
async fn enqueue_preview_cleanup(
    state: &AppState,
    app: &Application,
    pr: i32,
) -> Result<(), String> {
    state
        .queue
        .enqueue(
            "preview_cleanup",
            json!({ "application_uuid": app.uuid, "pull_request_id": pr }),
            None,
        )
        .await
        .map_err(|e| e.to_string())?;
    Ok(())
}

// --------------------------------------------------------------------------
// A normalized view of the incoming event, parsed per provider.
// --------------------------------------------------------------------------

enum Event {
    Ping,
    Push {
        branch: String,
        commit: Option<String>,
        skip: bool,
    },
    Pull {
        action: PrAction,
        pr: i32,
        html_url: Option<String>,
        base_branch: String,
        commit: Option<String>,
        author_association: Option<String>,
        is_fork: bool,
        pr_deployable_ok: bool, // false ⇒ skip-ci already matched
    },
}

fn s(v: &Value, ptr: &str) -> Option<String> {
    v.pointer(ptr).and_then(|x| x.as_str()).map(str::to_string)
}
fn i(v: &Value, ptr: &str) -> Option<i64> {
    v.pointer(ptr).and_then(|x| x.as_i64())
}

/// Whether an application allows a PR deploy for `action` (Coolify's
/// `!isPRDeployable() && action != closed` guard).
fn pr_guard_ok(app: &Application, action: PrAction) -> bool {
    app.is_pr_deployments_enabled || action == PrAction::Close
}

/// Shared open/close handling once an application matched + its signature
/// verified. Returns a per-application status payload, or `Err` to abort the
/// whole request with a 429.
async fn dispatch_pull(
    state: &AppState,
    app: &Application,
    git_type: &str,
    ev: &Event,
) -> Result<Value, Response> {
    let Event::Pull {
        action,
        pr,
        html_url,
        commit,
        author_association,
        is_fork,
        pr_deployable_ok,
        ..
    } = ev
    else {
        return Ok(
            json!({ "application": app.name, "status": "failed", "message": "not a PR event" }),
        );
    };

    if !pr_guard_ok(app, *action) {
        return Ok(json!({
            "application": app.name,
            "status": "failed",
            "message": "Preview deployments disabled.",
        }));
    }

    match action {
        PrAction::Close => {
            if enqueue_preview_cleanup(state, app, *pr).await.is_err() {
                return Ok(
                    json!({ "application": app.name, "status": "failed", "message": "cleanup enqueue failed" }),
                );
            }
            Ok(
                json!({ "application": app.name, "status": "success", "message": "Preview deployment closed." }),
            )
        }
        PrAction::Open => {
            if !pr_deployable_ok {
                return Ok(json!({
                    "application": app.name,
                    "status": "skipped",
                    "message": "PR title or commit contains [skip ci]/[skip cd].",
                }));
            }
            // Fork gate: uses the app setting OR the instance default.
            let public_enabled =
                app.is_pr_deployments_public_enabled || instance_public_enabled(state).await;
            if !webhook::fork_gate_allows(public_enabled, *is_fork, author_association.as_deref()) {
                return Ok(json!({
                    "application": app.name,
                    "status": "skipped",
                    "message": "Fork/untrusted PR rejected (public previews disabled).",
                }));
            }
            if deploy_queue_full(state).await {
                return Err(queue_full_response());
            }
            match enqueue_preview_deploy(
                state,
                app,
                *pr,
                html_url.as_deref(),
                commit.as_deref(),
                git_type,
            )
            .await
            {
                Ok(_) => Ok(
                    json!({ "application": app.name, "status": "success", "message": "Preview deployment queued." }),
                ),
                Err(e) => Ok(json!({ "application": app.name, "status": "failed", "message": e })),
            }
        }
        PrAction::Ignore => Ok(
            json!({ "application": app.name, "status": "failed", "message": "No action found." }),
        ),
    }
}

async fn instance_public_enabled(state: &AppState) -> bool {
    rustify_db::repos::SettingsRepo::new(state.pool.clone())
        .get()
        .await
        .map(|s| s.is_pr_deployments_public_enabled)
        .unwrap_or(false)
}

/// Shared push handling for a matched + verified application.
async fn dispatch_push(state: &AppState, app: &Application, ev: &Event) -> Result<Value, Response> {
    let Event::Push { commit, skip, .. } = ev else {
        return Ok(
            json!({ "application": app.name, "status": "failed", "message": "not a push event" }),
        );
    };
    if *skip {
        return Ok(json!({
            "application": app.name,
            "status": "skipped",
            "message": "All commits contain [skip cd] or [skip ci]. Skipping deployment.",
        }));
    }
    if deploy_queue_full(state).await {
        return Err(queue_full_response());
    }
    match enqueue_push_deploy(state, app, commit.as_deref()).await {
        Ok(uuid) => Ok(json!({
            "application": app.name,
            "status": "success",
            "message": "Deployment queued.",
            "deployment_uuid": uuid,
        })),
        Err(e) => Ok(json!({ "application": app.name, "status": "failed", "message": e })),
    }
}

// --------------------------------------------------------------------------
// GitHub — App mode (/webhooks/source/github/events)
// --------------------------------------------------------------------------

pub async fn github_app(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let event = header(&headers, "X-GitHub-Event")
        .unwrap_or_default()
        .to_ascii_lowercase();
    if event == "ping" {
        return "pong".into_response();
    }
    let Some(target_id) = header(&headers, "X-GitHub-Hook-Installation-Target-Id")
        .and_then(|s| s.parse::<i64>().ok())
    else {
        return "Nothing to do. No GitHub App found.".into_response();
    };
    let gh = match GithubAppRepo::new(state.pool.clone())
        .get_by_app_id(target_id)
        .await
    {
        Ok(Some(g)) => g,
        _ => return "Nothing to do. No GitHub App found.".into_response(),
    };
    let secret = match GithubAppRepo::new(state.pool.clone())
        .decrypt_webhook_secret(gh.id)
        .await
    {
        Ok(Some(s)) => s,
        _ => return "Invalid signature.".into_response(),
    };
    let sig = header(&headers, "X-Hub-Signature-256").unwrap_or_default();
    if !verify_hmac_sha256(secret.as_bytes(), &body, strip_sha256_prefix(&sig)) {
        return "Invalid signature.".into_response();
    }

    let payload: Value = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(_) => return "Nothing to do. Invalid payload.".into_response(),
    };

    let ev = match parse_github_like(Provider::Github, &event, &payload) {
        Some(e) => e,
        None => return format!("Nothing to do. Event '{event}' is not supported.").into_response(),
    };
    if matches!(ev, Event::Ping) {
        return "pong".into_response();
    }
    let repo_id = i(&payload, "/repository/id").unwrap_or(0);

    // App-mode candidates: private App source, repository id + branch match.
    let branch = event_branch(&ev);
    let apps = match ApplicationRepo::new(state.pool.clone())
        .list_by_source_repo_branch(gh.id, repo_id, &branch)
        .await
    {
        Ok(a) => a,
        Err(e) => return internal(e.to_string()),
    };
    run_for_apps(&state, apps, "github", &ev).await
}

// --------------------------------------------------------------------------
// GitHub / Gitea — manual mode (HMAC over raw body, X-Hub-Signature-256)
// --------------------------------------------------------------------------

pub async fn github_manual(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    hmac_manual(
        state,
        headers,
        body,
        Provider::Github,
        "github",
        "X-GitHub-Event",
    )
    .await
}

pub async fn gitea_manual(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    hmac_manual(
        state,
        headers,
        body,
        Provider::Gitea,
        "gitea",
        "X-Gitea-Event",
    )
    .await
}

async fn hmac_manual(
    state: AppState,
    headers: HeaderMap,
    body: Bytes,
    provider: Provider,
    git_type: &str,
    event_header: &str,
) -> Response {
    let event = header(&headers, event_header)
        .unwrap_or_default()
        .to_ascii_lowercase();
    if event == "ping" {
        return "pong".into_response();
    }
    let payload: Value = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(_) => return "Nothing to do. Invalid payload.".into_response(),
    };
    let ev = match parse_github_like(provider, &event, &payload) {
        Some(e) => e,
        None => return format!("Nothing to do. Event '{event}' is not supported.").into_response(),
    };
    let full_name = match s(&payload, "/repository/full_name").and_then(|f| normalize_full_name(&f))
    {
        Some(f) => f,
        None => return "Nothing to do. Invalid repository.".into_response(),
    };
    let branch = event_branch(&ev);
    let sig = strip_sha256_prefix(&header(&headers, "X-Hub-Signature-256").unwrap_or_default())
        .to_string();

    let candidates = match ApplicationRepo::new(state.pool.clone())
        .list_by_branch(&branch)
        .await
    {
        Ok(a) => a,
        Err(e) => return internal(e.to_string()),
    };
    let matched = filter_manual(&state, candidates, &full_name, git_type, |app_secret| {
        verify_hmac_sha256(app_secret.as_bytes(), &body, &sig)
    })
    .await;
    run_for_apps(&state, matched, git_type, &ev).await
}

// --------------------------------------------------------------------------
// GitLab — manual mode (plaintext X-Gitlab-Token)
// --------------------------------------------------------------------------

pub async fn gitlab_manual(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let payload: Value = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(_) => return "Nothing to do. Invalid payload.".into_response(),
    };
    let kind = s(&payload, "/object_kind").unwrap_or_default();
    let ev = match parse_gitlab(&kind, &payload) {
        Some(e) => e,
        None => {
            return "Event not allowed. Only push and merge_request events are allowed."
                .into_response();
        }
    };
    let full_name =
        match s(&payload, "/project/path_with_namespace").and_then(|f| normalize_full_name(&f)) {
            Some(f) => f,
            None => return "Nothing to do. Invalid repository.".into_response(),
        };
    let token = header(&headers, "X-Gitlab-Token").unwrap_or_default();
    let branch = event_branch(&ev);
    let candidates = match ApplicationRepo::new(state.pool.clone())
        .list_by_branch(&branch)
        .await
    {
        Ok(a) => a,
        Err(e) => return internal(e.to_string()),
    };
    let matched = filter_manual(&state, candidates, &full_name, "gitlab", |secret| {
        verify_plaintext_token(secret.as_bytes(), token.as_bytes())
    })
    .await;
    run_for_apps(&state, matched, "gitlab", &ev).await
}

// --------------------------------------------------------------------------
// Bitbucket — manual mode (HMAC over raw body, X-Hub-Signature)
// --------------------------------------------------------------------------

pub async fn bitbucket_manual(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let event = header(&headers, "X-Event-Key").unwrap_or_default();
    let payload: Value = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(_) => return "Nothing to do. Invalid payload.".into_response(),
    };
    let ev = match parse_bitbucket(&event, &payload) {
        Some(e) => e,
        None => return "Nothing to do. Event not handled.".into_response(),
    };
    let full_name = match s(&payload, "/repository/full_name").and_then(|f| normalize_full_name(&f))
    {
        Some(f) => f,
        None => return "Nothing to do. Invalid repository.".into_response(),
    };
    let sig =
        strip_sha256_prefix(&header(&headers, "X-Hub-Signature").unwrap_or_default()).to_string();
    let branch = event_branch(&ev);
    let candidates = match ApplicationRepo::new(state.pool.clone())
        .list_by_branch(&branch)
        .await
    {
        Ok(a) => a,
        Err(e) => return internal(e.to_string()),
    };
    let matched = filter_manual(&state, candidates, &full_name, "bitbucket", |secret| {
        verify_hmac_sha256(secret.as_bytes(), &body, &sig)
    })
    .await;
    run_for_apps(&state, matched, "bitbucket", &ev).await
}

// --------------------------------------------------------------------------
// Shared helpers
// --------------------------------------------------------------------------

fn internal(msg: String) -> Response {
    (StatusCode::INTERNAL_SERVER_ERROR, msg).into_response()
}

/// The branch a matched application must be on: push → the pushed branch, PR →
/// the base (target) branch.
fn event_branch(ev: &Event) -> String {
    match ev {
        Event::Push { branch, .. } => branch.clone(),
        Event::Pull { base_branch, .. } => base_branch.clone(),
        _ => String::new(),
    }
}

/// Narrow manual-mode candidates to those whose canonical `owner/repo` matches
/// `full_name` and whose per-provider secret verifies via `check`.
async fn filter_manual(
    state: &AppState,
    candidates: Vec<Application>,
    full_name: &str,
    git_type: &str,
    check: impl Fn(&str) -> bool,
) -> Vec<Application> {
    let repo = ApplicationRepo::new(state.pool.clone());
    let mut out = Vec::new();
    for app in candidates {
        if canonical_repository(&app.git_repository).as_deref() != Some(full_name) {
            continue;
        }
        let secret = match repo.decrypt_manual_webhook_secret(app.id, git_type).await {
            Ok(Some(s)) => s,
            _ => continue, // no secret ⇒ invalid signature
        };
        if check(&secret) {
            out.push(app);
        }
    }
    out
}

/// Run the parsed event against every matched application, returning the JSON
/// array of per-application results (or a 429 if the queue filled).
async fn run_for_apps(
    state: &AppState,
    apps: Vec<Application>,
    git_type: &str,
    ev: &Event,
) -> Response {
    let mut results = Vec::new();
    for app in &apps {
        let outcome = match ev {
            Event::Push { .. } => dispatch_push(state, app, ev).await,
            Event::Pull { .. } => dispatch_pull(state, app, git_type, ev).await,
            _ => {
                Ok(json!({ "application": app.name, "status": "failed", "message": "unsupported" }))
            }
        };
        match outcome {
            Ok(v) => results.push(v),
            Err(resp) => return resp, // 429 short-circuits the whole request
        }
    }
    axum::Json(results).into_response()
}

// --------------------------------------------------------------------------
// Per-provider payload parsing → Event
// --------------------------------------------------------------------------

/// GitHub + Gitea share a payload shape (`ref`, `pull_request.*`).
fn parse_github_like(provider: Provider, event: &str, payload: &Value) -> Option<Event> {
    match event {
        "ping" => Some(Event::Ping),
        "push" => {
            let git_ref = s(payload, "/ref")?;
            let branch = strip_refs_heads(&git_ref).to_string();
            let commit = s(payload, "/after");
            let messages: Vec<Option<String>> = payload
                .pointer("/commits")
                .and_then(|c| c.as_array())
                .map(|arr| arr.iter().map(|c| s(c, "/message")).collect())
                .unwrap_or_default();
            let skip = should_skip_deploy_all(&messages);
            Some(Event::Push {
                branch,
                commit,
                skip,
            })
        }
        "pull_request" => {
            let action_str = s(payload, "/action").unwrap_or_default();
            let action = classify_pr_action(provider, &action_str);
            let pr = i(payload, "/number")? as i32;
            let base_branch = s(payload, "/pull_request/base/ref")?;
            let commit = s(payload, "/pull_request/head/sha");
            let author_association = s(payload, "/pull_request/author_association");
            let is_fork = webhook::is_fork_pull_request(
                i(payload, "/pull_request/head/repo/id"),
                i(payload, "/pull_request/base/repo/id"),
            );
            let pr_deployable_ok = !should_skip_deploy_any(&[s(payload, "/pull_request/title")]);
            Some(Event::Pull {
                action,
                pr,
                html_url: s(payload, "/pull_request/html_url"),
                base_branch,
                commit,
                author_association,
                is_fork,
                pr_deployable_ok,
            })
        }
        _ => None,
    }
}

fn parse_gitlab(kind: &str, payload: &Value) -> Option<Event> {
    match kind {
        "push" => {
            let git_ref = s(payload, "/ref")?;
            let branch = strip_refs_heads(&git_ref).to_string();
            let commit = s(payload, "/after");
            let messages: Vec<Option<String>> = payload
                .pointer("/commits")
                .and_then(|c| c.as_array())
                .map(|arr| arr.iter().map(|c| s(c, "/message")).collect())
                .unwrap_or_default();
            Some(Event::Push {
                branch,
                commit,
                skip: should_skip_deploy_all(&messages),
            })
        }
        "merge_request" => {
            let action_str = s(payload, "/object_attributes/action").unwrap_or_default();
            let action = classify_pr_action(Provider::Gitlab, &action_str);
            let pr = i(payload, "/object_attributes/iid")? as i32;
            let base_branch = s(payload, "/object_attributes/target_branch")?;
            let commit = s(payload, "/object_attributes/last_commit/id");
            let title = s(payload, "/object_attributes/title");
            let latest = s(payload, "/object_attributes/last_commit/message");
            let pr_deployable_ok = !should_skip_deploy_any(&[title, latest]);
            Some(Event::Pull {
                action,
                pr,
                html_url: s(payload, "/object_attributes/url"),
                base_branch,
                commit,
                author_association: None,
                is_fork: false,
                pr_deployable_ok,
            })
        }
        _ => None,
    }
}

fn parse_bitbucket(event_key: &str, payload: &Value) -> Option<Event> {
    match event_key {
        "repo:push" => {
            let branch = s(payload, "/push/changes/0/new/name")?;
            let commit = s(payload, "/push/changes/0/new/target/hash");
            let messages: Vec<Option<String>> = payload
                .pointer("/push/changes/0/commits")
                .and_then(|c| c.as_array())
                .map(|arr| arr.iter().map(|c| s(c, "/message")).collect())
                .unwrap_or_default();
            Some(Event::Push {
                branch,
                commit,
                skip: should_skip_deploy_all(&messages),
            })
        }
        "pullrequest:created"
        | "pullrequest:updated"
        | "pullrequest:rejected"
        | "pullrequest:fulfilled" => {
            let action = classify_pr_action(Provider::Bitbucket, event_key);
            let pr = i(payload, "/pullrequest/id")? as i32;
            // Bitbucket's `destination` is the target (base) branch.
            let base_branch = s(payload, "/pullrequest/destination/branch/name")?;
            let commit = s(payload, "/pullrequest/source/commit/hash");
            let pr_deployable_ok = !should_skip_deploy_any(&[s(payload, "/pullrequest/title")]);
            Some(Event::Pull {
                action,
                pr,
                html_url: s(payload, "/pullrequest/links/html/href"),
                base_branch,
                commit,
                author_association: None,
                is_fork: false,
                pr_deployable_ok,
            })
        }
        _ => None,
    }
}
