# Rustify — Phase 1 Design: The Deploy Kernel

**Date:** 2026-07-02
**Status:** Approved by owner (design conversation, this date)
**Repo home:** `AE52/rustify` on GitHub (to be created at implementation start), plus a fork of `coollabsio/coolify` under AE52 for reference/attribution.

## 1. What Rustify is

Rustify is a from-scratch Rust reimplementation of [Coolify](https://github.com/coollabsio/coolify), the open-source self-hosting PaaS (Laravel 12 + Livewire, ~110k LOC of PHP app code, 54 models, 200 Livewire screens, 361 one-click service templates). The long-term goal is **full 1:1 feature parity**, reached through **vertical slices**: every phase ships a usable end-to-end product.

Decisions fixed during the design conversation:

| Decision | Choice |
|---|---|
| Goal | Full 1:1 feature port of Coolify, phased |
| Compatibility | **Clean slate** — own Postgres schema, own (Coolify-inspired) API. Behavior parity, not bit compatibility. No Laravel schema/APP_KEY migration path. |
| UI strategy | Rust API backend + separate **React + Vite** SPA (TypeScript), embedded into the binary as static files |
| Phasing | Vertical slices (each phase is a usable product) |
| GitHub | New repo + coolify fork under the **AE52** account |
| License | Apache-2.0 with `NOTICE` attribution to Coolify (behavioral derivation; later phases reuse Coolify's Apache-2.0 compose templates verbatim) |

### Phase roadmap (later phases get their own specs)

- **Phase 1 — Deploy kernel (this spec):** server onboarding over SSH → deploy an application from a public git repo (5 build packs) → Traefik proxy + Let's Encrypt + custom domains → live deployment logs → React UI + REST API + WS. Single user, single implicit team.
- **Phase 2 — Data layer:** 8 standalone database engines, S3 backups, scheduled tasks, one-click service templates (reuse Coolify's 361 templates + magic `SERVICE_*` env var system).
- **Phase 3 — Git depth + integrations:** GitHub App private repos, PR preview deployments, webhooks, notification channels (email/Discord/Telegram/Slack), railpack build pack.
- **Phase 4 — Multi-tenancy + operations:** teams/roles (MEMBER<ADMIN<OWNER), web terminal (xterm.js + PTY over WS), metrics (Sentinel equivalent), Hetzner provisioning, Docker Swarm, dedicated build servers, Cloudflare tunnels, Caddy proxy option.

## 2. Architecture

Coolify requires six cooperating processes (php-fpm, Horizon workers, Redis, Soketi, a Node terminal server, Postgres). **Rustify runs as one binary + Postgres.** Inside the single `rustify` process:

- **axum HTTP server** — REST API under `/api/v1`, WebSocket endpoint, and the React SPA embedded via `rust-embed` (served with SPA fallback).
- **Job runner** — a **Postgres-backed queue** (`FOR UPDATE SKIP LOCKED`), replacing Redis+Horizon. This is fidelity, not just simplification: Coolify's deployment admission control (per-server `concurrent_builds`, per-app single in-flight, dedupe) is already DB-arbitrated; Redis is only transport there.
- **Scheduler** — tokio-based cron loop (server health checks, container status reconciliation; Phase 2 adds backups/scheduled tasks here).

**Realtime:** native WS in axum replaces Soketi/Pusher. Coolify's ~21 broadcast "poke" events become one typed Rust event enum; unlike Coolify (event triggers a client refetch), events carry their payload.

### Workspace layout (monorepo)

```
rustify/
├── Cargo.toml              # workspace
├── crates/
│   ├── rustify-server/     # main binary: router, auth, WS, job-runner bootstrap
│   ├── rustify-core/       # domain types, state machines, error types (no IO)
│   ├── rustify-db/         # sqlx repositories + migrations
│   ├── rustify-ssh/        # SSH exec layer (ControlMaster mux, scp, heredoc transport)
│   ├── rustify-docker/     # docker CLI command builders (BuildCommand, compose gen, labels)
│   ├── rustify-deploy/     # deployment engine: state machine, build packs, rolling update
│   ├── rustify-proxy/      # Traefik config generation + lifecycle
│   └── rustify-jobs/       # Postgres queue + scheduler
├── web/                    # React + Vite SPA (TS strict, Tailwind v4, TanStack Query)
├── docs/
└── reference/              # local clone of coolify for study (gitignored)
```

### Stack

axum, tokio, sqlx (Postgres), serde/serde_yaml, **utoipa** (OpenAPI generated from code → `openapi-ts` generates the typed frontend client), rust-embed, argon2 (passwords), AES-GCM (at-rest encryption of sensitive columns, key from `RUSTIFY_SECRET_KEY` env — the analogue of Laravel's `APP_KEY`), thiserror.

## 3. Domain model (Phase 1 schema)

Coolify's hierarchy is preserved: **Team → Project → Environment → Resource**. `teams` and `team_id` FKs exist in the schema **from day 1** (to avoid painful Phase 4 migrations), but Phase 1 hardcodes a single default team and shows no team UI.

Tables: `users`, `sessions`, `api_tokens`, `teams`, `private_keys` (encrypted key material), `servers` + `server_settings`, `destinations` (docker networks; Coolify's standalone-docker equivalent), `projects`, `environments`, `applications`, `environment_variables` (polymorphic via `resource_kind` + `resource_id`; Phase 2 databases/services reuse it), `persistent_storages`, `deployments` (Coolify's `ApplicationDeploymentQueue`: status, commit SHA, force_rebuild, config snapshot), `deployment_logs`, `instance_settings`.

- **IDs:** sequential internal `id` + externally exposed CUID2 `uuid` (Coolify convention).
- **`deployment_logs` is append-only** — fixes Coolify's O(n²) read-modify-write JSON-blob column while keeping its fields: `deployment_id, order, type (stdout|stderr), hidden, batch, timestamp, content`.
- **Application fields (Phase 1):** git repository/branch/commit, build pack, base/publish directories, install/build/start commands, FQDN list, ports (expose + host mappings), health-check fields (enabled, path/host/port/method/status, interval, timeout, retries, start_period), resource limits (mem/cpu), env delivery mode (build-arg vs BuildKit secrets), custom docker options.

## 4. SSH layer (`rustify-ssh`)

Same approach as Coolify (`app/Helpers/SshMultiplexingHelper.php`): **shell out to system `ssh`**, do not use a pure-Rust SSH library. Rationale: OpenSSH ControlMaster multiplexing, `cloudflared access ssh` ProxyCommand support (Phase 4), and decades-hardened behavior come free.

Ported behaviors:

- **Mux lifecycle:** per-server socket, `ssh -fN -o ControlMaster=auto -o ControlPath=<sock> -o ControlPersist=<ttl>`; connection age + health checks (`ssh -O check`, `echo health_check_ok` probe); refresh on expiry; graceful fallback to non-multiplexed on lock/setup failure. In-memory per-server `tokio::sync::Mutex` + metadata replaces Laravel's Cache locks (single process).
- **Command transport:** Coolify's heredoc trick verbatim — `ssh user@host 'bash -se' << \DELIM … DELIM` with a random delimiter (eliminates shell-escaping bugs). stdout/stderr stream line-by-line into `deployment_logs` (tokio `AsyncBufRead`).
- **Key management:** private keys encrypted in DB, written to disk 0600, resynced when disk content diverges from DB (Coolify `validateSshKey` behavior).
- **Common options** identical: `StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null -o PasswordAuthentication=no -o ConnectTimeout=N -o ServerAliveInterval=N -o RequestTTY=no -o LogLevel=ERROR`, plus per-command `timeout N` wrapper. SCP variant for file upload.
- **Retry:** transient SSH failures retried with backoff (Coolify `SshRetryHandler` parity).

**Server onboarding flow (ported from `ValidateServer`/`InstallDocker`):** create private key (or generate ed25519 keypair) → create server (ip/port/user) → validate: uptime probe, OS detect, docker presence → install docker via official script if missing → create `rustify` default docker network (destination) → start proxy.

## 5. Deployment engine (`rustify-deploy`)

The behavioral port of Coolify's 4,894-line `ApplicationDeploymentJob`, restructured but semantics-preserving.

- **State machine (exact):** `Queued → InProgress → {Finished | Failed | Cancelled}`. Terminal states immutable; every transition drains the per-server queue; **cancellation checked before every remote command** (tokio `CancellationToken` + DB status check — replaces Coolify's magic exception code 69420). On failure the **new** container is removed, the **old** container is never touched — the running version keeps serving.
- **Admission control in Postgres (exact semantics):** per-server queued limit (default 25), one in-flight per application, per-server `concurrent_builds`, dedupe on (application, commit, force_rebuild) — implemented with `SELECT … FOR UPDATE`.
- **Helper container pattern kept:** all build work runs on the target server inside `ghcr.io/coollabsio/coolify-helper` (contains git, nixpacks, docker CLI), started `docker run -d --rm` with `/var/run/docker.sock` mounted; commands via `docker exec <uuid> bash -c`. Build scripts shipped base64-encoded to `/artifacts/build.sh`. A first-party `rustify-helper` image is future work.
- **Build packs (Phase 1, five):** `nixpacks`, `dockerfile`, `static` (two-stage nginx), `docker-image` (no build; pull `name:tag`/`name@sha256:…`), `docker-compose`. `railpack` deferred to Phase 3. Coolify's 30+ hand-assembled build command variants collapse into one **`BuildCommand` builder struct** (flags: no_cache, pull, target, buildkit, secrets, add_hosts, progress), validated by golden-file tests against Coolify-generated commands.
- **Env model (exact):** build-time vs runtime split. Build-time env file at `/artifacts/build-time.env`, deliberately **outside** the Docker context; precedence `nixpacks plan < RUSTIFY_* < SERVICE_* < user build-time vars`; build wrapped in `set -a && source … && set +a`. Runtime `.env` written **after** build to workdir + `{config_dir}/{uuid}`. Delivery starts with classic `--build-arg` + ARG injection after every `FROM`; BuildKit secrets mode (`--secret id=K,env=K`, `RUN --mount` injection, HMAC-SHA256 cache-bust hash) lands as a second step within Phase 1.
- **Image naming + skip-build (exact):** image tag = `{uuid}:{commit_sha}`; skip build when image exists (locally or via `docker pull`) AND the config-snapshot diff requires no rebuild (port of `ConfigurationDiffer` as a serde struct + content hash).
- **Rolling update (verbatim algorithm):** start new uniquely-named container via generated compose → wait `start_period` → poll `docker inspect .State.Health.Status` every `interval`, up to `retries` → healthy: gracefully stop old (`docker stop --time=<grace>` + `rm`); unhealthy: dump last 100 container log lines, remove new container, mark Failed. Rolling disabled (stop-then-start with brief downtime) for: host port mappings, consistent-container-name, custom internal name, custom `--ip`. A user `HEALTHCHECK` in the Dockerfile suppresses Rustify's; disabled health check ⇒ immediately "healthy".
- **Compose generation:** single-service compose with image, container name, destination network + aliases, resource limits, labels, `env_file`, healthcheck (curl/wget fallback chain), expose/ports, volumes (persistent storages), restart policy.
- **Label schema (load-bearing):** `rustify.applicationId`, `rustify.deploymentId`, `rustify.managed`, `com.docker.compose.service` + generated Traefik routing labels. Status reconciliation, stop actions, and proxy routing all key off labels, as in Coolify.
- **Secret redaction:** locked/one-shot env values are scrubbed from streamed log output before persistence.

## 6. Proxy (`rustify-proxy`)

Per-server **Traefik v3** container named `rustify-proxy`, launched from a generated compose file (port of `bootstrap/helpers/proxy.php`):

- Ports 80/443 (+443/udp for h3) and 8080 (dashboard, `api.insecure=false`).
- Static config via command flags: file provider watching `/traefik/dynamic/`, docker provider with `exposedbydefault=false`, Let's Encrypt ACME httpchallenge resolver with `/traefik/acme.json` storage, `--ping` + `wget /ping` healthcheck.
- Proxy joins every application network via `docker network create --attachable` + `docker network connect` (idempotent guards), Coolify's `connectProxyToNetworks` port.
- **Custom-command preservation:** user-added Traefik flags survive config regeneration (port of `extractCustomProxyCommands`: diff against the known default prefix list).
- Per-app routing comes from generated container labels (http/https routers, ACME cert resolver, optional www/non-www redirect in later phases). Caddy alternative deferred to Phase 4.

## 7. API, auth, realtime

- **REST `/api/v1`** with utoipa-generated OpenAPI served at `/docs`. Phase 1 resources: `auth` (login/logout/me), `private-keys`, `servers` (+ `validate`, `install`), `projects`, `environments`, `applications` (+ `deploy`, `stop`, `restart`, env CRUD, container logs), `deployments` (list/show/**cancel**), `settings`, `api-tokens`, `/health`.
- **Auth:** session cookie for the SPA (argon2id password hashing); **Bearer API tokens** for automation — stored hashed, with Sanctum-style abilities (`read`/`write`/`deploy`) structured in Phase 1 (single full-ability token type is enough initially).
- **Authorization at the action level** (Coolify parity): every mutating handler re-checks resource ownership through extractors/middleware, not just page-level guards.
- **WS `/ws`** (session/token authenticated): client subscribes to channels (e.g. one deployment's log stream); server pushes typed events: `DeploymentLogAppended`, `DeploymentStatusChanged`, `ApplicationStatusChanged`, `ServerReachabilityChanged`. Replaces Coolify's 2-second Livewire polling with direct worker→WS push.
- **Container status reconciliation** (port of `GetContainersStatus`): scheduler polls each server's containers, maps labels back to DB rows, aggregates status strings (`running:healthy`), counts restarts for crash-loop detection (`max_restart_count` ⇒ stop + mark), marks vanished apps `exited`, emits events.

## 8. Frontend (`web/`)

React 19 + Vite + TypeScript strict + Tailwind v4 + TanStack Query + React Router. The API client is **generated from the OpenAPI spec** (openapi-ts) so a backend handler change breaks the frontend build — end-to-end type safety.

Phase 1 screens (from Coolify's route inventory): login; **onboarding wizard** (port of the `Boarding` state machine: welcome → server type → SSH key → create server → validate/install with live output → project → first application); dashboard (servers + projects); server pages (settings, validate/install progress, proxy tab with config editor + start/stop, destinations); project → environment → application tabs (general config, environment variables, persistent storage, source, domains, deployments list); **live deployment log screen** (WS-fed, virtualized list for thousands of lines); instance settings + API tokens.

## 9. Error handling

- Per-crate `thiserror` enums; `anyhow` only at binary edges; API maps errors to one envelope `{code, message}` with correct HTTP status.
- Deployment: any remote command failure → log entry + `Failed` transition (except cancellation); SSH transient errors retried with backoff; mux failure falls back to non-multiplexed silently (warning logged).
- The failure path must never remove the old running container (see §5).
- Secret redaction happens before log persistence, not at display time.

## 10. Testing

- **Golden-file tests are the backbone of parity:** generated shell commands (`BuildCommand`), compose YAML, Traefik config, and label sets are pinned against real Coolify-generated outputs captured from the reference clone.
- **Unit:** state-machine transitions (terminal immutability, cancellation paths), env precedence table, admission-control rules, Dockerfile ARG/secret-mount rewriting.
- **Integration:** sqlx against ephemeral Postgres (testcontainers); queue concurrency/admission tests.
- **E2E:** a container running sshd + docker (dind) as the "test server" — the analogue of Coolify's `testing-host`; CI deploys a sample nixpacks app and a dockerfile app end-to-end, asserts the proxy routes traffic and the health check gates the rolling update.
- **Frontend:** vitest + testing-library for critical flows (onboarding, deploy trigger, log viewer).
- **CI:** GitHub Actions — `cargo fmt --check`, `clippy -D warnings`, `cargo test`, web build, E2E job.

## 11. Distribution

- Production: `docker-compose.prod.yml` (rustify image + postgres), single published Docker image; install script later.
- Development: `cargo run` (API on :8000) + `vite dev` (proxying `/api`).

## 12. Out of scope for Phase 1 (explicit)

Databases and backups, one-click service templates, GitHub App/private repos, PR previews, webhooks, notifications, teams/roles UI, web terminal, metrics, Swarm, build servers, Cloudflare tunnels, Caddy, railpack, multi-server-per-app fan-out, Stripe/cloud features, MCP server. Each lands in its phase per §1.

## 13. Known follow-ups

- Complete the Coolify subsystem maps for the 7 areas whose reader agents were interrupted (domain model detail, server/SSH internals, proxy internals, API surface, scheduler/queues, service templates, integrations) before writing Phase 2+ specs; resume workflow run `wf_b67fbc4c-ee3`.
- Create `AE52/rustify` GitHub repo + fork `coollabsio/coolify` to AE52 at implementation start.
- Decide the first-party `rustify-helper` image timing (currently reusing `ghcr.io/coollabsio/coolify-helper`).
