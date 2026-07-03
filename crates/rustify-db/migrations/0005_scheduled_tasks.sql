-- User scheduled tasks (cron-driven `docker exec` into an app/service container)
-- and their execution history. Behavioural port of Coolify's `scheduled_tasks`
-- and `scheduled_task_executions` tables (app/Models/ScheduledTask.php,
-- ScheduledTaskExecution.php). A task targets exactly one resource: an
-- application OR a service (enforced by the CHECK constraint).
CREATE TABLE scheduled_tasks (id BIGSERIAL PRIMARY KEY, uuid TEXT UNIQUE NOT NULL,
  enabled BOOLEAN NOT NULL DEFAULT true,
  name TEXT NOT NULL, command TEXT NOT NULL, frequency TEXT NOT NULL,
  container TEXT, timeout INT NOT NULL DEFAULT 300,
  team_id BIGINT REFERENCES teams(id) ON DELETE CASCADE,
  application_id BIGINT REFERENCES applications(id) ON DELETE CASCADE,
  service_id BIGINT REFERENCES services(id) ON DELETE CASCADE,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(), updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  CHECK (application_id IS NOT NULL OR service_id IS NOT NULL));
CREATE INDEX scheduled_tasks_application ON scheduled_tasks (application_id);
CREATE INDEX scheduled_tasks_service ON scheduled_tasks (service_id);
CREATE INDEX scheduled_tasks_enabled ON scheduled_tasks (enabled);

CREATE TABLE scheduled_task_executions (id BIGSERIAL PRIMARY KEY, uuid TEXT UNIQUE NOT NULL,
  scheduled_task_id BIGINT NOT NULL REFERENCES scheduled_tasks(id) ON DELETE CASCADE,
  status TEXT NOT NULL DEFAULT 'running', message TEXT, error_details TEXT,
  started_at TIMESTAMPTZ NOT NULL DEFAULT now(), finished_at TIMESTAMPTZ, duration INT);
CREATE INDEX scheduled_task_executions_task ON scheduled_task_executions (scheduled_task_id, started_at DESC);
