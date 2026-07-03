-- Web terminal: per-server gate for the interactive PTY route.
-- Parity with Coolify's `server_settings.is_terminal_enabled`
-- (Terminal.php: `$server->isTerminalEnabled()`).
ALTER TABLE server_settings
    ADD COLUMN is_terminal_enabled BOOLEAN NOT NULL DEFAULT true;
