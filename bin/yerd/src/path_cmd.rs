//! `yerd path install|uninstall|print` - put yerd's shim dir on PATH so a bare
//! `php`/`composer` resolves to the managed shims.
//!
//! Local, daemon-free, unprivileged: it edits only state the user owns. On Unix
//! that is the yerd-owned block in the shell rc file(s) (pure string logic in
//! `yerd_platform::pure::shell_profile`); on Windows it is the user's
//! `HKCU\Environment\Path` (pure list editing in
//! `yerd_platform::pure::win_path_env`, registry I/O in `yerd_platform`), plus a
//! copy of `yerd.exe` into `%LOCALAPPDATA%\Programs\yerd\bin`.

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
    /// (the NSIS installer will use the same location in Phase 6). `None` when
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
    fn install(quiet: bool) -> ExitCode {
        if let Err(msg) = copy_self_into_programs() {
            eprintln!("yerd: {msg}");
        }
        match upsert_path() {
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
    fn upsert_path() -> Result<bool, String> {
        let entries = path_entries();
        let refs: Vec<&str> = entries.iter().filter_map(|p| p.to_str()).collect();
        // A read failure must abort, never fall back to an empty PATH: the edit
        // below is written back wholesale, so treating "couldn't read" as "no
        // entries" would replace the user's real PATH with just Yerd's dirs.
        // An absent value (`Ok(None)`) genuinely is an empty starting point.
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

    /// Remove both dirs from the user PATH; returns the dirs that were present.
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
    }

    /// Copy the running `yerd.exe` into the program dir. Skips when the source is
    /// already the destination or the bytes are current. A locked destination
    /// (`ERROR_SHARING_VIOLATION`, code 32) is staged beside it as `yerd.exe.new`
    /// with a note; the full staged-swap is Phase 6.
    fn copy_self_into_programs() -> Result<(), String> {
        let Some(dir) = programs_bin() else {
            return Err("%LOCALAPPDATA% is not set; cannot locate the install dir".to_owned());
        };
        finish_pending_swap(&dir);
        let src = std::env::current_exe().map_err(|e| format!("cannot find yerd.exe: {e}"))?;
        let dest = dir.join("yerd.exe");
        if same_file(&src, &dest) || contents_match(&src, &dest) {
            return Ok(());
        }
        std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        match std::fs::copy(&src, &dest) {
            Ok(_) => Ok(()),
            Err(e) if e.raw_os_error() == Some(32) => {
                let staged = dir.join("yerd.exe.new");
                std::fs::copy(&src, &staged).map_err(|e| format!("{}: {e}", staged.display()))?;
                Err(format!(
                    "{} is in use; staged the update as {} - restart yerd to finish",
                    dest.display(),
                    staged.display()
                ))
            }
            Err(e) => Err(format!("{}: {e}", dest.display())),
        }
    }

    /// Complete a swap staged by a prior locked [`copy_self_into_programs`]: if
    /// `yerd.exe.new` exists, move the live `yerd.exe` aside to `yerd.exe.old`
    /// and promote `.new` into place, then clear the stale `.old`. Best-effort
    /// and idempotent: no `.new` means nothing to do. Called at the start of any
    /// `path install` / `ensure_installed_after_tool` so the "restart yerd to
    /// finish" note from the locked-copy path actually resolves.
    fn finish_pending_swap(dir: &Path) {
        let new = dir.join("yerd.exe.new");
        if !new.exists() {
            return;
        }
        let live = dir.join("yerd.exe");
        let old = dir.join("yerd.exe.old");
        let _ = std::fs::remove_file(&old);
        if live.exists() && std::fs::rename(&live, &old).is_err() {
            return;
        }
        if std::fs::rename(&new, &live).is_ok() {
            let _ = std::fs::remove_file(&old);
        } else {
            let _ = std::fs::rename(&old, &live);
        }
    }

    /// Best-effort deletion of the installed `yerd.exe` copy on uninstall. A
    /// running program can't delete its own image, so a failure is tolerated.
    fn delete_programs_copy() {
        if let Some(dir) = programs_bin() {
            let _ = std::fs::remove_file(dir.join("yerd.exe"));
            let _ = std::fs::remove_file(dir.join("yerd.exe.new"));
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

    #[cfg(test)]
    #[allow(clippy::unwrap_used)]
    mod tests {
        use super::finish_pending_swap;

        #[test]
        fn finish_pending_swap_promotes_new_over_live() {
            let tmp = tempfile::tempdir().unwrap();
            let dir = tmp.path();
            std::fs::write(dir.join("yerd.exe"), b"old").unwrap();
            std::fs::write(dir.join("yerd.exe.new"), b"new").unwrap();

            finish_pending_swap(dir);

            assert_eq!(std::fs::read(dir.join("yerd.exe")).unwrap(), b"new");
            assert!(!dir.join("yerd.exe.new").exists());
            assert!(!dir.join("yerd.exe.old").exists());
        }

        #[test]
        fn finish_pending_swap_is_noop_without_new() {
            let tmp = tempfile::tempdir().unwrap();
            let dir = tmp.path();
            std::fs::write(dir.join("yerd.exe"), b"live").unwrap();
            finish_pending_swap(dir);
            assert_eq!(std::fs::read(dir.join("yerd.exe")).unwrap(), b"live");
        }

        #[test]
        fn finish_pending_swap_promotes_when_live_absent() {
            let tmp = tempfile::tempdir().unwrap();
            let dir = tmp.path();
            std::fs::write(dir.join("yerd.exe.new"), b"new").unwrap();
            finish_pending_swap(dir);
            assert_eq!(std::fs::read(dir.join("yerd.exe")).unwrap(), b"new");
            assert!(!dir.join("yerd.exe.new").exists());
        }
    }
}
