//! ApplicationPreview repo: unique (application_id, pull_request_id) upsert +
//! fqdn/status/comment persistence.

use sqlx::PgPool;

use rustify_db::repos::PreviewRepo;

mod common;
use common::{new_app, setup};

#[sqlx::test(migrations = "../rustify-db/migrations")]
async fn upsert_is_unique_per_application_and_pr(pool: PgPool) {
    common::init_secret_key();
    let fx = setup(&pool, 2).await;
    let app_id = new_app(&pool, &fx).await;
    let repo = PreviewRepo::new(pool.clone());

    let a = repo
        .upsert(app_id, 7, Some("https://gh/pr/7"), Some("github"))
        .await
        .unwrap();
    // Re-upsert the same (app, pr) refreshes rather than duplicating.
    let b = repo
        .upsert(app_id, 7, Some("https://gh/pr/7-updated"), Some("github"))
        .await
        .unwrap();
    assert_eq!(a.id, b.id, "same (app, pr) is the same row");
    assert_eq!(b.pull_request_id, 7);
    assert_eq!(
        b.pull_request_html_url.as_deref(),
        Some("https://gh/pr/7-updated")
    );
    assert_eq!(b.status, "exited", "default status");

    // A different PR is a distinct row.
    let c = repo.upsert(app_id, 9, None, Some("github")).await.unwrap();
    assert_ne!(a.id, c.id);

    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM application_previews WHERE application_id = $1")
            .bind(app_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(count, 2);
}

#[sqlx::test(migrations = "../rustify-db/migrations")]
async fn set_fqdn_status_and_comment(pool: PgPool) {
    common::init_secret_key();
    let fx = setup(&pool, 2).await;
    let app_id = new_app(&pool, &fx).await;
    let repo = PreviewRepo::new(pool.clone());

    let row = repo.upsert(app_id, 3, None, Some("github")).await.unwrap();
    repo.set_fqdn(row.id, Some("https://3.example.com"))
        .await
        .unwrap();
    repo.set_status(row.id, "running").await.unwrap();
    repo.set_comment_id(row.id, Some(555)).await.unwrap();

    let got = repo.get(app_id, 3).await.unwrap().unwrap();
    assert_eq!(got.fqdn.as_deref(), Some("https://3.example.com"));
    assert_eq!(got.status, "running");
    assert!(
        got.last_online_at.is_some(),
        "status change stamps last_online_at"
    );
    assert_eq!(got.pull_request_issue_comment_id, Some(555));

    assert!(repo.delete(row.id).await.unwrap());
    assert!(repo.get(app_id, 3).await.unwrap().is_none());
}
