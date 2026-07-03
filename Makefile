# Rustify — developer tasks.
#
# The `e2e` target stands up the full end-to-end stack (Postgres + a privileged
# docker-in-docker "testhost") and runs the harness in
# crates/rustify-server/tests/e2e.rs behind the `e2e` feature.

E2E_DIR       := tests/e2e
FIXTURES      := $(E2E_DIR)/docker/fixtures
SSH_KEY       := $(abspath $(FIXTURES)/id_ed25519)
export SSH_PUBKEY := $(shell cat $(FIXTURES)/id_ed25519.pub)

SSH_PORT      ?= 2222
COMPOSE       := docker compose -f $(E2E_DIR)/compose.yml

# Server + harness configuration (consumed by the spawned rustify-server and by
# the test itself; see crates/rustify-server/tests/e2e.rs).
export DATABASE_URL          ?= postgres://rustify:rustify@127.0.0.1:5434/rustify
export RUSTIFY_SECRET_KEY    ?= $(shell printf 'rustify-e2e-test-secret-key-3232' | base64)
export RUSTIFY_ADMIN_EMAIL   ?= admin@rustify.test
export RUSTIFY_ADMIN_PASSWORD?= e2e-password-123
export RUSTIFY_COOKIE_SECURE ?= false
export E2E_SSH_HOST          ?= 127.0.0.1
export E2E_SSH_PORT          ?= $(SSH_PORT)
export E2E_SSH_KEY           := $(SSH_KEY)
export E2E_BASE_URL          ?= http://127.0.0.1:8000

SSH_OPTS := -i $(SSH_KEY) -p $(SSH_PORT) \
  -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null \
  -o LogLevel=ERROR -o IdentitiesOnly=yes

# The two sample apps become bare repos on the testhost at
# /srv/git/<name>.git, served over file:// to the deploy engine.
E2E_APPS := nixpacks-node dockerfile-app

.PHONY: e2e e2e-up e2e-web e2e-testhost e2e-seed e2e-run e2e-down smoke

# Full flow; always tears the stack down, then propagates the test exit code.
e2e: e2e-web e2e-testhost e2e-up e2e-seed
	@set +e; $(MAKE) e2e-run; code=$$?; $(MAKE) e2e-down; exit $$code

e2e-web:
	cd web && npm ci && npm run build

e2e-testhost:
	$(COMPOSE) build testhost

e2e-up:
	$(COMPOSE) up -d --wait

# Ship each sample app to the testhost, commit it, and expose it as a bare
# file:// repo under /srv/git.
e2e-seed:
	chmod 600 $(SSH_KEY)
	@for app in $(E2E_APPS); do \
	  echo "seeding $$app"; \
	  ssh $(SSH_OPTS) root@$(E2E_SSH_HOST) "rm -rf /tmp/$$app /srv/git/$$app.git && mkdir -p /tmp/$$app"; \
	  COPYFILE_DISABLE=1 tar -C $(E2E_DIR)/apps/$$app -cf - . | ssh $(SSH_OPTS) root@$(E2E_SSH_HOST) "tar -C /tmp/$$app -xf -"; \
	  ssh $(SSH_OPTS) root@$(E2E_SSH_HOST) "chown -R root:root /tmp/$$app && cd /tmp/$$app && git init -q -b master && git config user.email e2e@rustify.test && git config user.name e2e && git add -A && git commit -qm init && git clone --bare -q /tmp/$$app /srv/git/$$app.git"; \
	done

e2e-run:
	cargo test -p rustify-server --features e2e -- --test-threads 1 --nocapture

e2e-down:
	$(COMPOSE) down -v

smoke:
	bash scripts/e2e-smoke.sh
