-- Multi-tenancy: team metadata, the team_user membership pivot and pending
-- invitations. `users.team_id` stays as the user's CURRENT/active team pointer;
-- membership (and roles) now live in `team_user`.

ALTER TABLE teams ADD COLUMN description TEXT;
ALTER TABLE teams ADD COLUMN personal_team BOOLEAN NOT NULL DEFAULT true;
ALTER TABLE teams ADD COLUMN custom_server_limit INT;

-- Membership pivot: which users belong to which teams and with what role.
CREATE TABLE team_user (
  id BIGSERIAL PRIMARY KEY,
  team_id BIGINT NOT NULL REFERENCES teams(id) ON DELETE CASCADE,
  user_id BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  role TEXT NOT NULL DEFAULT 'member',
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  UNIQUE (team_id, user_id)
);

-- Backfill: every existing user becomes an owner of their (single) current team.
INSERT INTO team_user (team_id, user_id, role)
SELECT team_id, id, 'owner' FROM users;

-- Pending invitations to join a team.
CREATE TABLE team_invitations (
  id BIGSERIAL PRIMARY KEY,
  uuid TEXT UNIQUE NOT NULL,
  team_id BIGINT NOT NULL REFERENCES teams(id) ON DELETE CASCADE,
  email TEXT NOT NULL,
  role TEXT NOT NULL DEFAULT 'member',
  link TEXT,
  via TEXT NOT NULL DEFAULT 'link',
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  UNIQUE (team_id, email)
);
