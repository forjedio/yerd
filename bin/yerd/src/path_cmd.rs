//! `yerd path install|uninstall|print` - put yerd's shim dir on PATH so a bare
//! `php`/`composer` resolves to the managed shims.
//!
//! Local, daemon-free, unprivileged: it edits only state the user owns. On Unix
//! that is the yerd-owned block in the shell rc file(s) (pure string logic in
//! `yerd_platform::pure::shell_profile`); on Windows it is the user's
//! `HKCU\Environment\Path` (pure list editing in
//! `yerd_platform::pure::win_path_env`, registry I/O in `yerd_platform`), plus a
//! copy of `yerd.exe` into `%LOCALAPPDATA%\Programs\yerd\bin`.

use std::path::Path;
use std::process::ExitCode;

use crate::cli::PathAction;

/// Run `yerd path <action>`: edit the user's shell rc file(s) to add/remove
/// yerd's bin dir on PATH, or print the snippet. Returns the process exit code.
#[cfg(unix)]
pub fn run(action: PathAction) -> ExitCode {
    unix::run(action)
}

/// Run `yerd path <action>` on Windows: edit the user's `HKCU\Environment\Path`
/// to add/remove yerd's program + shim dirs, or print them.
#[cfg(windows)]
pub fn run(action: PathAction) -> ExitCode {
    windows::run(action)
}

/// Stub for platforms with no PATH management wired.
#[cfg(not(any(unix, windows)))]
pub fn run(_action: PathAction) -> ExitCode {
    use yerd_platform::{ActivePaths, Paths};
    let hint = ActivePaths::new().resolve().map_or_else(
        |_| "yerd's bin directory".to_owned(),
        |d| d.data.join("bin").display().to_string(),
    );
    eprintln!(
        "yerd: `yerd path` is not yet supported on this platform — add {hint} to PATH manually"
    );
    ExitCode::FAILURE
}

/// Idempotently add the PATH block after a successful tool install (best-effort,
/// quiet). Called from the CLI's install path so `composer`/`node`/`bun` resolve
/// in the user's shell without a separate `yerd path install`. The
/// `BinDirNotOnPath` doctor warning is the backstop when this can't run.
/// `quiet` (set under `--json`) still performs the rc edit but suppresses the
/// human note, so machine consumers reading stdout get clean JSON.
#[cfg(unix)]
pub fn ensure_installed_after_tool(quiet: bool) {
    unix::ensure_installed_after_tool(quiet);
}

/// Windows counterpart: idempotently add the program + shim dirs to the user PATH
/// after a tool install (best-effort, quiet). The `BinDirNotOnPath` doctor
/// warning is the backstop when this can't run.
#[cfg(windows)]
pub fn ensure_installed_after_tool(quiet: bool) {
    windows::ensure_installed_after_tool(quiet);
}

/// No PATH management wired here: no-op (doctor warns instead).
#[cfg(not(any(unix, windows)))]
pub fn ensure_installed_after_tool(_quiet: bool) {}

/// Remove the yerd PATH block from an explicit user's shell rc file(s), given
/// their home directory and login-shell basename (e.g. `zsh`). Unlike [`run`],
/// this reads neither `$HOME` nor `$SHELL` - `yerd uninstall`, run under sudo,
/// must target the *invoking* user, not root. Returns the list of files it
/// edited (the block was present and removed). Best-effort: unreadable files
/// are skipped.
#[cfg(unix)]
pub fn remove_block_for_user(
    home: &std::path::Path,
    shell_basename: &str,
) -> Vec<std::path::PathBuf> {
    unix::remove_block_for_user(home, shell_basename)
}

/// Remove yerd's program + shim dirs from the user `HKCU\Environment\Path`
/// (Windows uninstall). Returns the dirs it removed, or empty if none were
/// present. Best-effort: a registry failure yields an empty list.
#[cfg(windows)]
pub fn remove_from_path() -> Vec<std::path::PathBuf> {
    windows::remove_from_path()
}

/// Filename of the installed CLI in the program directory.
const LIVE_EXE: &str = "yerd.exe";
/// Filename a staged replacement is written to before promotion.
const NEW_EXE: &str = "yerd.exe.new";
/// Filename the live image is renamed to while a replacement is promoted.
const OLD_EXE: &str = "yerd.exe.old";

/// What [`reconcile_staged_exe`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SwapOutcome {
    /// No staged replacement and no leftover to recover; nothing to do.
    Nothing,
    /// A staged replacement was promoted into place.
    Promoted,
    /// The live image was missing and the aside copy was restored.
    Recovered,
    /// A staged replacement exists but the live image could not be moved aside,
    /// so it is still in place and the replacement is retained for a later run.
    Blocked,
    /// The promotion failed. `restored` reports whether the aside copy was put
    /// back, so the caller can say whether the CLI still works.
    Failed {
        /// Whether the live image was successfully restored.
        restored: bool,
    },
}

/// Finish or recover an interrupted executable swap in `dir`.
///
/// Windows cannot overwrite a running image, so a replacement is staged beside
/// it and promoted by renaming. Renames are cheap metadata operations, but a
/// crash can still land between them, so this runs at the start of every install
/// and is idempotent.
///
/// The order matters. The missing-live check comes first, because after a crash
/// between the two renames the aside copy is the only surviving image and
/// sweeping first would destroy it. For the same reason the sweep and the
/// move-aside happen only when the live image is present: with it absent the
/// aside copy is the previous CLI and is still needed as the rollback target if
/// promoting the replacement fails.
pub fn reconcile_staged_exe(dir: &Path) -> SwapOutcome {
    reconcile_staged_exe_with(dir, |from, to| std::fs::rename(from, to))
}

/// [`reconcile_staged_exe`] with the rename operation injected, so every failure
/// interleaving is table-tested on hosts that cannot run the Windows path.
pub fn reconcile_staged_exe_with<F>(dir: &Path, rename: F) -> SwapOutcome
where
    F: Fn(&Path, &Path) -> std::io::Result<()>,
{
    let live = dir.join(LIVE_EXE);
    let new = dir.join(NEW_EXE);
    let old = dir.join(OLD_EXE);

    if !live.exists() && old.exists() && !new.exists() {
        return if rename(&old, &live).is_ok() {
            SwapOutcome::Recovered
        } else {
            SwapOutcome::Failed { restored: false }
        };
    }

    if !new.exists() {
        let _ = std::fs::remove_file(&old);
        return SwapOutcome::Nothing;
    }

    if live.exists() {
        let _ = std::fs::remove_file(&old);
        if rename(&live, &old).is_err() {
            return SwapOutcome::Blocked;
        }
    }
    if rename(&new, &live).is_ok() {
        let _ = std::fs::remove_file(&old);
        return SwapOutcome::Promoted;
    }
    SwapOutcome::Failed {
        restored: rename(&old, &live).is_ok(),
    }
}

/// Whether a raw OS error code means the file is held open by someone else.
///
/// `ERROR_SHARING_VIOLATION` and `ERROR_LOCK_VIOLATION` are the two Windows
/// reports for a transiently locked file, which an antivirus or indexer can
/// cause on any write.
#[must_use]
pub fn is_sharing_violation(raw: Option<i32>) -> bool {
    const ERROR_SHARING_VIOLATION: i32 = 32;
    const ERROR_LOCK_VIOLATION: i32 = 33;
    matches!(raw, Some(ERROR_SHARING_VIOLATION | ERROR_LOCK_VIOLATION))
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod swap_tests {
    use super::*;

    fn write(dir: &Path, name: &str, body: &str) {
        std::fs::write(dir.join(name), body).unwrap();
    }

    fn read(dir: &Path, name: &str) -> String {
        std::fs::read_to_string(dir.join(name)).unwrap()
    }

    /// A rename that fails on the given 1-based call numbers and otherwise
    /// performs the real operation.
    fn failing_on(calls: &[u32]) -> impl Fn(&Path, &Path) -> std::io::Result<()> + '_ {
        let seen = std::cell::Cell::new(0u32);
        move |from, to| {
            seen.set(seen.get() + 1);
            if calls.contains(&seen.get()) {
                return Err(std::io::Error::other("injected rename failure"));
            }
            std::fs::rename(from, to)
        }
    }

    #[test]
    fn promotes_new_over_a_live_image() {
        let tmp = tempfile::tempdir().unwrap();
        write(tmp.path(), LIVE_EXE, "old");
        write(tmp.path(), NEW_EXE, "new");
        assert_eq!(reconcile_staged_exe(tmp.path()), SwapOutcome::Promoted);
        assert_eq!(read(tmp.path(), LIVE_EXE), "new");
        assert!(!tmp.path().join(NEW_EXE).exists());
        assert!(!tmp.path().join(OLD_EXE).exists());
    }

    #[test]
    fn is_a_noop_without_a_staged_replacement() {
        let tmp = tempfile::tempdir().unwrap();
        write(tmp.path(), LIVE_EXE, "live");
        assert_eq!(reconcile_staged_exe(tmp.path()), SwapOutcome::Nothing);
        assert_eq!(read(tmp.path(), LIVE_EXE), "live");
    }

    #[test]
    fn promotes_when_the_live_image_is_absent() {
        let tmp = tempfile::tempdir().unwrap();
        write(tmp.path(), NEW_EXE, "new");
        assert_eq!(reconcile_staged_exe(tmp.path()), SwapOutcome::Promoted);
        assert_eq!(read(tmp.path(), LIVE_EXE), "new");
    }

    /// A stale aside copy left by an earlier successful swap is swept, but only
    /// when the live image is present.
    #[test]
    fn sweeps_a_stale_aside_copy() {
        let tmp = tempfile::tempdir().unwrap();
        write(tmp.path(), LIVE_EXE, "live");
        write(tmp.path(), OLD_EXE, "stale");
        assert_eq!(reconcile_staged_exe(tmp.path()), SwapOutcome::Nothing);
        assert_eq!(read(tmp.path(), LIVE_EXE), "live");
        assert!(!tmp.path().join(OLD_EXE).exists());
    }

    /// After a crash between the two renames the aside copy is the only image
    /// left, so it must be restored rather than swept.
    #[test]
    fn recovers_the_aside_copy_when_the_live_image_vanished() {
        let tmp = tempfile::tempdir().unwrap();
        write(tmp.path(), OLD_EXE, "survivor");
        assert_eq!(reconcile_staged_exe(tmp.path()), SwapOutcome::Recovered);
        assert_eq!(read(tmp.path(), LIVE_EXE), "survivor");
    }

    /// A held live image cannot be moved aside. The CLI must keep working and
    /// the staged replacement must survive for a later run.
    #[test]
    fn blocked_leaves_the_live_image_working() {
        let tmp = tempfile::tempdir().unwrap();
        write(tmp.path(), LIVE_EXE, "live");
        write(tmp.path(), NEW_EXE, "new");
        let outcome = reconcile_staged_exe_with(tmp.path(), failing_on(&[1]));
        assert_eq!(outcome, SwapOutcome::Blocked);
        assert_eq!(read(tmp.path(), LIVE_EXE), "live");
        assert_eq!(read(tmp.path(), NEW_EXE), "new");
    }

    /// If the promotion fails the aside copy goes back, so the user still has a
    /// working CLI.
    #[test]
    fn a_failed_promotion_is_rolled_back() {
        let tmp = tempfile::tempdir().unwrap();
        write(tmp.path(), LIVE_EXE, "live");
        write(tmp.path(), NEW_EXE, "new");
        let outcome = reconcile_staged_exe_with(tmp.path(), failing_on(&[2]));
        assert_eq!(outcome, SwapOutcome::Failed { restored: true });
        assert_eq!(read(tmp.path(), LIVE_EXE), "live");
    }

    /// A crash between the two renames leaves no live image, an aside copy (the
    /// previous CLI) and a staged replacement. The aside copy is the only
    /// rollback target, so it must survive until the promotion has succeeded.
    #[test]
    fn keeps_the_aside_copy_when_the_live_image_is_missing() {
        let tmp = tempfile::tempdir().unwrap();
        write(tmp.path(), OLD_EXE, "previous");
        write(tmp.path(), NEW_EXE, "new");
        let outcome = reconcile_staged_exe_with(tmp.path(), failing_on(&[1]));
        assert_eq!(outcome, SwapOutcome::Failed { restored: true });
        assert_eq!(read(tmp.path(), LIVE_EXE), "previous");
    }

    /// The same interleaving when the promotion works: the replacement lands and
    /// the now-stale aside copy is swept.
    #[test]
    fn promotes_over_a_missing_live_image_and_sweeps_the_aside_copy() {
        let tmp = tempfile::tempdir().unwrap();
        write(tmp.path(), OLD_EXE, "previous");
        write(tmp.path(), NEW_EXE, "new");
        assert_eq!(reconcile_staged_exe(tmp.path()), SwapOutcome::Promoted);
        assert_eq!(read(tmp.path(), LIVE_EXE), "new");
        assert!(!tmp.path().join(OLD_EXE).exists());
    }

    /// The worst case: neither the promotion nor the rollback works. Both images
    /// must still exist under their own names so the user can rename one back.
    #[test]
    fn a_failed_rollback_leaves_both_images_present() {
        let tmp = tempfile::tempdir().unwrap();
        write(tmp.path(), LIVE_EXE, "live");
        write(tmp.path(), NEW_EXE, "new");
        let outcome = reconcile_staged_exe_with(tmp.path(), failing_on(&[2, 3]));
        assert_eq!(outcome, SwapOutcome::Failed { restored: false });
        assert_eq!(read(tmp.path(), OLD_EXE), "live");
        assert_eq!(read(tmp.path(), NEW_EXE), "new");
    }

    #[test]
    fn sharing_violations_are_the_two_lock_codes() {
        assert!(is_sharing_violation(Some(32)));
        assert!(is_sharing_violation(Some(33)));
        assert!(!is_sharing_violation(Some(5)));
        assert!(!is_sharing_violation(Some(0)));
        assert!(!is_sharing_violation(None));
    }
}

#[cfg(unix)]
mod unix {
    use std::path::{Path, PathBuf};
    use std::process::ExitCode;

    use yerd_platform::pure::shell_profile::{
        self, detect_shell, rc_relpaths, render_block, HostOs, Shell,
    };
    use yerd_platform::{ActivePaths, Paths};

    use crate::cli::PathAction;

    pub fn run(action: PathAction) -> ExitCode {
        let bin_dir = match ActivePaths::new().resolve() {
            Ok(d) => d.data.join("bin"),
            Err(e) => return fail(format!("cannot resolve yerd directories: {e}")),
        };

        let shell = detect_shell(&shell_basename());
        if matches!(action, PathAction::Print) {
            print!("{}", render_block(shell.unwrap_or(Shell::Posix), &bin_dir));
            return ExitCode::SUCCESS;
        }

        let Some(shell) = shell else {
            eprintln!(
                "yerd: could not detect your shell from $SHELL. Add this to your shell's startup file:\n\n{}",
                render_block(Shell::Posix, &bin_dir)
            );
            return ExitCode::FAILURE;
        };

        let home = match std::env::var_os("HOME") {
            Some(h) if !h.is_empty() => PathBuf::from(h),
            _ => return fail("$HOME is not set".to_owned()),
        };

        let install = matches!(action, PathAction::Install);
        let mut touched = Vec::new();
        let mut any_err = false;
        for rel in rc_relpaths(shell, host_os()) {
            let rc = home.join(&rel);
            if !install && !rc.exists() {
                continue;
            }
            match edit_one(&rc, shell, &bin_dir, install) {
                Ok(true) => touched.push(rc),
                Ok(false) => {}
                Err(e) => {
                    eprintln!("yerd: {}: {e}", rc.display());
                    any_err = true;
                }
            }
        }

        report(&touched, install, &bin_dir, any_err);
        if any_err {
            ExitCode::FAILURE
        } else {
            ExitCode::SUCCESS
        }
    }

    /// Add the PATH block after a tool install - idempotent and quiet. Does
    /// nothing when it's already present, or when the shell / `$HOME` can't be
    /// determined (the `BinDirNotOnPath` doctor warning is the backstop). Prints
    /// a one-line note only when it actually adds the block, so repeat installs
    /// stay silent.
    pub fn ensure_installed_after_tool(quiet: bool) {
        let Ok(d) = ActivePaths::new().resolve() else {
            return;
        };
        let bin_dir = d.data.join("bin");
        let Some(shell) = detect_shell(&shell_basename()) else {
            return;
        };
        let Some(home) = std::env::var_os("HOME")
            .filter(|h| !h.is_empty())
            .map(PathBuf::from)
        else {
            return;
        };
        let mut added = false;
        for rel in rc_relpaths(shell, host_os()) {
            if let Ok(true) = edit_one(&home.join(&rel), shell, &bin_dir, true) {
                added = true;
            }
        }
        if added && !quiet {
            println!(
                "\nyerd: added {} to your PATH. Open a new terminal to use installed tools.",
                bin_dir.display()
            );
        }
    }

    /// Remove the yerd PATH block from `home`'s rc file(s) for the shell named
    /// by `shell_basename`. Daemon-free and home-explicit (the uninstall path
    /// runs under sudo, where `$HOME`/`$SHELL` point at root). `bin_dir` is
    /// irrelevant to removal - `shell_profile::remove_block` matches the guarded
    /// markers - so a placeholder is passed. Returns the files actually changed.
    pub fn remove_block_for_user(home: &Path, shell_basename: &str) -> Vec<PathBuf> {
        let Some(shell) = detect_shell(shell_basename) else {
            return Vec::new();
        };
        let placeholder_bin = Path::new("");
        let mut touched = Vec::new();
        for rel in rc_relpaths(shell, host_os()) {
            let rc = home.join(&rel);
            if !rc.exists() {
                continue;
            }
            if let Ok(true) = edit_one(&rc, shell, placeholder_bin, false) {
                touched.push(rc);
            }
        }
        touched
    }

    /// Edit one rc file. Returns `Ok(true)` if the file's contents changed.
    fn edit_one(rc: &Path, shell: Shell, bin_dir: &Path, install: bool) -> std::io::Result<bool> {
        let real = resolve_symlink(rc)?;

        let existing = match std::fs::read_to_string(&real) {
            Ok(s) => s,
            Err(ref e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(e) => return Err(e),
        };

        let updated = if install {
            shell_profile::upsert_block(&existing, shell, bin_dir)
        } else {
            shell_profile::remove_block(&existing)
        };
        if updated == existing {
            return Ok(false);
        }

        if real.exists() {
            let bak = backup_path(&real);
            if !bak.exists() {
                let _ = std::fs::copy(&real, &bak);
            }
        }

        write_atomic(&real, &existing, &updated)?;
        Ok(true)
    }

    /// The real file behind `rc`: follows a symlink one or more hops via
    /// `canonicalize`; if `rc` doesn't exist yet, returns it unchanged (it'll be
    /// created). A broken/parent-relative case falls back to `rc` itself.
    fn resolve_symlink(rc: &Path) -> std::io::Result<PathBuf> {
        match std::fs::symlink_metadata(rc) {
            Ok(m) if m.file_type().is_symlink() => match std::fs::canonicalize(rc) {
                Ok(real) => Ok(real),
                Err(ref e) if e.kind() == std::io::ErrorKind::NotFound => Ok(rc.to_path_buf()),
                Err(e) => Err(e),
            },
            Ok(_) => Ok(rc.to_path_buf()),
            Err(ref e) if e.kind() == std::io::ErrorKind::NotFound => Ok(rc.to_path_buf()),
            Err(e) => Err(e),
        }
    }

    /// `<file>.yerd.bak` alongside the real file.
    fn backup_path(real: &Path) -> PathBuf {
        let mut name = real.file_name().unwrap_or_default().to_os_string();
        name.push(".yerd.bak");
        real.with_file_name(name)
    }

    /// Write `contents` to `dest` via a temp sibling + rename (atomic, and keeps
    /// the temp on the same filesystem as the real file so rename can't EXDEV).
    /// Creates parent dirs (needed for `~/.config/fish`) and preserves the
    /// existing file mode, defaulting to 0o644 for a new file.
    fn write_atomic(dest: &Path, prev: &str, contents: &str) -> std::io::Result<()> {
        use std::os::unix::fs::PermissionsExt;
        use std::sync::atomic::{AtomicU64, Ordering};

        static SEQ: AtomicU64 = AtomicU64::new(0);

        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mode = std::fs::metadata(dest).map(|m| m.permissions().mode()).ok();

        let seq = SEQ.fetch_add(1, Ordering::Relaxed);
        let mut name = dest.file_name().unwrap_or_default().to_os_string();
        name.push(format!(".yerd-tmp-{}-{seq}", std::process::id()));
        let tmp = dest.with_file_name(name);
        let _ = std::fs::remove_file(&tmp);

        if let Ok(current) = std::fs::read_to_string(dest) {
            if current != prev {
                return Err(std::io::Error::other(
                    "file changed on disk since it was read",
                ));
            }
        }

        std::fs::write(&tmp, contents)?;
        let m = mode.unwrap_or(0o644);
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(m))?;
        std::fs::rename(&tmp, dest)
    }

    fn report(touched: &[PathBuf], install: bool, bin_dir: &Path, had_errors: bool) {
        if touched.is_empty() {
            if had_errors {
                return;
            }
            if install {
                println!("yerd: PATH already configured — nothing to do.");
            } else {
                println!("yerd: no yerd PATH block found — nothing to remove.");
            }
            return;
        }
        let verb = if install { "Added to" } else { "Removed from" };
        for f in touched {
            println!("{verb} {}", f.display());
        }
        if install {
            let first = touched
                .first()
                .map(|p| p.display().to_string())
                .unwrap_or_default();
            println!(
                "\n{} is now on PATH for new shells. Open a new terminal, or run:\n  source {first}",
                bin_dir.display(),
            );
        } else {
            println!("\nOpen a new terminal for the change to take effect.");
        }
    }

    fn shell_basename() -> String {
        std::env::var_os("SHELL")
            .map(PathBuf::from)
            .as_deref()
            .and_then(basename)
            .filter(|s| !s.is_empty())
            .or_else(login_shell_basename)
            .unwrap_or_default()
    }

    /// The current user's login shell basename from the passwd database, or
    /// `None` if it can't be resolved. Used only as a `$SHELL` fallback.
    fn login_shell_basename() -> Option<String> {
        let user = nix::unistd::User::from_uid(nix::unistd::Uid::current())
            .ok()
            .flatten()?;
        basename(&user.shell).filter(|s| !s.is_empty())
    }

    /// File-name component of `p` as an owned `String`.
    fn basename(p: &Path) -> Option<String> {
        p.file_name().map(|s| s.to_string_lossy().into_owned())
    }

    fn host_os() -> HostOs {
        if cfg!(target_os = "macos") {
            HostOs::MacOs
        } else {
            HostOs::Linux
        }
    }

    fn fail(msg: String) -> ExitCode {
        eprintln!("yerd: {msg}");
        ExitCode::FAILURE
    }

    #[cfg(test)]
    #[allow(clippy::unwrap_used, clippy::panic, clippy::indexing_slicing)]
    mod tests {
        use super::*;
        use std::fs;

        /// Build the exact guarded PATH block `yerd path install` writes, so the
        /// removal path matches the real markers byte-for-byte.
        fn block_for(shell: Shell, bin: &Path) -> String {
            render_block(shell, bin)
        }

        #[test]
        fn remove_block_for_user_removes_zsh_block_and_reports_file() {
            let home = tempfile::tempdir().unwrap();
            let rc = home.path().join(".zshrc");
            let bin = Path::new("/data/io.yerd.Yerd/bin");
            let original = format!("# my zshrc\nexport FOO=1\n\n{}", block_for(Shell::Zsh, bin));
            fs::write(&rc, &original).unwrap();

            let touched = remove_block_for_user(home.path(), "zsh");

            assert_eq!(touched, vec![rc.clone()]);
            let after = fs::read_to_string(&rc).unwrap();
            assert!(
                !shell_profile::contains_block(&after),
                "block remained: {after}"
            );
            assert!(after.contains("export FOO=1"));
            assert_eq!(after, "# my zshrc\nexport FOO=1\n");
        }

        #[test]
        fn remove_block_for_user_no_block_present_returns_empty() {
            let home = tempfile::tempdir().unwrap();
            let rc = home.path().join(".zshrc");
            fs::write(&rc, "export FOO=1\n").unwrap();

            let touched = remove_block_for_user(home.path(), "zsh");
            assert!(touched.is_empty());
            assert_eq!(fs::read_to_string(&rc).unwrap(), "export FOO=1\n");
        }

        #[test]
        fn remove_block_for_user_unknown_shell_is_noop() {
            let home = tempfile::tempdir().unwrap();
            assert!(remove_block_for_user(home.path(), "nushell").is_empty());
            assert!(remove_block_for_user(home.path(), "").is_empty());
        }

        #[test]
        fn remove_block_for_user_missing_rc_file_is_skipped() {
            let home = tempfile::tempdir().unwrap();
            assert!(remove_block_for_user(home.path(), "zsh").is_empty());
        }

        #[test]
        fn remove_block_for_user_bash_touches_only_files_with_the_block() {
            let home = tempfile::tempdir().unwrap();
            let bin = Path::new("/data/io.yerd.Yerd/bin");
            let bashrc = home.path().join(".bashrc");
            let bash_profile = home.path().join(".bash_profile");
            fs::write(&bashrc, block_for(Shell::Bash, bin)).unwrap();
            fs::write(&bash_profile, "export EDITOR=vim\n").unwrap();

            let touched = remove_block_for_user(home.path(), "bash");

            assert_eq!(touched, vec![bashrc.clone()]);
            assert_eq!(fs::read_to_string(&bashrc).unwrap(), "");
            assert_eq!(
                fs::read_to_string(&bash_profile).unwrap(),
                "export EDITOR=vim\n"
            );
        }

        /// A dotfiles setup where `~/.zshrc` is a symlink to a real file
        /// elsewhere: removal must write through the link, leaving the symlink
        /// intact.
        #[test]
        fn remove_block_for_user_follows_symlinked_rc() {
            let home = tempfile::tempdir().unwrap();
            let store = tempfile::tempdir().unwrap();
            let real = store.path().join("zshrc");
            let bin = Path::new("/data/io.yerd.Yerd/bin");
            fs::write(
                &real,
                format!("export KEEP=1\n\n{}", block_for(Shell::Zsh, bin)),
            )
            .unwrap();
            let link = home.path().join(".zshrc");
            std::os::unix::fs::symlink(&real, &link).unwrap();

            let touched = remove_block_for_user(home.path(), "zsh");
            assert_eq!(touched.len(), 1);
            assert!(fs::symlink_metadata(&link)
                .unwrap()
                .file_type()
                .is_symlink());
            let after = fs::read_to_string(&real).unwrap();
            assert!(!shell_profile::contains_block(&after));
            assert!(after.contains("export KEEP=1"));
        }
    }
}

#[cfg(windows)]
mod windows {
    use std::path::{Path, PathBuf};
    use std::process::ExitCode;

    use yerd_platform::pure::win_path_env;
    use yerd_platform::{ActivePaths, Paths};

    use crate::cli::PathAction;

    /// `%LOCALAPPDATA%\Programs\yerd\bin` - where the installed `yerd.exe` lives
    /// (the NSIS installer uses the same location). `None` when
    /// `%LOCALAPPDATA%` is unset.
    fn programs_bin() -> Option<PathBuf> {
        std::env::var_os("LOCALAPPDATA")
            .filter(|v| !v.is_empty())
            .map(|l| PathBuf::from(l).join("Programs").join("yerd").join("bin"))
    }

    /// `{data}\bin` - the managed `.cmd` shim directory.
    fn shim_dir() -> Option<PathBuf> {
        ActivePaths::new()
            .resolve()
            .ok()
            .map(|d| d.data.join("bin"))
    }

    /// The two dirs Yerd puts on the user PATH: the program dir (holding
    /// `yerd.exe`) and the shim dir (holding `php.cmd` and friends).
    fn path_entries() -> Vec<PathBuf> {
        let mut out = Vec::new();
        if let Some(p) = programs_bin() {
            out.push(p);
        }
        if let Some(s) = shim_dir() {
            out.push(s);
        }
        out
    }

    pub fn run(action: PathAction) -> ExitCode {
        match action {
            PathAction::Install => install(false),
            PathAction::Uninstall => uninstall(),
            PathAction::Print => print(),
        }
    }

    /// Copy `yerd.exe` into the program dir and add both dirs to the user PATH.
    ///
    /// A failed copy is not fatal to the PATH edit, but it must still fail the
    /// command: the NSIS postinstall hook and any scripted caller branch on the
    /// exit code, and a PATH entry pointing at a directory with no `yerd.exe` in
    /// it is not an installed CLI.
    fn install(quiet: bool) -> ExitCode {
        let mut copy_failed = false;
        if let Err(msg) = copy_self_into_programs() {
            eprintln!("yerd: {msg}");
            copy_failed = true;
        }
        match upsert_path() {
            Ok(_) if copy_failed => ExitCode::FAILURE,
            Ok(changed) => {
                if !quiet {
                    if changed {
                        for e in path_entries() {
                            println!("Added to PATH: {}", e.display());
                        }
                        println!(
                            "\nOpen a new terminal (or log off and back on) to pick up the change."
                        );
                    } else {
                        println!("yerd: PATH already configured - nothing to do.");
                    }
                }
                ExitCode::SUCCESS
            }
            Err(msg) => {
                eprintln!("yerd: {msg}");
                ExitCode::FAILURE
            }
        }
    }

    /// Idempotent, quiet variant used after a tool install.
    pub fn ensure_installed_after_tool(quiet: bool) {
        let _ = copy_self_into_programs();
        if let Ok(true) = upsert_path() {
            if !quiet {
                if let Some(s) = shim_dir() {
                    println!(
                        "\nyerd: added {} to your PATH. Open a new terminal to use installed tools.",
                        s.display()
                    );
                }
            }
        }
    }

    fn uninstall() -> ExitCode {
        let removed = remove_from_path();
        if removed.is_empty() {
            println!("yerd: no yerd PATH entries found - nothing to remove.");
        } else {
            for e in &removed {
                println!("Removed from PATH: {}", e.display());
            }
            println!("\nOpen a new terminal for the change to take effect.");
        }
        delete_programs_copy();
        ExitCode::SUCCESS
    }

    fn print() -> ExitCode {
        for e in path_entries() {
            println!("{}", e.display());
        }
        println!("\nAdd both directories above to your user PATH (Settings > Edit environment");
        println!("variables for your account), or run `yerd path install`.");
        ExitCode::SUCCESS
    }

    /// Add both dirs to the user PATH (idempotent) and broadcast the change.
    /// Returns whether the stored PATH value actually changed.
    ///
    /// A read failure must abort, never fall back to an empty PATH: the edit is
    /// written back wholesale, so treating "couldn't read" as "no entries" would
    /// replace the user's real PATH with just Yerd's dirs. An absent value
    /// (`Ok(None)`) genuinely is an empty starting point.
    fn upsert_path() -> Result<bool, String> {
        let entries = path_entries();
        let refs: Vec<&str> = entries.iter().filter_map(|p| p.to_str()).collect();
        let current = yerd_platform::user_path()
            .map_err(|e| format!("cannot read your PATH, refusing to modify it: {e}"))?
            .unwrap_or_default();
        let changed = if let Some(updated) = win_path_env::upsert_entries(&current, &refs) {
            yerd_platform::set_user_path(&updated).map_err(|e| e.to_string())?;
            true
        } else {
            false
        };
        if let Some(s) = shim_dir() {
            let _ = yerd_platform::broadcast_user_env_marker(&s);
        }
        Ok(changed)
    }

    /// Remove both dirs from the user PATH; returns only the dirs that were
    /// actually on it, since the caller prints each one back as removed.
    pub fn remove_from_path() -> Vec<PathBuf> {
        let entries = path_entries();
        let refs: Vec<&str> = entries.iter().filter_map(|p| p.to_str()).collect();
        let Ok(Some(current)) = yerd_platform::user_path() else {
            return Vec::new();
        };
        let Some(updated) = win_path_env::remove_entries(&current, &refs) else {
            return Vec::new();
        };
        if yerd_platform::set_user_path(&updated).is_err() {
            return Vec::new();
        }
        if let Some(s) = shim_dir() {
            let _ = yerd_platform::broadcast_user_env_marker(&s);
        }
        entries
            .into_iter()
            .filter(|p| {
                p.to_str()
                    .is_some_and(|s| win_path_env::contains_entry(&current, s))
            })
            .collect()
    }

    /// Copy `src` over `dest`, retrying a transient lock a few times.
    ///
    /// An antivirus scanner or the search indexer can hold a freshly-written
    /// file open for a few milliseconds. Retrying here means a hiccup does not
    /// burn a rename swap and leave an aside copy that cannot be cleared until
    /// the holder exits.
    fn copy_with_retry(src: &Path, dest: &Path) -> std::io::Result<()> {
        const MAX_ATTEMPTS: u64 = 3;
        let mut attempt = 1;
        loop {
            match std::fs::copy(src, dest) {
                Ok(_) => return Ok(()),
                Err(e)
                    if attempt < MAX_ATTEMPTS && super::is_sharing_violation(e.raw_os_error()) =>
                {
                    std::thread::sleep(std::time::Duration::from_millis(50 * attempt));
                    attempt += 1;
                }
                Err(e) => return Err(e),
            }
        }
    }

    /// Copy the running `yerd.exe` into the program dir. Skips when the source is
    /// already the destination or the bytes are current.
    ///
    /// A destination held open by another `yerd` process cannot be overwritten,
    /// so the replacement is staged beside it and promoted by renaming, which
    /// works on a running image. Any outcome that leaves the CLI unusable is
    /// reported with the filenames needed to put it right by hand.
    fn copy_self_into_programs() -> Result<(), String> {
        let Some(dir) = programs_bin() else {
            return Err("%LOCALAPPDATA% is not set; cannot locate the install dir".to_owned());
        };
        let _ = super::reconcile_staged_exe(&dir);
        let src = std::env::current_exe().map_err(|e| format!("cannot find yerd.exe: {e}"))?;
        let dest = dir.join(super::LIVE_EXE);
        if same_file(&src, &dest) || contents_match(&src, &dest) {
            return Ok(());
        }
        std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        match copy_with_retry(&src, &dest) {
            Ok(()) => Ok(()),
            Err(ref e) if super::is_sharing_violation(e.raw_os_error()) => {
                let staged = dir.join(super::NEW_EXE);
                copy_with_retry(&src, &staged).map_err(|e| format!("{}: {e}", staged.display()))?;
                match super::reconcile_staged_exe(&dir) {
                    super::SwapOutcome::Promoted | super::SwapOutcome::Recovered => Ok(()),
                    super::SwapOutcome::Blocked => Err(format!(
                        "{} is in use; the update is staged as {}. Close any other running yerd \
                         and re-run `yerd path install` to finish it",
                        dest.display(),
                        staged.display()
                    )),
                    super::SwapOutcome::Failed { restored: true } => Err(format!(
                        "the update could not be put in place and was rolled back; {} is unchanged",
                        dest.display()
                    )),
                    super::SwapOutcome::Failed { restored: false } => Err(format!(
                        "the update could not be put in place; renaming either {} or {} to {} \
                         restores the CLI",
                        dir.join(super::OLD_EXE).display(),
                        staged.display(),
                        super::LIVE_EXE
                    )),
                    super::SwapOutcome::Nothing => Err(format!(
                        "the staged update at {} disappeared before it could be applied",
                        staged.display()
                    )),
                }
            }
            Err(e) => Err(format!("{}: {e}", dest.display())),
        }
    }

    /// Best-effort deletion of the installed `yerd.exe` copy on uninstall. A
    /// running program can't delete its own image, so a failure is tolerated.
    fn delete_programs_copy() {
        if let Some(dir) = programs_bin() {
            let _ = std::fs::remove_file(dir.join(super::LIVE_EXE));
            let _ = std::fs::remove_file(dir.join(super::NEW_EXE));
            let _ = std::fs::remove_file(dir.join(super::OLD_EXE));
        }
    }

    fn same_file(a: &Path, b: &Path) -> bool {
        match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
            (Ok(a), Ok(b)) => a == b,
            _ => false,
        }
    }

    fn contents_match(a: &Path, b: &Path) -> bool {
        match (std::fs::read(a), std::fs::read(b)) {
            (Ok(a), Ok(b)) => a == b,
            _ => false,
        }
    }
}
