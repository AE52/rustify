# Rustify release image — multi-stage:
#   1. web:   build the React SPA (Node 22) into web/dist
#   2. build: cargo build --release (the SPA is embedded into the binary)
#   3. run:   slim Debian runtime with the binary + openssh-client + docker CLI
#
# The server drives remote hosts over SSH and talks to the local Docker daemon
# for the proxy, so the runtime image ships both `ssh` and `docker`.

# ---- 1. web -----------------------------------------------------------------
FROM node:22-bookworm-slim AS web
WORKDIR /web
COPY web/package.json web/package-lock.json web/.npmrc ./
RUN npm ci
COPY web/ ./
RUN npm run build

# ---- 2. build ---------------------------------------------------------------
FROM rust:1.85-bookworm AS build
WORKDIR /src
# Copy the whole workspace; the web build output must be present before the
# server crate compiles because rust-embed reads web/dist at build time.
COPY . .
COPY --from=web /web/dist ./web/dist
RUN cargo build --release --bin rustify-server \
    && strip target/release/rustify-server

# ---- 3. runtime -------------------------------------------------------------
FROM debian:bookworm-slim AS runtime
RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        ca-certificates openssh-client docker.io curl \
    && rm -rf /var/lib/apt/lists/*
COPY --from=build /src/target/release/rustify-server /usr/local/bin/rustify-server

# Working data dir (SSH mux sockets + materialised keys, per Config::from_env).
ENV RUSTIFY_DATA_DIR=/data/rustify
RUN mkdir -p /data/rustify
VOLUME ["/data/rustify"]

EXPOSE 8000
ENTRYPOINT ["/usr/local/bin/rustify-server"]
