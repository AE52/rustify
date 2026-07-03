#![forbid(unsafe_code)]

//! rustify-docker: pure, DB-free generators for the Docker artifacts Rustify
//! produces — `docker build` commands, single-service compose files, container
//! labels (Contract C7) — plus parsers for `docker inspect`/`docker ps` output.

pub mod build_command;
pub mod compose;
pub mod db_compose;
pub mod inspect;
pub mod labels;
pub mod service_compose;
pub mod service_manifest;

pub use build_command::BuildCommand;
pub use compose::{AppComposeInput, HealthCheck, generate_compose};
pub use db_compose::{
    DatabaseComposeInput, generate_database_compose, generate_db_proxy_compose,
    generate_db_proxy_nginx_conf,
};
pub use inspect::{ContainerHealth, ManagedContainer, parse_containers, parse_health};
pub use labels::traefik_labels;
pub use service_compose::{MutatedService, ServiceComposeError, parse_and_mutate_service};
pub use service_manifest::{ServiceTemplate, build_manifest, load_manifest, parse_template};

#[cfg(test)]
pub(crate) mod test_support {
    /// Load a golden file from `tests/golden/`, stripping the leading `#` header
    /// comment lines (which cite the Coolify source) and any blank line that
    /// immediately follows them.
    pub fn load_golden(name: &str) -> String {
        let path = format!("{}/tests/golden/{name}", env!("CARGO_MANIFEST_DIR"));
        let raw = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {path}: {e}"));
        let mut lines = raw.lines().peekable();
        while let Some(line) = lines.peek() {
            if line.starts_with('#') {
                lines.next();
            } else {
                break;
            }
        }
        // Drop a single blank separator line after the header block.
        if lines.peek().map(|l| l.trim().is_empty()).unwrap_or(false) {
            lines.next();
        }
        lines.collect::<Vec<_>>().join("\n")
    }
}
