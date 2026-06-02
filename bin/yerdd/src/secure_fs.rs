//! Filesystem hardening helpers for daemon-owned paths.
//!
//! `yerd-platform`'s `PlatformDirs` contract makes the *caller* responsible
//! for locking down the runtime directory and the secrets it holds —
//! specifically because the Linux fallback when `XDG_RUNTIME_DIR` is unset is
//! the world-traversable `/tmp/yerd-$UID`. The daemon's only access control
//! over the IPC socket is the directory/socket permissions, so these helpers
//! enforce `0o700` on the runtime dir and `0o600` on the socket and on the CA
//! private key.
//!
//! On non-Unix targets the mode operations are no-ops (Windows ACL hardening
//! is a Phase-2 item); the directory is still created.

use std::io;
use std::path::Path;

/// Create `path` (and parents) and, on Unix, force its mode to `0o700`.
///
/// `create_dir_all` is idempotent; the subsequent `set_permissions` tightens
/// the mode whether the directory was just created (umask may have widened it)
/// or already existed. If a different user pre-created the directory, the
/// `chmod` fails with `PermissionDenied` and the daemon refuses to start
/// rather than trusting a directory it cannot lock down — fail-closed.
pub fn create_private_dir(path: &Path) -> io::Result<()> {
    std::fs::create_dir_all(path)?;
    set_mode(path, 0o700)
}

/// On Unix, set `path`'s mode to `0o600` (owner read/write only). No-op
/// elsewhere. Used for the CA private key and the IPC socket.
pub fn restrict_to_owner(path: &Path) -> io::Result<()> {
    set_mode(path, 0o600)
}

/// On Unix, set `path`'s mode to `0o644` (owner read/write, others read-only).
/// No-op elsewhere. Used for the **public** CA certificate: world-readable is
/// fine for a cert, but it must not be group/world-*writable* or the trust
/// helper refuses to install it (a tamper guard). Newly-created files inherit
/// the umask, which on common setups (`umask 002`) leaves `0o664` —
/// group-writable — so we force the mode explicitly.
pub fn restrict_writes_to_owner(path: &Path) -> io::Result<()> {
    set_mode(path, 0o644)
}

#[cfg(unix)]
fn set_mode(path: &Path, mode: u32) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
}

#[cfg(not(unix))]
fn set_mode(_path: &Path, _mode: u32) -> io::Result<()> {
    Ok(())
}

#[cfg(all(test, unix))]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn create_private_dir_is_0700() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("runtime");
        create_private_dir(&dir).unwrap();
        let mode = std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o700);
    }

    #[test]
    fn create_private_dir_tightens_existing_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("runtime");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o777)).unwrap();
        create_private_dir(&dir).unwrap();
        let mode = std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o700);
    }

    #[test]
    fn restrict_to_owner_is_0600() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("ca.key.pem");
        std::fs::write(&file, b"secret").unwrap();
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o644)).unwrap();
        restrict_to_owner(&file).unwrap();
        let mode = std::fs::metadata(&file).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }

    #[test]
    fn restrict_writes_to_owner_strips_group_world_write() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("ca.cert.pem");
        std::fs::write(&file, b"public cert").unwrap();
        // Simulate a umask-002 write: group/world-writable.
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o664)).unwrap();
        restrict_writes_to_owner(&file).unwrap();
        let mode = std::fs::metadata(&file).unwrap().permissions().mode() & 0o777;
        assert_eq!(
            mode, 0o644,
            "cert must be world-readable but owner-write only"
        );
        // The property the trust helper checks: no group/world write bits.
        assert_eq!(mode & 0o022, 0);
    }
}
