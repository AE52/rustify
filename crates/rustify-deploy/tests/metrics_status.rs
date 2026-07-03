//! The metrics-status query path: `latest_host_ts` (the newest host sample the
//! `GET /servers/{uuid}/metrics/status` route reads) folded through
//! [`rustify_deploy::metrics::is_stale`] to a freshness verdict.

use chrono::{Duration, Utc};
use sqlx::PgPool;

use rustify_db::repos::{MetricSample, MetricsRepo};
use rustify_deploy::metrics::is_stale;

mod common;

use common::setup;

fn host_sample(server_id: i64) -> MetricSample {
    MetricSample {
        server_id,
        container_uuid: None,
        cpu_percent: Some(12.5),
        mem_percent: Some(40.0),
        mem_used_bytes: Some(1024),
        disk_percent: Some(55.0),
    }
}

#[sqlx::test(migrations = "../rustify-db/migrations")]
async fn status_query_reports_fresh_after_insert_and_stale_when_empty(pool: PgPool) {
    common::init_secret_key();
    let fx = setup(&pool, 2).await;
    let repo = MetricsRepo::new(pool.clone());
    // The server-settings row created by `setup` carries the refresh rate the
    // status route feeds into `is_stale` (default 10s → 120s floor).
    let refresh = 10;

    // No sample yet: the status query returns None and the verdict is stale.
    let last = repo.latest_host_ts(fx.server_id).await.unwrap();
    assert!(last.is_none(), "no host sample collected yet");
    assert!(
        is_stale(last, refresh, Utc::now()),
        "never-collected is stale"
    );

    // After a fresh insert the newest ts is within the window → not stale.
    repo.insert(&host_sample(fx.server_id)).await.unwrap();
    let last = repo.latest_host_ts(fx.server_id).await.unwrap();
    assert!(
        last.is_some(),
        "status query returns the newest host sample"
    );
    assert!(
        !is_stale(last, refresh, Utc::now()),
        "a just-collected sample must be fresh"
    );

    // A sample far in the past is stale (older than the 120s floor).
    let old = Utc::now() - Duration::seconds(3600);
    assert!(
        is_stale(Some(old), refresh, Utc::now()),
        "an hour-old sample is stale"
    );
}
