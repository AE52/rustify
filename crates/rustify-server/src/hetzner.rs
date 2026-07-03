//! Hetzner Cloud API client.
//!
//! Behavioural port of Coolify's `HetznerService`
//! (app/Services/HetznerService.php): a bearer-authenticated REST client for
//! `https://api.hetzner.cloud/v1` with a 30s timeout, up to 3 retries honouring
//! `RateLimit-Reset`/`Retry-After` (capped at 60s), and cursor pagination
//! (`per_page=50`, following `meta.pagination.next_page`).
//!
//! The HTTP layer is abstracted behind [`HetznerTransport`] so the client's
//! retry/pagination/body logic is unit-tested without network access; production
//! uses [`ReqwestTransport`].

use async_trait::async_trait;
use md5::{Digest, Md5};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// Base URL of the Hetzner Cloud API.
pub const BASE_URL: &str = "https://api.hetzner.cloud/v1";
/// Request timeout (Coolify: 30s).
const TIMEOUT_SECS: u64 = 30;
/// Maximum attempts per request (Coolify: `retry(3, …)`).
const MAX_ATTEMPTS: u32 = 3;
/// Cap on the rate-limit backoff wait, in seconds (Coolify: `min($wait, 60)`).
const RATE_LIMIT_CAP_SECS: i64 = 60;
/// Page size for paginated list endpoints (Coolify: `per_page = 50`).
const PER_PAGE: u32 = 50;

#[derive(Debug, thiserror::Error)]
pub enum HetznerError {
    #[error("hetzner rate limit exceeded")]
    RateLimited { retry_after: Option<i64> },
    #[error("hetzner api error: {0}")]
    Api(String),
    #[error("hetzner transport error: {0}")]
    Transport(String),
    #[error("hetzner response decode error: {0}")]
    Decode(String),
}

/// One HTTP request the client wants performed.
pub struct HetznerHttpRequest {
    pub method: String,
    pub url: String,
    pub token: String,
    pub body: Option<Value>,
}

/// The pieces of an HTTP response the client inspects.
pub struct HetznerHttpResponse {
    pub status: u16,
    pub retry_after: Option<i64>,
    pub rate_limit_reset: Option<i64>,
    pub body: Value,
}

impl HetznerHttpResponse {
    fn is_success(&self) -> bool {
        (200..300).contains(&self.status)
    }
}

/// Pluggable HTTP transport (production: reqwest; tests: a fake).
#[async_trait]
pub trait HetznerTransport: Send + Sync {
    async fn send(&self, req: HetznerHttpRequest) -> Result<HetznerHttpResponse, HetznerError>;
}

/// Parameters for `POST /servers`. Serialises to exactly the body Coolify sends
/// (app/Livewire/Server/New/ByHetzner.php:444-455).
#[derive(Debug, Clone, Serialize)]
pub struct CreateServerParams {
    pub name: String,
    pub server_type: String,
    pub image: i64,
    pub location: String,
    pub start_after_create: bool,
    pub ssh_keys: Vec<i64>,
    pub public_net: PublicNetRequest,
}

#[derive(Debug, Clone, Serialize)]
pub struct PublicNetRequest {
    pub enable_ipv4: bool,
    pub enable_ipv6: bool,
}

impl CreateServerParams {
    /// Build the create-server body. The name is lowercased for RFC 1123
    /// compliance (Coolify: `strtolower(trim(...))`).
    pub fn new(
        name: &str,
        server_type: &str,
        image: i64,
        location: &str,
        ssh_keys: Vec<i64>,
    ) -> Self {
        Self {
            name: name.trim().to_lowercase(),
            server_type: server_type.to_string(),
            image,
            location: location.to_string(),
            start_after_create: true,
            ssh_keys,
            public_net: PublicNetRequest {
                enable_ipv4: true,
                enable_ipv6: true,
            },
        }
    }
}

/// A Hetzner server object (the subset Rustify needs).
#[derive(Debug, Clone, Deserialize)]
pub struct HetznerServer {
    pub id: i64,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub public_net: PublicNet,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct PublicNet {
    #[serde(default)]
    pub ipv4: Option<IpField>,
    #[serde(default)]
    pub ipv6: Option<IpField>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct IpField {
    #[serde(default)]
    pub ip: Option<String>,
}

impl HetznerServer {
    /// Preferred public IP: IPv4 when present, else IPv6.
    pub fn public_ip(&self) -> Option<String> {
        self.public_net
            .ipv4
            .as_ref()
            .and_then(|f| f.ip.clone())
            .filter(|s| !s.is_empty())
            .or_else(|| {
                self.public_net
                    .ipv6
                    .as_ref()
                    .and_then(|f| f.ip.clone())
                    .filter(|s| !s.is_empty())
            })
    }
}

/// The Hetzner client, generic over its transport.
pub struct HetznerClient<T: HetznerTransport> {
    transport: T,
    token: String,
    base_url: String,
}

impl<T: HetznerTransport> HetznerClient<T> {
    pub fn new(transport: T, token: impl Into<String>) -> Self {
        Self {
            transport,
            token: token.into(),
            base_url: BASE_URL.to_string(),
        }
    }

    /// Override the base URL (tests).
    pub fn with_base_url(mut self, base_url: impl Into<String>) -> Self {
        self.base_url = base_url.into();
        self
    }

    fn build_url(&self, path: &str, query: &[(String, String)]) -> String {
        if query.is_empty() {
            format!("{}{}", self.base_url, path)
        } else {
            let qs = query
                .iter()
                .map(|(k, v)| format!("{k}={v}"))
                .collect::<Vec<_>>()
                .join("&");
            format!("{}{}?{}", self.base_url, path, qs)
        }
    }

    /// Perform one request with retry/rate-limit handling.
    async fn request(
        &self,
        method: &str,
        path: &str,
        query: &[(String, String)],
        body: Option<Value>,
    ) -> Result<Value, HetznerError> {
        let mut attempt = 0;
        loop {
            attempt += 1;
            let req = HetznerHttpRequest {
                method: method.to_string(),
                url: self.build_url(path, query),
                token: self.token.clone(),
                body: body.clone(),
            };
            match self.transport.send(req).await {
                Ok(resp) if resp.status == 429 => {
                    let now = chrono::Utc::now().timestamp();
                    let wait = resp
                        .rate_limit_reset
                        .map(|reset| (reset - now).clamp(0, RATE_LIMIT_CAP_SECS))
                        .unwrap_or(0);
                    if attempt < MAX_ATTEMPTS {
                        if wait > 0 {
                            tokio::time::sleep(std::time::Duration::from_secs(wait as u64)).await;
                        }
                        continue;
                    }
                    let retry_after = resp
                        .retry_after
                        .or_else(|| resp.rate_limit_reset.map(|reset| (reset - now).max(0)));
                    return Err(HetznerError::RateLimited { retry_after });
                }
                Ok(resp) if resp.is_success() => return Ok(resp.body),
                Ok(resp) => {
                    let message = resp
                        .body
                        .pointer("/error/message")
                        .and_then(Value::as_str)
                        .unwrap_or("Unknown error")
                        .to_string();
                    return Err(HetznerError::Api(message));
                }
                Err(e) => {
                    if attempt < MAX_ATTEMPTS {
                        // Exponential-ish backoff: 100ms, 200ms (Coolify).
                        tokio::time::sleep(std::time::Duration::from_millis(
                            (attempt as u64) * 100,
                        ))
                        .await;
                        continue;
                    }
                    return Err(e);
                }
            }
        }
    }

    /// Follow cursor pagination, concatenating `resource_key` arrays across
    /// pages until `meta.pagination.next_page` is null.
    async fn request_paginated(
        &self,
        path: &str,
        resource_key: &str,
        extra_query: &[(String, String)],
    ) -> Result<Vec<Value>, HetznerError> {
        let mut all = Vec::new();
        let mut page = 1u32;
        loop {
            let mut query: Vec<(String, String)> = extra_query.to_vec();
            query.push(("page".to_string(), page.to_string()));
            query.push(("per_page".to_string(), PER_PAGE.to_string()));
            let body = self.request("GET", path, &query, None).await?;
            if let Some(items) = body.get(resource_key).and_then(Value::as_array) {
                all.extend(items.iter().cloned());
            }
            let next = body
                .pointer("/meta/pagination/next_page")
                .and_then(|v| if v.is_null() { None } else { v.as_u64() });
            match next {
                Some(n) => page = n as u32,
                None => break,
            }
        }
        Ok(all)
    }

    pub async fn get_locations(&self) -> Result<Vec<Value>, HetznerError> {
        self.request_paginated("/locations", "locations", &[]).await
    }

    pub async fn get_images(&self) -> Result<Vec<Value>, HetznerError> {
        self.request_paginated(
            "/images",
            "images",
            &[("type".to_string(), "system".to_string())],
        )
        .await
    }

    pub async fn get_server_types(&self) -> Result<Vec<Value>, HetznerError> {
        let types = self
            .request_paginated("/server_types", "server_types", &[])
            .await?;
        // Drop entries explicitly marked deprecated (Coolify getServerTypes).
        Ok(types
            .into_iter()
            .filter(|t| t.get("deprecated").and_then(Value::as_bool) != Some(true))
            .collect())
    }

    pub async fn get_ssh_keys(&self) -> Result<Vec<Value>, HetznerError> {
        self.request_paginated("/ssh_keys", "ssh_keys", &[]).await
    }

    pub async fn upload_ssh_key(
        &self,
        name: &str,
        public_key: &str,
    ) -> Result<Value, HetznerError> {
        let body = self
            .request(
                "POST",
                "/ssh_keys",
                &[],
                Some(json!({ "name": name, "public_key": public_key })),
            )
            .await?;
        Ok(body.get("ssh_key").cloned().unwrap_or(Value::Null))
    }

    /// Ensure our public key exists on Hetzner (dedupe by MD5 fingerprint),
    /// returning the Hetzner ssh-key id.
    pub async fn ensure_ssh_key(&self, name: &str, public_key: &str) -> Result<i64, HetznerError> {
        let fingerprint = md5_fingerprint(public_key);
        let existing = self.get_ssh_keys().await?;
        if let (Some(fp), Some(found)) = (
            &fingerprint,
            existing.iter().find(|k| {
                fingerprint
                    .as_deref()
                    .is_some_and(|f| k.get("fingerprint").and_then(Value::as_str) == Some(f))
            }),
        ) {
            let _ = fp;
            if let Some(id) = found.get("id").and_then(Value::as_i64) {
                return Ok(id);
            }
        }
        let uploaded = self.upload_ssh_key(name, public_key).await?;
        uploaded
            .get("id")
            .and_then(Value::as_i64)
            .ok_or_else(|| HetznerError::Decode("uploaded ssh key has no id".to_string()))
    }

    pub async fn create_server(
        &self,
        params: &CreateServerParams,
    ) -> Result<HetznerServer, HetznerError> {
        let body_json =
            serde_json::to_value(params).map_err(|e| HetznerError::Decode(e.to_string()))?;
        let body = self
            .request("POST", "/servers", &[], Some(body_json))
            .await?;
        let server = body.get("server").cloned().ok_or_else(|| {
            HetznerError::Decode("create server response has no server".to_string())
        })?;
        serde_json::from_value(server).map_err(|e| HetznerError::Decode(e.to_string()))
    }

    pub async fn get_server(&self, id: i64) -> Result<HetznerServer, HetznerError> {
        let body = self
            .request("GET", &format!("/servers/{id}"), &[], None)
            .await?;
        let server = body
            .get("server")
            .cloned()
            .ok_or_else(|| HetznerError::Decode("get server response has no server".to_string()))?;
        serde_json::from_value(server).map_err(|e| HetznerError::Decode(e.to_string()))
    }

    pub async fn power_on_server(&self, id: i64) -> Result<(), HetznerError> {
        self.request("POST", &format!("/servers/{id}/actions/poweron"), &[], None)
            .await?;
        Ok(())
    }

    pub async fn delete_server(&self, id: i64) -> Result<(), HetznerError> {
        self.request("DELETE", &format!("/servers/{id}"), &[], None)
            .await?;
        Ok(())
    }
}

/// Compute an SSH public key's MD5 fingerprint as colon-separated lowercase hex
/// (matches Hetzner's `ssh_keys[].fingerprint`). Returns `None` if the key isn't
/// a valid `<type> <base64>` string.
pub fn md5_fingerprint(public_key: &str) -> Option<String> {
    use base64::Engine as _;
    let blob_b64 = public_key.split_whitespace().nth(1)?;
    let blob = base64::engine::general_purpose::STANDARD
        .decode(blob_b64)
        .ok()?;
    let digest = Md5::digest(&blob);
    Some(
        digest
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<Vec<_>>()
            .join(":"),
    )
}

/// Production transport backed by a shared `reqwest::Client`.
pub struct ReqwestTransport {
    client: reqwest::Client,
}

impl ReqwestTransport {
    pub fn new() -> Self {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(TIMEOUT_SECS))
            .build()
            .unwrap_or_default();
        Self { client }
    }
}

impl Default for ReqwestTransport {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl HetznerTransport for ReqwestTransport {
    async fn send(&self, req: HetznerHttpRequest) -> Result<HetznerHttpResponse, HetznerError> {
        let method = reqwest::Method::from_bytes(req.method.as_bytes())
            .map_err(|e| HetznerError::Transport(e.to_string()))?;
        let mut builder = self
            .client
            .request(method, &req.url)
            .bearer_auth(&req.token);
        if let Some(body) = &req.body {
            builder = builder.json(body);
        }
        let resp = builder
            .send()
            .await
            .map_err(|e| HetznerError::Transport(e.to_string()))?;
        let status = resp.status().as_u16();
        let header_i64 = |resp: &reqwest::Response, name: &str| -> Option<i64> {
            resp.headers()
                .get(name)
                .and_then(|v| v.to_str().ok())
                .and_then(|s| s.trim().parse::<i64>().ok())
        };
        let retry_after = header_i64(&resp, "Retry-After");
        let rate_limit_reset = header_i64(&resp, "RateLimit-Reset");
        let body: Value = resp.json().await.unwrap_or(Value::Null);
        Ok(HetznerHttpResponse {
            status,
            retry_after,
            rate_limit_reset,
            body,
        })
    }
}

/// Periodic Hetzner power-state sync (a scheduler task, mirrors the deploy
/// engine's `status_sync_task`). For every Hetzner-provisioned server: fetch the
/// live server, cache its status, and power it on when it reports `off`.
pub fn hetzner_status_sync_task(
    pool: sqlx::PgPool,
) -> impl Fn() -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>> + Send + 'static {
    move || {
        let pool = pool.clone();
        Box::pin(async move {
            if let Err(e) = sync_hetzner_all(&pool).await {
                tracing::warn!(error = %e, "hetzner status sync failed");
            }
        })
    }
}

/// One Hetzner power-state reconciliation sweep across all provisioned servers.
pub async fn sync_hetzner_all(pool: &sqlx::PgPool) -> Result<(), sqlx::Error> {
    let repo = rustify_db::repos::ServerRepo::new(pool.clone());
    let servers = repo
        .hetzner_servers()
        .await
        .map_err(|e| sqlx::Error::Protocol(e.to_string()))?;
    for server in servers {
        let (Some(hz_id), Some(token_id)) =
            (server.hetzner_server_id, server.cloud_provider_token_id)
        else {
            continue;
        };
        // Decrypt the owning team's token.
        let enc: Option<Vec<u8>> =
            sqlx::query_scalar("SELECT token_enc FROM cloud_provider_tokens WHERE id = $1")
                .bind(token_id)
                .fetch_optional(pool)
                .await?;
        let Some(enc) = enc else { continue };
        let Ok(plain) = rustify_core::crypto::decrypt(&enc) else {
            continue;
        };
        let Ok(token) = String::from_utf8(plain) else {
            continue;
        };

        let client = HetznerClient::new(ReqwestTransport::new(), token);
        let live = match client.get_server(hz_id).await {
            Ok(s) => s,
            Err(e) => {
                tracing::debug!(server = %server.uuid, error = %e, "hetzner get_server failed");
                continue;
            }
        };
        if let Some(status) = &live.status {
            let _ = repo.set_hetzner_status(server.id, status).await;
            if status == "off" {
                let _ = client.power_on_server(hz_id).await;
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// Scripted transport: pops a queued response per call and records requests.
    struct FakeTransport {
        responses: Mutex<Vec<HetznerHttpResponse>>,
        requests: Mutex<Vec<(String, String, Option<Value>)>>,
    }

    impl FakeTransport {
        fn new(responses: Vec<HetznerHttpResponse>) -> Self {
            Self {
                responses: Mutex::new(responses.into_iter().rev().collect()),
                requests: Mutex::new(Vec::new()),
            }
        }
        fn requests(&self) -> Vec<(String, String, Option<Value>)> {
            self.requests.lock().unwrap().clone()
        }
    }

    #[async_trait]
    impl HetznerTransport for FakeTransport {
        async fn send(&self, req: HetznerHttpRequest) -> Result<HetznerHttpResponse, HetznerError> {
            self.requests.lock().unwrap().push((
                req.method.clone(),
                req.url.clone(),
                req.body.clone(),
            ));
            self.responses
                .lock()
                .unwrap()
                .pop()
                .ok_or_else(|| HetznerError::Transport("no scripted response".to_string()))
        }
    }

    fn ok(body: Value) -> HetznerHttpResponse {
        HetznerHttpResponse {
            status: 200,
            retry_after: None,
            rate_limit_reset: None,
            body,
        }
    }

    #[test]
    fn create_server_body_matches_golden() {
        let params = CreateServerParams::new("My-Server", "cx22", 114690387, "nbg1", vec![7, 9]);
        let got = serde_json::to_value(&params).unwrap();
        let want = json!({
            "name": "my-server",
            "server_type": "cx22",
            "image": 114690387,
            "location": "nbg1",
            "start_after_create": true,
            "ssh_keys": [7, 9],
            "public_net": { "enable_ipv4": true, "enable_ipv6": true }
        });
        assert_eq!(got, want);
    }

    #[test]
    fn md5_fingerprint_is_colon_hex() {
        // "ssh-ed25519 <base64 of 4 zero bytes>" → md5 of those 4 bytes.
        let key = "ssh-ed25519 AAAAAA==";
        let fp = md5_fingerprint(key).unwrap();
        assert_eq!(fp.matches(':').count(), 15, "16 hex byte groups");
        assert!(fp.chars().all(|c| c == ':' || c.is_ascii_hexdigit()));
        assert!(md5_fingerprint("not-a-key").is_none());
    }

    #[tokio::test]
    async fn pagination_follows_next_page() {
        let page1 = json!({
            "locations": [{"name": "nbg1"}, {"name": "fsn1"}],
            "meta": {"pagination": {"next_page": 2}}
        });
        let page2 = json!({
            "locations": [{"name": "hel1"}],
            "meta": {"pagination": {"next_page": null}}
        });
        let fake = FakeTransport::new(vec![ok(page1), ok(page2)]);
        let client = HetznerClient::new(fake, "tok").with_base_url("http://test");
        let locations = client.get_locations().await.unwrap();
        assert_eq!(locations.len(), 3);
        assert_eq!(locations[2]["name"], "hel1");
    }

    #[tokio::test]
    async fn pagination_sends_per_page_50() {
        let page = json!({ "server_types": [], "meta": {"pagination": {"next_page": null}} });
        let fake = FakeTransport::new(vec![ok(page)]);
        let client = HetznerClient::new(fake, "tok").with_base_url("http://test");
        let _ = client.get_server_types().await.unwrap();
        let reqs = client.transport.requests();
        assert_eq!(reqs.len(), 1);
        assert!(reqs[0].1.contains("per_page=50"), "url: {}", reqs[0].1);
        assert!(reqs[0].1.contains("page=1"));
    }

    #[tokio::test]
    async fn rate_limit_retries_then_succeeds() {
        // First a 429 with a reset in the past (wait = 0), then a 200.
        let past = chrono::Utc::now().timestamp() - 5;
        let limited = HetznerHttpResponse {
            status: 429,
            retry_after: None,
            rate_limit_reset: Some(past),
            body: Value::Null,
        };
        let success =
            ok(json!({ "server": { "id": 42, "public_net": { "ipv4": { "ip": "1.2.3.4" } } } }));
        let fake = FakeTransport::new(vec![limited, success]);
        let client = HetznerClient::new(fake, "tok").with_base_url("http://test");
        let params = CreateServerParams::new("s", "cx22", 1, "nbg1", vec![1]);
        let server = client.create_server(&params).await.unwrap();
        assert_eq!(server.id, 42);
        assert_eq!(server.public_ip().as_deref(), Some("1.2.3.4"));
        // Two attempts were made (retry after the 429).
        assert_eq!(client.transport.requests().len(), 2);
    }

    #[tokio::test]
    async fn rate_limit_gives_up_after_max_attempts() {
        let past = chrono::Utc::now().timestamp() - 5;
        let mk = || HetznerHttpResponse {
            status: 429,
            retry_after: Some(30),
            rate_limit_reset: Some(past),
            body: Value::Null,
        };
        let fake = FakeTransport::new(vec![mk(), mk(), mk()]);
        let client = HetznerClient::new(fake, "tok").with_base_url("http://test");
        let err = client.get_locations().await.unwrap_err();
        match err {
            HetznerError::RateLimited { retry_after } => assert_eq!(retry_after, Some(30)),
            other => panic!("expected RateLimited, got {other:?}"),
        }
        assert_eq!(client.transport.requests().len(), 3);
    }

    #[tokio::test]
    async fn deprecated_server_types_filtered() {
        let page = json!({
            "server_types": [
                {"name": "cx11", "deprecated": true},
                {"name": "cx22", "deprecated": false},
                {"name": "cx32"}
            ],
            "meta": {"pagination": {"next_page": null}}
        });
        let fake = FakeTransport::new(vec![ok(page)]);
        let client = HetznerClient::new(fake, "tok").with_base_url("http://test");
        let types = client.get_server_types().await.unwrap();
        let names: Vec<&str> = types.iter().filter_map(|t| t["name"].as_str()).collect();
        assert_eq!(names, vec!["cx22", "cx32"]);
    }

    #[tokio::test]
    async fn ensure_ssh_key_dedupes_by_fingerprint() {
        let key = "ssh-ed25519 AAAAAA==";
        let fp = md5_fingerprint(key).unwrap();
        let list = json!({
            "ssh_keys": [{"id": 55, "fingerprint": fp}],
            "meta": {"pagination": {"next_page": null}}
        });
        let fake = FakeTransport::new(vec![ok(list)]);
        let client = HetznerClient::new(fake, "tok").with_base_url("http://test");
        let id = client.ensure_ssh_key("k", key).await.unwrap();
        assert_eq!(id, 55);
        // Only the list call — no upload, since the key already exists.
        assert_eq!(client.transport.requests().len(), 1);
    }

    #[tokio::test]
    async fn ensure_ssh_key_uploads_when_absent() {
        let key = "ssh-ed25519 AAAAAA==";
        let list = json!({ "ssh_keys": [], "meta": {"pagination": {"next_page": null}} });
        let uploaded = ok(json!({ "ssh_key": { "id": 88 } }));
        let fake = FakeTransport::new(vec![ok(list), uploaded]);
        let client = HetznerClient::new(fake, "tok").with_base_url("http://test");
        let id = client.ensure_ssh_key("k", key).await.unwrap();
        assert_eq!(id, 88);
        let reqs = client.transport.requests();
        assert_eq!(reqs.len(), 2);
        assert_eq!(reqs[1].0, "POST");
        assert!(reqs[1].1.ends_with("/ssh_keys"));
    }
}
