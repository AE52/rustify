//! Standalone database engines.
//!
//! A clean-slate collapse of Coolify's eight `Standalone*` models +
//! `Start*.php` actions (app/Actions/Database/) into a single enum. Each engine
//! carries a static [`EngineDescriptor`] (image, ports, named volumes, type
//! slug) and a credential → environment-variable mapping ported verbatim from
//! the per-engine `generate_environment_variables()` methods, e.g.
//! StartPostgresql.php:257-283, StartMysql / StartMariadb / StartMongodb /
//! StartClickhouse / StartRedis `generate_environment_variables()`.
//!
//! The container start command for the in-memory stores (redis / keydb /
//! dragonfly) is ported from their `buildStartCommand()` (StartRedis.php:279,
//! StartKeydb.php:279, StartDragonfly.php `buildStartCommand`), and the
//! healthcheck probes from each action's `healthCheckConfiguration([...])` call.

use serde::{Deserialize, Serialize};

use crate::passwords::gen_password;

/// One of the eight supported standalone database engines. Serialises as its
/// lowercase snake_case name (`postgresql`, `mysql`, ...).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DatabaseEngine {
    Postgresql,
    Mysql,
    Mariadb,
    Mongodb,
    Redis,
    Keydb,
    Dragonfly,
    Clickhouse,
}

/// Every engine, in declaration order — for exhaustive iteration in tests and
/// API validation.
pub const ALL_ENGINES: [DatabaseEngine; 8] = [
    DatabaseEngine::Postgresql,
    DatabaseEngine::Mysql,
    DatabaseEngine::Mariadb,
    DatabaseEngine::Mongodb,
    DatabaseEngine::Redis,
    DatabaseEngine::Keydb,
    DatabaseEngine::Dragonfly,
    DatabaseEngine::Clickhouse,
];

/// Static per-engine facts the compose generator and repos need.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EngineDescriptor {
    /// Default image when the create request omits one (e.g. `postgres:16-alpine`).
    pub default_image: &'static str,
    /// The port the server listens on inside the container.
    pub internal_port: u16,
    /// The TLS port (differs from `internal_port` only for redis/keydb/dragonfly).
    pub tls_port: u16,
    /// Named docker volumes as `(volume_name, container_mount_path)`.
    pub volume_mounts: &'static [(&'static str, &'static str)],
    /// The `rustify.database.subType` label value (`standalone-<engine>`).
    pub type_slug: &'static str,
}

/// Engine credentials, encrypted at rest (AES-GCM JSON, `credentials_enc`).
///
/// Not every field is meaningful for every engine (e.g. redis has no
/// `database`, only postgres/mysql/mariadb use a separate user vs. root
/// password); [`DatabaseEngine::credential_env`] projects out exactly the
/// variables each engine consumes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DatabaseCredentials {
    #[serde(default)]
    pub username: String,
    pub password: String,
    #[serde(default)]
    pub database: String,
    /// Root/superuser password (mysql/mariadb `*_ROOT_PASSWORD`).
    #[serde(default)]
    pub root_password: String,
}

impl DatabaseEngine {
    /// Parse the snake_case engine name (as accepted by the create API).
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "postgresql" => Self::Postgresql,
            "mysql" => Self::Mysql,
            "mariadb" => Self::Mariadb,
            "mongodb" => Self::Mongodb,
            "redis" => Self::Redis,
            "keydb" => Self::Keydb,
            "dragonfly" => Self::Dragonfly,
            "clickhouse" => Self::Clickhouse,
            _ => return None,
        })
    }

    /// The snake_case engine name (matches the serde representation).
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Postgresql => "postgresql",
            Self::Mysql => "mysql",
            Self::Mariadb => "mariadb",
            Self::Mongodb => "mongodb",
            Self::Redis => "redis",
            Self::Keydb => "keydb",
            Self::Dragonfly => "dragonfly",
            Self::Clickhouse => "clickhouse",
        }
    }

    /// Static descriptor for this engine.
    pub fn descriptor(&self) -> EngineDescriptor {
        match self {
            Self::Postgresql => EngineDescriptor {
                default_image: "postgres:16-alpine",
                internal_port: 5432,
                tls_port: 5432,
                volume_mounts: &[("postgres-data", "/var/lib/postgresql/data")],
                type_slug: "standalone-postgresql",
            },
            Self::Mysql => EngineDescriptor {
                default_image: "mysql:8",
                internal_port: 3306,
                tls_port: 3306,
                volume_mounts: &[("mysql-data", "/var/lib/mysql")],
                type_slug: "standalone-mysql",
            },
            Self::Mariadb => EngineDescriptor {
                default_image: "mariadb:11",
                internal_port: 3306,
                tls_port: 3306,
                volume_mounts: &[("mariadb-data", "/var/lib/mysql")],
                type_slug: "standalone-mariadb",
            },
            Self::Mongodb => EngineDescriptor {
                default_image: "mongo:7",
                internal_port: 27017,
                tls_port: 27017,
                volume_mounts: &[
                    ("mongodb-configdb", "/data/configdb"),
                    ("mongodb-db", "/data/db"),
                ],
                type_slug: "standalone-mongodb",
            },
            Self::Redis => EngineDescriptor {
                default_image: "redis:7.2",
                internal_port: 6379,
                tls_port: 6380,
                volume_mounts: &[("redis-data", "/data")],
                type_slug: "standalone-redis",
            },
            Self::Keydb => EngineDescriptor {
                default_image: "eqalpha/keydb:latest",
                internal_port: 6379,
                tls_port: 6380,
                volume_mounts: &[("keydb-data", "/data")],
                type_slug: "standalone-keydb",
            },
            Self::Dragonfly => EngineDescriptor {
                default_image: "docker.dragonflydb.io/dragonflydb/dragonfly",
                internal_port: 6379,
                tls_port: 6380,
                volume_mounts: &[("dragonfly-data", "/data")],
                type_slug: "standalone-dragonfly",
            },
            Self::Clickhouse => EngineDescriptor {
                default_image: "clickhouse/clickhouse-server:25.11",
                internal_port: 9000,
                tls_port: 9000,
                volume_mounts: &[("clickhouse-data", "/var/lib/clickhouse")],
                type_slug: "standalone-clickhouse",
            },
        }
    }

    /// Default credentials for a freshly created database: a random 64-char
    /// alphanumeric password (and a separate root password for mysql/mariadb)
    /// plus the engine's conventional default user/db name. Mirrors the model
    /// defaults each Coolify `Standalone*` model seeds on create.
    pub fn default_credentials(&self) -> DatabaseCredentials {
        let (username, database) = match self {
            Self::Postgresql => ("postgres", "postgres"),
            Self::Mysql => ("mysql", "default"),
            Self::Mariadb => ("mariadb", "default"),
            Self::Mongodb => ("root", "default"),
            Self::Clickhouse => ("default", "default"),
            Self::Redis => ("default", ""),
            Self::Keydb | Self::Dragonfly => ("", ""),
        };
        DatabaseCredentials {
            username: username.to_string(),
            password: gen_password(64, false),
            database: database.to_string(),
            root_password: gen_password(64, false),
        }
    }

    /// The environment variables the engine's image consumes to bootstrap its
    /// user/password/database, in the exact order Coolify pushes them.
    pub fn credential_env(&self, creds: &DatabaseCredentials) -> Vec<(String, String)> {
        let pairs: Vec<(&str, &str)> = match self {
            // StartPostgresql.php:257-283
            Self::Postgresql => vec![
                ("POSTGRES_USER", &creds.username),
                ("POSTGRES_PASSWORD", &creds.password),
                ("POSTGRES_DB", &creds.database),
                ("PGUSER", &creds.username),
            ],
            // StartMysql.php generate_environment_variables()
            Self::Mysql => vec![
                ("MYSQL_ROOT_PASSWORD", &creds.root_password),
                ("MYSQL_DATABASE", &creds.database),
                ("MYSQL_USER", &creds.username),
                ("MYSQL_PASSWORD", &creds.password),
            ],
            // StartMariadb.php generate_environment_variables()
            Self::Mariadb => vec![
                ("MARIADB_ROOT_PASSWORD", &creds.root_password),
                ("MARIADB_DATABASE", &creds.database),
                ("MARIADB_USER", &creds.username),
                ("MARIADB_PASSWORD", &creds.password),
            ],
            // StartMongodb.php:312-319
            Self::Mongodb => vec![
                ("MONGO_INITDB_ROOT_USERNAME", &creds.username),
                ("MONGO_INITDB_ROOT_PASSWORD", &creds.password),
                ("MONGO_INITDB_DATABASE", &creds.database),
            ],
            // StartClickhouse.php generate_environment_variables()
            Self::Clickhouse => vec![
                ("CLICKHOUSE_USER", &creds.username),
                ("CLICKHOUSE_PASSWORD", &creds.password),
                ("CLICKHOUSE_DB", &creds.database),
            ],
            // StartRedis.php generate_environment_variables() — redis carries a
            // username; keydb/dragonfly authenticate with the password only.
            Self::Redis => vec![
                ("REDIS_PASSWORD", &creds.password),
                ("REDIS_USERNAME", &creds.username),
            ],
            Self::Keydb | Self::Dragonfly => vec![("REDIS_PASSWORD", &creds.password)],
        };
        pairs
            .into_iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    /// The container `command`, for engines that need one to enable auth. Ported
    /// from `buildStartCommand()` (StartRedis.php:279, StartKeydb.php:279,
    /// StartDragonfly.php). Datastore engines (postgres/mysql/... ) rely on their
    /// image entrypoint and return `None`.
    pub fn command(&self, creds: &DatabaseCredentials) -> Option<String> {
        match self {
            Self::Redis => Some(format!(
                "redis-server --requirepass {} --appendonly yes",
                creds.password
            )),
            Self::Keydb => Some(format!(
                "keydb-server --requirepass {} --appendonly yes",
                creds.password
            )),
            Self::Dragonfly => Some(format!("dragonfly --requirepass {}", creds.password)),
            _ => None,
        }
    }

    /// The docker healthcheck `test` array for this engine, ported from each
    /// action's `healthCheckConfiguration([...])` call.
    pub fn healthcheck_test(&self, creds: &DatabaseCredentials) -> Vec<String> {
        let parts: Vec<String> = match self {
            // StartPostgresql.php:110
            Self::Postgresql => vec![
                "CMD".into(),
                "psql".into(),
                "-U".into(),
                creds.username.clone(),
                "-d".into(),
                creds.database.clone(),
                "-c".into(),
                "SELECT 1".into(),
            ],
            // StartMysql.php healthCheckConfiguration
            Self::Mysql => vec![
                "CMD".into(),
                "mysqladmin".into(),
                "ping".into(),
                "-h".into(),
                "localhost".into(),
                "-u".into(),
                "root".into(),
                format!("-p{}", creds.root_password),
            ],
            // StartMariadb.php:106
            Self::Mariadb => vec![
                "CMD".into(),
                "healthcheck.sh".into(),
                "--connect".into(),
                "--innodb_initialized".into(),
            ],
            // StartMongodb.php:112
            Self::Mongodb => vec!["CMD".into(), "echo".into(), "ok".into()],
            // StartClickhouse.php healthCheckConfiguration
            Self::Clickhouse => vec![
                "CMD".into(),
                "clickhouse-client".into(),
                "--user".into(),
                creds.username.clone(),
                "--password".into(),
                creds.password.clone(),
                "--query".into(),
                "SELECT 1".into(),
            ],
            // StartRedis.php / StartKeydb.php healthCheckConfiguration
            Self::Redis | Self::Keydb => {
                vec!["CMD-SHELL".into(), "redis-cli".into(), "ping".into()]
            }
            // StartDragonfly.php healthCheckConfiguration
            Self::Dragonfly => vec![
                "CMD".into(),
                "redis-cli".into(),
                "-a".into(),
                creds.password.clone(),
                "ping".into(),
            ],
        };
        parts
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn creds() -> DatabaseCredentials {
        DatabaseCredentials {
            username: "u".into(),
            password: "p".into(),
            database: "d".into(),
            root_password: "r".into(),
        }
    }

    #[test]
    fn parse_roundtrips_all_engines() {
        for e in ALL_ENGINES {
            assert_eq!(DatabaseEngine::parse(e.as_str()), Some(e));
        }
        assert_eq!(DatabaseEngine::parse("nope"), None);
    }

    #[test]
    fn descriptors_are_exact() {
        let d = DatabaseEngine::Postgresql.descriptor();
        assert_eq!(d.default_image, "postgres:16-alpine");
        assert_eq!(d.internal_port, 5432);
        assert_eq!(
            d.volume_mounts,
            &[("postgres-data", "/var/lib/postgresql/data")]
        );
        assert_eq!(d.type_slug, "standalone-postgresql");

        let m = DatabaseEngine::Mongodb.descriptor();
        assert_eq!(m.default_image, "mongo:7");
        assert_eq!(m.internal_port, 27017);
        assert_eq!(
            m.volume_mounts,
            &[
                ("mongodb-configdb", "/data/configdb"),
                ("mongodb-db", "/data/db")
            ]
        );

        // redis/keydb/dragonfly expose a distinct TLS port.
        for e in [
            DatabaseEngine::Redis,
            DatabaseEngine::Keydb,
            DatabaseEngine::Dragonfly,
        ] {
            let d = e.descriptor();
            assert_eq!(d.internal_port, 6379);
            assert_eq!(d.tls_port, 6380);
        }

        assert_eq!(DatabaseEngine::Mysql.descriptor().default_image, "mysql:8");
        assert_eq!(
            DatabaseEngine::Mariadb.descriptor().default_image,
            "mariadb:11"
        );
        assert_eq!(
            DatabaseEngine::Keydb.descriptor().default_image,
            "eqalpha/keydb:latest"
        );
        assert_eq!(
            DatabaseEngine::Dragonfly.descriptor().default_image,
            "docker.dragonflydb.io/dragonflydb/dragonfly"
        );
        assert_eq!(
            DatabaseEngine::Clickhouse.descriptor().default_image,
            "clickhouse/clickhouse-server:25.11"
        );

        // type_slug is standalone-<engine> for every engine.
        for e in ALL_ENGINES {
            assert_eq!(
                e.descriptor().type_slug,
                format!("standalone-{}", e.as_str())
            );
        }
    }

    #[test]
    fn credential_env_is_exact_for_all_engines() {
        let c = creds();
        assert_eq!(
            DatabaseEngine::Postgresql.credential_env(&c),
            vec![
                ("POSTGRES_USER".into(), "u".into()),
                ("POSTGRES_PASSWORD".into(), "p".into()),
                ("POSTGRES_DB".into(), "d".into()),
                ("PGUSER".into(), "u".into()),
            ]
        );
        assert_eq!(
            DatabaseEngine::Mysql.credential_env(&c),
            vec![
                ("MYSQL_ROOT_PASSWORD".into(), "r".into()),
                ("MYSQL_DATABASE".into(), "d".into()),
                ("MYSQL_USER".into(), "u".into()),
                ("MYSQL_PASSWORD".into(), "p".into()),
            ]
        );
        assert_eq!(
            DatabaseEngine::Mariadb.credential_env(&c),
            vec![
                ("MARIADB_ROOT_PASSWORD".into(), "r".into()),
                ("MARIADB_DATABASE".into(), "d".into()),
                ("MARIADB_USER".into(), "u".into()),
                ("MARIADB_PASSWORD".into(), "p".into()),
            ]
        );
        assert_eq!(
            DatabaseEngine::Mongodb.credential_env(&c),
            vec![
                ("MONGO_INITDB_ROOT_USERNAME".into(), "u".into()),
                ("MONGO_INITDB_ROOT_PASSWORD".into(), "p".into()),
                ("MONGO_INITDB_DATABASE".into(), "d".into()),
            ]
        );
        assert_eq!(
            DatabaseEngine::Clickhouse.credential_env(&c),
            vec![
                ("CLICKHOUSE_USER".into(), "u".into()),
                ("CLICKHOUSE_PASSWORD".into(), "p".into()),
                ("CLICKHOUSE_DB".into(), "d".into()),
            ]
        );
        assert_eq!(
            DatabaseEngine::Redis.credential_env(&c),
            vec![
                ("REDIS_PASSWORD".into(), "p".into()),
                ("REDIS_USERNAME".into(), "u".into()),
            ]
        );
        assert_eq!(
            DatabaseEngine::Keydb.credential_env(&c),
            vec![("REDIS_PASSWORD".into(), "p".into())]
        );
        assert_eq!(
            DatabaseEngine::Dragonfly.credential_env(&c),
            vec![("REDIS_PASSWORD".into(), "p".into())]
        );
    }

    #[test]
    fn only_in_memory_stores_have_a_command() {
        let c = creds();
        assert_eq!(
            DatabaseEngine::Redis.command(&c).as_deref(),
            Some("redis-server --requirepass p --appendonly yes")
        );
        assert_eq!(
            DatabaseEngine::Keydb.command(&c).as_deref(),
            Some("keydb-server --requirepass p --appendonly yes")
        );
        assert_eq!(
            DatabaseEngine::Dragonfly.command(&c).as_deref(),
            Some("dragonfly --requirepass p")
        );
        assert_eq!(DatabaseEngine::Postgresql.command(&c), None);
        assert_eq!(DatabaseEngine::Mongodb.command(&c), None);
    }

    #[test]
    fn default_credentials_have_expected_shape() {
        let c = DatabaseEngine::Postgresql.default_credentials();
        assert_eq!(c.username, "postgres");
        assert_eq!(c.database, "postgres");
        assert_eq!(c.password.len(), 64);
        assert_eq!(
            DatabaseEngine::Mysql.default_credentials().database,
            "default"
        );
        assert_eq!(
            DatabaseEngine::Redis.default_credentials().username,
            "default"
        );
    }
}
