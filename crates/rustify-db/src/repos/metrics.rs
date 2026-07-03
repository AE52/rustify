//! Server + container metrics time-series: insert samples and read a windowed
//! `[unix_time, value]` series (the shape Coolify's charts consume — see
//! app/Traits/HasMetrics.php `getMetrics`, which maps each Sentinel row to
//! `[(int) time, (float) value]`).

use chrono::{DateTime, Utc};
use sqlx::PgPool;

use crate::DbResult;

/// One metrics sample to persist. `container_uuid = None` is the host row.
#[derive(Debug, Clone)]
pub struct MetricSample {
    pub server_id: i64,
    pub container_uuid: Option<String>,
    pub cpu_percent: Option<f64>,
    pub mem_percent: Option<f64>,
    pub mem_used_bytes: Option<i64>,
    pub disk_percent: Option<f64>,
}

/// Which numeric column a series query projects. A closed enum so the column
/// name interpolated into SQL is always a compile-time whitelist (never user
/// input).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MetricColumn {
    Cpu,
    MemPercent,
    MemUsedBytes,
    Disk,
}

impl MetricColumn {
    fn column(self) -> &'static str {
        match self {
            MetricColumn::Cpu => "cpu_percent",
            MetricColumn::MemPercent => "mem_percent",
            MetricColumn::MemUsedBytes => "mem_used_bytes",
            MetricColumn::Disk => "disk_percent",
        }
    }
}

/// A single `[unix_seconds, value]` chart point.
pub type MetricPoint = (i64, f64);

#[derive(Clone)]
pub struct MetricsRepo {
    pool: PgPool,
}

impl MetricsRepo {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Insert one sample row.
    pub async fn insert(&self, s: &MetricSample) -> DbResult<()> {
        sqlx::query(
            "INSERT INTO server_metrics
               (server_id, container_uuid, cpu_percent, mem_percent, mem_used_bytes, disk_percent)
             VALUES ($1, $2, $3, $4, $5, $6)",
        )
        .bind(s.server_id)
        .bind(&s.container_uuid)
        .bind(s.cpu_percent)
        .bind(s.mem_percent)
        .bind(s.mem_used_bytes)
        .bind(s.disk_percent)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Windowed host series (`container_uuid IS NULL`) for one column, ordered
    /// oldest-first. NULL values for the column are skipped so a chart never
    /// plots a gap sample.
    pub async fn host_series(
        &self,
        server_id: i64,
        column: MetricColumn,
        from: DateTime<Utc>,
    ) -> DbResult<Vec<MetricPoint>> {
        let sql = format!(
            "SELECT (extract(epoch FROM ts))::bigint AS t, ({col})::double precision AS v
               FROM server_metrics
              WHERE server_id = $1 AND container_uuid IS NULL AND ts >= $2 AND {col} IS NOT NULL
              ORDER BY ts",
            col = column.column()
        );
        let rows: Vec<(i64, f64)> = sqlx::query_as(&sql)
            .bind(server_id)
            .bind(from)
            .fetch_all(&self.pool)
            .await?;
        Ok(rows)
    }

    /// Windowed per-container series for one column, ordered oldest-first.
    /// `container_uuid` (the application/service uuid) is globally unique, so no
    /// server id is required.
    pub async fn container_series(
        &self,
        container_uuid: &str,
        column: MetricColumn,
        from: DateTime<Utc>,
    ) -> DbResult<Vec<MetricPoint>> {
        let sql = format!(
            "SELECT (extract(epoch FROM ts))::bigint AS t, ({col})::double precision AS v
               FROM server_metrics
              WHERE container_uuid = $1 AND ts >= $2 AND {col} IS NOT NULL
              ORDER BY ts",
            col = column.column()
        );
        let rows: Vec<(i64, f64)> = sqlx::query_as(&sql)
            .bind(container_uuid)
            .bind(from)
            .fetch_all(&self.pool)
            .await?;
        Ok(rows)
    }

    /// Timestamp of the newest host sample for a server, used for stale
    /// detection. `None` when no sample has ever been collected.
    pub async fn latest_host_ts(&self, server_id: i64) -> DbResult<Option<DateTime<Utc>>> {
        let ts: Option<DateTime<Utc>> = sqlx::query_scalar(
            "SELECT max(ts) FROM server_metrics WHERE server_id = $1 AND container_uuid IS NULL",
        )
        .bind(server_id)
        .fetch_one(&self.pool)
        .await?;
        Ok(ts)
    }

    /// Daily retention prune: drop every sample older than the owning server's
    /// `metrics_history_days`. Returns the number of rows deleted.
    pub async fn prune_expired(&self) -> DbResult<u64> {
        let deleted = sqlx::query(
            "DELETE FROM server_metrics sm
               USING server_settings ss
              WHERE ss.server_id = sm.server_id
                AND sm.ts < now() - (COALESCE(ss.metrics_history_days, 7) || ' days')::interval",
        )
        .execute(&self.pool)
        .await?
        .rows_affected();
        Ok(deleted)
    }
}
