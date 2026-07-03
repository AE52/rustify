//! MetricsRepo: sample insert + windowed `[unix, value]` series reads and the
//! per-server retention prune.

use chrono::{Duration, Utc};
use sqlx::PgPool;

use rustify_db::repos::{MetricColumn, MetricSample, MetricsRepo};

mod common;
use common::setup;

fn host_sample(server_id: i64, cpu: f64, mem_pct: f64, disk: f64) -> MetricSample {
    MetricSample {
        server_id,
        container_uuid: None,
        cpu_percent: Some(cpu),
        mem_percent: Some(mem_pct),
        mem_used_bytes: Some(1024),
        disk_percent: Some(disk),
    }
}

#[sqlx::test]
async fn insert_and_windowed_host_series_returns_pairs(pool: PgPool) {
    let fx = setup(&pool, 2).await;
    let repo = MetricsRepo::new(pool.clone());

    repo.insert(&host_sample(fx.server_id, 10.0, 40.0, 55.0))
        .await
        .unwrap();
    repo.insert(&host_sample(fx.server_id, 20.0, 45.0, 56.0))
        .await
        .unwrap();

    let from = Utc::now() - Duration::minutes(5);
    let cpu = repo
        .host_series(fx.server_id, MetricColumn::Cpu, from)
        .await
        .unwrap();
    assert_eq!(cpu.len(), 2);
    // Each point is a (unix_seconds, value) pair, oldest first.
    assert_eq!(cpu[0].1, 10.0);
    assert_eq!(cpu[1].1, 20.0);
    assert!(cpu[0].0 > 0 && cpu[1].0 >= cpu[0].0);

    let disk = repo
        .host_series(fx.server_id, MetricColumn::Disk, from)
        .await
        .unwrap();
    assert_eq!(
        disk.iter().map(|p| p.1).collect::<Vec<_>>(),
        vec![55.0, 56.0]
    );
}

#[sqlx::test]
async fn window_excludes_older_samples(pool: PgPool) {
    let fx = setup(&pool, 2).await;
    let repo = MetricsRepo::new(pool.clone());
    repo.insert(&host_sample(fx.server_id, 5.0, 10.0, 10.0))
        .await
        .unwrap();

    // A window that starts in the future excludes the just-inserted sample.
    let future = Utc::now() + Duration::minutes(5);
    let series = repo
        .host_series(fx.server_id, MetricColumn::Cpu, future)
        .await
        .unwrap();
    assert!(series.is_empty());
}

#[sqlx::test]
async fn container_series_reads_by_uuid(pool: PgPool) {
    let fx = setup(&pool, 2).await;
    let repo = MetricsRepo::new(pool.clone());
    repo.insert(&MetricSample {
        server_id: fx.server_id,
        container_uuid: Some("app-xyz".into()),
        cpu_percent: Some(3.5),
        mem_percent: Some(12.0),
        mem_used_bytes: Some(4096),
        disk_percent: None,
    })
    .await
    .unwrap();

    let from = Utc::now() - Duration::minutes(5);
    // Container memory is reported as used bytes.
    let mem = repo
        .container_series("app-xyz", MetricColumn::MemUsedBytes, from)
        .await
        .unwrap();
    assert_eq!(mem.len(), 1);
    assert_eq!(mem[0].1, 4096.0);

    // Host series must not see the container row.
    let host = repo
        .host_series(fx.server_id, MetricColumn::Cpu, from)
        .await
        .unwrap();
    assert!(host.is_empty());
}

#[sqlx::test]
async fn latest_host_ts_tracks_newest(pool: PgPool) {
    let fx = setup(&pool, 2).await;
    let repo = MetricsRepo::new(pool.clone());
    assert!(repo.latest_host_ts(fx.server_id).await.unwrap().is_none());

    repo.insert(&host_sample(fx.server_id, 1.0, 1.0, 1.0))
        .await
        .unwrap();
    let ts = repo.latest_host_ts(fx.server_id).await.unwrap();
    assert!(ts.is_some());
}

#[sqlx::test]
async fn retention_prunes_samples_past_history_window(pool: PgPool) {
    let fx = setup(&pool, 2).await;
    let repo = MetricsRepo::new(pool.clone());

    // history window = 7 days; insert one row backdated 10 days and one recent.
    sqlx::query(
        "INSERT INTO server_metrics (server_id, container_uuid, ts, cpu_percent)
         VALUES ($1, NULL, now() - interval '10 days', 9.0)",
    )
    .bind(fx.server_id)
    .execute(&pool)
    .await
    .unwrap();
    repo.insert(&host_sample(fx.server_id, 1.0, 1.0, 1.0))
        .await
        .unwrap();

    let deleted = repo.prune_expired().await.unwrap();
    assert_eq!(deleted, 1, "only the 10-day-old row is pruned");

    let remaining: i64 =
        sqlx::query_scalar("SELECT count(*) FROM server_metrics WHERE server_id = $1")
            .bind(fx.server_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(remaining, 1);
}
