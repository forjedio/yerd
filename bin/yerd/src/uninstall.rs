//! `yerd uninstall` (no subcommand) - remove yerd entirely from this machine.
//!
//! Fully local and daemon-independent: the daemon is usually the first thing to
//! go, and may already be down. The flow is, in order:
//!
//! 1. Resolve the *invoking* user (under `sudo`, `$HOME`/`$SHELL` point at root,
//!    so the user is recovered from `SUDO_UID` + the passwd database).
//! 2. Capture the facts a manual/automatic unelevate needs (`tld`, CA
//!    fingerprint) from disk **before** anything is deleted - otherwise deleting
//!    the data dir would strand a trusted root CA with no way to identify it.
//! 3. Confirm (unless `--yes`).
//! 4. Root only: revert the system changes from `yerd elevate` (CA trust, DNS
//!    resolver, macOS pf redirect) by driving `yerd-helper` directly. Without
//!    root we print the exact manual steps and continue.
//! 5. Stop + disable the daemon service and reap the daemon (a graceful SIGTERM
//!    lets it reap its php-fpm / DB / mail children).
//! 6. Remove the PATH block, the service unit files, the user dirs, and the
//!    binaries (or advise `apt purge` for a `.deb` install).

use std::process::ExitCode;

/// Run the full self-uninstall. Returns the process exit code.
#[cfg(unix)]
pub fn run(yes: bool) -> ExitCode {
    unix_impl::run(yes)
}

/// Non-Unix: the daemon service, sudo model, and shell rc handling are all
/// Unix-shaped; mirror `elevate`/`path` and decline here.
#[cfg(not(unix))]
pub fn run(_yes: bool) -> ExitCode {
    eprintln!("yerd: `yerd uninstall` (full) is only supported on Unix (macOS/Linux)");
    ExitCode::from(78)
}

#[cfg(unix)]
mod unix_impl {
    use std::path::{Path, PathBuf};
    use std::process::{Command, ExitCode};

    use yerd_platform::{CaFingerprint, HelperInvocation, PlatformDirs};

    use crate::{elevate, path_cmd};

    /// The user yerd is being uninstalled for - the invoking user, even under
    /// sudo. All user-owned artefacts (dirs, rc files, `~/.local/bin`) live in
    /// `home`; service teardown runs in this user's session.
    struct Actor {
        uid: u32,
        name: String,
        home: PathBuf,
        shell: PathBuf,
    }

    /// Facts captured from disk before deletion, used to revert (or print the
    /// manual steps to revert) the system changes made by `yerd elevate`.
    struct CapturedFacts {
        tld: Option<String>,
        ca_fp: Option<CaFingerprint>,
    }

    impl CapturedFacts {
        fn capture(dirs: &PlatformDirs) -> Self {
            let tld = yerd_config::Config::load(&dirs.config.join("yerd.toml"))
                .ok()
                .map(|c| c.tld.as_str().to_owned());
            let ca_fp = std::fs::read_to_string(dirs.data.join("ca.cert.pem"))
                .ok()
                .and_then(|pem| CaFingerprint::from_pem(&pem));
            Self { tld, ca_fp }
        }
    }

    pub fn run(yes: bool) -> ExitCode {
        let actor = match resolve_actor() {
            Ok(a) => a,
            Err(e) => {
                eprintln!("yerd: {e}");
                return ExitCode::from(74);
            }
        };
        let dirs = PlatformDirs::for_user(&actor.home, actor.uid);
        let root = elevate::is_root();

        let facts = CapturedFacts::capture(&dirs);

        print_header(&actor, &dirs, root);
        if !root {
            print_unelevate_warning(&facts);
        }

        if !yes && !confirm() {
            println!("yerd: aborted — nothing was changed.");
            return ExitCode::from(1);
        }

        let mut residue: Vec<String> = Vec::new();

        if root {
            revert_system_changes(&facts, &mut residue);
        } else {
            residue.push(
                "system changes from `yerd elevate` (CA trust, DNS resolver, ports) — \
                 see the manual steps printed above"
                    .to_owned(),
            );
        }

        stop_daemon_service(&actor, root);
        reap_daemon(actor.uid);

        let touched = path_cmd::remove_block_for_user(&actor.home, &shell_basename(&actor.shell));
        for f in &touched {
            println!("  removed PATH entry from {}", f.display());
        }

        remove_service_unit(&actor);

        for dir in dirs_to_delete(&dirs, actor.uid) {
            match std::fs::remove_dir_all(&dir) {
                Ok(()) => println!("  removed {}", dir.display()),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => residue.push(format!("{} ({e})", dir.display())),
            }
        }

        remove_binaries(&actor, &mut residue);

        print_summary(&residue);
        ExitCode::SUCCESS
    }

    /// Resolve the invoking user. Under sudo, `SUDO_UID` + the passwd database
    /// give the real user (the process env points at root). The uid comes from
    /// `nix` (no `unsafe`; `bin/yerd` is `#![forbid(unsafe_code)]`).
    fn resolve_actor() -> Result<Actor, String> {
        use nix::unistd::{Uid, User};

        let sudo = elevate::sudo_uid();
        let uid = sudo.unwrap_or_else(|| Uid::current().as_raw());

        if let Ok(Some(u)) = User::from_uid(Uid::from_raw(uid)) {
            return Ok(Actor {
                uid,
                name: u.name,
                home: u.dir,
                shell: u.shell,
            });
        }

        if sudo.is_some() {
            return Err(format!(
                "cannot resolve the invoking user (uid {uid}) — no passwd entry"
            ));
        }
        let home = std::env::var_os("HOME")
            .filter(|h| !h.is_empty())
            .map(PathBuf::from)
            .ok_or_else(|| "cannot resolve your home directory ($HOME is unset)".to_owned())?;
        let name = std::env::var("USER").unwrap_or_default();
        let shell =
            std::env::var_os("SHELL").map_or_else(|| PathBuf::from("/bin/sh"), PathBuf::from);
        Ok(Actor {
            uid,
            name,
            home,
            shell,
        })
    }

    /// Revert the privileged system changes by driving `yerd-helper` directly
    /// from the captured on-disk facts - no daemon needed. Best-effort: every
    /// failure is recorded in `residue`, never fatal.
    fn revert_system_changes(facts: &CapturedFacts, residue: &mut Vec<String>) {
        let helper = match elevate::sibling_binaries() {
            Ok((helper, _yerdd)) => helper,
            Err(e) => {
                residue.push(format!(
                    "could not locate yerd-helper to revert system changes: {e}"
                ));
                return;
            }
        };

        match facts.ca_fp {
            Some(fp) => run_helper(
                &helper,
                &HelperInvocation::UninstallCa { fp },
                "remove the CA from the system trust store",
                residue,
            ),
            None => residue.push(format!(
                "a yerd CA may remain in the system trust store (its cert was \
                 not on disk to identify it) — remove it manually: {}",
                manual_ca_removal_hint()
            )),
        }

        if let Some(tld) = &facts.tld {
            run_helper(
                &helper,
                &HelperInvocation::UninstallResolver { tld: tld.clone() },
                "remove the DNS resolver entry",
                residue,
            );
        }

        #[cfg(target_os = "macos")]
        run_helper(
            &helper,
            &HelperInvocation::UninstallPortRedirect,
            "remove the pf port redirect",
            residue,
        );
    }

    /// Spawn the helper for one operation and classify the outcome.
    fn run_helper(helper: &Path, inv: &HelperInvocation, what: &str, residue: &mut Vec<String>) {
        print!("==> {what} … ");
        let _ = std::io::Write::flush(&mut std::io::stdout());
        match elevate::spawn_helper(helper, inv) {
            Ok(Some(0)) => println!("ok"),
            Ok(Some(78)) => println!("skipped (not configured)"),
            Ok(Some(code)) => {
                println!("failed (exit {code})");
                residue.push(format!("{what} (yerd-helper exit {code})"));
            }
            Ok(None) => {
                println!("failed (terminated by signal)");
                residue.push(format!("{what} (yerd-helper was killed)"));
            }
            Err(e) => {
                println!("failed");
                residue.push(format!("{what} ({e})"));
            }
        }
    }

    /// Stop and disable the per-user daemon service, in the invoking user's
    /// session. Best-effort - the service may not be installed.
    fn stop_daemon_service(actor: &Actor, root: bool) {
        #[cfg(target_os = "linux")]
        {
            if root && elevate::sudo_uid().is_some() {
                let xdg = format!("XDG_RUNTIME_DIR=/run/user/{}", actor.uid);
                let dbus = format!(
                    "DBUS_SESSION_BUS_ADDRESS=unix:path=/run/user/{}/bus",
                    actor.uid
                );
                let _ = run_quiet(
                    "runuser",
                    &[
                        "-u",
                        &actor.name,
                        "--",
                        "env",
                        &xdg,
                        &dbus,
                        "systemctl",
                        "--user",
                        "disable",
                        "--now",
                        "yerd",
                    ],
                );
            } else {
                let _ = run_quiet("systemctl", &["--user", "disable", "--now", "yerd"]);
            }
        }
        #[cfg(target_os = "macos")]
        {
            let _ = root;
            let target = format!("gui/{}/dev.yerd.daemon", actor.uid);
            let _ = run_quiet("launchctl", &["bootout", &target]);
        }
        #[cfg(not(any(target_os = "linux", target_os = "macos")))]
        {
            let _ = (actor, root);
        }
    }

    /// Reap any still-running `yerdd` for this user: SIGTERM (graceful - it
    /// reaps its php-fpm/DB/mail children), wait a bounded grace, then SIGKILL
    /// any holdout. `pgrep`/`pkill -U <uid>` match by *real* uid on both
    /// Linux and macOS; `-x yerdd` matches the exact process name (so it never
    /// touches the running `yerd` uninstaller or `yerd-helper`).
    ///
    /// The grace is deliberately generous (~30s). The daemon's own shutdown
    /// (`bin/yerdd/src/lib.rs`) walks several task-join timeouts before it
    /// gracefully stops (then force-kills) its php-fpm/DB/mail children; killing
    /// the daemon too early would skip that and orphan those children. In
    /// practice it exits in well under a second - we only wait the full budget
    /// if one of its tasks is wedged.
    fn reap_daemon(uid: u32) {
        let uid = uid.to_string();
        let _ = run_quiet("pkill", &["-TERM", "-U", &uid, "-x", "yerdd"]);
        for _ in 0..300 {
            if !yerdd_running(&uid) {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        let _ = run_quiet("pkill", &["-KILL", "-U", &uid, "-x", "yerdd"]);
    }

    fn yerdd_running(uid: &str) -> bool {
        Command::new("pgrep")
            .args(["-U", uid, "-x", "yerdd"])
            .output()
            .is_ok_and(|o| o.status.success())
    }

    /// Remove the daemon's service unit file (best-effort).
    fn remove_service_unit(actor: &Actor) {
        #[cfg(target_os = "linux")]
        let unit = actor.home.join(".config/systemd/user/yerd.service");
        #[cfg(target_os = "macos")]
        let unit = actor
            .home
            .join("Library/LaunchAgents/dev.yerd.daemon.plist");
        #[cfg(not(any(target_os = "linux", target_os = "macos")))]
        let unit: PathBuf = {
            let _ = actor;
            return;
        };

        #[cfg(any(target_os = "linux", target_os = "macos"))]
        if std::fs::remove_file(&unit).is_ok() {
            println!("  removed {}", unit.display());
        }
    }

    /// Remove the yerd binaries across the candidate dirs (the running exe's
    /// dir - `current_exe` resolves symlinks, so this is the real install dir -
    /// plus `~/.local/bin` and the system dirs). A symlink at a candidate path
    /// is unlinked (its target is cleaned only if that target also lands in a
    /// candidate dir); a real file in a package-managed system path is left for
    /// `apt purge`; anything else is deleted. `yerd` (this running binary) is
    /// removed last - unlinking a running executable is safe on Unix.
    fn remove_binaries(actor: &Actor, residue: &mut Vec<String>) {
        let mut dirs: Vec<PathBuf> = Vec::new();
        if let Ok(exe) = std::env::current_exe() {
            if let Some(d) = exe.parent() {
                dirs.push(d.to_path_buf());
            }
        }
        dirs.push(actor.home.join(".local").join("bin"));
        dirs.push(PathBuf::from("/usr/local/bin"));
        dirs.push(PathBuf::from("/usr/bin"));
        let dirs = dedup(dirs);

        let mut packaged = false;
        for name in ["yerdd", "yerd-helper", "yerd"] {
            for dir in &dirs {
                let path = dir.join(name);
                let Ok(md) = std::fs::symlink_metadata(&path) else {
                    continue;
                };
                if md.file_type().is_symlink() {
                    if std::fs::remove_file(&path).is_ok() {
                        println!("  removed symlink {}", path.display());
                    }
                    continue;
                }
                if is_system_install_dir(dir) {
                    packaged = true;
                    continue;
                }
                match std::fs::remove_file(&path) {
                    Ok(()) => println!("  removed {}", path.display()),
                    Err(e) => residue.push(format!("{} ({e})", path.display())),
                }
            }
        }
        if packaged {
            #[cfg(target_os = "linux")]
            residue.push(
                "system-installed binaries (a .deb install) — remove with: sudo apt purge yerd"
                    .to_owned(),
            );
            #[cfg(not(target_os = "linux"))]
            residue.push(
                "binaries in a system path — remove them with your package manager".to_owned(),
            );
        }
    }

    // ── pure helpers (unit-tested) ───────────────────────────────────────────

    /// The user dirs to delete, de-duplicated. On macOS config/data/state all
    /// coincide; both Linux runtime candidates are included since the active one
    /// can't be recovered from a stripped sudo env.
    fn dirs_to_delete(dirs: &PlatformDirs, uid: u32) -> Vec<PathBuf> {
        dedup(vec![
            dirs.config.clone(),
            dirs.data.clone(),
            dirs.state.clone(),
            dirs.cache.clone(),
            dirs.runtime.clone(),
            PathBuf::from(format!("/run/user/{uid}/yerd")),
            PathBuf::from(format!("/tmp/yerd-{uid}")),
        ])
    }

    /// A package-managed location whose binaries dpkg owns - advise removal via
    /// the package manager rather than `rm`. `/usr/local/bin` is deliberately
    /// excluded: Debian Policy reserves `/usr/local` for the local admin and no
    /// package may own files there, so a yerd binary in `/usr/local/bin` is a
    /// manual install we should unlink like any other.
    fn is_system_install_dir(dir: &Path) -> bool {
        matches!(
            dir.to_str(),
            Some("/usr/bin" | "/usr/sbin" | "/bin" | "/sbin")
        )
    }

    /// The shell's basename (e.g. `/bin/zsh` → `zsh`).
    fn shell_basename(shell: &Path) -> String {
        shell
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default()
    }

    fn dedup(paths: Vec<PathBuf>) -> Vec<PathBuf> {
        let mut seen = std::collections::HashSet::new();
        paths
            .into_iter()
            .filter(|p| seen.insert(p.clone()))
            .collect()
    }

    fn run_quiet(program: &str, args: &[&str]) -> bool {
        Command::new(program)
            .args(args)
            .output()
            .is_ok_and(|o| o.status.success())
    }

    // ── interaction / output ─────────────────────────────────────────────────

    fn confirm() -> bool {
        use std::io::{IsTerminal, Write};
        if !std::io::stdin().is_terminal() {
            eprintln!(
                "yerd: refusing to uninstall without confirmation — \
                 re-run with --yes to proceed non-interactively."
            );
            return false;
        }
        print!("\nProceed with uninstalling yerd? Type 'yes' to confirm: ");
        let _ = std::io::stdout().flush();
        let mut line = String::new();
        if std::io::stdin().read_line(&mut line).is_err() {
            return false;
        }
        matches!(line.trim(), "y" | "Y" | "yes" | "Yes" | "YES")
    }

    fn print_header(actor: &Actor, dirs: &PlatformDirs, root: bool) {
        println!(
            "This will uninstall yerd for user '{}'. It removes:",
            actor.name
        );
        println!("  • config:  {}", dirs.config.display());
        println!(
            "  • data:    {}  (certs, installed PHP versions, tools, downloads)",
            dirs.data.display()
        );
        println!("  • cache:   {}", dirs.cache.display());
        println!("  • the yerd PATH entry from your shell startup files");
        println!("  • the yerd daemon service and the yerd / yerdd / yerd-helper binaries");
        if root {
            println!("  • system changes from `yerd elevate` (CA trust, DNS resolver, ports)");
        }
    }

    /// Per-OS one-liner for manually removing yerd's CA from the system trust
    /// store. Shared by the not-root warning and the root-path residue note.
    fn manual_ca_removal_hint() -> &'static str {
        #[cfg(target_os = "macos")]
        {
            "open Keychain Access → System and delete the 'Yerd Local CA' certificate"
        }
        #[cfg(not(target_os = "macos"))]
        {
            "remove yerd's CA from your trust anchors, then `sudo update-ca-certificates \
             --fresh` (or `update-ca-trust`)"
        }
    }

    fn print_unelevate_warning(facts: &CapturedFacts) {
        let tld = facts.tld.as_deref().unwrap_or("<your-tld>");
        eprintln!();
        eprintln!("WARNING: not running as root (sudo).");
        eprintln!("  The system-level changes from `yerd elevate` need root to revert, and they");
        eprintln!("  CANNOT be reverted after yerd is uninstalled (the binary will be gone).");
        eprintln!(
            "  They will be left in place. Either abort and re-run as `sudo yerd uninstall`,"
        );
        eprintln!("  or remove them manually afterwards:");
        eprintln!("    • CA trust: {}", manual_ca_removal_hint());
        if let Some(fp) = facts.ca_fp {
            eprintln!(
                "                (yerd CA SHA-256 fingerprint: {})",
                fp.to_hex()
            );
        }
        #[cfg(target_os = "macos")]
        {
            eprintln!("    • resolver: sudo rm /etc/resolver/{tld}");
            eprintln!(
                "    • ports:    sudo launchctl bootout system/dev.yerd.pf 2>/dev/null; \
                 sudo rm -f /Library/LaunchDaemons/dev.yerd.pf.plist"
            );
        }
        #[cfg(not(target_os = "macos"))]
        {
            eprintln!(
                "    • resolver: sudo rm /etc/systemd/resolved.conf.d/yerd-{tld}.conf && \
                 sudo systemctl restart systemd-resolved"
            );
            eprintln!(
                "    • ports:    the setcap grant is dropped automatically when yerdd is deleted"
            );
        }
    }

    fn print_summary(residue: &[String]) {
        println!();
        if residue.is_empty() {
            println!("yerd has been uninstalled.");
        } else {
            println!("yerd has been uninstalled, with some leftovers to handle manually:");
            for r in residue {
                println!("  • {r}");
            }
        }
        println!("Open a new terminal so the PATH change takes effect.");
    }

    #[cfg(test)]
    #[allow(clippy::unwrap_used, clippy::panic, clippy::indexing_slicing)]
    mod tests {
        use super::*;

        #[test]
        fn shell_basename_takes_file_name() {
            assert_eq!(shell_basename(Path::new("/bin/zsh")), "zsh");
            assert_eq!(shell_basename(Path::new("/usr/bin/fish")), "fish");
            assert_eq!(shell_basename(Path::new("")), "");
        }

        #[test]
        fn system_dirs_are_recognised() {
            assert!(is_system_install_dir(Path::new("/usr/bin")));
            assert!(!is_system_install_dir(Path::new("/home/u/.local/bin")));
            assert!(!is_system_install_dir(Path::new("/usr/local/bin")));
        }

        #[test]
        fn dedup_preserves_order_and_drops_repeats() {
            let v = dedup(vec![
                PathBuf::from("/a"),
                PathBuf::from("/b"),
                PathBuf::from("/a"),
                PathBuf::from("/c"),
            ]);
            assert_eq!(
                v,
                vec![
                    PathBuf::from("/a"),
                    PathBuf::from("/b"),
                    PathBuf::from("/c")
                ]
            );
        }

        #[test]
        fn dirs_to_delete_dedups_and_includes_both_runtime_candidates() {
            let dirs = PlatformDirs::for_user(Path::new("/home/u"), 1000);
            let out = dirs_to_delete(&dirs, 1000);
            let unique: std::collections::HashSet<_> = out.iter().collect();
            assert_eq!(unique.len(), out.len());
            assert!(out.contains(&PathBuf::from("/run/user/1000/yerd")));
            assert!(out.contains(&PathBuf::from("/tmp/yerd-1000")));
            assert!(out.contains(&dirs.config));
            assert!(out.contains(&dirs.data));
            assert!(out.contains(&dirs.cache));
        }
    }
}
