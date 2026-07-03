//! `docker-compose.yml` generation for a standalone database and its optional
//! public TCP proxy sidecar.
//!
//! A clean-slate collapse of Coolify's eight `Start*.php` `$docker_compose`
//! builders (e.g. StartPostgresql.php:100-131, StartRedis.php:70-105,
//! StartMongodb.php:100-131) and `StartDatabaseProxy.php` into two DB-free
//! generators. Container name and compose service key are both the database
//! uuid; the network is external+attachable (StartPostgresql.php:117-122).

use std::collections::BTreeMap;

use rustify_core::{DatabaseCredentials, DatabaseEngine};
use serde::Serialize;

/// DB-free description of a standalone database to render into a compose file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DatabaseComposeInput {
    /// Container name and compose service key.
    pub uuid: String,
    pub engine: DatabaseEngine,
    pub image: String,
    /// Destination network (default `rustify`).
    pub network: String,
    /// Engine credentials (decrypted) — projected to env vars + command.
    pub credentials: DatabaseCredentials,
    /// Memory limit; `"0"` (unlimited) omits the key.
    pub limits_memory: String,
    /// CPU limit; `"0"` (unlimited) omits the key.
    pub limits_cpus: String,
    pub health_check_enabled: bool,
    pub health_check_interval: u32,
    pub health_check_timeout: u32,
    pub health_check_retries: u32,
    pub health_check_start_period: u32,
    /// Host published port mappings (e.g. `["5433:5432"]`); usually empty.
    pub ports_mappings: Vec<String>,
}

#[derive(Serialize)]
struct DbCompose {
    services: BTreeMap<String, DbService>,
    networks: BTreeMap<String, Network>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    volumes: BTreeMap<String, NamedVolume>,
}

#[derive(Serialize)]
struct DbService {
    image: String,
    container_name: String,
    restart: String,
    networks: Vec<String>,
    environment: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    command: Option<String>,
    labels: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    healthcheck: Option<Healthcheck>,
    #[serde(skip_serializing_if = "Option::is_none")]
    mem_limit: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    cpus: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    volumes: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    ports: Vec<String>,
}

#[derive(Serialize)]
struct Healthcheck {
    test: Vec<String>,
    interval: String,
    timeout: String,
    retries: u32,
    start_period: String,
}

/// External docker network reference (StartPostgresql.php:117-122).
#[derive(Serialize)]
struct Network {
    external: bool,
    attachable: bool,
    name: String,
}

/// A named docker volume declared under top-level `volumes:` (external: false).
#[derive(Serialize)]
struct NamedVolume {
    external: bool,
}

/// Labels every managed database container carries (Contract C7 + Phase 2
/// conventions; port of `defaultDatabaseLabels`, docker.php:218-231).
fn database_labels(uuid: &str, type_slug: &str) -> Vec<String> {
    vec![
        "rustify.managed=true".to_string(),
        "rustify.type=database".to_string(),
        format!("rustify.databaseUuid={uuid}"),
        format!("rustify.database.subType={type_slug}"),
    ]
}

/// Render a single-service compose file for a standalone database. Deterministic.
pub fn generate_database_compose(db: &DatabaseComposeInput) -> String {
    let descriptor = db.engine.descriptor();

    let environment: Vec<String> = db
        .engine
        .credential_env(&db.credentials)
        .into_iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect();

    let volumes: Vec<String> = descriptor
        .volume_mounts
        .iter()
        .map(|(name, path)| format!("{name}:{path}"))
        .collect();

    let mut named_volumes = BTreeMap::new();
    for (name, _) in descriptor.volume_mounts {
        named_volumes.insert((*name).to_string(), NamedVolume { external: false });
    }

    let healthcheck = db.health_check_enabled.then(|| Healthcheck {
        test: db.engine.healthcheck_test(&db.credentials),
        interval: format!("{}s", db.health_check_interval),
        timeout: format!("{}s", db.health_check_timeout),
        retries: db.health_check_retries,
        start_period: format!("{}s", db.health_check_start_period),
    });

    let mem_limit = (db.limits_memory != "0").then(|| db.limits_memory.clone());
    let cpus = (db.limits_cpus != "0").then(|| db.limits_cpus.clone());

    let service = DbService {
        image: db.image.clone(),
        container_name: db.uuid.clone(),
        restart: "unless-stopped".to_string(),
        networks: vec![db.network.clone()],
        environment,
        command: db.engine.command(&db.credentials),
        labels: database_labels(&db.uuid, descriptor.type_slug),
        healthcheck,
        mem_limit,
        cpus,
        volumes,
        ports: db.ports_mappings.clone(),
    };

    let mut services = BTreeMap::new();
    services.insert(db.uuid.clone(), service);
    let mut networks = BTreeMap::new();
    networks.insert(
        db.network.clone(),
        Network {
            external: true,
            attachable: true,
            name: db.network.clone(),
        },
    );

    let compose = DbCompose {
        services,
        networks,
        volumes: named_volumes,
    };
    serde_yaml::to_string(&compose).unwrap_or_default()
}

// ----- public TCP proxy sidecar (StartDatabaseProxy.php) -------------------

#[derive(Serialize)]
struct ProxyCompose {
    services: BTreeMap<String, ProxyService>,
    networks: BTreeMap<String, Network>,
}

#[derive(Serialize)]
struct ProxyService {
    image: String,
    container_name: String,
    restart: String,
    ports: Vec<String>,
    networks: Vec<String>,
    volumes: Vec<String>,
    healthcheck: Healthcheck,
}

/// Render the nginx stream-proxy `docker-compose.yml` that publishes a database
/// on `public_port` (StartDatabaseProxy.php:81-118). The proxy container is
/// `{uuid}-proxy`; its `nginx.conf` (see [`generate_db_proxy_nginx_conf`]) is
/// bind-mounted from the compose project directory.
pub fn generate_db_proxy_compose(
    uuid: &str,
    public_port: u16,
    _internal_port: u16,
    _timeout_secs: u32,
    network: &str,
) -> String {
    let proxy_name = format!("{uuid}-proxy");
    let service = ProxyService {
        image: "nginx:stable-alpine".to_string(),
        container_name: proxy_name.clone(),
        restart: "unless-stopped".to_string(),
        ports: vec![format!("{public_port}:{public_port}")],
        networks: vec![network.to_string()],
        volumes: vec!["./nginx.conf:/etc/nginx/nginx.conf".to_string()],
        healthcheck: Healthcheck {
            test: vec![
                "CMD-SHELL".to_string(),
                "stat /etc/nginx/nginx.conf || exit 1".to_string(),
            ],
            interval: "5s".to_string(),
            timeout: "5s".to_string(),
            retries: 3,
            start_period: "1s".to_string(),
        },
    };
    let mut services = BTreeMap::new();
    services.insert(proxy_name, service);
    let mut networks = BTreeMap::new();
    networks.insert(
        network.to_string(),
        Network {
            external: true,
            attachable: true,
            name: network.to_string(),
        },
    );
    let compose = ProxyCompose { services, networks };
    serde_yaml::to_string(&compose).unwrap_or_default()
}

/// The nginx `stream {}` config that forwards `public_port` to the database
/// container's internal port (StartDatabaseProxy.php:74-80).
pub fn generate_db_proxy_nginx_conf(
    uuid: &str,
    public_port: u16,
    internal_port: u16,
    timeout_secs: u32,
) -> String {
    let timeout = if timeout_secs < 1 { 3600 } else { timeout_secs };
    format!(
        "user  nginx;\n\
         worker_processes  auto;\n\
         \n\
         error_log  /var/log/nginx/error.log;\n\
         \n\
         events {{\n\
         \x20   worker_connections  1024;\n\
         }}\n\
         stream {{\n\
         \x20  server {{\n\
         \x20       listen {public_port};\n\
         \x20       proxy_pass {uuid}:{internal_port};\n\
         \x20       proxy_timeout {timeout}s;\n\
         \x20  }}\n\
         }}\n"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustify_core::DatabaseCredentials;

    fn creds() -> DatabaseCredentials {
        DatabaseCredentials {
            username: "postgres".into(),
            password: "SECRETPW".into(),
            database: "postgres".into(),
            root_password: "ROOTPW".into(),
        }
    }

    fn input(engine: DatabaseEngine) -> DatabaseComposeInput {
        DatabaseComposeInput {
            uuid: "db-uuid".into(),
            engine,
            image: engine.descriptor().default_image.to_string(),
            network: "rustify".into(),
            credentials: creds(),
            limits_memory: "0".into(),
            limits_cpus: "0".into(),
            health_check_enabled: true,
            health_check_interval: 15,
            health_check_timeout: 5,
            health_check_retries: 5,
            health_check_start_period: 5,
            ports_mappings: vec![],
        }
    }

    #[test]
    fn postgres_matches_golden() {
        let generated = generate_database_compose(&input(DatabaseEngine::Postgresql));
        let golden = crate::test_support::load_golden("db-postgres.yaml");
        assert_eq!(generated.trim(), golden.trim());
    }

    #[test]
    fn redis_matches_golden() {
        let generated = generate_database_compose(&input(DatabaseEngine::Redis));
        let golden = crate::test_support::load_golden("db-redis.yaml");
        assert_eq!(generated.trim(), golden.trim());
    }

    #[test]
    fn mongo_matches_golden() {
        let generated = generate_database_compose(&input(DatabaseEngine::Mongodb));
        let golden = crate::test_support::load_golden("db-mongo.yaml");
        assert_eq!(generated.trim(), golden.trim());
    }

    #[test]
    fn redis_has_requirepass_command() {
        let compose = generate_database_compose(&input(DatabaseEngine::Redis));
        assert!(compose.contains("redis-server --requirepass SECRETPW --appendonly yes"));
    }

    #[test]
    fn unlimited_resources_are_omitted() {
        let compose = generate_database_compose(&input(DatabaseEngine::Postgresql));
        assert!(!compose.contains("mem_limit"));
        assert!(!compose.contains("cpus"));
    }

    #[test]
    fn resource_limits_render_when_set() {
        let mut i = input(DatabaseEngine::Postgresql);
        i.limits_memory = "512m".into();
        i.limits_cpus = "1.5".into();
        let compose = generate_database_compose(&i);
        assert!(compose.contains("mem_limit: 512m"));
        assert!(compose.contains("cpus: '1.5'") || compose.contains("cpus: 1.5"));
    }

    #[test]
    fn disabled_healthcheck_is_omitted() {
        let mut i = input(DatabaseEngine::Postgresql);
        i.health_check_enabled = false;
        let compose = generate_database_compose(&i);
        assert!(!compose.contains("healthcheck"));
    }

    #[test]
    fn mongodb_declares_two_named_volumes() {
        let compose = generate_database_compose(&input(DatabaseEngine::Mongodb));
        assert!(compose.contains("mongodb-configdb:/data/configdb"));
        assert!(compose.contains("mongodb-db:/data/db"));
    }

    #[test]
    fn proxy_compose_publishes_public_port() {
        let compose = generate_db_proxy_compose("db-uuid", 5433, 5432, 3600, "rustify");
        assert!(compose.contains("db-uuid-proxy"));
        assert!(compose.contains("image: nginx:stable-alpine"));
        assert!(compose.contains("5433:5433"));
    }

    #[test]
    fn proxy_nginx_conf_streams_to_internal_port() {
        let conf = generate_db_proxy_nginx_conf("db-uuid", 5433, 5432, 3600);
        assert!(conf.contains("listen 5433;"));
        assert!(conf.contains("proxy_pass db-uuid:5432;"));
        assert!(conf.contains("proxy_timeout 3600s;"));
    }

    #[test]
    fn proxy_nginx_conf_defaults_zero_timeout() {
        let conf = generate_db_proxy_nginx_conf("db-uuid", 5433, 5432, 0);
        assert!(conf.contains("proxy_timeout 3600s;"));
    }
}
