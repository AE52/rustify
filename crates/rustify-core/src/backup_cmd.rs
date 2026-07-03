//! Database dump command construction (Contract: exact `mysqldump`/`pg_dump`/…
//! command strings).
//!
//! Behavioural port of Coolify's `DatabaseBackupJob` per-engine
//! `backup_standalone_*` methods (app/Jobs/DatabaseBackupJob.php): the command
//! returned here is the *inner* command run inside `docker exec <container>` by
//! the deploy handler, which appends the `> <location>` host redirect. The
//! password is embedded verbatim (matching Coolify's `-p<pw>` /
//! `PGPASSWORD=<pw>` forms); the caller must never log the returned string.
//!
//! Engines that Coolify cannot logically dump (redis / keydb / dragonfly /
//! clickhouse) return `None`.

use crate::db_engine::{DatabaseCredentials, DatabaseEngine};

/// Build the dump command for `engine`. `db` is the single database name to
/// dump (ignored when `dump_all`); returns `None` for engines with no dump
/// support.
///
/// Ported command-for-command from DatabaseBackupJob.php:
/// - postgres single: line 582 (`pg_dump --format=custom --no-acl --no-owner`)
/// - postgres all:    line 577 (`pg_dumpall | gzip`)
/// - mysql single:    line 608 (`mysqldump -u root -p<pw> <db>`)
/// - mysql all:       line 603 (`--all-databases --single-transaction --quick
///   --lock-tables=false --compress | gzip`)
/// - mariadb single:  line 632 (`mariadb-dump -u root -p<pw> <db>`)
/// - mariadb all:     line 627 (`--all-databases --compress`; the contract
///   drops the `--single-transaction --quick --lock-tables=false` flags and the
///   `> file` is a plain redirect with no gzip pipe)
/// - mongo single:    line 538 (`mongodump --authenticationDatabase=admin
///   --uri=<url> --db <db> --gzip --archive`)
/// - mongo all:       line 518 (same, omitting `--db`)
pub fn dump_command(
    engine: DatabaseEngine,
    creds: &DatabaseCredentials,
    db: &str,
    dump_all: bool,
) -> Option<String> {
    let pw = &creds.password;
    let user = &creds.username;
    Some(match engine {
        DatabaseEngine::Postgresql => {
            if dump_all {
                format!("PGPASSWORD={pw} pg_dumpall -U {user} | gzip")
            } else {
                format!(
                    "PGPASSWORD={pw} pg_dump --format=custom --no-acl --no-owner -U {user} {db}"
                )
            }
        }
        DatabaseEngine::Mysql => {
            if dump_all {
                format!(
                    "mysqldump -u root -p{pw} --all-databases --single-transaction --quick \
                     --lock-tables=false --compress | gzip"
                )
            } else {
                format!("mysqldump -u root -p{pw} {db}")
            }
        }
        DatabaseEngine::Mariadb => {
            if dump_all {
                format!("mariadb-dump -u root -p{pw} --all-databases --compress")
            } else {
                format!("mariadb-dump -u root -p{pw} {db}")
            }
        }
        DatabaseEngine::Mongodb => {
            let url = mongo_uri(creds);
            if dump_all {
                format!("mongodump --authenticationDatabase=admin --uri={url} --gzip --archive")
            } else {
                format!(
                    "mongodump --authenticationDatabase=admin --uri={url} --db {db} --gzip --archive"
                )
            }
        }
        DatabaseEngine::Redis
        | DatabaseEngine::Keydb
        | DatabaseEngine::Dragonfly
        | DatabaseEngine::Clickhouse => return None,
    })
}

/// The MongoDB connection URI used inside the container. The dump runs via
/// `docker exec <container> …`, so `127.0.0.1` addresses the mongod in the same
/// container. Credentials are auto-generated alphanumeric (`gen_password(_,
/// false)`), so no percent-encoding is required.
fn mongo_uri(creds: &DatabaseCredentials) -> String {
    format!(
        "mongodb://{}:{}@127.0.0.1:27017",
        creds.username, creds.password
    )
}

/// The backup file extension (no leading dot) for `engine`, or `None` when the
/// engine is not dumpable. Mirrors the `$this->backup_file` suffixes in
/// DatabaseBackupJob.php: single `.dmp` (line 327/362/376), pg-all `.gz` (329),
/// mysql/mariadb-all `.gz` (364/378 — note mariadb-all is *not* gzipped yet
/// still uses `.gz`), mongo `.tar.gz` (351).
pub fn backup_extension(engine: DatabaseEngine, dump_all: bool) -> Option<&'static str> {
    Some(match engine {
        DatabaseEngine::Mongodb => "tar.gz",
        DatabaseEngine::Postgresql | DatabaseEngine::Mysql | DatabaseEngine::Mariadb => {
            if dump_all { "gz" } else { "dmp" }
        }
        DatabaseEngine::Redis
        | DatabaseEngine::Keydb
        | DatabaseEngine::Dragonfly
        | DatabaseEngine::Clickhouse => return None,
    })
}

/// The filename prefix Coolify uses per engine (`<prefix>-dump-<db>-<ts>.<ext>`),
/// e.g. `pg`, `mysql`, `mariadb`, `mongo` (DatabaseBackupJob.php:327/362/376/351).
pub fn dump_prefix(engine: DatabaseEngine) -> Option<&'static str> {
    Some(match engine {
        DatabaseEngine::Postgresql => "pg",
        DatabaseEngine::Mysql => "mysql",
        DatabaseEngine::Mariadb => "mariadb",
        DatabaseEngine::Mongodb => "mongo",
        DatabaseEngine::Redis
        | DatabaseEngine::Keydb
        | DatabaseEngine::Dragonfly
        | DatabaseEngine::Clickhouse => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn creds() -> DatabaseCredentials {
        DatabaseCredentials {
            username: "u".into(),
            password: "secretpw".into(),
            database: "d".into(),
            root_password: "r".into(),
        }
    }

    #[test]
    fn postgres_single_matches_contract() {
        // DatabaseBackupJob.php:582
        assert_eq!(
            dump_command(DatabaseEngine::Postgresql, &creds(), "mydb", false).unwrap(),
            "PGPASSWORD=secretpw pg_dump --format=custom --no-acl --no-owner -U u mydb"
        );
    }

    #[test]
    fn postgres_all_pipes_gzip() {
        // DatabaseBackupJob.php:577
        assert_eq!(
            dump_command(DatabaseEngine::Postgresql, &creds(), "ignored", true).unwrap(),
            "PGPASSWORD=secretpw pg_dumpall -U u | gzip"
        );
    }

    #[test]
    fn mysql_single_and_all() {
        // DatabaseBackupJob.php:608 / 603
        assert_eq!(
            dump_command(DatabaseEngine::Mysql, &creds(), "app", false).unwrap(),
            "mysqldump -u root -psecretpw app"
        );
        assert_eq!(
            dump_command(DatabaseEngine::Mysql, &creds(), "x", true).unwrap(),
            "mysqldump -u root -psecretpw --all-databases --single-transaction --quick \
             --lock-tables=false --compress | gzip"
        );
    }

    #[test]
    fn mariadb_single_and_all_no_gzip() {
        // DatabaseBackupJob.php:632 / 627 (contract: no gzip pipe for all)
        assert_eq!(
            dump_command(DatabaseEngine::Mariadb, &creds(), "app", false).unwrap(),
            "mariadb-dump -u root -psecretpw app"
        );
        let all = dump_command(DatabaseEngine::Mariadb, &creds(), "x", true).unwrap();
        assert_eq!(
            all,
            "mariadb-dump -u root -psecretpw --all-databases --compress"
        );
        assert!(!all.contains("gzip"), "mariadb all has no gzip pipe");
    }

    #[test]
    fn mongo_single_and_all() {
        // DatabaseBackupJob.php:538 / 518
        assert_eq!(
            dump_command(DatabaseEngine::Mongodb, &creds(), "app", false).unwrap(),
            "mongodump --authenticationDatabase=admin \
             --uri=mongodb://u:secretpw@127.0.0.1:27017 --db app --gzip --archive"
        );
        assert_eq!(
            dump_command(DatabaseEngine::Mongodb, &creds(), "x", true).unwrap(),
            "mongodump --authenticationDatabase=admin \
             --uri=mongodb://u:secretpw@127.0.0.1:27017 --gzip --archive"
        );
    }

    #[test]
    fn unsupported_engines_return_none() {
        for e in [
            DatabaseEngine::Redis,
            DatabaseEngine::Keydb,
            DatabaseEngine::Dragonfly,
            DatabaseEngine::Clickhouse,
        ] {
            assert!(dump_command(e, &creds(), "d", false).is_none());
            assert!(backup_extension(e, false).is_none());
            assert!(dump_prefix(e).is_none());
        }
    }

    #[test]
    fn extensions_are_exact() {
        assert_eq!(
            backup_extension(DatabaseEngine::Postgresql, false),
            Some("dmp")
        );
        assert_eq!(
            backup_extension(DatabaseEngine::Postgresql, true),
            Some("gz")
        );
        assert_eq!(backup_extension(DatabaseEngine::Mysql, true), Some("gz"));
        assert_eq!(backup_extension(DatabaseEngine::Mariadb, true), Some("gz"));
        assert_eq!(
            backup_extension(DatabaseEngine::Mariadb, false),
            Some("dmp")
        );
        assert_eq!(
            backup_extension(DatabaseEngine::Mongodb, false),
            Some("tar.gz")
        );
        assert_eq!(
            backup_extension(DatabaseEngine::Mongodb, true),
            Some("tar.gz")
        );
    }

    #[test]
    fn prefixes_match_coolify() {
        assert_eq!(dump_prefix(DatabaseEngine::Postgresql), Some("pg"));
        assert_eq!(dump_prefix(DatabaseEngine::Mysql), Some("mysql"));
        assert_eq!(dump_prefix(DatabaseEngine::Mariadb), Some("mariadb"));
        assert_eq!(dump_prefix(DatabaseEngine::Mongodb), Some("mongo"));
    }
}
