//! End-to-end harness (contract C5), gated behind the `e2e` feature.
//!
//! This exercises the *whole* product against real infrastructure: a Postgres
//! and a privileged docker-in-docker "testhost" (see `tests/e2e/compose.yml`).
//! It spawns the real `rustify-server` binary, drives it purely over the public
//! REST API with `reqwest`, and asserts side effects on the target host over
//! SSH (`docker ps`, `curl`).
//!
//! SCOPE: the full flow can only pass once Task Z wires the deploy engine +
//! job handlers into `main.rs`. It is written now against the pinned contract
//! so it passes unchanged the moment the binary is complete. Until then the
//! infrastructure is verified independently by `scripts/e2e-smoke.sh`.
//!
//! Run via `make e2e` (never the default test gate). Expects these env vars,
//! all set by the Makefile: `E2E_BASE_URL`, `E2E_SSH_HOST`, `E2E_SSH_PORT`,
//! `E2E_SSH_KEY`, plus the server's own `DATABASE_URL`, `RUSTIFY_SECRET_KEY`,
//! `RUSTIFY_ADMIN_EMAIL`, `RUSTIFY_ADMIN_PASSWORD` (inherited by the child).
#![cfg(feature = "e2e")]

use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

/// Read a required env var or panic with a helpful message.
fn env(key: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| panic!("{key} must be set (see Makefile `e2e` target)"))
}

fn base_url() -> String {
    std::env::var("E2E_BASE_URL").unwrap_or_else(|_| "http://127.0.0.1:8000".to_string())
}

/// A spawned `rustify-server` process, killed on drop so a panicking test never
/// leaks the child or holds the listen port.
struct ServerProc(Child);

impl Drop for ServerProc {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// Spawn the server binary (inheriting `DATABASE_URL` etc. from the Makefile)
/// and block until `/health` answers or we time out.
async fn start_server(client: &reqwest::Client) -> ServerProc {
    // `CARGO_BIN_EXE_<name>` is injected by Cargo for integration tests.
    let bin = env!("CARGO_BIN_EXE_rustify-server");
    let child = Command::new(bin)
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .spawn()
        .expect("spawn rustify-server");
    let proc = ServerProc(child);

    let health = format!("{}/api/v1/health", base_url());
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        if let Ok(resp) = client.get(&health).send().await {
            if resp.status().is_success() {
                return proc;
            }
        }
        assert!(
            Instant::now() < deadline,
            "server never became healthy at {health}"
        );
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
}

/// SSH into the testhost with the fixture key and return captured stdout,
/// asserting the remote command succeeded.
async fn ssh(cmd: &str) -> String {
    let host = std::env::var("E2E_SSH_HOST").unwrap_or_else(|_| "127.0.0.1".to_string());
    let port = std::env::var("E2E_SSH_PORT").unwrap_or_else(|_| "2222".to_string());
    let key = env("E2E_SSH_KEY");
    let out = tokio::process::Command::new("ssh")
        .args([
            "-i",
            &key,
            "-p",
            &port,
            "-o",
            "StrictHostKeyChecking=no",
            "-o",
            "UserKnownHostsFile=/dev/null",
            "-o",
            "LogLevel=ERROR",
            "-o",
            "IdentitiesOnly=yes",
            &format!("root@{host}"),
            cmd,
        ])
        .output()
        .await
        .expect("run ssh");
    assert!(
        out.status.success(),
        "ssh `{cmd}` failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// POST helper that returns `(status, json)`.
async fn post(client: &reqwest::Client, path: &str, body: Value) -> (reqwest::StatusCode, Value) {
    let resp = client
        .post(format!("{}{path}", base_url()))
        .json(&body)
        .send()
        .await
        .expect("post");
    let status = resp.status();
    let json = resp.json::<Value>().await.unwrap_or(Value::Null);
    (status, json)
}

/// GET helper that returns `(status, json)`.
async fn get(client: &reqwest::Client, path: &str) -> (reqwest::StatusCode, Value) {
    let resp = client
        .get(format!("{}{path}", base_url()))
        .send()
        .await
        .expect("get");
    let status = resp.status();
    let json = resp.json::<Value>().await.unwrap_or(Value::Null);
    (status, json)
}

/// Poll `GET {path}` until `pick(&json)` is true, or panic after `timeout`.
async fn poll_until<F>(
    client: &reqwest::Client,
    path: &str,
    timeout: Duration,
    mut pick: F,
) -> Value
where
    F: FnMut(&Value) -> bool,
{
    let deadline = Instant::now() + timeout;
    loop {
        let (_status, json) = get(client, path).await;
        if pick(&json) {
            return json;
        }
        assert!(
            Instant::now() < deadline,
            "condition on {path} not met within {timeout:?}; last body: {json}"
        );
        tokio::time::sleep(Duration::from_secs(3)).await;
    }
}

async fn login(client: &reqwest::Client) {
    let (status, _) = post(
        client,
        "/api/v1/auth/login",
        json!({ "email": env("RUSTIFY_ADMIN_EMAIL"), "password": env("RUSTIFY_ADMIN_PASSWORD") }),
    )
    .await;
    assert_eq!(status, reqwest::StatusCode::OK, "admin login");
}

/// Register the committed fixture private key and return its uuid.
async fn create_fixture_key(client: &reqwest::Client) -> String {
    let pem = std::fs::read_to_string(env("E2E_SSH_KEY")).expect("read fixture private key");
    let (status, key) = post(
        client,
        "/api/v1/private-keys",
        json!({ "name": "e2e-fixture", "private_key": pem }),
    )
    .await;
    assert_eq!(status, reqwest::StatusCode::CREATED, "create key: {key}");
    key["uuid"].as_str().unwrap().to_string()
}

/// Create + validate the testhost as a server; return its uuid once `usable`.
async fn create_validated_server(client: &reqwest::Client, key_uuid: &str) -> String {
    let host = std::env::var("E2E_SSH_HOST").unwrap_or_else(|_| "127.0.0.1".to_string());
    let port: i32 = std::env::var("E2E_SSH_PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(2222);
    let (status, server) = post(
        client,
        "/api/v1/servers",
        json!({
            "name": "testhost",
            "ip": host,
            "port": port,
            "user": "root",
            "private_key_uuid": key_uuid,
        }),
    )
    .await;
    assert_eq!(
        status,
        reqwest::StatusCode::CREATED,
        "create server: {server}"
    );
    let uuid = server["uuid"].as_str().unwrap().to_string();

    let (status, _) = post(
        client,
        &format!("/api/v1/servers/{uuid}/validate"),
        json!({}),
    )
    .await;
    assert_eq!(status, reqwest::StatusCode::ACCEPTED, "enqueue validate");

    poll_until(
        client,
        &format!("/api/v1/servers/{uuid}"),
        Duration::from_secs(120),
        |s| s["usable"].as_bool() == Some(true),
    )
    .await;
    uuid
}

/// Create a project and return its uuid (auto-creates `production`).
async fn create_project(client: &reqwest::Client) -> String {
    let (status, project) = post(client, "/api/v1/projects", json!({ "name": "e2e" })).await;
    assert_eq!(
        status,
        reqwest::StatusCode::CREATED,
        "create project: {project}"
    );
    project["uuid"].as_str().unwrap().to_string()
}

/// Create the nixpacks sample app pointing at the file:// bare repo on the
/// testhost, with a fixed host port mapping so we can curl it.
async fn create_app(
    client: &reqwest::Client,
    project_uuid: &str,
    server_uuid: &str,
) -> (String, u16) {
    let host_port: u16 = 3000;
    let (status, app) = post(
        client,
        "/api/v1/applications",
        json!({
            "project_uuid": project_uuid,
            "environment_name": "production",
            "server_uuid": server_uuid,
            "name": "nixpacks-node",
            "git_repository": "file:///srv/git/nixpacks-node.git",
            "git_branch": "master",
            "build_pack": "nixpacks",
            "ports_exposes": "3000",
        }),
    )
    .await;
    assert_eq!(status, reqwest::StatusCode::CREATED, "create app: {app}");
    let uuid = app["uuid"].as_str().unwrap().to_string();

    // Publish a host port so the harness can reach the container directly.
    let resp = client
        .patch(format!("{}/api/v1/applications/{uuid}", base_url()))
        .json(&json!({ "ports_mappings": format!("{host_port}:3000") }))
        .send()
        .await
        .expect("patch app");
    assert!(resp.status().is_success(), "set ports_mappings");
    (uuid, host_port)
}

/// Trigger a deploy and return the deployment uuid.
async fn deploy(client: &reqwest::Client, app_uuid: &str, force_rebuild: bool) -> String {
    let (status, resp) = post(
        client,
        &format!("/api/v1/applications/{app_uuid}/deploy"),
        json!({ "force_rebuild": force_rebuild }),
    )
    .await;
    assert!(status.is_success(), "deploy enqueue ({status}): {resp}");
    resp["deployment_uuid"].as_str().unwrap().to_string()
}

/// Poll a deployment until it reaches a terminal state; returns the detail body.
async fn await_terminal(client: &reqwest::Client, dep_uuid: &str, timeout: Duration) -> Value {
    poll_until(
        client,
        &format!("/api/v1/deployments/{dep_uuid}"),
        timeout,
        |d| {
            matches!(
                d["deployment"]["status"].as_str(),
                Some("finished") | Some("failed") | Some("cancelled")
            )
        },
    )
    .await
}

fn status_of(detail: &Value) -> &str {
    detail["deployment"]["status"].as_str().unwrap_or("")
}

fn logs_of(detail: &Value) -> &Vec<Value> {
    detail["logs"].as_array().expect("logs array")
}

#[tokio::test]
async fn e2e_full_flow() {
    let client = reqwest::Client::builder()
        .cookie_store(true)
        .build()
        .expect("build client");
    let _server = start_server(&client).await;

    // ---- Auth + resources (contract C5) --------------------------------
    login(&client).await;
    let key_uuid = create_fixture_key(&client).await;
    let server_uuid = create_validated_server(&client, &key_uuid).await;
    let project_uuid = create_project(&client).await;
    let (app_uuid, host_port) = create_app(&client, &project_uuid, &server_uuid).await;

    // ---- First deploy: must finish within 5 minutes -------------------
    let dep1 = deploy(&client, &app_uuid, false).await;
    let detail = await_terminal(&client, &dep1, Duration::from_secs(300)).await;
    assert_eq!(status_of(&detail), "finished", "first deploy: {detail}");

    // Logs are present and strictly ordered by `order`.
    let logs = logs_of(&detail);
    assert!(!logs.is_empty(), "deployment logs must be non-empty");
    let orders: Vec<i64> = logs.iter().map(|l| l["order"].as_i64().unwrap()).collect();
    let mut sorted = orders.clone();
    sorted.sort_unstable();
    assert_eq!(orders, sorted, "logs must be ordered by `order`");

    // ---- Container assertions over SSH --------------------------------
    let ps = ssh(&format!(
        "docker ps --filter label=rustify.applicationUuid={app_uuid} \
         --format '{{{{.ID}}}} {{{{.Label \"rustify.managed\"}}}}'"
    ))
    .await;
    assert!(
        ps.contains("true"),
        "expected a managed container for {app_uuid}, got: {ps:?}"
    );

    // The app answers through the published host port on the testhost.
    let body = ssh(&format!(
        "curl -sf --retry 10 --retry-delay 2 http://localhost:{host_port}/"
    ))
    .await;
    assert!(
        body.contains("hello from rustify nixpacks-node"),
        "unexpected app response: {body:?}"
    );

    // ---- Second deploy with no changes hits the build cache -----------
    let dep2 = deploy(&client, &app_uuid, false).await;
    let detail2 = await_terminal(&client, &dep2, Duration::from_secs(300)).await;
    assert_eq!(status_of(&detail2), "finished", "second deploy: {detail2}");
    let skipped = logs_of(&detail2).iter().any(|l| {
        l["content"]
            .as_str()
            .is_some_and(|c| c.contains("Image already exists"))
    });
    assert!(
        skipped,
        "second deploy should skip the build (Image already exists)"
    );

    // ---- Cancel mid-deploy ⇒ Cancelled + helper container gone --------
    let dep3 = deploy(&client, &app_uuid, true).await;
    // Give the helper a moment to come up, then cancel.
    tokio::time::sleep(Duration::from_secs(2)).await;
    let (status, _) = post(
        &client,
        &format!("/api/v1/deployments/{dep3}/cancel"),
        json!({}),
    )
    .await;
    assert_eq!(status, reqwest::StatusCode::ACCEPTED, "cancel enqueue");

    let cancelled = await_terminal(&client, &dep3, Duration::from_secs(120)).await;
    assert_eq!(
        status_of(&cancelled),
        "cancelled",
        "third deploy cancelled: {cancelled}"
    );

    // Helper container is named after the deployment uuid (contract C7).
    let helper = ssh(&format!(
        "docker ps -a --filter name=^/{dep3}$ --format '{{{{.ID}}}}'"
    ))
    .await;
    assert!(
        helper.trim().is_empty(),
        "helper container {dep3} should be removed, got: {helper:?}"
    );
}
