CREATE TYPE deployment_status AS ENUM ('queued','in_progress','finished','failed','cancelled');

CREATE TABLE teams (id BIGSERIAL PRIMARY KEY, uuid TEXT UNIQUE NOT NULL, name TEXT NOT NULL,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(), updated_at TIMESTAMPTZ NOT NULL DEFAULT now());
CREATE TABLE users (id BIGSERIAL PRIMARY KEY, uuid TEXT UNIQUE NOT NULL, team_id BIGINT NOT NULL REFERENCES teams(id),
  email TEXT UNIQUE NOT NULL, name TEXT NOT NULL, password_hash TEXT NOT NULL,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(), updated_at TIMESTAMPTZ NOT NULL DEFAULT now());
CREATE TABLE sessions (id TEXT PRIMARY KEY, user_id BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  expires_at TIMESTAMPTZ NOT NULL, created_at TIMESTAMPTZ NOT NULL DEFAULT now());
CREATE TABLE api_tokens (id BIGSERIAL PRIMARY KEY, uuid TEXT UNIQUE NOT NULL, team_id BIGINT NOT NULL REFERENCES teams(id),
  name TEXT NOT NULL, token_hash TEXT UNIQUE NOT NULL, abilities TEXT[] NOT NULL DEFAULT '{read,write,deploy}',
  last_used_at TIMESTAMPTZ, created_at TIMESTAMPTZ NOT NULL DEFAULT now());
CREATE TABLE private_keys (id BIGSERIAL PRIMARY KEY, uuid TEXT UNIQUE NOT NULL, team_id BIGINT NOT NULL REFERENCES teams(id),
  name TEXT NOT NULL, private_key_enc BYTEA NOT NULL, public_key TEXT NOT NULL,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(), updated_at TIMESTAMPTZ NOT NULL DEFAULT now());
CREATE TABLE servers (id BIGSERIAL PRIMARY KEY, uuid TEXT UNIQUE NOT NULL, team_id BIGINT NOT NULL REFERENCES teams(id),
  name TEXT NOT NULL, ip TEXT NOT NULL, port INT NOT NULL DEFAULT 22, ssh_user TEXT NOT NULL DEFAULT 'root',
  private_key_id BIGINT NOT NULL REFERENCES private_keys(id),
  reachable BOOLEAN NOT NULL DEFAULT false, usable BOOLEAN NOT NULL DEFAULT false,
  validation_logs TEXT, created_at TIMESTAMPTZ NOT NULL DEFAULT now(), updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  UNIQUE (ip, port, ssh_user));
CREATE TABLE server_settings (id BIGSERIAL PRIMARY KEY, server_id BIGINT UNIQUE NOT NULL REFERENCES servers(id) ON DELETE CASCADE,
  concurrent_builds INT NOT NULL DEFAULT 2, deployment_queue_limit INT NOT NULL DEFAULT 25,
  dynamic_timeout INT NOT NULL DEFAULT 3600, connection_timeout INT NOT NULL DEFAULT 10,
  proxy_type TEXT NOT NULL DEFAULT 'traefik', proxy_status TEXT NOT NULL DEFAULT 'exited',
  proxy_custom_config TEXT, is_build_server BOOLEAN NOT NULL DEFAULT false,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(), updated_at TIMESTAMPTZ NOT NULL DEFAULT now());
CREATE TABLE destinations (id BIGSERIAL PRIMARY KEY, uuid TEXT UNIQUE NOT NULL,
  server_id BIGINT NOT NULL REFERENCES servers(id) ON DELETE CASCADE, network TEXT NOT NULL,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(), UNIQUE (server_id, network));
CREATE TABLE projects (id BIGSERIAL PRIMARY KEY, uuid TEXT UNIQUE NOT NULL, team_id BIGINT NOT NULL REFERENCES teams(id),
  name TEXT NOT NULL, description TEXT, created_at TIMESTAMPTZ NOT NULL DEFAULT now(), updated_at TIMESTAMPTZ NOT NULL DEFAULT now());
CREATE TABLE environments (id BIGSERIAL PRIMARY KEY, uuid TEXT UNIQUE NOT NULL,
  project_id BIGINT NOT NULL REFERENCES projects(id) ON DELETE CASCADE, name TEXT NOT NULL,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(), UNIQUE (project_id, name));
CREATE TABLE applications (id BIGSERIAL PRIMARY KEY, uuid TEXT UNIQUE NOT NULL,
  environment_id BIGINT NOT NULL REFERENCES environments(id) ON DELETE CASCADE,
  destination_id BIGINT NOT NULL REFERENCES destinations(id),
  name TEXT NOT NULL, fqdn TEXT, git_repository TEXT NOT NULL, git_branch TEXT NOT NULL DEFAULT 'main',
  git_commit_sha TEXT NOT NULL DEFAULT 'HEAD', build_pack TEXT NOT NULL DEFAULT 'nixpacks',
  static_image TEXT NOT NULL DEFAULT 'nginx:alpine', docker_registry_image_name TEXT, docker_registry_image_tag TEXT,
  dockerfile_location TEXT NOT NULL DEFAULT '/Dockerfile', docker_compose_location TEXT NOT NULL DEFAULT '/docker-compose.yaml',
  base_directory TEXT NOT NULL DEFAULT '/', publish_directory TEXT,
  install_command TEXT, build_command TEXT, start_command TEXT,
  ports_exposes TEXT NOT NULL DEFAULT '80', ports_mappings TEXT,
  health_check_enabled BOOLEAN NOT NULL DEFAULT false, health_check_path TEXT NOT NULL DEFAULT '/',
  health_check_port TEXT, health_check_host TEXT NOT NULL DEFAULT 'localhost',
  health_check_method TEXT NOT NULL DEFAULT 'GET', health_check_return_code INT NOT NULL DEFAULT 200,
  health_check_interval INT NOT NULL DEFAULT 5, health_check_timeout INT NOT NULL DEFAULT 5,
  health_check_retries INT NOT NULL DEFAULT 10, health_check_start_period INT NOT NULL DEFAULT 5,
  limits_memory TEXT NOT NULL DEFAULT '0', limits_cpus TEXT NOT NULL DEFAULT '0',
  custom_docker_run_options TEXT, status TEXT NOT NULL DEFAULT 'exited',
  restart_count INT NOT NULL DEFAULT 0, max_restart_count INT NOT NULL DEFAULT 10,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(), updated_at TIMESTAMPTZ NOT NULL DEFAULT now());
CREATE TABLE environment_variables (id BIGSERIAL PRIMARY KEY, uuid TEXT UNIQUE NOT NULL,
  resource_kind TEXT NOT NULL, resource_id BIGINT NOT NULL, key TEXT NOT NULL, value_enc BYTEA NOT NULL,
  is_buildtime BOOLEAN NOT NULL DEFAULT false, is_literal BOOLEAN NOT NULL DEFAULT false,
  is_shown_once BOOLEAN NOT NULL DEFAULT false,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(), updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  UNIQUE (resource_kind, resource_id, key));
CREATE TABLE persistent_storages (id BIGSERIAL PRIMARY KEY, uuid TEXT UNIQUE NOT NULL,
  resource_kind TEXT NOT NULL, resource_id BIGINT NOT NULL, name TEXT NOT NULL,
  mount_path TEXT NOT NULL, host_path TEXT, created_at TIMESTAMPTZ NOT NULL DEFAULT now());
CREATE TABLE deployments (id BIGSERIAL PRIMARY KEY, uuid TEXT UNIQUE NOT NULL,
  application_id BIGINT NOT NULL REFERENCES applications(id) ON DELETE CASCADE,
  server_id BIGINT NOT NULL REFERENCES servers(id),
  status deployment_status NOT NULL DEFAULT 'queued',
  commit_sha TEXT, commit_message TEXT, force_rebuild BOOLEAN NOT NULL DEFAULT false,
  rollback BOOLEAN NOT NULL DEFAULT false, config_snapshot JSONB,
  started_at TIMESTAMPTZ, finished_at TIMESTAMPTZ, created_at TIMESTAMPTZ NOT NULL DEFAULT now());
CREATE INDEX deployments_app_status ON deployments (application_id, status);
CREATE INDEX deployments_server_status ON deployments (server_id, status);
CREATE TABLE deployment_logs (id BIGSERIAL PRIMARY KEY,
  deployment_id BIGINT NOT NULL REFERENCES deployments(id) ON DELETE CASCADE,
  ord BIGINT NOT NULL, kind TEXT NOT NULL, content TEXT NOT NULL,
  hidden BOOLEAN NOT NULL DEFAULT false, batch INT NOT NULL DEFAULT 1,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(), UNIQUE (deployment_id, ord));
CREATE TABLE instance_settings (id BIGSERIAL PRIMARY KEY,
  fqdn TEXT, wildcard_domain TEXT, registration_enabled BOOLEAN NOT NULL DEFAULT false,
  updated_at TIMESTAMPTZ NOT NULL DEFAULT now());
CREATE TABLE jobs (id BIGSERIAL PRIMARY KEY, kind TEXT NOT NULL, payload JSONB NOT NULL,
  run_at TIMESTAMPTZ NOT NULL DEFAULT now(), locked_at TIMESTAMPTZ, locked_by TEXT,
  attempts INT NOT NULL DEFAULT 0, last_error TEXT, created_at TIMESTAMPTZ NOT NULL DEFAULT now());
CREATE INDEX jobs_poll ON jobs (run_at) WHERE locked_at IS NULL;
