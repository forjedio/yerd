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

    // `fs4` uses `flock(2)` on Linux, where locks are held per
    // open-file-description, so a second `acquire()` in the same process
    // against a different fd would succeed. True cross-process validation
    // needs a subprocess; that lives in `tests/lifecycle.rs`. This unit test
    // only covers the success path on a fresh directory.

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
