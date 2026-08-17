//! Per-operation implementations. Per-OS branches live inside each
//! file behind `#[cfg(target_os)]` so an op can be audited end-to-end
//! in a single file.

pub mod ca;
pub mod lan_port_redirect;
pub mod port_redirect;
pub mod resolver;
pub mod setcap;

use std::ffi::OsStr;
#[cfg(any(unix, windows))]
use std::process::Command;

use crate::error::{CommandReason, HelperError};

/// Pinned `PATH` for every Unix subprocess invocation. Matches
/// `/usr/sbin:/usr/bin:/sbin:/bin` on both Linux and macOS.
#[cfg(unix)]
const PINNED_PATH: &str = "/usr/sbin:/usr/bin:/sbin:/bin";

/// Map a finished command's output into a typed result: `Ok` on success,
/// [`HelperError::Command`] otherwise (not-found, spawn, non-zero, signal).
#[cfg(any(unix, windows))]
fn map_command_output(
    tool: &'static str,
    output: std::io::Result<std::process::Output>,
) -> Result<std::process::Output, HelperError> {
    let output = output.map_err(|source| HelperError::Command {
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

/// Spawn `program` with `args`, with `env_clear()` plus the pinned
/// `PATH`. Returns the process output on success; maps every failure
/// mode into a typed [`HelperError::Command`]. Unix-only: it trusts
/// `PATH` to resolve `program`, so callers pass a bare tool name.
#[cfg(unix)]
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
    map_command_output(tool, cmd.output())
}

/// System PowerShell modules directory, derived from
/// [`yerd_platform::system_root`], the workspace's single `%SystemRoot%`
/// derivation.
///
/// This is pinned as the sole `PSModulePath` for elevated spawns (security:
/// avoid user-writable module autoload). The default Windows `PSModulePath`
/// lists the user-writable `%USERPROFILE%\Documents\WindowsPowerShell\Modules`
/// first, so inheriting it would let a same-user attacker who plants a module
/// exporting `Add-DnsClientNrptRule` there run code as admin. Pinning to the
/// non-user-writable system dir keeps the `DnsClient` cmdlets autoloading while
/// closing that hijack.
#[cfg(windows)]
fn system_ps_modules_dir() -> std::path::PathBuf {
    yerd_platform::system_root()
        .join("System32")
        .join("WindowsPowerShell")
        .join("v1.0")
        .join("Modules")
}

/// Spawn an **absolute-path** `program` with `args` (no `PATH` trust), with
/// `env_clear()` plus only the minimal env that lets PowerShell's `DnsClient`
/// module autoload: `SystemRoot`, `windir`, and a `PSModulePath` pinned to the
/// system modules dir (M3). The elevated helper launches from a UAC-clean
/// context, so nothing else is carried through; `PSModulePath` is set
/// explicitly rather than inherited to avoid user-writable module autoload (see
/// [`system_ps_modules_dir`]). Returns the process output; maps every failure
/// mode into a typed [`HelperError::Command`].
///
/// `CREATE_NO_WINDOW` keeps `powershell.exe`/`netsh.exe` from ever painting a
/// console: the helper is launched hidden through UAC, and the flag makes that
/// independent of how the launcher happened to be shown.
#[cfg(windows)]
pub fn run_command_abs<I, S>(
    tool: &'static str,
    program: &std::path::Path,
    args: I,
) -> Result<std::process::Output, HelperError>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    use std::os::windows::process::CommandExt as _;

    let mut cmd = Command::new(program);
    cmd.creation_flags(yerd_platform::CREATE_NO_WINDOW);
    cmd.env_clear();
    for var in ["SystemRoot", "windir"] {
        if let Some(value) = std::env::var_os(var) {
            cmd.env(var, value);
        }
    }
    cmd.env("PSModulePath", system_ps_modules_dir());
    for a in args {
        cmd.arg(a);
    }
    map_command_output(tool, cmd.output())
}

/// Write the advisory result file the CLI reads after an elevated helper run
/// (`ShellExecuteEx` yields no stdio). Best-effort: any failure is logged to
/// stderr and swallowed, never touching the exit code. `token` is already
/// validated as `[0-9a-f]{32}` by `cli::parse`, so the file name cannot
/// traverse or name a reparse point. Uses `create_new` to refuse following a
/// pre-planted file.
#[cfg(windows)]
pub fn write_result_file(token: &str, result: &Result<(), HelperError>) {
    use yerd_platform::pure::helper_result::{self, HelperResult};

    let outcome = match result {
        Ok(()) => HelperResult::Ok,
        Err(e) => HelperResult::Error(e.to_string()),
    };
    if let Err(e) = try_write_result_file(token, &helper_result::render(&outcome)) {
        eprintln!("yerd-helper: note: could not write result file: {e}");
    }
}

#[cfg(windows)]
fn try_write_result_file(token: &str, body: &str) -> std::io::Result<()> {
    use yerd_platform::{ActivePaths, Paths};

    let dirs = ActivePaths::new()
        .resolve()
        .map_err(|e| std::io::Error::other(e.to_string()))?;
    write_result_to_dir(&dirs.runtime, token, body)
}

/// Create `runtime/helper-result-<token>.txt` with `body` (plus a trailing
/// newline), refusing to follow a pre-planted file via `create_new`. Factored
/// out of [`try_write_result_file`] so it is testable against a temp dir.
#[cfg(windows)]
fn write_result_to_dir(runtime: &std::path::Path, token: &str, body: &str) -> std::io::Result<()> {
    use std::io::Write as _;

    use yerd_platform::pure::helper_result;

    std::fs::create_dir_all(runtime)?;
    let path = runtime.join(helper_result::result_file_name(token));
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)?;
    file.write_all(body.as_bytes())?;
    file.write_all(b"\n")?;
    Ok(())
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

    #[cfg(unix)]
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

    #[cfg(windows)]
    #[test]
    fn run_command_abs_returns_not_found_for_missing_tool() {
        let err = run_command_abs(
            "yerd-bogus-tool",
            std::path::Path::new(r"C:\Windows\System32\this-binary-does-not-exist-xyz.exe"),
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

    #[cfg(unix)]
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

    #[cfg(unix)]
    #[test]
    fn run_command_succeeds_for_true() {
        let out = run_command("true", "/usr/bin/true", Vec::<&str>::new()).unwrap();
        assert!(out.status.success());
    }

    #[cfg(windows)]
    #[test]
    fn system_ps_modules_dir_is_pinned_system_path_not_user_writable() {
        let dir = system_ps_modules_dir();
        assert!(
            dir.ends_with(r"System32\WindowsPowerShell\v1.0\Modules"),
            "expected pinned system modules dir, got {}",
            dir.display()
        );
        let lower = dir.to_string_lossy().to_lowercase();
        assert!(
            !lower.contains("documents"),
            "pinned PSModulePath must not include a user-writable profile path, got {}",
            dir.display()
        );
    }

    #[cfg(windows)]
    #[test]
    fn write_result_to_dir_writes_ok_line() {
        let dir = tempfile::tempdir().unwrap();
        let token = "0123456789abcdef0123456789abcdef";
        write_result_to_dir(dir.path(), token, "ok").unwrap();
        let path = dir.path().join(format!("helper-result-{token}.txt"));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "ok\n");
    }

    #[cfg(windows)]
    #[test]
    fn write_result_to_dir_writes_error_line() {
        let dir = tempfile::tempdir().unwrap();
        let token = "abcdefabcdefabcdefabcdefabcdefab";
        write_result_to_dir(dir.path(), token, "error: boom").unwrap();
        let path = dir.path().join(format!("helper-result-{token}.txt"));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "error: boom\n");
    }

    #[cfg(windows)]
    #[test]
    fn write_result_to_dir_refuses_to_follow_preplanted_file() {
        let dir = tempfile::tempdir().unwrap();
        let token = "11112222333344445555666677778888";
        let path = dir.path().join(format!("helper-result-{token}.txt"));
        std::fs::write(&path, "pre-planted").unwrap();
        let err = write_result_to_dir(dir.path(), token, "ok").unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::AlreadyExists);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "pre-planted");
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
