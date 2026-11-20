//! `yerd uninstall` (no subcommand) - remove yerd entirely from this machine.
//!
//! Fully local and daemon-independent: the daemon is usually the first thing to
//! go, and may already be down. The flow is, in order:
//!
//! 1. Resolve the *invoking* user (under `sudo`, `$HOME`/`$SHELL` point at root,
//!    so the user is recovered from `SUDO_UID` + the passwd database). On macOS,
//!    refuse without root when a headless system launchd daemon is registered:
//!    only root can remove it, and launchd would restart a killed `yerdd`.
//! 2. Capture the facts a manual/automatic unelevate needs (`tld`, CA
//!    fingerprint) from disk **before** anything is deleted - otherwise deleting
//!    the data dir would strand a trusted root CA with no way to identify it.
//! 3. Confirm (unless `--yes`).
//! 4. Root only: revert the system changes from `yerd elevate` (CA trust, DNS
//!    resolver, macOS port forwarder) by driving `yerd-helper` directly. Without
//!    root we print the exact manual steps and continue.
//! 5. Stop + disable the daemon service and reap the daemon (a graceful SIGTERM
//!    lets it reap its php-fpm / DB / mail children).
//! 6. Remove the PATH block, the service unit files, the user dirs, and the
//!    binaries (or advise `apt purge` for a `.deb` install). A headless install
//!    also loses its install receipt and, once empty, its install dir; on macOS
//!    its system launchd daemon is removed by the helper.

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
    use std::ffi::OsStr;
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

        if let Some(refusal) = system_job_refusal(root, system_daemon_job_registered()) {
            eprintln!("yerd: {refusal}");
            return ExitCode::from(77);
        }

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
        let headless = matches!(
            yerd_platform::install_mode::detect(),
            Ok(yerd_platform::pure::install_receipt::InstallMode::Headless)
        );

        if root {
            if let Err(e) = remove_daemon_job() {
                eprintln!(
                    "yerd: could not remove the headless daemon job: {e}. Nothing else was \
                     removed; fix that and re-run `yerd uninstall`"
                );
                return ExitCode::from(1);
            }
            revert_system_changes(&facts, &mut residue);
        } else {
            residue.push(
                "system changes from `yerd elevate` (CA trust, DNS resolver, ports) — \
                 see the manual steps printed above"
                    .to_owned(),
            );
        }

        let unit_fragment = stop_daemon_service(&actor, root, &mut residue);
        reap_daemon(actor.uid);

        let touched = path_cmd::remove_block_for_user(&actor.home, &shell_basename(&actor.shell));
        for f in &touched {
            println!("  removed PATH entry from {}", f.display());
        }

        remove_service_unit(&actor, root, unit_fragment.as_deref(), &mut residue);

        for dir in dirs_to_delete(&dirs, actor.uid) {
            match std::fs::remove_dir_all(&dir) {
                Ok(()) => println!("  removed {}", dir.display()),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => residue.push(format!("{} ({e})", dir.display())),
            }
        }

        remove_binaries(&actor, &mut residue);

        if headless {
            print_linger_note(&actor);
        }
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
            "remove the port forwarder",
            residue,
        );

        #[cfg(target_os = "macos")]
        run_helper(
            &helper,
            &HelperInvocation::UninstallLanPortRedirect,
            "remove the LAN port forwarder",
            residue,
        );
    }

    /// Remove a headless install's system launchd daemon (macOS, root). Runs
    /// before anything else is deleted and is fatal on failure: deleting the
    /// daemon's files (and the helper that can remove the job) while launchd
    /// keeps the job would leave a registration nothing can clean up.
    #[cfg_attr(not(target_os = "macos"), allow(clippy::unnecessary_wraps))]
    fn remove_daemon_job() -> Result<(), String> {
        #[cfg(target_os = "macos")]
        {
            if !system_daemon_job_registered() {
                return Ok(());
            }
            let (helper, _yerdd) = elevate::sibling_binaries().map_err(|e| e.to_string())?;
            print!("==> remove the headless daemon job … ");
            let _ = std::io::Write::flush(&mut std::io::stdout());
            match elevate::spawn_helper(&helper, &HelperInvocation::UninstallDaemonJob) {
                Ok(Some(0)) => {
                    println!("ok");
                    Ok(())
                }
                Ok(Some(code)) => {
                    println!("failed");
                    Err(format!("yerd-helper exited {code}"))
                }
                Ok(None) => {
                    println!("failed");
                    Err("yerd-helper was killed by a signal".to_owned())
                }
                Err(e) => {
                    println!("failed");
                    Err(e.to_string())
                }
            }
        }
        #[cfg(not(target_os = "macos"))]
        {
            Ok(())
        }
    }

    /// Whether a headless install's system launchd daemon is registered
    /// (macOS): its plist exists, or the job is loaded even though its plist
    /// is gone.
    fn system_daemon_job_registered() -> bool {
        #[cfg(target_os = "macos")]
        {
            use yerd_platform::pure::launchd_daemon::{DAEMON_LABEL, DAEMON_PLIST};
            Path::new(DAEMON_PLIST).exists()
                || run_quiet("launchctl", &["print", &format!("system/{DAEMON_LABEL}")])
        }
        #[cfg(not(target_os = "macos"))]
        {
            false
        }
    }

    /// Without root, a registered system launchd daemon can't be removed, and
    /// launchd restarts a killed `yerdd` (`KeepAlive` on unclean exit), so
    /// refuse before anything is deleted.
    fn system_job_refusal(root: bool, system_job: bool) -> Option<String> {
        (system_job && !root).then(|| {
            let yerd = std::env::current_exe()
                .map_or_else(|_| PathBuf::from("yerd"), |exe| exe.with_file_name("yerd"));
            system_job_refusal_message(&yerd)
        })
    }

    fn system_job_refusal_message(yerd: &Path) -> String {
        format!(
            "the daemon is registered as a system launchd job; run `sudo {} uninstall`",
            yerd.display()
        )
    }

    /// Linux headless installs enabled linger for the daemon; uninstall leaves
    /// it, since other user services may rely on it.
    fn print_linger_note(actor: &Actor) {
        #[cfg(target_os = "linux")]
        println!(
            "  note: linger was left enabled for {0}; turn it off with \
             `sudo loginctl disable-linger {0}` if nothing else needs it",
            actor.name
        );
        #[cfg(not(target_os = "linux"))]
        let _ = actor;
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
    /// session. On Linux, returns the unit file systemd loaded `yerd` from
    /// (asked before disabling it), so cleanup finds a unit under an
    /// `$XDG_CONFIG_HOME` that sudo's environment no longer carries. A failed
    /// stop is residue, unless systemd confirmed there is no `yerd` unit.
    fn stop_daemon_service(
        actor: &Actor,
        root: bool,
        residue: &mut Vec<String>,
    ) -> Option<PathBuf> {
        #[cfg(target_os = "linux")]
        {
            let show = user_systemctl(
                actor,
                root,
                &["show", "yerd", "-p", "FragmentPath", "--value"],
            );
            let fragment = show
                .as_ref()
                .ok()
                .map(|out| PathBuf::from(out.trim()))
                .filter(|p| !p.as_os_str().is_empty());
            let confirmed_absent = show.as_ref().is_ok_and(|out| out.trim().is_empty());
            if let Err(e) = user_systemctl(actor, root, &["disable", "--now", "yerd"]) {
                if !confirmed_absent {
                    residue.push(format!(
                        "stopping and disabling the yerd user service ({e})"
                    ));
                }
            }
            fragment
        }
        #[cfg(target_os = "macos")]
        {
            let _ = (root, residue);
            let target = format!("gui/{}/dev.yerd.daemon", actor.uid);
            let _ = run_quiet("launchctl", &["bootout", &target]);
            None
        }
        #[cfg(not(any(target_os = "linux", target_os = "macos")))]
        {
            let _ = (actor, root, residue);
            None
        }
    }

    /// Whether this is a sudo run, where user-owned files are changed with the
    /// invoking user's authority rather than root's.
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    fn as_sudo(root: bool) -> bool {
        root && elevate::sudo_uid().is_some()
    }

    /// Run `systemctl --user <args>` in the invoking user's manager (through
    /// `runuser` under sudo). Returns stdout on success, or why it failed.
    #[cfg(target_os = "linux")]
    fn user_systemctl(actor: &Actor, root: bool, args: &[&str]) -> Result<String, String> {
        let mut cmd = if as_sudo(root) {
            let mut cmd = Command::new("runuser");
            cmd.args(["-u", &actor.name, "--", "env"])
                .arg(format!("XDG_RUNTIME_DIR=/run/user/{}", actor.uid))
                .arg(format!(
                    "DBUS_SESSION_BUS_ADDRESS=unix:path=/run/user/{}/bus",
                    actor.uid
                ))
                .arg("systemctl");
            cmd
        } else {
            Command::new("systemctl")
        };
        let out = cmd
            .arg("--user")
            .args(args)
            .output()
            .map_err(|e| format!("running systemctl: {e}"))?;
        if out.status.success() {
            Ok(String::from_utf8_lossy(&out.stdout).into_owned())
        } else {
            Err(String::from_utf8_lossy(&out.stderr).trim().to_owned())
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

    /// Remove the daemon's service unit file. `fragment` is where systemd
    /// said it loaded the unit from (Linux). Under sudo, Linux unit files are
    /// deleted as the invoking user (through `runuser`), never as root, so a
    /// path in a user-writable dir can't redirect a root delete. A unit that
    /// exists but can't be removed is reported as residue.
    fn remove_service_unit(
        actor: &Actor,
        root: bool,
        fragment: Option<&Path>,
        residue: &mut Vec<String>,
    ) {
        #[cfg(target_os = "linux")]
        let units = {
            let mut units: Vec<PathBuf> = fragment
                .filter(|p| is_user_unit(p))
                .map(Path::to_path_buf)
                .into_iter()
                .collect();
            units.extend(linux_unit_paths(
                &actor.home,
                std::env::var_os("XDG_CONFIG_HOME").as_deref(),
                elevate::sudo_uid().is_some(),
            ));
            dedup(units)
        };
        #[cfg(target_os = "macos")]
        let units = {
            let _ = fragment;
            vec![actor
                .home
                .join("Library/LaunchAgents/dev.yerd.daemon.plist")]
        };
        #[cfg(not(any(target_os = "linux", target_os = "macos")))]
        let units: Vec<PathBuf> = {
            let _ = (actor, fragment);
            Vec::new()
        };

        for unit in units {
            if std::fs::symlink_metadata(&unit).is_err() {
                continue;
            }
            match remove_user_file(actor, root, &unit) {
                Ok(()) => println!("  removed {}", unit.display()),
                Err(e) => residue.push(format!("{} ({e})", unit.display())),
            }
        }
    }

    /// Delete a user-owned file: as the invoking user through `runuser` under
    /// sudo on Linux, otherwise directly.
    fn remove_user_file(actor: &Actor, root: bool, path: &Path) -> Result<(), String> {
        #[cfg(target_os = "linux")]
        if as_sudo(root) {
            let out = Command::new("runuser")
                .args(["-u", &actor.name, "--", "rm", "--"])
                .arg(path)
                .output()
                .map_err(|e| format!("running runuser: {e}"))?;
            return if out.status.success() {
                Ok(())
            } else {
                Err(String::from_utf8_lossy(&out.stderr).trim().to_owned())
            };
        }
        let _ = (actor, root);
        std::fs::remove_file(path).map_err(|e| e.to_string())
    }

    /// Whether `path` is a `yerd.service` in a user unit dir (`…/systemd/user`)
    /// outside the system's own unit trees, so a path reported by systemd is
    /// only deleted when it is a user's unit file.
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    fn is_user_unit(path: &Path) -> bool {
        const SYSTEM_TREES: [&str; 4] = ["/etc", "/usr", "/lib", "/run"];
        path.is_absolute()
            && path.file_name() == Some(OsStr::new("yerd.service"))
            && path
                .parent()
                .is_some_and(|dir| dir.ends_with(Path::new("systemd").join("user")))
            && !SYSTEM_TREES.iter().any(|tree| path.starts_with(tree))
            && path.components().all(|c| {
                matches!(
                    c,
                    std::path::Component::RootDir | std::path::Component::Normal(_)
                )
            })
            && path.components().all(|c| {
                matches!(
                    c,
                    std::path::Component::RootDir | std::path::Component::Normal(_)
                )
            })
    }

    /// Where a `yerd.service` user unit may have been written: under
    /// `$XDG_CONFIG_HOME` (both the desktop app and `yerd daemon install` honour
    /// it) and under `~/.config`, deduplicated. Under sudo the environment may
    /// be root's, so `$XDG_CONFIG_HOME` is only trusted inside the user's home.
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    fn linux_unit_paths(home: &Path, xdg_config: Option<&OsStr>, sudo: bool) -> Vec<PathBuf> {
        let unit = Path::new("systemd").join("user").join("yerd.service");
        let xdg = xdg_config
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
            .filter(|p| p.is_absolute() && (!sudo || p.starts_with(home)));
        let mut paths: Vec<PathBuf> = xdg.into_iter().map(|base| base.join(&unit)).collect();
        paths.push(home.join(".config").join(&unit));
        dedup(paths)
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
        let exe_dir = std::env::current_exe()
            .ok()
            .and_then(|exe| exe.parent().map(Path::to_path_buf));
        let headless_dir = yerd_platform::install_mode::current_exe_dir().filter(|d| {
            d.join(yerd_platform::pure::install_receipt::RECEIPT_FILE)
                .is_file()
        });
        if let Some(d) = exe_dir {
            dirs.push(d);
        }
        if let Some(d) = &headless_dir {
            dirs.push(d.clone());
        }
        dirs.push(actor.home.join(".local").join("bin"));
        dirs.push(PathBuf::from("/usr/local/bin"));
        dirs.push(PathBuf::from("/usr/bin"));
        let dirs = dedup(dirs);

        let mut packaged = false;
        for name in INSTALLED_FILES {
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
        if let Some(dir) = headless_dir {
            remove_headless_extras(&dir, residue);
            if std::fs::remove_dir(&dir).is_ok() {
                println!("  removed {}", dir.display());
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

    /// Remove the `yerd-` prefixed regular files a newer headless release added
    /// to a receipt-marked install dir (and any leftover swap backups), so the
    /// dir can be removed. Files without the prefix are never touched, since
    /// `YERD_INSTALL_DIR` may point at a dir shared with the user's own files.
    fn remove_headless_extras(dir: &Path, residue: &mut Vec<String>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        let names: Vec<String> = entries
            .filter_map(Result::ok)
            .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
            .filter_map(|e| e.file_name().into_string().ok())
            .collect();
        for name in headless_extras(&names) {
            let path = dir.join(&name);
            match std::fs::remove_file(&path) {
                Ok(()) => println!("  removed {}", path.display()),
                Err(e) => residue.push(format!("{} ({e})", path.display())),
            }
        }
    }

    // ── pure helpers (unit-tested) ───────────────────────────────────────────

    /// The names in a headless install dir that uninstall removes beyond
    /// [`INSTALLED_FILES`]: anything carrying the extra-file prefix the
    /// applier enforces (`crate::apply::EXTRA_PREFIX`).
    fn headless_extras(names: &[String]) -> Vec<String> {
        names
            .iter()
            .filter(|n| n.starts_with(crate::apply::EXTRA_PREFIX))
            .filter(|n| !INSTALLED_FILES.contains(&n.as_str()))
            .cloned()
            .collect()
    }

    /// Files an install puts beside the binaries, removal order: `yerd` (this
    /// running binary) last.
    const INSTALLED_FILES: [&str; 5] = [
        "yerdd",
        "yerd-helper",
        "yerd-forward",
        yerd_platform::pure::install_receipt::RECEIPT_FILE,
        "yerd",
    ];

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
        println!(
            "  • the yerd daemon service and the yerd / yerdd / yerd-helper / yerd-forward binaries"
        );
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
            for label in [
                yerd_platform::pure::launchd_ports::PORTS_HTTP_LABEL,
                yerd_platform::pure::launchd_ports::PORTS_HTTPS_LABEL,
                yerd_platform::pure::launchd_ports::LAN_PORTS_HTTP_LABEL,
                yerd_platform::pure::launchd_ports::LAN_PORTS_HTTPS_LABEL,
            ] {
                eprintln!(
                    "    • ports:    sudo launchctl bootout system/{label} 2>/dev/null; \
                     sudo rm -f /Library/LaunchDaemons/{label}.plist"
                );
            }
            eprintln!(
                "    • legacy:   sudo launchctl bootout system/dev.yerd.pf 2>/dev/null; \
                 sudo launchctl bootout system/dev.yerd.lan.pf 2>/dev/null; \
                 sudo rm -f /Library/LaunchDaemons/dev.yerd.pf.plist \
                 /Library/LaunchDaemons/dev.yerd.lan.pf.plist"
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
        fn installed_files_include_the_receipt_and_end_with_yerd() {
            assert!(INSTALLED_FILES.contains(&"yerd-install-receipt"));
            for bin in ["yerdd", "yerd-helper", "yerd-forward"] {
                assert!(INSTALLED_FILES.contains(&bin), "{bin}");
            }
            assert_eq!(INSTALLED_FILES.last(), Some(&"yerd"));
        }

        #[test]
        fn headless_extras_takes_only_prefixed_non_core_files() {
            let names: Vec<String> = [
                "yerd",
                "yerdd",
                "yerd-helper",
                "yerd-install-receipt",
                "yerd-mcp",
                "yerd-helper.yerd-old-42",
                "README.md",
                "my-script",
                "yerdx",
            ]
            .iter()
            .map(|s| (*s).to_owned())
            .collect();
            assert_eq!(
                headless_extras(&names),
                vec!["yerd-mcp".to_owned(), "yerd-helper.yerd-old-42".to_owned()]
            );
        }

        #[test]
        fn system_job_refusal_only_without_root() {
            for (root, job, refused) in [
                (false, true, true),
                (true, true, false),
                (false, false, false),
                (true, false, false),
            ] {
                assert_eq!(
                    system_job_refusal(root, job).is_some(),
                    refused,
                    "root={root} job={job}"
                );
            }
            assert_eq!(
                system_job_refusal_message(Path::new("/Users/me/.yerd/bin/yerd")),
                "the daemon is registered as a system launchd job; \
                 run `sudo /Users/me/.yerd/bin/yerd uninstall`"
            );
        }

        #[test]
        fn linux_unit_paths_table() {
            let home = Path::new("/home/u");
            let dot = PathBuf::from("/home/u/.config/systemd/user/yerd.service");
            let xdg = PathBuf::from("/home/u/xdg/systemd/user/yerd.service");
            let other = PathBuf::from("/srv/cfg/systemd/user/yerd.service");
            let cases: [(Option<&str>, bool, Vec<PathBuf>); 8] = [
                (None, false, vec![dot.clone()]),
                (Some(""), false, vec![dot.clone()]),
                (Some("relative/cfg"), false, vec![dot.clone()]),
                (Some("/home/u/xdg"), false, vec![xdg.clone(), dot.clone()]),
                (Some("/home/u/.config"), false, vec![dot.clone()]),
                (Some("/srv/cfg"), false, vec![other, dot.clone()]),
                (Some("/home/u/xdg"), true, vec![xdg, dot.clone()]),
                (Some("/root/.config"), true, vec![dot]),
            ];
            for (xdg_config, sudo, want) in cases {
                assert_eq!(
                    linux_unit_paths(home, xdg_config.map(OsStr::new), sudo),
                    want,
                    "{xdg_config:?} sudo={sudo}"
                );
            }
        }

        #[test]
        fn is_user_unit_table() {
            let cases = [
                ("/home/u/xdg/systemd/user/yerd.service", true),
                ("/home/u/.config/systemd/user/yerd.service", true),
                ("/srv/cfg/systemd/user/yerd.service", true),
                ("/etc/systemd/user/yerd.service", false),
                ("/usr/lib/systemd/user/yerd.service", false),
                ("/run/user/1000/systemd/user/yerd.service", false),
                ("/home/u/xdg/systemd/user/other.service", false),
                ("/home/u/xdg/yerd.service", false),
                ("/home/u/../v/systemd/user/yerd.service", false),
                ("relative/systemd/user/yerd.service", false),
            ];
            for (path, want) in cases {
                assert_eq!(is_user_unit(Path::new(path)), want, "{path}");
            }
        }

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
        fn manual_ca_removal_hint_is_os_appropriate() {
            let hint = manual_ca_removal_hint();
            assert!(!hint.is_empty());
            #[cfg(target_os = "macos")]
            assert!(hint.contains("Keychain Access"), "{hint}");
            #[cfg(not(target_os = "macos"))]
            assert!(
                hint.contains("ca-certificates") || hint.contains("ca-trust"),
                "{hint}"
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
