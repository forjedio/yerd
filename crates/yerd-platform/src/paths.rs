//! `Paths` trait + [`PlatformDirs`] struct.

use std::path::PathBuf;

use crate::PlatformError;

/// The five directories Yerd cares about. Returned by [`Paths::resolve`].
///
/// **Existence is not guaranteed.** Callers are responsible for
/// `std::fs::create_dir_all` before writing into any of these paths.
/// `runtime` is security-sensitive on Linux when the fallback to
/// `/tmp/yerd-$UID` kicks in — caller should `mkdir(mode=0o700)` and, if
/// the directory already exists, verify ownership (`uid == geteuid()`)
/// and mode (`0o700`) before using it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlatformDirs {
    /// User-owned config directory (e.g. `~/.config/yerd` on Linux).
    pub config: PathBuf,
    /// User-owned persistent data directory (CA + leaf certs).
    pub data: PathBuf,
    /// User-owned long-lived state. Distinct from `data` on Linux
    /// (XDG_STATE_HOME); on macOS it coincides with `data`.
    pub state: PathBuf,
    /// User-owned cache directory (logs, downloads).
    pub cache: PathBuf,
    /// Runtime directory for the IPC socket and PID file.
    ///
    /// Linux: `XDG_RUNTIME_DIR/yerd` or, when `XDG_RUNTIME_DIR` is unset,
    /// `/tmp/yerd-$UID` (see struct-level docs for the caller contract).
    /// macOS: a `yerd-$UID` directory inside `std::env::temp_dir()`.
    pub runtime: PathBuf,
}

/// OS path-discovery abstraction.
pub trait Paths {
    /// Resolve every directory in one call.
    ///
    /// The daemon calls this once at startup. The returned paths are not
    /// guaranteed to exist on disk; the caller is responsible for
    /// `create_dir_all` before writing into them.
    fn resolve(&self) -> Result<PlatformDirs, PlatformError>;
}
