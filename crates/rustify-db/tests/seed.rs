//! seed_default: idempotent team #1 + argon2id admin from env.

use sqlx::PgPool;

use rustify_db::repos::{seed_default, users};

/// Set the admin-credential env vars once for this test binary.
fn set_admin_env() {
    use std::sync::Once;
    static ADMIN: Once = Once::new();
    ADMIN.call_once(|| {
        // SAFETY: run once, before any read, in a dedicated test binary.
        unsafe {
            std::env::set_var("RUSTIFY_ADMIN_EMAIL", "admin@example.com");
            std::env::set_var("RUSTIFY_ADMIN_PASSWORD", "correct horse battery staple");
        }
    });
}

#[sqlx::test]
async fn seed_creates_team_and_hashed_admin_and_is_idempotent(pool: PgPool) {
    set_admin_env();

    seed_default(&pool).await.unwrap();
    // Second call must not create duplicates.
    seed_default(&pool).await.unwrap();

    let teams: i64 = sqlx::query_scalar("SELECT count(*) FROM teams")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(teams, 1, "exactly one team (the root team)");

    // The seeded team is the instance-wide root team (id 0).
    let root: i64 = sqlx::query_scalar("SELECT id FROM teams ORDER BY id LIMIT 1")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(root, 0, "root team has id 0");

    // The admin owns the root team via the membership pivot (idempotently).
    let (pivot_team, role): (i64, String) = sqlx::query_as("SELECT team_id, role FROM team_user")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(pivot_team, 0);
    assert_eq!(role, "owner");

    let (email, name, hash): (String, String, String) =
        sqlx::query_as("SELECT email, name, password_hash FROM users")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(email, "admin@example.com");
    assert_eq!(name, "Admin");

    // Password is stored as a verifiable argon2id PHC hash, not plaintext.
    assert!(
        hash.starts_with("$argon2id$"),
        "argon2id hash expected: {hash}"
    );
    assert!(users::verify_password(
        "correct horse battery staple",
        &hash
    ));
    assert!(!users::verify_password("wrong password", &hash));
}
