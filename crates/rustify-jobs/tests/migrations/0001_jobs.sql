-- TEST-ONLY mirror of the `jobs` table from contract C6 (rustify-db migrations/0001_init.sql, Track A2).
-- Duplicated here because rustify-db's migrations were not yet merged when Track D landed.
-- Task Z removes this duplicate in favor of rustify_db::MIGRATOR.
CREATE TABLE jobs (id BIGSERIAL PRIMARY KEY, kind TEXT NOT NULL, payload JSONB NOT NULL,
  run_at TIMESTAMPTZ NOT NULL DEFAULT now(), locked_at TIMESTAMPTZ, locked_by TEXT,
  attempts INT NOT NULL DEFAULT 0, last_error TEXT, created_at TIMESTAMPTZ NOT NULL DEFAULT now());
CREATE INDEX jobs_poll ON jobs (run_at) WHERE locked_at IS NULL;
