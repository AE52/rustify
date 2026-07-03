-- Database backups (Phase 2 wave-2, track p2-backups). Clean-slate port of
-- Coolify's s3_storages / scheduled_database_backups /
-- scheduled_database_backup_executions tables. S3 access key + secret are stored
-- AES-GCM encrypted (rustify_core::crypto) in *_enc columns, never plaintext.

CREATE TABLE s3_storages (
  id BIGSERIAL PRIMARY KEY,
  uuid TEXT UNIQUE NOT NULL,
  team_id BIGINT NOT NULL REFERENCES teams(id) ON DELETE CASCADE,
  name TEXT NOT NULL,
  region TEXT NOT NULL DEFAULT 'us-east-1',
  endpoint TEXT,
  bucket TEXT NOT NULL,
  key_enc BYTEA NOT NULL,
  secret_enc BYTEA NOT NULL,
  path TEXT NOT NULL DEFAULT '/',
  use_path_style BOOLEAN NOT NULL DEFAULT true,
  is_usable BOOLEAN NOT NULL DEFAULT false,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX s3_storages_team ON s3_storages (team_id);

CREATE TABLE scheduled_database_backups (
  id BIGSERIAL PRIMARY KEY,
  uuid TEXT UNIQUE NOT NULL,
  database_id BIGINT NOT NULL REFERENCES standalone_databases(id) ON DELETE CASCADE,
  enabled BOOLEAN NOT NULL DEFAULT true,
  frequency TEXT NOT NULL,
  save_s3 BOOLEAN NOT NULL DEFAULT false,
  s3_storage_id BIGINT REFERENCES s3_storages(id),
  databases_to_backup TEXT,
  dump_all BOOLEAN NOT NULL DEFAULT false,
  disable_local_backup BOOLEAN NOT NULL DEFAULT false,
  retention_amount_local INT NOT NULL DEFAULT 0,
  retention_days_local INT NOT NULL DEFAULT 0,
  retention_max_gb_local INT NOT NULL DEFAULT 0,
  retention_amount_s3 INT NOT NULL DEFAULT 0,
  retention_days_s3 INT NOT NULL DEFAULT 0,
  retention_max_gb_s3 INT NOT NULL DEFAULT 0,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX scheduled_database_backups_db ON scheduled_database_backups (database_id);
CREATE INDEX scheduled_database_backups_enabled ON scheduled_database_backups (enabled);

CREATE TABLE scheduled_database_backup_executions (
  id BIGSERIAL PRIMARY KEY,
  uuid TEXT UNIQUE NOT NULL,
  scheduled_database_backup_id BIGINT NOT NULL
    REFERENCES scheduled_database_backups(id) ON DELETE CASCADE,
  status TEXT NOT NULL DEFAULT 'running',
  filename TEXT,
  size BIGINT NOT NULL DEFAULT 0,
  s3_uploaded BOOLEAN,
  local_storage_deleted BOOLEAN NOT NULL DEFAULT false,
  s3_storage_deleted BOOLEAN NOT NULL DEFAULT false,
  message TEXT,
  started_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  finished_at TIMESTAMPTZ,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX sdbe_backup ON scheduled_database_backup_executions (scheduled_database_backup_id);
