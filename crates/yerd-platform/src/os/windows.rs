//! Windows OS implementation.
//!
//! Only [`WindowsPaths`] is a real implementation in Phase 1. Every other trait
//! is a type alias to the `os::unsupported` stub, so the trait impls come for
//! free and stay total. Later phases replace one alias at a time with a real
//! `Windows*` type in the same change that adds its full trait impl (the
//! "never half-flip" rule).

use std::path::PathBuf;
use std::sync::OnceLock;

use crate::paths::{Paths, PlatformDirs};
use crate::pure::win_pipe;
use crate::PlatformError;

pub use super::unsupported::{
    UnsupportedPortBinder as WindowsPortBinder, UnsupportedPortRedirector as WindowsPortRedirector,
    UnsupportedResolverInstaller as WindowsResolverInstaller,
    UnsupportedSystemMetrics as WindowsSystemMetrics,
    UnsupportedTerminalLauncher as WindowsTerminalLauncher,
    UnsupportedTrustStore as WindowsTrustStore,
};

/// Read `%VAR%` as a non-empty directory path, or `MissingHomeDir` when unset or
/// empty. Windows has no single `HOME`; the known-folder env vars are the
/// closest equivalent, so a missing one reuses that error rather than adding a
/// near-duplicate variant.
fn env_dir(var: &str) -> Result<PathBuf, PlatformError> {
    match std::env::var_os(var) {
        Some(v) if !v.is_empty() => Ok(PathBuf::from(v)),
        _ => Err(PlatformError::MissingHomeDir),
    }
}

/// Real `Paths` for Windows.
///
/// Reads the known-folder env vars directly rather than using the `directories`
/// crate, whose Windows mapping (`%APPDATA%\Yerd\config`, different casing and
/// nesting) does not match Yerd's locked layout.
///
/// Layout decisions:
/// - `config` = `%APPDATA%\yerd` (roaming, like the Unix config home).
/// - `data`/`state`/`cache` are subdirectories of one `%LOCALAPPDATA%\yerd`
///   root so an uninstall can remove a single tree plus `%APPDATA%\yerd`.
/// - `state` stays distinct from `data` (as on Linux, unlike macOS): cheap now,
///   avoids a migration later.
/// - `runtime` = `std::env::temp_dir().join("yerd")`. It is per-user because
///   `%TEMP%` is per-user on Windows, so there is no `/tmp` sticky-bit trade-off
///   as on Linux.
#[derive(Debug, Default, Clone, Copy)]
pub struct WindowsPaths;

impl WindowsPaths {
    /// Construct.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

impl Paths for WindowsPaths {
    fn resolve(&self) -> Result<PlatformDirs, PlatformError> {
        let config = env_dir("APPDATA")?.join("yerd");
        let local = env_dir("LOCALAPPDATA")?.join("yerd");
        Ok(PlatformDirs {
            config,
            data: local.join("data"),
            state: local.join("state"),
            cache: local.join("cache"),
            runtime: std::env::temp_dir().join("yerd"),
        })
    }
}

/// Process-lifetime cache of the resolved SID, so [`current_user_sid`] spawns
/// `whoami` at most once per process.
static USER_SID: OnceLock<String> = OnceLock::new();

/// The current user's SID, e.g. `S-1-5-21-...`.
///
/// Runs `%SystemRoot%\System32\whoami.exe /user /fo csv /nh` (an absolute path,
/// never trusting `PATH`) and parses the SID from its CSV output. Cached for the
/// process lifetime. No `unsafe` and no new crates: the `GetTokenInformation`
/// alternative is `unsafe` FFI, which this crate's `#![forbid(unsafe_code)]`
/// rules out.
pub fn current_user_sid() -> Result<String, PlatformError> {
    if let Some(sid) = USER_SID.get() {
        return Ok(sid.clone());
    }
    let sid = spawn_whoami_sid()?;
    Ok(USER_SID.get_or_init(|| sid).clone())
}

/// Absolute path to `whoami.exe`, from `%SystemRoot%` (falling back to the
/// conventional location), so the lookup never resolves an attacker-planted
/// `whoami` on `PATH`.
fn whoami_path() -> PathBuf {
    let root =
        std::env::var_os("SystemRoot").map_or_else(|| PathBuf::from(r"C:\Windows"), PathBuf::from);
    root.join("System32").join("whoami.exe")
}

fn spawn_whoami_sid() -> Result<String, PlatformError> {
    let output = std::process::Command::new(whoami_path())
        .args(["/user", "/fo", "csv", "/nh"])
        .output()
        .map_err(|e| PlatformError::SidLookup {
            detail: format!("whoami spawn failed: {e}"),
        })?;
    if !output.status.success() {
        return Err(PlatformError::SidLookup {
            detail: format!("whoami exited with {}", output.status),
        });
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    win_pipe::parse_whoami_sid(&stdout).ok_or_else(|| PlatformError::SidLookup {
        detail: "whoami output had no parseable SID".to_owned(),
    })
}

/// The daemon pipe name for the current user under `dirs.runtime`: the single
/// shared derivation used by the daemon listener and every client.
pub fn daemon_pipe_name(dirs: &PlatformDirs) -> Result<String, PlatformError> {
    Ok(win_pipe::pipe_name(&current_user_sid()?, &dirs.runtime))
}
