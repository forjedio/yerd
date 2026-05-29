//! Exclusive file lock that prevents two `yerdd` processes from running
//! concurrently for the same user.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::error::DaemonError;

/// Holds the lock for the lifetime of the daemon process. Dropping the
/// struct closes the file and releases the OS-level advisory lock.
pub struct InstanceLock {
    _file: File,
    path: PathBuf,
}

impl InstanceLock {
    /// Acquire `dirs.runtime/yerd.lock` exclusively.
    ///
    /// On Linux/macOS this is an `flock`-style advisory lock; on Windows
    /// it's `LockFileEx`. Returns [`DaemonError::AlreadyRunning`] when
    /// another process holds the lock.
    pub fn acquire(dirs: &yerd_platform::PlatformDirs) -> Result<Self, DaemonError> {
        use fs4::fs_std::FileExt;

        // Harden the runtime dir to 0o700 before placing the lock or socket
        // in it: the IPC socket's only access control is directory/socket
        // permissions, and the XDG-less fallback is a world-traversable
        // `/tmp/yerd-$UID`. See `crate::secure_fs`.
        crate::secure_fs::create_private_dir(&dirs.runtime).map_err(|source| DaemonError::Io {
            path: dirs.runtime.clone(),
            source,
        })?;
        let path = dirs.runtime.join("yerd.lock");
        let mut file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(true)
            .open(&path)
            .map_err(|source| DaemonError::Io {
                path: path.clone(),
                source,
            })?;
        // `fs4` 0.13's `try_lock_exclusive` returns `io::Result<bool>`:
        // `Ok(false)` means another process holds the lock. Treating that
        // as success would silently allow a second daemon to start.
        match file.try_lock_exclusive() {
            Ok(true) => {}
            Ok(false) => {
                return Err(DaemonError::AlreadyRunning { path });
            }
            Err(source) => {
                return Err(DaemonError::Io { path, source });
            }
        }
        let _ = file.write_all(std::process::id().to_string().as_bytes());
        Ok(Self { _file: file, path })
    }

    /// Path the lock was acquired on (for log output).
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod tests {
    use super::*;

    // Note on cross-process semantics:
    // `fs4` uses `flock(2)` on Linux, where a single process holds locks
    // per open-file-description — a second `acquire()` in the *same
    // process* against a different file descriptor would succeed. Truly
    // cross-process validation requires spawning a subprocess, which is
    // covered by `tests/lifecycle.rs` (out-of-process boot sanity).
    //
    // The unit test here only validates the success path: acquire works
    // on a fresh directory, and the returned path matches expectations.

    #[test]
    fn acquire_succeeds_on_fresh_runtime_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let dirs = yerd_platform::PlatformDirs {
            config: tmp.path().join("c"),
            data: tmp.path().join("d"),
            state: tmp.path().join("s"),
            cache: tmp.path().join("ca"),
            runtime: tmp.path().join("r"),
        };
        let lock = InstanceLock::acquire(&dirs).unwrap();
        assert_eq!(lock.path(), dirs.runtime.join("yerd.lock"));
        drop(lock);
    }

    #[test]
    fn acquire_creates_runtime_dir_if_missing() {
        let tmp = tempfile::tempdir().unwrap();
        let dirs = yerd_platform::PlatformDirs {
            config: tmp.path().join("c"),
            data: tmp.path().join("d"),
            state: tmp.path().join("s"),
            cache: tmp.path().join("ca"),
            runtime: tmp.path().join("does-not-exist-yet"),
        };
        assert!(!dirs.runtime.exists());
        let _lock = InstanceLock::acquire(&dirs).unwrap();
        assert!(dirs.runtime.exists());
    }
}
