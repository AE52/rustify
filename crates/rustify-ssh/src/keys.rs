//! Private-key materialization on the Rustify host.
//!
//! Behavior ported from Coolify's `SshMultiplexingHelper::validateSshKey`
//! (coolify/app/Helpers/SshMultiplexingHelper.php:293-326): the decrypted key
//! is written to disk only when absent or when the on-disk content diverges
//! from the source of truth (resync), and the file permissions are forced to
//! `0600`.

use rustify_core::exec::ExecError;
use std::fs;
use std::path::{Path, PathBuf};

/// On-disk filename for a private key, keyed by its uuid (parity with
/// Coolify's `ssh_key@{uuid}`).
fn key_filename(uuid: &str) -> String {
    format!("ssh_key@{uuid}")
}

/// Write `decrypted_pem` to `dir/ssh_key@{uuid}` with `0600` permissions,
/// returning the path. Only rewrites when the file is missing or its content
/// diverges from `decrypted_pem`; permissions are (re)asserted every call.
pub fn materialize(uuid: &str, decrypted_pem: &str, dir: &Path) -> Result<PathBuf, ExecError> {
    fs::create_dir_all(dir).map_err(|e| ExecError::Io(e.to_string()))?;
    let path = dir.join(key_filename(uuid));

    let needs_write = match fs::read(&path) {
        Ok(existing) => existing != decrypted_pem.as_bytes(),
        Err(_) => true,
    };
    if needs_write {
        if fs::metadata(&path).is_ok() {
            tracing::warn!(
                key_uuid = uuid,
                "ssh key on disk diverged from source, resyncing"
            );
        }
        fs::write(&path, decrypted_pem).map_err(|e| ExecError::Io(e.to_string()))?;
    }

    set_secure_perms(&path)?;
    Ok(path)
}

#[cfg(unix)]
fn set_secure_perms(path: &Path) -> Result<(), ExecError> {
    use std::os::unix::fs::PermissionsExt;
    let perms = fs::Permissions::from_mode(0o600);
    fs::set_permissions(path, perms).map_err(|e| ExecError::Io(e.to_string()))
}

#[cfg(not(unix))]
fn set_secure_perms(_path: &Path) -> Result<(), ExecError> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const PEM: &str = "-----BEGIN OPENSSH PRIVATE KEY-----\nabc\n-----END-----\n";

    #[cfg(unix)]
    fn mode(path: &Path) -> u32 {
        use std::os::unix::fs::PermissionsExt;
        fs::metadata(path).unwrap().permissions().mode() & 0o777
    }

    #[test]
    fn writes_key_with_0600() {
        let dir = tempfile::tempdir().unwrap();
        let path = materialize("k1", PEM, dir.path()).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), PEM);
        assert_eq!(path.file_name().unwrap().to_str().unwrap(), "ssh_key@k1");
        #[cfg(unix)]
        assert_eq!(mode(&path), 0o600);
    }

    #[test]
    fn resyncs_on_content_divergence() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ssh_key@k1");
        fs::write(&path, "STALE WRONG CONTENT").unwrap();

        let out = materialize("k1", PEM, dir.path()).unwrap();
        assert_eq!(out, path);
        assert_eq!(fs::read_to_string(&path).unwrap(), PEM);
        #[cfg(unix)]
        assert_eq!(mode(&path), 0o600);
    }

    #[cfg(unix)]
    #[test]
    fn reasserts_perms_when_content_matches() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = materialize("k1", PEM, dir.path()).unwrap();
        // Loosen perms behind materialize's back, then re-run.
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        materialize("k1", PEM, dir.path()).unwrap();
        assert_eq!(mode(&path), 0o600);
    }
}
