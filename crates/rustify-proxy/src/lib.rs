#![forbid(unsafe_code)]

//! rustify-proxy: generators for the per-server Traefik reverse proxy — its
//! `docker-compose.yml`, custom-command survival across regeneration, and the
//! start/stop shell scripts. Naming follows Contract C7 (`rustify-proxy`,
//! `/data/rustify/proxy`, network `rustify`).

pub mod caddy;
pub mod config;
pub mod lifecycle;

pub use caddy::generate_caddy_proxy_compose;
pub use config::{
    PROXY_CONTAINER, PROXY_DIR, PROXY_NETWORK, extract_custom_commands, generate_proxy_compose,
};
pub use lifecycle::{start_script, stop_script};
