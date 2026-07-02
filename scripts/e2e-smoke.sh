#!/usr/bin/env bash
# E2E infrastructure smoke test.
#
# Proves the harness plumbing works WITHOUT the full deploy flow (which needs
# Task Z to wire the deploy engine into the server binary):
#   1. builds the testhost image,
#   2. boots it --privileged,
#   3. SSHes in with the committed fixture key and runs `docker info`
#      (verifies docker-in-docker is live),
#   4. clones a bare repo fixture into /srv/git and clones it back out
#      (verifies the file:// git-serving path).
#
# Prints "SMOKE OK" and exits 0 on success.
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
E2E_DIR="$HERE/tests/e2e"
FIXTURES="$E2E_DIR/docker/fixtures"
KEY="$FIXTURES/id_ed25519"

IMAGE="rustify-e2e-testhost:smoke"
CONTAINER="rustify-e2e-smoke"
SSH_PORT="${SSH_PORT:-2222}"

SSH_OPTS=(-i "$KEY" -p "$SSH_PORT"
  -o StrictHostKeyChecking=no
  -o UserKnownHostsFile=/dev/null
  -o LogLevel=ERROR
  -o IdentitiesOnly=yes)

cleanup() {
  docker rm -f "$CONTAINER" >/dev/null 2>&1 || true
}
trap cleanup EXIT

echo "[smoke] fixture key must be 0600 for ssh"
chmod 600 "$KEY"

echo "[smoke] building testhost image"
docker build \
  --build-arg SSH_PUBKEY="$(cat "$FIXTURES/id_ed25519.pub")" \
  -f "$E2E_DIR/docker/testhost.Dockerfile" \
  -t "$IMAGE" \
  "$E2E_DIR"

cleanup
echo "[smoke] booting testhost (--privileged)"
docker run -d --privileged --name "$CONTAINER" -p "$SSH_PORT:22" "$IMAGE" >/dev/null

echo "[smoke] waiting for sshd + docker-in-docker"
ok=""
for _ in $(seq 1 60); do
  if ssh "${SSH_OPTS[@]}" root@127.0.0.1 'docker info >/dev/null 2>&1'; then
    ok=1
    break
  fi
  sleep 2
done
if [ -z "$ok" ]; then
  echo "[smoke] FAILED: testhost never became reachable with docker running" >&2
  docker logs "$CONTAINER" 2>&1 | tail -30 >&2 || true
  exit 1
fi

echo "[smoke] docker version inside testhost:"
ssh "${SSH_OPTS[@]}" root@127.0.0.1 'docker info --format "{{.ServerVersion}}"'

echo "[smoke] seeding + cloning a bare git repo over the fixture key"
ssh "${SSH_OPTS[@]}" root@127.0.0.1 'set -e
  rm -rf /tmp/src /srv/git/smoke.git /tmp/out
  mkdir -p /tmp/src
  cd /tmp/src
  git init -q
  git config user.email e2e@rustify.test
  git config user.name e2e
  echo hello > file.txt
  git add -A
  git commit -qm init
  git clone --bare -q /tmp/src /srv/git/smoke.git
  git clone -q file:///srv/git/smoke.git /tmp/out
  grep -q hello /tmp/out/file.txt'

echo "SMOKE OK"
