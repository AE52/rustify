-- One-click services: templated multi-container compose stacks.
-- Service env vars reuse `environment_variables` with resource_kind = 'service'.
CREATE TABLE services (id BIGSERIAL PRIMARY KEY, uuid TEXT UNIQUE NOT NULL,
  environment_id BIGINT NOT NULL REFERENCES environments(id) ON DELETE CASCADE,
  destination_id BIGINT NOT NULL REFERENCES destinations(id),
  name TEXT NOT NULL, template_key TEXT NOT NULL,
  compose_raw TEXT NOT NULL, compose_mutated TEXT,
  status TEXT NOT NULL DEFAULT 'exited', config_hash TEXT,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(), updated_at TIMESTAMPTZ NOT NULL DEFAULT now());
CREATE INDEX services_environment ON services (environment_id);

CREATE TABLE service_applications (id BIGSERIAL PRIMARY KEY, uuid TEXT UNIQUE NOT NULL,
  service_id BIGINT NOT NULL REFERENCES services(id) ON DELETE CASCADE,
  name TEXT NOT NULL, fqdn TEXT, image TEXT,
  status TEXT NOT NULL DEFAULT 'exited', is_database BOOLEAN NOT NULL DEFAULT false,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(), UNIQUE (service_id, name));
CREATE INDEX service_applications_service ON service_applications (service_id);
