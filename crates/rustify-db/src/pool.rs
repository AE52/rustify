//! Connection pool construction and the embedded migrator.

use sqlx::PgPool;
use sqlx::migrate::Migrator;
use sqlx::postgres::PgPoolOptions;

use crate::DbResult;

/// All migrations under `crates/rustify-db/migrations`, embedded at compile
/// time. `rustify-server` runs `MIGRATOR.run(&pool)` on startup; `#[sqlx::test]`
/// applies the same set to each ephemeral test database.
pub static MIGRATOR: Migrator = sqlx::migrate!("./migrations");

/// Open a connection pool to `url` (a `postgres://` DSN).
pub async fn connect(url: &str) -> DbResult<PgPool> {
    let pool = PgPoolOptions::new()
        .max_connections(10)
        .connect(url)
        .await?;
    Ok(pool)
}

/// Open a pool and run all pending migrations. Idempotent.
pub async fn connect_and_migrate(url: &str) -> DbResult<PgPool> {
    let pool = connect(url).await?;
    MIGRATOR.run(&pool).await?;
    Ok(pool)
}
