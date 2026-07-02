# Throwaway target host for the Rustify E2E harness.
#
# Base: docker:dind (Alpine) — ships a full Docker Engine so deployments can
# build images and run containers *inside* this container (docker-in-docker).
# MUST be run with --privileged for the inner dockerd to start.
#
# Adds an OpenSSH server (root, key-only login) so rustify-server can drive it
# exactly like a real server over SSH, plus git for the file:// repo fixtures.
FROM docker:28-dind

# Public half of the committed test-only fixture keypair
# (tests/e2e/docker/fixtures/id_ed25519.pub), injected at build time.
ARG SSH_PUBKEY

RUN apk add --no-cache openssh-server git curl \
    && ssh-keygen -A \
    && mkdir -p /root/.ssh /srv/git \
    && chmod 700 /root/.ssh \
    && printf '%s\n' "$SSH_PUBKEY" > /root/.ssh/authorized_keys \
    && chmod 600 /root/.ssh/authorized_keys \
    && printf '%s\n' \
        'PermitRootLogin prohibit-password' \
        'PasswordAuthentication no' \
        'PubkeyAuthentication yes' \
        'AuthorizedKeysFile .ssh/authorized_keys' \
        'ClientAliveInterval 30' \
        > /etc/ssh/sshd_config.d/rustify-e2e.conf

# Start the inner Docker daemon (base image entrypoint) in the background,
# then run sshd in the foreground as PID 1's child so the container stays up.
RUN printf '%s\n' \
    '#!/bin/sh' \
    'set -e' \
    'dockerd-entrypoint.sh dockerd >/var/log/dockerd.log 2>&1 &' \
    'for i in $(seq 1 60); do docker info >/dev/null 2>&1 && break; sleep 1; done' \
    'exec /usr/sbin/sshd -D -e' \
    > /usr/local/bin/testhost-entrypoint.sh \
    && chmod +x /usr/local/bin/testhost-entrypoint.sh

EXPOSE 22
ENTRYPOINT ["/usr/local/bin/testhost-entrypoint.sh"]
