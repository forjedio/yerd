//! Per-operation implementations. Per-OS branches live inside each
//! file behind `#[cfg(target_os)]` so an op can be audited end-to-end
//! in a single file.

pub mod ca;
pub mod port_redirect;
pub mod resolver;
pub mod setcap;

use std::ffi::OsStr;
use std::process::Command;

use crate::error::{CommandReason, HelperError};

/// Pinned `PATH` for every subprocess invocation. Matches
/// `/usr/sbin:/usr/bin:/sbin:/bin` on both Linux and macOS.
const PINNED_PATH: &str = "/usr/sbin:/usr/bin:/sbin:/bin";

/// Spawn `program` with `args`, with `env_clear()` plus the pinned
/// `PATH`. Returns the process output on success; maps every failure
/// mode into a typed [`HelperError::Command`].
pub fn run_command<I, S>(
    tool: &'static str,
    program: &str,
    args: I,
) -> Result<std::process::Output, HelperError>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let mut cmd = Command::new(program);
    cmd.env_clear().env("PATH", PINNED_PATH);
    for a in args {
        cmd.arg(a);
    }
    let output = cmd.output().map_err(|source| HelperError::Command {
        tool,
        reason: if source.kind() == std::io::ErrorKind::NotFound {
            CommandReason::NotFound
        } else {
            CommandReason::Spawn(source)
        },
    })?;
    if output.status.success() {
        return Ok(output);
    }
    let reason = output
        .status
        .code()
        .map_or(CommandReason::Signal, CommandReason::NonZero);
    Err(HelperError::Command { tool, reason })
}

/// Write `data` to `path` atomically with the given mode.
///
/// `mode_public = false` → 0o600 (anchor PEMs).
/// `mode_public = true`  → 0o644 (resolver files, drop-ins).
///
/// Atomicity: writes to a `.tmp` sibling in the same directory, fsyncs
/// it, then `rename(2)`s into place. Mode is set at creation time via
/// `OpenOptionsExt::mode` - no race window between create and chmod.
#[cfg(unix)]
pub fn atomic_write(
    path: &std::path::Path,
    data: &[u8],
    mode_public: bool,
) -> Result<(), HelperError> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;

    let parent = path.parent().ok_or_else(|| HelperError::Io {
        path: path.to_path_buf(),
        source: std::io::Error::new(std::io::ErrorKind::InvalidInput, "no parent dir"),
    })?;
    std::fs::create_dir_all(parent).map_err(|source| HelperError::Io {
        path: parent.to_path_buf(),
        source,
    })?;
    let mode = if mode_public { 0o644 } else { 0o600 };
    let tmp = parent.join(format!(
        ".{}.yerd-tmp",
        path.file_name().and_then(|n| n.to_str()).unwrap_or("file")
    ));
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(mode)
        .open(&tmp)
        .map_err(|source| HelperError::Io {
            path: tmp.clone(),
            source,
        })?;
    f.write_all(data).map_err(|source| HelperError::Io {
        path: tmp.clone(),
        source,
    })?;
    f.sync_all().map_err(|source| HelperError::Io {
        path: tmp.clone(),
        source,
    })?;
    drop(f);
    std::fs::rename(&tmp, path).map_err(|source| HelperError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    Ok(())
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

    #[test]
    fn run_command_returns_not_found_for_missing_tool() {
        let err = run_command(
            "yerd-bogus-tool",
            "/usr/bin/this-binary-does-not-exist-xyz",
            ["arg"],
        )
        .unwrap_err();
        assert!(matches!(
            err,
            HelperError::Command {
                reason: CommandReason::NotFound | CommandReason::Spawn(_),
                ..
            }
        ));
    }

    #[test]
    fn run_command_propagates_nonzero_exit() {
        let err = run_command("false", "/usr/bin/false", Vec::<&str>::new()).unwrap_err();
        match err {
            HelperError::Command {
                reason: CommandReason::NonZero(code),
                ..
            } => assert_eq!(code, 1),
            other => panic!("unexpected error: {other:?}"),
        }
    }

    #[test]
    fn run_command_succeeds_for_true() {
        let out = run_command("true", "/usr/bin/true", Vec::<&str>::new()).unwrap();
        assert!(out.status.success());
    }

    #[test]
    #[cfg(unix)]
    fn atomic_write_writes_and_sets_mode_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("ca.pem");
        atomic_write(&p, b"hello", false).unwrap();
        let contents = std::fs::read(&p).unwrap();
        assert_eq!(contents, b"hello");
        let perms = std::fs::metadata(&p).unwrap().permissions();
        assert_eq!(perms.mode() & 0o777, 0o600);
    }

    #[test]
    #[cfg(unix)]
    fn atomic_write_writes_and_sets_mode_world_readable() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("resolver-test");
        atomic_write(&p, b"world readable", true).unwrap();
        let perms = std::fs::metadata(&p).unwrap().permissions();
        assert_eq!(perms.mode() & 0o777, 0o644);
    }

    #[test]
    #[cfg(unix)]
    fn atomic_write_creates_parent_dirs() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("a/b/c/file");
        atomic_write(&p, b"x", true).unwrap();
        assert!(p.exists());
    }

    #[test]
    #[cfg(unix)]
    fn atomic_write_replaces_existing_file() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("file");
        std::fs::write(&p, b"old").unwrap();
        atomic_write(&p, b"new", true).unwrap();
        assert_eq!(std::fs::read(&p).unwrap(), b"new");
    }
}
