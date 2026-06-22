//! `yerd path install|uninstall|print` — manage the yerd-owned PATH block in the
//! user's shell rc file(s), so a bare `php`/`composer` resolves to `{data}/bin`.
//!
//! Local, daemon-free, unprivileged: it only edits files the user owns. The pure
//! string logic lives in `yerd_platform::pure::shell_profile`; this module is the
//! I/O edge — it reads `$SHELL`/`$HOME`, picks the rc file(s), reads/writes them
//! atomically (preserving dotfiles symlinks), and reports what changed.

use std::process::ExitCode;

use crate::cli::PathAction;

/// Run `yerd path <action>`: edit the user's shell rc file(s) to add/remove
/// yerd's bin dir on PATH, or print the snippet. Returns the process exit code.
#[cfg(unix)]
pub fn run(action: PathAction) -> ExitCode {
    unix::run(action)
}

/// Non-Unix stub: PATH management isn't wired for this platform yet.
#[cfg(not(unix))]
pub fn run(_action: PathAction) -> ExitCode {
    use yerd_platform::{ActivePaths, Paths};
    let hint = ActivePaths::new()
        .resolve()
        .map(|d| d.data.join("bin").display().to_string())
        .unwrap_or_else(|_| "yerd's bin directory".to_owned());
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

/// Non-Unix: no-op (PATH management isn't wired here yet; doctor warns instead).
#[cfg(not(unix))]
pub fn ensure_installed_after_tool(_quiet: bool) {}

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

        // `Print` doesn't need a real rc file — just emit the guarded block for
        // the detected (or POSIX-fallback) shell so the user can eval/paste it.
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
            // On uninstall, skip files that don't exist (nothing to remove).
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

    /// Add the PATH block after a tool install — idempotent and quiet. Does
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

    /// Edit one rc file. Returns `Ok(true)` if the file's contents changed.
    fn edit_one(rc: &Path, shell: Shell, bin_dir: &Path, install: bool) -> std::io::Result<bool> {
        // Resolve a dotfiles symlink to its real target so we write *through* it
        // (a plain rename would replace the symlink with a regular file).
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

        // One-time pristine backup, never overwriting an earlier one.
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
            // A broken (dangling-target) symlink can't be canonicalized — fall
            // back to `rc` itself, as documented, instead of aborting.
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

        // Monotonic per-call counter so two concurrent edits in the same process
        // (or a retry loop) can't collide on the same temp path.
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

        // Best-effort guard against a concurrent edit between the read we based
        // `contents` on and this write: if `dest` no longer matches `prev`,
        // someone else changed it under us — bail rather than clobber.
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
            // All edits failed (already reported above) — don't claim "nothing to do".
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
            .and_then(|p| p.file_name().map(|s| s.to_string_lossy().into_owned()))
            .unwrap_or_default()
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
}
