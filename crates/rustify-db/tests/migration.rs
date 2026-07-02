//! The migration (contract C6) applies cleanly and produces the pinned schema.

use sqlx::PgPool;

#[sqlx::test]
async fn migration_applies_and_core_tables_exist(pool: PgPool) {
    // `#[sqlx::test]` already ran MIGRATOR; assert the pinned tables + enum are
    // present and queryable.
    for table in [
        "teams",
        "users",
        "sessions",
        "api_tokens",
        "private_keys",
        "servers",
        "server_settings",
        "destinations",
        "projects",
        "environments",
        "applications",
        "environment_variables",
        "persistent_storages",
        "deployments",
        "deployment_logs",
        "instance_settings",
        "jobs",
    ] {
        let count: i64 = sqlx::query_scalar(&format!("SELECT count(*) FROM {table}"))
            .fetch_one(&pool)
            .await
            .unwrap_or_else(|e| panic!("table {table} missing: {e}"));
        assert_eq!(count, 0, "fresh {table} should be empty");
    }

    // The deployment_status enum exists with the C6 labels.
    let labels: Vec<String> = sqlx::query_scalar(
        "SELECT e.enumlabel FROM pg_enum e
         JOIN pg_type t ON t.oid = e.enumtypid
         WHERE t.typname = 'deployment_status' ORDER BY e.enumsortorder",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(
        labels,
        vec!["queued", "in_progress", "finished", "failed", "cancelled"]
    );
}
