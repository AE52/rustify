-- Server + container metrics via periodic SSH pull (Rustify skips Coolify's
-- Sentinel agent; app/Traits/HasMetrics.php reads a Sentinel HTTP API, we pull
-- over SSH and persist the same [time, value] samples here instead).
--
-- One row per sample. `container_uuid IS NULL` is the host (server) row; a
-- non-null `container_uuid` is a per-container sample keyed by the resource's
-- application/service uuid (the `rustify.applicationUuid` label).

CREATE TABLE server_metrics (
  id BIGSERIAL PRIMARY KEY,
  server_id BIGINT NOT NULL REFERENCES servers(id) ON DELETE CASCADE,
  container_uuid TEXT,
  ts TIMESTAMPTZ NOT NULL DEFAULT now(),
  cpu_percent REAL,
  mem_percent REAL,
  mem_used_bytes BIGINT,
  disk_percent REAL
);

-- Windowed reads always filter by (server, container, time-range); this index
-- serves both the host series (container_uuid IS NULL) and per-container series.
CREATE INDEX server_metrics_lookup ON server_metrics (server_id, container_uuid, ts);

-- Per-server metrics collection knobs (Coolify: server_settings.is_metrics_enabled
-- + sentinel refresh rate). refresh_rate drives the collector cadence/staleness;
-- history_days drives the daily retention prune.
ALTER TABLE server_settings ADD COLUMN metrics_enabled BOOL DEFAULT true;
ALTER TABLE server_settings ADD COLUMN metrics_refresh_rate_seconds INT DEFAULT 10;
ALTER TABLE server_settings ADD COLUMN metrics_history_days INT DEFAULT 7;
