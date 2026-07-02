# Rustify Phase 1 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship the Rustify deploy kernel: onboard a server over SSH, deploy an app from a public git repo through 5 build packs with rolling updates, route it through Traefik with Let's Encrypt, all driven by a React SPA + REST/WS API served from one Rust binary.

**Architecture:** Cargo workspace of 8 crates around a single `rustify-server` binary (axum HTTP+WS, Postgres-backed job queue, tokio scheduler) plus a `web/` React+Vite SPA embedded via rust-embed. All remote effects go through system `ssh` (ControlMaster mux). Parallel tracks are decoupled by traits and pinned contracts defined in this plan (§Contracts).

**Tech Stack:** Rust stable ≥1.85 (edition 2024), axum 0.8, tokio 1, sqlx 0.8 (postgres, runtime-tokio), serde/serde_yaml/serde_json, utoipa 5 + utoipa-swagger-ui, rust-embed 8, argon2 0.5, aes-gcm 0.10, thiserror 2, tracing, React 19 + Vite 6 + TS strict + Tailwind v4 + TanStack Query 5 + React Router 7, openapi-typescript.

**Spec:** `docs/superpowers/specs/2026-07-02-rustify-phase1-design.md` (approved). Coolify reference clone: `~/Desktop/coolify` (read-only; cite behaviors from it, never copy PHP).

## Global Constraints

- **Commits: max 4 words, imperative, lowercase** (e.g. `add ssh mux manager`). **No AI attribution, no Co-Authored-By, no emoji.**
- Branch model: `master` is trunk. Each track works on `track/<name>` in its **own git worktree** (`git worktree add ../rustify-wt-<name> -b track/<name> master` from the repo root; NEVER work directly in the main checkout while other tracks run). PR to `master`, merge fast (squash), delete branch.
- Every task ends green: `cargo fmt --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace` (plus `npm run build` when `web/` changed).
- TDD: write the failing test first for every behavior named in a task's Tests block.
- Encrypted-at-rest columns use `rustify_core::crypto` (AES-256-GCM, key from `RUSTIFY_SECRET_KEY`, 32-byte base64). Never log decrypted values.
- All external IDs are CUID2 via `cuid2` crate; DB rows also have `BIGSERIAL id`.
- SQL naming: snake_case, plural tables, `created_at`/`updated_at TIMESTAMPTZ NOT NULL DEFAULT now()` everywhere.
- Rust naming: types `PascalCase`, fields/functions `snake_case`; no `unwrap()`/`expect()` outside tests; errors via per-crate `thiserror` enums.
- YAGNI: build exactly Phase 1 scope (spec §12 exclusions apply); but keep `team_id` columns per spec §3.

---

## Contracts (pinned; all tracks code against these)

### C1. Core execution trait (in `rustify-core`, implemented by `rustify-ssh`, mocked by `rustify-deploy` tests)

```rust
// crates/rustify-core/src/exec.rs
use tokio::sync::mpsc;

#[derive(Debug, Clone)]
pub struct ServerConn {
    pub uuid: String,
    pub host: String,          // ip or hostname
    pub port: u16,             // ssh port
    pub user: String,          // ssh user
    pub key_path: std::path::PathBuf, // 0600 key file on rustify host
    pub connection_timeout_secs: u32, // default 10
}

#[derive(Debug, Clone, Default)]
pub struct ExecOpts {
    pub timeout_secs: Option<u32>,   // wraps remote cmd in `timeout N`; None = 3600 default
    pub disable_mux: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ExecEvent {
    Stdout(String),   // one line, no trailing \n
    Stderr(String),
}

#[derive(Debug, Clone)]
pub struct ExecOutput {
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
}

#[derive(Debug, thiserror::Error)]
pub enum ExecError {
    #[error("connection failed: {0}")] Connection(String),
    #[error("command failed with exit code {code}: {stderr}")] NonZero { code: i32, stdout: String, stderr: String },
    #[error("timed out after {0}s")] Timeout(u32),
    #[error("io: {0}")] Io(String),
}

#[async_trait::async_trait]
pub trait CommandExecutor: Send + Sync {
    /// Run `script` (multi-line bash) on the server; buffered result. Ok even on non-zero when `allow_failure`—callers use `exec_checked` normally.
    async fn exec(&self, conn: &ServerConn, script: &str, opts: ExecOpts) -> Result<ExecOutput, ExecError>;
    /// Same, but streams line events into `tx` as they arrive, then returns the final output.
    async fn exec_streaming(&self, conn: &ServerConn, script: &str, opts: ExecOpts, tx: mpsc::Sender<ExecEvent>) -> Result<ExecOutput, ExecError>;
    /// scp a local file to remote path.
    async fn upload(&self, conn: &ServerConn, local: &std::path::Path, remote: &str) -> Result<(), ExecError>;
}
```

### C2. Deployment state machine (in `rustify-core`)

```rust
// crates/rustify-core/src/deployment.rs
#[derive(Debug, Clone, Copy, PartialEq, Eq, sqlx::Type, serde::Serialize, serde::Deserialize)]
#[sqlx(type_name = "deployment_status", rename_all = "snake_case")]
#[serde(rename_all = "snake_case")]
pub enum DeploymentStatus { Queued, InProgress, Finished, Failed, Cancelled }

impl DeploymentStatus {
    pub fn is_terminal(self) -> bool { matches!(self, Self::Finished | Self::Failed | Self::Cancelled) }
    /// The ONLY legality check. Queued→InProgress|Cancelled; InProgress→Finished|Failed|Cancelled; terminal→nothing.
    pub fn can_transition_to(self, next: Self) -> bool;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BuildPack { Nixpacks, Dockerfile, Static, DockerImage, DockerCompose }
```

### C3. Log line shape (DB row + WS payload; used by deploy, db, server, web)

```rust
// crates/rustify-core/src/logline.rs
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct LogLine {
    pub order: i64,                 // monotonic per deployment
    pub kind: String,               // "stdout" | "stderr" | "info"
    pub content: String,            // redacted already
    pub hidden: bool,               // internal commands hidden from UI by default
    pub batch: i32,                 // command batch number
    pub timestamp: chrono::DateTime<chrono::Utc>,
}
```

### C4. WS envelope (server→client JSON; `rustify-server` emits, `web` consumes)

```json
{ "channel": "deployment:<uuid>" | "team:<uuid>",
  "event": "deployment_log_appended" | "deployment_status_changed" | "application_status_changed" | "server_reachability_changed",
  "data": { } }
```
Client→server: `{ "action": "subscribe" | "unsubscribe", "channel": "deployment:<uuid>" }`. Auth: same session cookie or `?token=` bearer at upgrade.

### C5. REST surface (all under `/api/v1`, JSON; errors `{"code": "...", "message": "..."}`)

| Method+Path | Body → Response (key fields) |
|---|---|
| POST `/auth/login` | `{email,password}` → sets session cookie, `{user}` |
| POST `/auth/logout` | → 204 |
| GET `/auth/me` | → `{id,email,name}` |
| GET/POST `/private-keys` ; GET/PATCH/DELETE `/private-keys/{uuid}` | `{name,private_key}` → `{uuid,name,public_key}` (private_key write-only) |
| POST `/private-keys/generate` | `{name}` → ed25519 pair, returns public key |
| GET/POST `/servers` ; GET/PATCH/DELETE `/servers/{uuid}` | `{name,ip,port,user,private_key_uuid}` |
| POST `/servers/{uuid}/validate` | → `{job_uuid}` (streams via WS channel `server:<uuid>`) |
| GET `/servers/{uuid}/proxy` / PATCH ... / POST `/servers/{uuid}/proxy/{start\|stop\|restart}` | proxy compose config get/save/lifecycle |
| GET/POST `/projects` ; GET/PATCH/DELETE `/projects/{uuid}` | `{name,description}`; project auto-creates `production` environment |
| GET/POST `/projects/{uuid}/environments` | `{name}` |
| GET/POST `/applications` ; GET/PATCH/DELETE `/applications/{uuid}` | create: `{project_uuid,environment_name,server_uuid,name,git_repository,git_branch,build_pack,ports_exposes,...}` |
| POST `/applications/{uuid}/deploy` | `{force_rebuild?}` → `{deployment_uuid}` |
| POST `/applications/{uuid}/stop` / `/restart` | → 202 |
| GET/POST/PATCH/DELETE `/applications/{uuid}/envs[...]` | `{key,value,is_buildtime,is_literal,is_shown_once}` |
| GET `/applications/{uuid}/logs?lines=100` | container logs (docker logs -n) |
| GET `/deployments?application_uuid=` ; GET `/deployments/{uuid}` (incl. `logs[]` of C3) ; POST `/deployments/{uuid}/cancel` | |
| GET/PATCH `/settings` | instance settings |
| GET/POST/DELETE `/api-tokens` | `{name}` → token shown once |
| GET `/health` | `{status:"ok"}` no auth |

### C6. Database schema (migration `0001_init.sql`, exact)

```sql
CREATE TYPE deployment_status AS ENUM ('queued','in_progress','finished','failed','cancelled');

CREATE TABLE teams (id BIGSERIAL PRIMARY KEY, uuid TEXT UNIQUE NOT NULL, name TEXT NOT NULL,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(), updated_at TIMESTAMPTZ NOT NULL DEFAULT now());
CREATE TABLE users (id BIGSERIAL PRIMARY KEY, uuid TEXT UNIQUE NOT NULL, team_id BIGINT NOT NULL REFERENCES teams(id),
  email TEXT UNIQUE NOT NULL, name TEXT NOT NULL, password_hash TEXT NOT NULL,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(), updated_at TIMESTAMPTZ NOT NULL DEFAULT now());
CREATE TABLE sessions (id TEXT PRIMARY KEY, user_id BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  expires_at TIMESTAMPTZ NOT NULL, created_at TIMESTAMPTZ NOT NULL DEFAULT now());
CREATE TABLE api_tokens (id BIGSERIAL PRIMARY KEY, uuid TEXT UNIQUE NOT NULL, team_id BIGINT NOT NULL REFERENCES teams(id),
  name TEXT NOT NULL, token_hash TEXT UNIQUE NOT NULL, abilities TEXT[] NOT NULL DEFAULT '{read,write,deploy}',
  last_used_at TIMESTAMPTZ, created_at TIMESTAMPTZ NOT NULL DEFAULT now());
CREATE TABLE private_keys (id BIGSERIAL PRIMARY KEY, uuid TEXT UNIQUE NOT NULL, team_id BIGINT NOT NULL REFERENCES teams(id),
  name TEXT NOT NULL, private_key_enc BYTEA NOT NULL, public_key TEXT NOT NULL,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(), updated_at TIMESTAMPTZ NOT NULL DEFAULT now());
CREATE TABLE servers (id BIGSERIAL PRIMARY KEY, uuid TEXT UNIQUE NOT NULL, team_id BIGINT NOT NULL REFERENCES teams(id),
  name TEXT NOT NULL, ip TEXT NOT NULL, port INT NOT NULL DEFAULT 22, ssh_user TEXT NOT NULL DEFAULT 'root',
  private_key_id BIGINT NOT NULL REFERENCES private_keys(id),
  reachable BOOLEAN NOT NULL DEFAULT false, usable BOOLEAN NOT NULL DEFAULT false,
  validation_logs TEXT, created_at TIMESTAMPTZ NOT NULL DEFAULT now(), updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  UNIQUE (ip, port, ssh_user));
CREATE TABLE server_settings (id BIGSERIAL PRIMARY KEY, server_id BIGINT UNIQUE NOT NULL REFERENCES servers(id) ON DELETE CASCADE,
  concurrent_builds INT NOT NULL DEFAULT 2, deployment_queue_limit INT NOT NULL DEFAULT 25,
  dynamic_timeout INT NOT NULL DEFAULT 3600, connection_timeout INT NOT NULL DEFAULT 10,
  proxy_type TEXT NOT NULL DEFAULT 'traefik', proxy_status TEXT NOT NULL DEFAULT 'exited',
  proxy_custom_config TEXT, is_build_server BOOLEAN NOT NULL DEFAULT false,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(), updated_at TIMESTAMPTZ NOT NULL DEFAULT now());
CREATE TABLE destinations (id BIGSERIAL PRIMARY KEY, uuid TEXT UNIQUE NOT NULL,
  server_id BIGINT NOT NULL REFERENCES servers(id) ON DELETE CASCADE, network TEXT NOT NULL,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(), UNIQUE (server_id, network));
CREATE TABLE projects (id BIGSERIAL PRIMARY KEY, uuid TEXT UNIQUE NOT NULL, team_id BIGINT NOT NULL REFERENCES teams(id),
  name TEXT NOT NULL, description TEXT, created_at TIMESTAMPTZ NOT NULL DEFAULT now(), updated_at TIMESTAMPTZ NOT NULL DEFAULT now());
CREATE TABLE environments (id BIGSERIAL PRIMARY KEY, uuid TEXT UNIQUE NOT NULL,
  project_id BIGINT NOT NULL REFERENCES projects(id) ON DELETE CASCADE, name TEXT NOT NULL,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(), UNIQUE (project_id, name));
CREATE TABLE applications (id BIGSERIAL PRIMARY KEY, uuid TEXT UNIQUE NOT NULL,
  environment_id BIGINT NOT NULL REFERENCES environments(id) ON DELETE CASCADE,
  destination_id BIGINT NOT NULL REFERENCES destinations(id),
  name TEXT NOT NULL, fqdn TEXT, git_repository TEXT NOT NULL, git_branch TEXT NOT NULL DEFAULT 'main',
  git_commit_sha TEXT NOT NULL DEFAULT 'HEAD', build_pack TEXT NOT NULL DEFAULT 'nixpacks',
  static_image TEXT NOT NULL DEFAULT 'nginx:alpine', docker_registry_image_name TEXT, docker_registry_image_tag TEXT,
  dockerfile_location TEXT NOT NULL DEFAULT '/Dockerfile', docker_compose_location TEXT NOT NULL DEFAULT '/docker-compose.yaml',
  base_directory TEXT NOT NULL DEFAULT '/', publish_directory TEXT,
  install_command TEXT, build_command TEXT, start_command TEXT,
  ports_exposes TEXT NOT NULL DEFAULT '80', ports_mappings TEXT,
  health_check_enabled BOOLEAN NOT NULL DEFAULT false, health_check_path TEXT NOT NULL DEFAULT '/',
  health_check_port TEXT, health_check_host TEXT NOT NULL DEFAULT 'localhost',
  health_check_method TEXT NOT NULL DEFAULT 'GET', health_check_return_code INT NOT NULL DEFAULT 200,
  health_check_interval INT NOT NULL DEFAULT 5, health_check_timeout INT NOT NULL DEFAULT 5,
  health_check_retries INT NOT NULL DEFAULT 10, health_check_start_period INT NOT NULL DEFAULT 5,
  limits_memory TEXT NOT NULL DEFAULT '0', limits_cpus TEXT NOT NULL DEFAULT '0',
  custom_docker_run_options TEXT, status TEXT NOT NULL DEFAULT 'exited',
  restart_count INT NOT NULL DEFAULT 0, max_restart_count INT NOT NULL DEFAULT 10,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(), updated_at TIMESTAMPTZ NOT NULL DEFAULT now());
CREATE TABLE environment_variables (id BIGSERIAL PRIMARY KEY, uuid TEXT UNIQUE NOT NULL,
  resource_kind TEXT NOT NULL, resource_id BIGINT NOT NULL, key TEXT NOT NULL, value_enc BYTEA NOT NULL,
  is_buildtime BOOLEAN NOT NULL DEFAULT false, is_literal BOOLEAN NOT NULL DEFAULT false,
  is_shown_once BOOLEAN NOT NULL DEFAULT false,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(), updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  UNIQUE (resource_kind, resource_id, key));
CREATE TABLE persistent_storages (id BIGSERIAL PRIMARY KEY, uuid TEXT UNIQUE NOT NULL,
  resource_kind TEXT NOT NULL, resource_id BIGINT NOT NULL, name TEXT NOT NULL,
  mount_path TEXT NOT NULL, host_path TEXT, created_at TIMESTAMPTZ NOT NULL DEFAULT now());
CREATE TABLE deployments (id BIGSERIAL PRIMARY KEY, uuid TEXT UNIQUE NOT NULL,
  application_id BIGINT NOT NULL REFERENCES applications(id) ON DELETE CASCADE,
  server_id BIGINT NOT NULL REFERENCES servers(id),
  status deployment_status NOT NULL DEFAULT 'queued',
  commit_sha TEXT, commit_message TEXT, force_rebuild BOOLEAN NOT NULL DEFAULT false,
  rollback BOOLEAN NOT NULL DEFAULT false, config_snapshot JSONB,
  started_at TIMESTAMPTZ, finished_at TIMESTAMPTZ, created_at TIMESTAMPTZ NOT NULL DEFAULT now());
CREATE INDEX deployments_app_status ON deployments (application_id, status);
CREATE INDEX deployments_server_status ON deployments (server_id, status);
CREATE TABLE deployment_logs (id BIGSERIAL PRIMARY KEY,
  deployment_id BIGINT NOT NULL REFERENCES deployments(id) ON DELETE CASCADE,
  ord BIGINT NOT NULL, kind TEXT NOT NULL, content TEXT NOT NULL,
  hidden BOOLEAN NOT NULL DEFAULT false, batch INT NOT NULL DEFAULT 1,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(), UNIQUE (deployment_id, ord));
CREATE TABLE instance_settings (id BIGSERIAL PRIMARY KEY,
  fqdn TEXT, wildcard_domain TEXT, registration_enabled BOOLEAN NOT NULL DEFAULT false,
  updated_at TIMESTAMPTZ NOT NULL DEFAULT now());
CREATE TABLE jobs (id BIGSERIAL PRIMARY KEY, kind TEXT NOT NULL, payload JSONB NOT NULL,
  run_at TIMESTAMPTZ NOT NULL DEFAULT now(), locked_at TIMESTAMPTZ, locked_by TEXT,
  attempts INT NOT NULL DEFAULT 0, last_error TEXT, created_at TIMESTAMPTZ NOT NULL DEFAULT now());
CREATE INDEX jobs_poll ON jobs (run_at) WHERE locked_at IS NULL;
```

### C7. Docker/label/naming conventions (rustify-docker produces; deploy/proxy/status consume)

- Container name: `{app_uuid}-{6char_random}`; helper container name = `{deployment_uuid}`.
- Image name `{app_uuid}:{commit_sha}`; build stage image `{app_uuid}:{commit_sha}-build`.
- Labels on every managed container: `rustify.managed=true`, `rustify.applicationId={app_id}`, `rustify.applicationUuid={app_uuid}`, `rustify.deploymentId={deployment_uuid}`, plus Traefik: `traefik.enable=true`, `traefik.http.routers.{app_uuid}.rule=Host(`{domain}`)`, `traefik.http.routers.{app_uuid}.entrypoints=http|https`, `traefik.http.routers.{app_uuid}-secure.tls.certresolver=letsencrypt`, `traefik.http.services.{app_uuid}.loadbalancer.server.port={port}`.
- Proxy container: `rustify-proxy`, config dir on server `/data/rustify/proxy`, apps config dir `/data/rustify/applications/{app_uuid}`, artifacts `/artifacts/{deployment_uuid}` (inside helper).
- Default destination network: `rustify`.

---

## File Structure (who owns what)

```
crates/rustify-core/src/{lib,exec,deployment,logline,crypto,ids,error}.rs      # Track A
crates/rustify-db/{migrations/0001_init.sql, src/{lib,pool,repos/*.rs}}        # Track A
crates/rustify-ssh/src/{lib,mux,command,keys,retry}.rs                          # Track B
crates/rustify-docker/src/{lib,build_command,compose,labels,inspect}.rs        # Track C
crates/rustify-proxy/src/{lib,config,lifecycle}.rs                              # Track C
crates/rustify-jobs/src/{lib,queue,scheduler}.rs                                # Track D
crates/rustify-deploy/src/{lib,engine,admission,buildpacks/{mod,nixpacks,dockerfile,static_site,docker_image,compose},rolling,envfile,git,status_sync,server_setup}.rs  # Track E
crates/rustify-server/src/{main,app,auth,ws,error,routes/*.rs,embed.rs}        # Track F
web/src/{main.tsx, api/{client.ts,ws.ts}, routes/*, components/*}               # Track G
tests/e2e/{docker/testhost.Dockerfile, e2e.rs, apps/{nixpacks-node,dockerfile-app}} # Track H
.github/workflows/ci.yml                                                        # Track 0
```

---

### Task 0: Workspace scaffolding (serial — merges to master before all tracks)

**Files:** Create: workspace `Cargo.toml`, all 8 crate skeletons (lib.rs with `#![forbid(unsafe_code)]`), `rust-toolchain.toml` (stable), `.github/workflows/ci.yml` (fmt/clippy/test/web-build jobs), `web/` via `npm create vite@latest` (react-ts) + Tailwind v4 + deps, `docker-compose.dev.yml` (postgres:17 on 5433), `README.md` (3 lines), `LICENSE` (Apache-2.0), `NOTICE` (Coolify attribution), `.env.example` (`DATABASE_URL`, `RUSTIFY_SECRET_KEY`).

**Interfaces:** Produces the compiling empty workspace every track branches from.

- [ ] Scaffold everything above; `cargo build --workspace` and `cd web && npm run build` both succeed
- [ ] CI workflow runs the Global Constraints gate on push/PR
- [ ] Commit (`scaffold cargo workspace`, `add web scaffold`, `add ci pipeline`) and push `master`

### Track A — Task A1: rustify-core

**Files:** Create: `crates/rustify-core/src/{exec,deployment,logline,crypto,ids,error}.rs`
**Interfaces:** Produces C1, C2, C3 verbatim; `crypto::{encrypt(plain: &[u8]) -> Vec<u8>, decrypt(blob: &[u8]) -> Result<Vec<u8>>}` reading key from env once via `OnceLock`; `ids::new_uuid() -> String` (cuid2); redaction helper `redact(content: &str, secrets: &[&str]) -> String` replacing each secret with `[REDACTED]`.

**Tests (write first):** `deployment::tests`: table-test every (from,to) pair of `can_transition_to` — terminal states reject all; Queued→Finished is illegal; Queued→Cancelled legal. `crypto::tests`: roundtrip + tamper detection (flip a byte ⇒ error). `redact`: overlapping secrets, empty secret list. 
- [ ] Failing tests → implement → green → commit `add core domain types`

### Track A — Task A2: rustify-db

**Files:** Create: `crates/rustify-db/migrations/0001_init.sql` (C6 verbatim), `src/pool.rs` (`connect(url) -> PgPool` + `MIGRATOR: sqlx::migrate!()`), `src/repos/{teams,users,keys,servers,projects,applications,env_vars,deployments,settings}.rs` — one repo struct per aggregate with the queries the API and engine need (typed with `sqlx::query_as!` where possible; runtime queries acceptable to avoid offline-mode friction, but then add `sqlx::test` coverage).
**Interfaces:** Consumes A1 types. Produces for E: `DeploymentRepo::{create_queued, transition(id, next: DeploymentStatus) -> Result<bool>, append_logs(id, &[LogLine]), cancel_requested(id) -> bool, next_queuable(server_id) -> Option<Deployment>}` — `transition` enforces C2 legality **in SQL** (`UPDATE ... WHERE status = $from AND ...`) returning false on race; `next_queuable` implements admission control: per-server `concurrent_builds` cap + one in-flight per application + FIFO, using `FOR UPDATE SKIP LOCKED`. Produces for F: CRUD for all C5 resources; `seed_default(pool)` creating team #1 + admin user from `RUSTIFY_ADMIN_EMAIL`/`RUSTIFY_ADMIN_PASSWORD` env.
**Tests:** `#[sqlx::test]` (auto test-db per test): migration applies; `transition` race (two tasks race Queued→InProgress, exactly one wins); admission control matrix (3 queued on same app ⇒ 1 in-flight; server cap 2 with 3 apps ⇒ 2 in-flight); env var unique upsert; append_logs preserves order.
- [ ] Failing tests → implement → green → PR `track/core-db` → squash-merge `add db layer`

### Track B — Task B1: rustify-ssh

**Files:** Create: `crates/rustify-ssh/src/{mux,command,keys,retry}.rs`
**Interfaces:** Consumes C1. Produces `SshExecutor::new(mux_dir: PathBuf) -> Self` implementing `CommandExecutor`. Behaviors ported from Coolify `SshMultiplexingHelper` (see spec §4): heredoc transport `'bash -se' << \RUSTIFY_EOF_{nonce}`; mux socket per server `mux_{uuid}` with `ControlMaster=auto/ControlPersist=3600`, established via `ssh -fN`, health-checked (`ssh -O check`, age ≤ 3600s), refresh on expiry, per-server `tokio::Mutex` guard, silent fallback to non-mux; common options exactly: `-i {key} -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null -o PasswordAuthentication=no -o ConnectTimeout={n} -o ServerAliveInterval=20 -o RequestTTY=no -o LogLevel=ERROR -p {port}`; remote command wrapped `timeout {n}`; retry with 2s/4s/8s backoff on connection-class errors only (`retry.rs`); `keys::materialize(uuid, decrypted_pem, dir) -> PathBuf` writes 0600 + resyncs on divergence.
**Tests:** unit — command assembly golden strings (given conn+script, the full argv/ssh string matches expected, nonce factored out); keys: perm bits 0600 asserted, divergence rewrite. integration (feature-gated `ssh-tests`, used by Track H): against local sshd container run `echo hello`, streaming order stdout/stderr interleave, non-zero exit maps to `NonZero`, timeout maps to `Timeout`.
- [ ] Failing tests → implement → green → PR `track/ssh` → squash-merge `add ssh layer`

### Track C — Task C1: rustify-docker

**Files:** Create: `crates/rustify-docker/src/{build_command,compose,labels,inspect}.rs`
**Interfaces:** Consumes C2/C7. Produces:
```rust
pub struct BuildCommand { pub context: String, pub dockerfile: Option<String>, pub image: String,
  pub build_args: Vec<(String,String)>, pub no_cache: bool, pub pull: bool, pub target: Option<String>,
  pub buildkit: bool, pub env_file: Option<String> /* sourced wrapper */ }
impl BuildCommand { pub fn render(&self) -> String }  // one place all variants are born
pub fn generate_compose(app: &AppComposeInput) -> String  // single-service compose per spec §5 (serde_yaml)
pub fn traefik_labels(app: &AppComposeInput) -> Vec<String> // C7 labels incl. http+https routers
pub fn parse_health(inspect_json: &str) -> ContainerHealth  // healthy|unhealthy|starting|none
pub fn parse_containers(ps_json: &str) -> Vec<ManagedContainer> // from `docker ps --format json`, reads rustify.* labels
```
`AppComposeInput` is a plain struct (no DB deps) with exactly the fields §C6 applications exposes (name/image/network/ports/labels/healthcheck/limits/volumes/env_file).
**Tests:** golden files in `tests/golden/*.txt|yaml` — build command for {nixpacks image, dockerfile+args, no_cache, buildkit} variants; compose YAML for a full-featured app (healthcheck curl||wget fallback chain exactly as Coolify: `curl -f http://H:P/path || wget -qO- http://H:P/path || exit 1`) and a minimal app; labels set for app with fqdn `https://x.example.com` (http router + https router + certresolver). Inspect parsing from captured real `docker inspect` JSON fixtures.
- [ ] Failing tests → implement → green → commit `add docker builders`

### Track C — Task C2: rustify-proxy

**Files:** Create: `crates/rustify-proxy/src/{config,lifecycle}.rs`
**Interfaces:** Produces `generate_proxy_compose(custom_commands: &[String]) -> String` (port of `generateDefaultProxyConfiguration` Traefik branch: image `traefik:v3.6`, name `rustify-proxy`, ports 80/443/443udp/8080, ping healthcheck, file provider `/traefik/dynamic/`, ACME resolver/storage, docker provider not-exposed-by-default, docker.sock ro, `/data/rustify/proxy:/traefik`); `extract_custom_commands(existing_yaml: &str) -> Vec<String>` (non-default-prefix survival, same prefix list as Coolify); `start_script() / stop_script() -> String` (mkdir dirs, write compose via heredoc, `docker network create --attachable rustify || true`, `docker compose -f /data/rustify/proxy/docker-compose.yml up -d`, connect networks idempotently).
**Tests:** golden compose YAML vs `tests/golden/proxy-compose.yaml` (hand-verified against Coolify's output structure); custom command survival (inject `--log.level=error` + custom flag, regenerate, custom flag survives, default ones don't duplicate); start script contains network-create guard.
- [ ] Failing tests → implement → green → PR `track/docker-proxy` → squash-merge `add proxy generator`

### Track D — Task D1: rustify-jobs

**Files:** Create: `crates/rustify-jobs/src/{queue,scheduler}.rs`
**Interfaces:** Produces `JobQueue::new(pool)` with `enqueue(kind: &str, payload: serde_json::Value, run_at: Option<DateTime>)`, `run(workers: usize, registry: JobRegistry, shutdown: CancellationToken)` — poll loop with `FOR UPDATE SKIP LOCKED` claim on `jobs` (C6), heartbeat re-lock, `attempts+1` + backoff on error, drop after 3 attempts recording `last_error`. `JobRegistry::register(kind, handler: Arc<dyn JobHandler>)`; `trait JobHandler { async fn run(&self, payload: Value) -> anyhow::Result<()>; }`. `Scheduler::every(Duration, name, fn)` tokio interval loop with per-tick skip-if-still-running.
**Tests:** `#[sqlx::test]` — claim exclusivity under 4 concurrent workers (each job runs exactly once — counter table); retry/backoff on failing handler; shutdown drains current job then stops; scheduler tick-skip.
- [ ] Failing tests → implement → green → PR `track/jobs` → squash-merge `add job queue`

### Track E — Task E1: deployment engine (the big one; starts after A merges, uses B/C via traits+mocks, integrates for real after B/C merge)

**Files:** Create: `crates/rustify-deploy/src/{engine,admission,buildpacks/*,rolling,envfile,git,status_sync,server_setup}.rs`
**Interfaces:** Consumes `CommandExecutor` (C1), `DeploymentRepo` (A2), docker builders (C1 of Track C), `JobHandler` (D1). Produces `DeployJobHandler` registered as kind `"deploy"` with payload `{deployment_uuid}`; `ServerSetupHandler` kind `"server_validate"`; `StatusSyncTask` for the scheduler (30s); public `DeployEngineDeps { executor: Arc<dyn CommandExecutor>, pool: PgPool, events: EventBus }` where `EventBus = tokio::sync::broadcast::Sender<WsEvent>` (WsEvent enum matching C4 events).
**Engine flow (each step streams LogLines via repo + EventBus, checks cancellation before every remote command):**
1. claim deployment (repo.transition Queued→InProgress; bail if lost race)
2. helper up: `docker run -d --rm --name {dep_uuid} --network {net} -v /var/run/docker.sock:/var/run/docker.sock ghcr.io/coollabsio/coolify-helper:latest`
3. `git ls-remote` resolve SHA (in helper); persist commit_sha
4. skip-build check: `docker images -q {image}` non-empty && !force_rebuild ⇒ jump to 7
5. clone in helper (`git clone -b {branch} --single-branch --depth 1 {repo} /artifacts/{dep_uuid}`; read `git log -1 --pretty=%s`)
6. buildpack build (see per-pack table below) — build-time env file written to `/artifacts/build-time.env` via base64 heredoc, build wrapped `set -a && source ... && set +a &&`
7. write runtime `.env` + generated compose to `/data/rustify/applications/{app_uuid}/` (upload via executor)
8. rolling update (`rolling.rs`): eligibility per spec §5; start new, poll `docker inspect --format '{{json .State.Health.Status}}'` every `interval`s after `start_period`, up to `retries`; healthy ⇒ stop+rm old containers matching `rustify.applicationUuid` label except new; unhealthy ⇒ `docker logs -n 100`, rm new, FAIL
9. helper cleanup (`docker rm -f {dep_uuid}`, always — also on failure/cancel via drop-guard)
10. transition Finished + `queue_next` for that server; emit events
**Buildpacks:** nixpacks: `nixpacks build /artifacts/{d} --name {image} --no-error-without-start -o /artifacts/{d}` then BuildCommand on generated Dockerfile (matches Coolify's plan→build split: `nixpacks plan` → write config → build); dockerfile: BuildCommand with app dockerfile_location; static: two-stage — build user image if build_command set else skip, then generate nginx Dockerfile `FROM {static_image}` + COPY publish_directory + default.conf, BuildCommand; docker-image: `docker pull {name}:{tag}`, no build; docker-compose: upload user compose + injected `env_file`, network attach + labels via override file, `docker compose up -d --build` (no rolling).
**server_setup.rs (ServerSetupHandler):** uptime probe → `command -v docker` → if missing `curl -fsSL https://get.docker.com | sh` → `docker network create --attachable rustify || true` → mark reachable/usable → enqueue proxy start. Streams to WS channel `server:{uuid}`.
**status_sync.rs:** per server `docker ps -a --format '{{json .}}'` → map `rustify.applicationUuid` labels → update `applications.status` (`running:healthy` style), restart_count crash-loop (≥ max ⇒ stop app + status `crashed`), missing ⇒ `exited`; emit `application_status_changed` on change.
**Tests:** all engine logic against `FakeExecutor` (scripted responses, records every script; in `rustify-deploy/tests/fake.rs`): happy nixpacks deploy produces expected script sequence (assert order: helper run → ls-remote → clone → build → compose up); cancellation between steps 5 and 6 ⇒ Cancelled, helper cleanup script issued, no rolling update; unhealthy path keeps old container (no stop script for old), removes new; skip-build path issues no clone/build; static pack generates nginx dockerfile with publish dir; env precedence (nixpacks < RUSTIFY_* < user buildtime) in generated build-time.env; redaction: `is_shown_once` env value never appears in persisted LogLines. `#[sqlx::test]` integration: full deploy against FakeExecutor writes ordered deployment_logs and Finished status; queue drain triggers next queued deployment.
- [ ] Failing tests → implement → green → PR `track/deploy-engine` → squash-merge `add deploy engine`

### Track F — Task F1: rustify-server (API+auth+WS+embed)

**Files:** Create: `crates/rustify-server/src/{main,app,auth,ws,error,embed,routes/{auth,keys,servers,projects,applications,deployments,settings,tokens,health}.rs}`
**Interfaces:** Consumes A2 repos, D1 queue (enqueue deploy/server_validate), EventBus (E1's broadcast channel — server owns the `broadcast::channel(1024)` and passes sender to engine deps). Produces the C5 surface exactly, utoipa-annotated, spec served at `/api/v1/openapi.json` + swagger at `/docs`; session auth (cookie `rustify_session`, argon2id verify, 30-day expiry, sessions table) + bearer tokens (`Authorization: Bearer` → sha256 hash lookup); auth middleware injects `CurrentTeam`; WS `/ws` per C4 (subscribe/unsubscribe, fan-out from broadcast with per-connection channel filter); `embed.rs` serves `web/dist` via rust-embed with SPA fallback to `index.html`; `main.rs` wires: pool → migrate → seed → EventBus → JobQueue workers (registry: deploy, server_validate) → Scheduler (status_sync 30s) → axum serve on `:8000`.
**Tests:** axum `tower::ServiceExt` handler tests with `#[sqlx::test]`: login/logout/me cycle; 401 without session; token auth hits `/servers`; app create validates git URL (`https://` or `git@` prefix) and build_pack enum; deploy endpoint enqueues job row + returns deployment_uuid; cancel on running deployment flips cancel flag; settings PATCH roundtrip; openapi.json contains all C5 paths (snapshot count assert); WS: connect+subscribe, push a fake event through EventBus, client receives exactly matching-channel messages.
- [ ] Failing tests → implement → green → PR `track/api-server` → squash-merge `add api server`

### Track G — Task G1: web SPA

**Files:** Create: `web/src/{main.tsx,api/{client.ts,ws.ts,types.gen.ts},routes/{login,onboarding,dashboard,servers/[uuid],projects/[uuid],applications/[uuid]/{index,envs,source,domains,deployments},deployments/[uuid],settings}.tsx,components/{LogViewer,StatusBadge,Wizard,Layout,ConfirmDanger}.tsx}`
**Interfaces:** Consumes C4 (ws.ts: reconnecting client with subscribe API) and C5 via `types.gen.ts` — generated with `npx openapi-typescript` from `crates/rustify-server`'s committed `openapi.json` snapshot (regenerate script `npm run gen:api`; until F merges, use the C5 table hand-written snapshot committed at `web/openapi.snapshot.json` — replace after F merge). Produces the Phase 1 screens (spec §8): login; onboarding wizard state machine (welcome→key→server→validate[live WS output]→project→app→deploy); dashboard; server page (settings/proxy tabs, proxy config editor textarea + start/stop); project→env→app pages (general/envs/storage/source/domains/deployments tabs); deployment page with virtualized `LogViewer` (WS `deployment_log_appended` append + fetch-on-load, auto-scroll w/ pin, stderr coloring, hidden-line toggle); settings + api tokens (token shown once).
**Tests:** vitest + testing-library: wizard transitions (can't advance without required fields); LogViewer appends WS lines in `ord` order and dedupes on refetch; api client attaches credentials; StatusBadge maps `running:healthy`→green, `exited`→gray, `crashed`→red. `npm run build` green.
- [ ] Failing tests → implement → green → PR `track/web-ui` → squash-merge `add web ui`

### Track H — Task H1: E2E harness + CI wiring

**Files:** Create: `tests/e2e/docker/testhost.Dockerfile` (ubuntu + sshd + docker-in-docker, root key from build arg), `tests/e2e/apps/nixpacks-node/` (10-line express app + package.json), `tests/e2e/apps/dockerfile-app/` (static hello + Dockerfile), `crates/rustify-server/tests/e2e.rs` (feature `e2e`), `.github/workflows/ci.yml` e2e job.
**Flow:** compose up postgres + testhost → start rustify-server (test config) → REST: create key(from testhost fixture)+server → validate (poll until usable) → create project/app (git: local file:// bare repo of sample app pushed in setup — no network dependency) → deploy → poll deployment until Finished (≤5min) → assert: `docker ps` on testhost shows app container with rustify labels; `curl` through testhost port mapping returns app response; deployment_logs non-empty ordered; second deploy with no changes hits skip-build (log line `Image already exists`); cancel mid-deploy test ⇒ Cancelled + helper gone.
**Tests:** the harness IS the test. Runs in CI on PRs to master (ubuntu-latest, dind service) and locally via `make e2e`.
- [ ] Harness runs green locally → PR `track/e2e` → squash-merge `add e2e harness`

### Task Z: Integration + release polish (serial, after all tracks merged)

**Files:** Modify: `crates/rustify-server/src/main.rs` (final wiring pass), `web/openapi.snapshot.json` (regenerate from live server), `docker-compose.prod.yml`, `Dockerfile` (multi-stage: web build → cargo build → distroless-ish runtime with ssh client + docker cli), `README.md` (real quickstart).
- [ ] Full workspace gate green; e2e green against the real wiring
- [ ] Regenerate openapi snapshot; web rebuild green
- [ ] `docker build` of release image succeeds; compose-prod boots and serves `/health`
- [ ] Commit `add release packaging` → tag `v0.1.0-alpha.1`

---

## Execution notes

- Merge order: 0 → A → {B, C, D in any order} → E → F → {G, H} → Z. E may branch early and develop against mocks; it rebases on A+C before PR.
- Golden files that claim Coolify parity must be derived by reading `~/Desktop/coolify` sources (cite file+line in the golden file header comment).
- Any interface change to §Contracts requires editing THIS document in the same PR.

## Self-review (done at write time)

- Spec coverage: §2→0/F, §3→A2, §4→B1, §5→E1+C1, §6→C2, §7→F1, §8→G1, §9→all error enums+engine paths, §10→per-task tests+H1, §11→Z. BuildKit-secrets second step (spec §5) intentionally deferred within Phase 1 — tracked as follow-up, not blocking v0.1.0-alpha.1.
- No placeholder patterns beyond deliberate per-task code ownership (signatures pinned in §Contracts).
- Type consistency: C1..C7 cross-checked against every task's Interfaces block.
