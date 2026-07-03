# Rustify

A from-scratch Rust reimplementation of [Coolify](https://github.com/coollabsio/coolify),
a self-hosting PaaS: deploy applications to your own servers over SSH with
Docker and Traefik. See `docs/` for the Phase 1 design spec and plan.

Rustify is a single Rust binary that serves a REST API (`/api/v1`), a realtime
WebSocket feed, and an embedded React SPA. It drives remote servers over SSH,
builds images with Nixpacks / Dockerfiles / static / compose buildpacks, and
rolls containers behind a Traefik proxy.

## Quickstart (Docker Compose)

Requires Docker with the Compose plugin. From a checkout:

```sh
# 1. A 32-byte base64 key for at-rest encryption of secrets (SSH keys, env vars).
export RUSTIFY_SECRET_KEY=$(head -c 32 /dev/urandom | base64)

# 2. First-run admin credentials (seeded only if the users table is empty).
export RUSTIFY_ADMIN_EMAIL=admin@example.com
export RUSTIFY_ADMIN_PASSWORD=change-me-please

# 3. Build the release image and boot the stack (server + Postgres).
docker compose -f docker-compose.prod.yml up -d --build
```

Then open <http://localhost:8000> and log in with the admin email/password you
set above. `GET http://localhost:8000/api/v1/health` returns `{"status":"ok"}`
once the server is up; the API docs (Swagger UI) live at `/docs`.

To deploy an app: register an SSH private key, add a server (validated over
SSH), create a project + application from a git repository, then trigger a
deploy — logs stream live in the UI.

## Configuration

| Env var | Required | Default | Purpose |
|---|---|---|---|
| `DATABASE_URL` | yes | — | Postgres DSN (`postgres://…`). |
| `RUSTIFY_SECRET_KEY` | yes | — | base64 of 32 bytes; AES-256-GCM key for secrets at rest. |
| `RUSTIFY_ADMIN_EMAIL` | first run | `admin@rustify.local` | Seed admin login. |
| `RUSTIFY_ADMIN_PASSWORD` | first run | `changeme` | Seed admin password. |
| `RUSTIFY_DATA_DIR` | no | `$HOME/.rustify` (image: `/data/rustify`) | SSH mux sockets + materialised keys. |
| `RUSTIFY_COOKIE_SECURE` | no | `true` | Set `false` when serving over plain HTTP. |

The server listens on `0.0.0.0:8000` and mounts the host Docker socket to run
the proxy and manage local containers.

## Development

```sh
# Postgres for local dev on :5433
docker compose -f docker-compose.dev.yml up -d

# Full workspace gate
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
DATABASE_URL=postgres://rustify:rustify@127.0.0.1:5433/rustify cargo test --workspace

# Web SPA (embedded into the binary at build time)
cd web && npm ci && npm run build
```

### End-to-end tests

`make e2e` stands up Postgres + a privileged docker-in-docker "testhost", seeds
sample apps as `file://` bare repos, spawns the real server binary, and drives
the whole deploy flow over the public API. Requires a running Docker daemon.
