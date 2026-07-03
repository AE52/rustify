-- OPS (p4-ops): cloud-provider tokens (Hetzner), Hetzner-provisioned server
-- bookkeeping, the Cloudflare-tunnel + build-server flags, and per-application
-- build-server selection. `proxy_type` is already TEXT so the Caddy option
-- needs no column.

-- Encrypted API tokens for cloud providers (currently Hetzner). `token_enc` is
-- AES-256-GCM encrypted via rustify_core::crypto and is never logged.
CREATE TABLE cloud_provider_tokens (
  id BIGSERIAL PRIMARY KEY,
  uuid TEXT UNIQUE NOT NULL,
  team_id BIGINT NOT NULL REFERENCES teams(id) ON DELETE CASCADE,
  provider TEXT NOT NULL,
  name TEXT,
  token_enc BYTEA NOT NULL,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX cloud_provider_tokens_team_idx ON cloud_provider_tokens (team_id);

-- Hetzner-provisioned server bookkeeping. `ip_previous` remembers the direct IP
-- while a Cloudflare tunnel is active so it can be restored on disable.
ALTER TABLE servers ADD COLUMN hetzner_server_id BIGINT;
ALTER TABLE servers ADD COLUMN hetzner_server_status TEXT;
ALTER TABLE servers ADD COLUMN ip_previous TEXT;
ALTER TABLE servers ADD COLUMN cloud_provider_token_id BIGINT REFERENCES cloud_provider_tokens(id) ON DELETE SET NULL;

-- Build-server + Cloudflare-tunnel flags. `is_build_server` already exists from
-- 0001_init; guard it so this migration stays idempotent against that baseline.
ALTER TABLE server_settings ADD COLUMN IF NOT EXISTS is_build_server BOOLEAN NOT NULL DEFAULT false;
ALTER TABLE server_settings ADD COLUMN IF NOT EXISTS is_cloudflare_tunnel BOOLEAN NOT NULL DEFAULT false;

-- An application may pin its image builds to a dedicated build server; the image
-- is then pushed to a registry and pulled on the deploy server.
ALTER TABLE applications ADD COLUMN build_server_id BIGINT REFERENCES servers(id) ON DELETE SET NULL;
