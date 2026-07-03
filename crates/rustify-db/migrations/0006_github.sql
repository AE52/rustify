-- GitHub App sources + generic deploy-key clone support for private repos.
-- Ports coolify app/Models/GithubApp.php (columns) and the source/deploy-key
-- columns of app/Models/Application.php.

CREATE TABLE github_apps (
  id BIGSERIAL PRIMARY KEY,
  uuid TEXT UNIQUE NOT NULL,
  team_id BIGINT NOT NULL REFERENCES teams(id),
  private_key_id BIGINT REFERENCES private_keys(id),
  name TEXT NOT NULL,
  organization TEXT,
  api_url TEXT NOT NULL DEFAULT 'https://api.github.com',
  html_url TEXT NOT NULL DEFAULT 'https://github.com',
  custom_user TEXT NOT NULL DEFAULT 'git',
  custom_port INT NOT NULL DEFAULT 22,
  app_id BIGINT,
  installation_id BIGINT,
  client_id TEXT,
  client_secret_enc BYTEA,
  webhook_secret_enc BYTEA,
  is_system_wide BOOLEAN NOT NULL DEFAULT false,
  is_public BOOLEAN NOT NULL DEFAULT false,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX github_apps_team ON github_apps (team_id);

-- Application source wiring: a github_app source OR a raw deploy key.
ALTER TABLE applications
  ADD COLUMN source_type TEXT,
  ADD COLUMN source_id BIGINT REFERENCES github_apps(id),
  ADD COLUMN private_key_id BIGINT REFERENCES private_keys(id),
  ADD COLUMN repository_project_id BIGINT,
  ADD COLUMN manual_webhook_secret_github_enc BYTEA;
