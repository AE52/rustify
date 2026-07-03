//! Server + container metrics chart series (contract C5).
//!
//! Returns the exact shape Coolify's charts consume (app/Traits/HasMetrics.php
//! `getMetrics`): a JSON array of `[unix_time, value]` pairs, oldest-first. CPU
//! is a percentage, server memory is used-percent, container memory is used
//! bytes, disk is a percentage. When the window exceeds 60 minutes and more than
//! 1000 points would be returned, the series is downsampled with LTTB to 1000
//! points (ported from `downsampleLTTB` in bootstrap/helpers/shared.php).

use axum::Json;
use axum::extract::{Path, Query, State};
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use rustify_db::repos::{MetricColumn, MetricPoint, MetricsRepo, ServerRepo};
use rustify_deploy::metrics::is_stale;

use crate::app::AppState;
use crate::auth::CurrentTeam;
use crate::error::{ApiError, ApiResult};
use crate::routes::servers;

/// Default metrics refresh cadence (seconds) when a server has no settings row.
const DEFAULT_REFRESH_SECS: i32 = 10;

/// Default look-back when `from` is omitted.
const DEFAULT_WINDOW_MINUTES: i64 = 60;
/// LTTB target point count (Coolify parity).
const DOWNSAMPLE_TARGET: usize = 1000;

#[derive(Debug, Deserialize)]
pub struct MetricsQuery {
    /// ISO-8601 Zulu start time (e.g. `2026-07-03T10:00:00Z`).
    pub from: Option<String>,
}

/// Parse the `from` window start, defaulting to `now - 60m`.
fn window_start(q: &MetricsQuery) -> ApiResult<DateTime<Utc>> {
    match &q.from {
        Some(s) => DateTime::parse_from_rfc3339(s)
            .map(|dt| dt.with_timezone(&Utc))
            .map_err(|_| ApiError::Validation(format!("invalid `from` timestamp: {s}"))),
        None => Ok(Utc::now() - Duration::minutes(DEFAULT_WINDOW_MINUTES)),
    }
}

/// Downsample only for long windows with many points (Coolify: `mins > 60 &&
/// count > 1000`), then serialise `[unix, value]` pairs.
fn shape(series: Vec<MetricPoint>, from: DateTime<Utc>) -> Json<Vec<MetricPoint>> {
    let window_minutes = (Utc::now() - from).num_minutes();
    if window_minutes > 60 && series.len() > DOWNSAMPLE_TARGET {
        Json(downsample_lttb(&series, DOWNSAMPLE_TARGET))
    } else {
        Json(series)
    }
}

/// Map the `{metric}` path segment to a server (host) metric column.
fn server_column(metric: &str) -> ApiResult<MetricColumn> {
    match metric {
        "cpu" => Ok(MetricColumn::Cpu),
        "memory" => Ok(MetricColumn::MemPercent),
        "disk" => Ok(MetricColumn::Disk),
        other => Err(ApiError::Validation(format!("unknown metric: {other}"))),
    }
}

/// Map the `{metric}` path segment to a container metric column (container
/// memory is reported as used bytes, per Coolify's `used` field).
fn container_column(metric: &str) -> ApiResult<MetricColumn> {
    match metric {
        "cpu" => Ok(MetricColumn::Cpu),
        "memory" => Ok(MetricColumn::MemUsedBytes),
        other => Err(ApiError::Validation(format!("unknown metric: {other}"))),
    }
}

#[utoipa::path(get, path = "/servers/{uuid}/metrics/{metric}", operation_id = "server_metrics",
    tag = "metrics",
    params(
        ("uuid" = String, Path, description = "Server uuid"),
        ("metric" = String, Path, description = "One of: cpu, memory, disk"),
        ("from" = Option<String>, Query, description = "ISO-8601 Zulu window start"),
    ),
    responses((status = 200, description = "Array of [unix_time, value] points", body = [[f64; 2]])))]
pub async fn server_metrics(
    State(state): State<AppState>,
    team: CurrentTeam,
    Path((uuid, metric)): Path<(String, String)>,
    Query(q): Query<MetricsQuery>,
) -> ApiResult<Json<Vec<MetricPoint>>> {
    let server = servers::owned(&state, &team, &uuid).await?;
    let column = server_column(&metric)?;
    let from = window_start(&q)?;
    let series = MetricsRepo::new(state.pool.clone())
        .host_series(server.id, column, from)
        .await?;
    Ok(shape(series, from))
}

/// Freshness of a server's metrics collection: the newest host sample time and
/// whether it is stale per [`rustify_deploy::metrics::is_stale`].
#[derive(Debug, Serialize, ToSchema)]
pub struct MetricsStatus {
    /// ISO-8601 Zulu timestamp of the newest host sample, or `null` if none.
    pub last_seen: Option<String>,
    /// True when no sample is newer than the staleness threshold.
    pub stale: bool,
}

#[utoipa::path(get, path = "/servers/{uuid}/metrics/status", operation_id = "server_metrics_status",
    tag = "metrics",
    params(("uuid" = String, Path, description = "Server uuid")),
    responses((status = 200, description = "Metrics freshness", body = MetricsStatus)))]
pub async fn server_metrics_status(
    State(state): State<AppState>,
    team: CurrentTeam,
    Path(uuid): Path<String>,
) -> ApiResult<Json<MetricsStatus>> {
    let server = servers::owned(&state, &team, &uuid).await?;
    let last = MetricsRepo::new(state.pool.clone())
        .latest_host_ts(server.id)
        .await?;
    let refresh = ServerRepo::new(state.pool.clone())
        .settings(server.id)
        .await?
        .map(|s| s.metrics_refresh_rate_seconds)
        .unwrap_or(DEFAULT_REFRESH_SECS);
    Ok(Json(MetricsStatus {
        last_seen: last.map(|ts| ts.to_rfc3339()),
        stale: is_stale(last, refresh, Utc::now()),
    }))
}

#[utoipa::path(get, path = "/containers/{uuid}/metrics/{metric}", operation_id = "container_metrics",
    tag = "metrics",
    params(
        ("uuid" = String, Path, description = "Application/service (container) uuid"),
        ("metric" = String, Path, description = "One of: cpu, memory"),
        ("from" = Option<String>, Query, description = "ISO-8601 Zulu window start"),
    ),
    responses((status = 200, description = "Array of [unix_time, value] points", body = [[f64; 2]])))]
pub async fn container_metrics(
    State(state): State<AppState>,
    team: CurrentTeam,
    Path((uuid, metric)): Path<(String, String)>,
    Query(q): Query<MetricsQuery>,
) -> ApiResult<Json<Vec<MetricPoint>>> {
    ensure_container_owned(&state, &team, &uuid).await?;
    let column = container_column(&metric)?;
    let from = window_start(&q)?;
    let series = MetricsRepo::new(state.pool.clone())
        .container_series(&uuid, column, from)
        .await?;
    Ok(shape(series, from))
}

/// Enforce team ownership of a container uuid, which is an application or
/// service uuid reached via its environment → project → team chain.
async fn ensure_container_owned(state: &AppState, team: &CurrentTeam, uuid: &str) -> ApiResult<()> {
    let owned: bool = sqlx::query_scalar(
        "SELECT EXISTS (
             SELECT 1 FROM applications a
               JOIN environments e ON e.id = a.environment_id
               JOIN projects p ON p.id = e.project_id
              WHERE a.uuid = $1 AND p.team_id = $2
             UNION ALL
             SELECT 1 FROM services s
               JOIN environments e ON e.id = s.environment_id
               JOIN projects p ON p.id = e.project_id
              WHERE s.uuid = $1 AND p.team_id = $2
         )",
    )
    .bind(uuid)
    .bind(team.id)
    .fetch_one(&state.pool)
    .await
    .map_err(|e| ApiError::Internal(e.to_string()))?;
    if owned {
        Ok(())
    } else {
        Err(ApiError::NotFound)
    }
}

/// Largest-Triangle-Three-Buckets downsampling — a direct port of Coolify's
/// `downsampleLTTB` (bootstrap/helpers/shared.php:4062). Always keeps the first
/// and last points; returns the input unchanged when `threshold >= len` or
/// `threshold <= 2`.
// Index arithmetic mirrors the PHP source (floored bucket bounds referencing
// `data[a]`, `data[j]`, and the next bucket), so a range loop is clearest here.
#[allow(clippy::needless_range_loop)]
pub fn downsample_lttb(data: &[MetricPoint], threshold: usize) -> Vec<MetricPoint> {
    let n = data.len();
    if threshold >= n || threshold <= 2 {
        return data.to_vec();
    }

    let mut sampled: Vec<MetricPoint> = Vec::with_capacity(threshold);
    sampled.push(data[0]); // always keep first

    let bucket_size = (n as f64 - 2.0) / (threshold as f64 - 2.0);
    let mut a = 0usize; // index of previously selected point

    for i in 0..(threshold - 2) {
        let bucket_start = ((i as f64 + 1.0) * bucket_size).floor() as usize + 1;
        let bucket_end = (((i as f64 + 2.0) * bucket_size).floor() as usize + 1).min(n - 1);

        // Average point of the *next* bucket, used as the triangle's third vertex.
        let next_start = ((i as f64 + 2.0) * bucket_size).floor() as usize + 1;
        let next_end = (((i as f64 + 3.0) * bucket_size).floor() as usize + 1).min(n - 1);

        let (mut avg_x, mut avg_y) = (0.0f64, 0.0f64);
        let next_count = next_end as i64 - next_start as i64 + 1;
        if next_count > 0 {
            for j in next_start..=next_end {
                avg_x += data[j].0 as f64;
                avg_y += data[j].1;
            }
            avg_x /= next_count as f64;
            avg_y /= next_count as f64;
        }

        let point_a_x = data[a].0 as f64;
        let point_a_y = data[a].1;

        let mut max_area = -1.0f64;
        let mut max_area_index = bucket_start;
        for j in bucket_start..=bucket_end {
            let area = ((point_a_x - avg_x) * (data[j].1 - point_a_y)
                - (point_a_x - data[j].0 as f64) * (avg_y - point_a_y))
                .abs()
                * 0.5;
            if area > max_area {
                max_area = area;
                max_area_index = j;
            }
        }

        sampled.push(data[max_area_index]);
        a = max_area_index;
    }

    sampled.push(data[n - 1]); // always keep last
    sampled
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lttb_returns_input_when_below_threshold() {
        let data = vec![(1000, 10.5), (2000, 20.3), (3000, 15.7)];
        assert_eq!(downsample_lttb(&data, 1000), data);
    }

    #[test]
    fn lttb_returns_input_when_threshold_two_or_less() {
        let data = vec![(1, 1.0), (2, 2.0), (3, 3.0), (4, 4.0), (5, 5.0)];
        assert_eq!(downsample_lttb(&data, 2), data);
        assert_eq!(downsample_lttb(&data, 1), data);
    }

    #[test]
    fn lttb_hits_target_count() {
        let data: Vec<MetricPoint> = (0..100)
            .map(|i| (i as i64 * 1000, (i % 10) as f64))
            .collect();
        assert_eq!(downsample_lttb(&data, 10).len(), 10);
    }

    #[test]
    fn lttb_preserves_endpoints() {
        let data: Vec<MetricPoint> = (0..100)
            .map(|i| (i as i64 * 1000, i as f64 * 1.5))
            .collect();
        let out = downsample_lttb(&data, 20);
        assert_eq!(out.first(), data.first());
        assert_eq!(out.last(), data.last());
    }

    #[test]
    fn lttb_keeps_chronological_order() {
        let data: Vec<MetricPoint> = (0..500)
            .map(|i| (i as i64 * 60000, (i as f64 / 10.0).sin() * 50.0 + 50.0))
            .collect();
        let out = downsample_lttb(&data, 50);
        let mut prev = i64::MIN;
        for (t, _) in &out {
            assert!(*t >= prev);
            prev = *t;
        }
    }

    #[test]
    fn lttb_preserves_peak_and_valley() {
        let data: Vec<MetricPoint> = (0..100)
            .map(|i| {
                let v = if i == 25 {
                    100.0
                } else if i == 75 {
                    0.0
                } else {
                    50.0
                };
                (i as i64 * 1000, v)
            })
            .collect();
        let out = downsample_lttb(&data, 20);
        let values: Vec<f64> = out.iter().map(|(_, v)| *v).collect();
        assert!(values.contains(&100.0));
        assert!(values.contains(&0.0));
    }

    #[test]
    fn metric_columns_map_correctly() {
        assert_eq!(server_column("cpu").unwrap(), MetricColumn::Cpu);
        assert_eq!(server_column("memory").unwrap(), MetricColumn::MemPercent);
        assert_eq!(server_column("disk").unwrap(), MetricColumn::Disk);
        assert!(server_column("bogus").is_err());
        assert_eq!(
            container_column("memory").unwrap(),
            MetricColumn::MemUsedBytes
        );
        assert!(container_column("disk").is_err());
    }
}
