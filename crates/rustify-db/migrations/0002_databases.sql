-- Standalone databases (Phase 2, track p2-db). Clean-slate simplification of
-- Coolify's eight standalone_* tables (StandalonePostgresql, StandaloneMysql,
-- ...) into a single polymorphic table discriminated by `engine`. Engine
-- credentials are stored AES-GCM encrypted in `credentials_enc`
-- (rustify_core::crypto), never in plaintext columns.
CREATE TABLE standalone_databases (
  id BIGSERIAL PRIMARY KEY,
  uuid TEXT UNIQUE NOT NULL,
  environment_id BIGINT NOT NULL REFERENCES environments(id) ON DELETE CASCADE,
  destination_id BIGINT NOT NULL REFERENCES destinations(id),
  name TEXT NOT NULL,
  description TEXT,
  engine TEXT NOT NULL,
  image TEXT NOT NULL,
  status TEXT NOT NULL DEFAULT 'exited',
  is_public BOOLEAN NOT NULL DEFAULT false,
  public_port INT,
  public_port_timeout INT NOT NULL DEFAULT 3600,
  ports_mappings TEXT,
  credentials_enc BYTEA NOT NULL,
  engine_config JSONB NOT NULL DEFAULT '{}',
  limits_memory TEXT NOT NULL DEFAULT '0',
  limits_cpus TEXT NOT NULL DEFAULT '0',
  health_check_enabled BOOLEAN NOT NULL DEFAULT true,
  health_check_interval INT NOT NULL DEFAULT 15,
  health_check_timeout INT NOT NULL DEFAULT 5,
  health_check_retries INT NOT NULL DEFAULT 5,
  health_check_start_period INT NOT NULL DEFAULT 5,
  restart_count INT NOT NULL DEFAULT 0,
  started_at TIMESTAMPTZ,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX standalone_databases_env ON standalone_databases (environment_id);
