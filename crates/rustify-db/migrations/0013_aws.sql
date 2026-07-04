-- AWS (aws-provision): EC2-provisioned server bookkeeping + Docker Swarm role
-- flags. Reuses `cloud_provider_tokens` (provider `aws`, encrypted JSON
-- {access_key_id, secret_access_key}) and `servers.cloud_provider_token_id`
-- from 0012_ops; nothing in earlier migrations is edited.

-- EC2-provisioned server bookkeeping. `aws_instance_id` is the `i-…` id and
-- `aws_region` the region the instance lives in (needed to rebuild a
-- region-scoped client for the periodic status sync).
ALTER TABLE servers ADD COLUMN aws_instance_id TEXT;
ALTER TABLE servers ADD COLUMN aws_region TEXT;

-- Docker Swarm role flags. A multi-node AWS provision forms a swarm: the first
-- node becomes the manager (`is_swarm_manager`) and the rest join as workers
-- (`is_swarm_worker`). Guarded so this migration stays idempotent even if a
-- future baseline already defines them.
ALTER TABLE server_settings ADD COLUMN IF NOT EXISTS is_swarm_manager BOOLEAN NOT NULL DEFAULT false;
ALTER TABLE server_settings ADD COLUMN IF NOT EXISTS is_swarm_worker BOOLEAN NOT NULL DEFAULT false;
