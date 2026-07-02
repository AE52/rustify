# E2E fixture keypair

`id_ed25519` / `id_ed25519.pub` is a **test-only** SSH keypair, deliberately
committed to the repository. It exists solely so the E2E harness can log into
the throwaway `testhost` container (see `../testhost.Dockerfile`). It grants no
access to anything real — never reuse it, and never treat a commit of this key
as a secret leak.
