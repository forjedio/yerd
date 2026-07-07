//! Self-update applier: install a staged, verified artifact.
//!
//! Two invocation modes, both ending in [`run`]:
//! - **CLI** (`yerd update --yes`): calls [`run`] **in-process** (via
//!   `spawn_blocking`). The CLI is short-lived - it swaps the bundle off its own
//!   old inode and then exits.
//! - **GUI** (Update button): the GUI quits and spawns this binary **detached**,
//!   gated by the `YERD_APPLY_UPDATE` env var (see [`run_from_env`]) so it never
//!   shows in help or completions.
//!
//! It runs **unprivileged in the user session**; only the minimal privileged
//! step is elevated (Linux `dpkg` via `pkexec`; macOS only if `/Applications`
//! isn't user-writable).
//!
//! Flow:
//! 1. **Re-verify** the staged artifact's minisign signature (closes the
//!    daemon-verify → swap TOCTOU window).
//! 2. Stop the daemon, install the new bundle/package, restart the daemon, and
//!    optionally relaunch the GUI.
//!
//! ## Verified vs. gated
//!
//! The bundle-swap mechanics ([`swap_bundle`]) are unit-tested on temp dirs. The
//! live elevation, the real Gatekeeper/SMAppService behaviour, and whether a
//! bundle swap preserves the `SMAppService` Login-Item registration are **not**
//! exercisable in CI - they are the Phase B hardware-spike preconditions.
//!
//! ## Atomicity note
//!
//! The macOS swap uses rename-aside → rename-in (safe `std::fs::rename`), which
//! has a sub-millisecond window where the bundle path does not exist. An atomic
//! `renamex_np(RENAME_SWAP)` would close that window but needs `unsafe` FFI;
//! `bin/yerd` forbids `unsafe`, so that is a documented hardening follow-up
//! (move the swap into a small unsafe-permitting module or `yerd-helper`).

use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

use yerd_ipc::StagedArtifact;
use yerd_update::{verify_minisign, UPDATE_PUBLIC_KEY};

/// Env var that switches `yerd` into applier mode. Its presence (set by the
/// spawner) is what makes this a hidden, non-discoverable entry point - there is
/// no clap subcommand, so it never appears in `--help` or shell completions.
pub const APPLY_ENV: &str = "YERD_APPLY_UPDATE";
/// Env var carrying the staged artifact path.
pub const APPLY_PATH_ENV: &str = "YERD_APPLY_PATH";
/// Env var carrying the artifact kind: `"deb"`, `"pacman"`, `"rpm"`, or `"app_tar_gz"`.
pub const APPLY_KIND_ENV: &str = "YERD_APPLY_KIND";
/// Env var: `"1"` to relaunch the GUI after the install.
pub const APPLY_RELAUNCH_GUI_ENV: &str = "YERD_APPLY_RELAUNCH_GUI";
/// argv sentinel for the elevated Linux deb-install re-exec. `pkexec` strips the
/// environment, so the staged path is passed positionally. Internal; not a clap
/// subcommand, so it never appears in help/completions.
pub const INSTALL_DEB_ARG: &str = "__yerd-install-deb";

/// argv sentinel for the elevated Arch pacman-install re-exec. Mirrors
/// [`INSTALL_DEB_ARG`] for the `.pkg.tar.zst` install path.
pub const INSTALL_PACMAN_ARG: &str = "__yerd-install-pacman";

/// argv sentinel for the elevated Fedora rpm-install re-exec. Mirrors
/// [`INSTALL_DEB_ARG`] for the `.rpm` install path.
pub const INSTALL_RPM_ARG: &str = "__yerd-install-rpm";

/// If invoked as the elevated deb installer (`yerd __yerd-install-deb <path>`),
/// run it and return the exit code; otherwise `None` (normal dispatch proceeds).
/// Parsed from argv (not env) because `pkexec` sanitizes the environment.
#[must_use]
pub fn run_install_deb_from_args() -> Option<ExitCode> {
    let mut args = std::env::args_os().skip(1);
    if args.next()?.to_str() != Some(INSTALL_DEB_ARG) {
        return None;
    }
    let Some(path) = args.next() else {
        eprintln!("yerd: {INSTALL_DEB_ARG} requires a path");
        return Some(ExitCode::from(2));
    };
    Some(install_deb_entry(Path::new(&path)))
}

/// If invoked as the elevated pacman installer (`yerd __yerd-install-pacman
/// <path>`), run it and return the exit code; otherwise `None`. The pacman
/// counterpart of [`run_install_deb_from_args`].
#[must_use]
pub fn run_install_pacman_from_args() -> Option<ExitCode> {
    let mut args = std::env::args_os().skip(1);
    if args.next()?.to_str() != Some(INSTALL_PACMAN_ARG) {
        return None;
    }
    let Some(path) = args.next() else {
        eprintln!("yerd: {INSTALL_PACMAN_ARG} requires a path");
        return Some(ExitCode::from(2));
    };
    Some(install_pacman_entry(Path::new(&path)))
}

/// If invoked as the elevated rpm installer (`yerd __yerd-install-rpm <path>`),
/// run it and return the exit code; otherwise `None`. The rpm counterpart of
/// [`run_install_deb_from_args`].
#[must_use]
pub fn run_install_rpm_from_args() -> Option<ExitCode> {
    let mut args = std::env::args_os().skip(1);
    if args.next()?.to_str() != Some(INSTALL_RPM_ARG) {
        return None;
    }
    let Some(path) = args.next() else {
        eprintln!("yerd: {INSTALL_RPM_ARG} requires a path");
        return Some(ExitCode::from(2));
    };
    Some(install_rpm_entry(Path::new(&path)))
}

/// Run the elevated deb install (Linux). The cfg split lives in a helper with a
/// uniform signature to avoid `#[cfg]`-block-as-tail-expression footguns.
#[cfg(target_os = "linux")]
fn install_deb_entry(path: &Path) -> ExitCode {
    match elevated_install_deb(path) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("yerd: {e}");
            ExitCode::from(1)
        }
    }
}

#[cfg(not(target_os = "linux"))]
fn install_deb_entry(_path: &Path) -> ExitCode {
    eprintln!("yerd: elevated deb install is Linux-only");
    ExitCode::from(1)
}

/// Run the elevated pacman install (Linux). Mirror of [`install_deb_entry`].
#[cfg(target_os = "linux")]
fn install_pacman_entry(path: &Path) -> ExitCode {
    match elevated_install_pacman(path) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("yerd: {e}");
            ExitCode::from(1)
        }
    }
}

#[cfg(not(target_os = "linux"))]
fn install_pacman_entry(_path: &Path) -> ExitCode {
    eprintln!("yerd: elevated pacman install is Linux-only");
    ExitCode::from(1)
}

/// Run the elevated rpm install (Linux). Mirror of [`install_deb_entry`].
#[cfg(target_os = "linux")]
fn install_rpm_entry(path: &Path) -> ExitCode {
    match elevated_install_rpm(path) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("yerd: {e}");
            ExitCode::from(1)
        }
    }
}

#[cfg(not(target_os = "linux"))]
fn install_rpm_entry(_path: &Path) -> ExitCode {
    eprintln!("yerd: elevated rpm install is Linux-only");
    ExitCode::from(1)
}

/// If invoked in applier mode (the [`APPLY_ENV`] var is set), run the apply and
/// return its exit code; otherwise `None` (normal CLI dispatch proceeds). All
/// inputs travel via env vars so nothing leaks into the argv-driven help.
#[must_use]
pub fn run_from_env() -> Option<ExitCode> {
    std::env::var_os(APPLY_ENV)?;
    let Some(path) = std::env::var_os(APPLY_PATH_ENV) else {
        eprintln!("yerd: {APPLY_PATH_ENV} is required in apply mode");
        return Some(ExitCode::from(2));
    };
    let kind = match std::env::var(APPLY_KIND_ENV).as_deref() {
        Ok("deb") => StagedArtifact::Deb,
        Ok("pacman") => StagedArtifact::Pacman,
        Ok("rpm") => StagedArtifact::Rpm,
        Ok("app_tar_gz") => StagedArtifact::AppTarGz,
        other => {
            eprintln!(
                "yerd: invalid {APPLY_KIND_ENV}={other:?} (expected \"deb\", \"pacman\", \"rpm\" or \"app_tar_gz\")"
            );
            return Some(ExitCode::from(2));
        }
    };
    let relaunch_gui = std::env::var(APPLY_RELAUNCH_GUI_ENV).as_deref() == Ok("1");
    Some(run(Path::new(&path), kind, relaunch_gui))
}

/// Entry point for the applier subprocess. `staged` is the verified artifact the
/// daemon downloaded; `kind` selects the install method; `relaunch_gui` asks for
/// the GUI to be reopened after the daemon restarts.
#[must_use]
pub fn run(staged: &Path, kind: StagedArtifact, relaunch_gui: bool) -> ExitCode {
    if let Err(e) = reverify(staged) {
        eprintln!("yerd: update verification failed: {e}");
        return ExitCode::from(1);
    }
    let result = match kind {
        StagedArtifact::AppTarGz => apply_macos(staged, relaunch_gui),
        StagedArtifact::Deb => apply_linux(staged, relaunch_gui),
        StagedArtifact::Pacman => apply_linux_pacman(staged, relaunch_gui),
        StagedArtifact::Rpm => apply_linux_rpm(staged, relaunch_gui),
        _ => Err("unknown staged artifact kind from the daemon".to_owned()),
    };
    match result {
        Ok(()) => {
            println!("yerd: update applied; Yerd is restarting");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("yerd: update failed: {e}");
            if relaunch_gui {
                relaunch_gui_app();
            }
            ExitCode::from(1)
        }
    }
}

/// Re-verify the staged artifact against its sibling `.sig` and the embedded key.
fn reverify(staged: &Path) -> Result<(), String> {
    let bytes = std::fs::read(staged).map_err(|e| format!("reading staged artifact: {e}"))?;
    let sig_path = sibling_sig(staged);
    let sig = std::fs::read_to_string(&sig_path)
        .map_err(|e| format!("reading signature {}: {e}", sig_path.display()))?;
    verify_minisign(UPDATE_PUBLIC_KEY, &sig, &bytes).map_err(|e| e.to_string())
}

/// `<artifact>.sig` beside the staged artifact.
fn sibling_sig(staged: &Path) -> PathBuf {
    let mut name = staged.file_name().unwrap_or_default().to_os_string();
    name.push(".sig");
    staged.with_file_name(name)
}

/// The `yerdd` binary beside the running `yerd` (same bundle / install dir),
/// used by the restart step.
fn sibling_yerdd() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let dir = exe.parent()?;
    let cand = dir.join("yerdd");
    cand.exists().then_some(cand)
}

/// Restart the daemon and (optionally) relaunch the GUI after an install.
fn restart_services(relaunch_gui: bool) {
    if let Some(yerdd) = sibling_yerdd() {
        let ctl = yerd_service_ctl::ServiceCtl::new(yerdd);
        if let Err(e) = ctl.restart() {
            eprintln!("yerd: daemon restart reported: {e} (it may auto-start)");
        }
    }
    if relaunch_gui {
        relaunch_gui_app();
    }
}

// ── macOS: swap the .app bundle ──────────────────────────────────────────────

#[cfg(target_os = "macos")]
fn apply_macos(staged: &Path, relaunch_gui: bool) -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| format!("current_exe: {e}"))?;
    let bundle = exe
        .ancestors()
        .nth(3)
        .filter(|p| p.extension().is_some_and(|x| x == "app"))
        .ok_or_else(|| "not running from an .app bundle (dev build?)".to_owned())?
        .to_path_buf();

    if !bundle.starts_with("/Applications/") {
        return Err(format!(
            "Yerd must be in /Applications to self-update (it is at {}); move it there first",
            bundle.display()
        ));
    }
    let parent = bundle
        .parent()
        .ok_or_else(|| "bundle has no parent dir".to_owned())?
        .to_path_buf();
    if !dir_is_writable(&parent) {
        return Err(format!(
            "{} is not writable by you; elevated self-update is not yet wired — \
             reinstall from the .dmg",
            parent.display()
        ));
    }

    let uniq = unique_suffix();
    let stage = parent.join(format!(".yerd-staging-{uniq}"));
    std::fs::create_dir(&stage)
        .map_err(|e| format!("creating staging dir {}: {e}", stage.display()))?;

    let mut stopped = false;
    let result = (|| -> Result<(), String> {
        same_volume(&parent, &stage)?;
        let status = Command::new("tar")
            .arg("-xpf")
            .arg(staged)
            .arg("-C")
            .arg(&stage)
            .status()
            .map_err(|e| format!("spawning tar: {e}"))?;
        if !status.success() {
            return Err("extracting the update archive failed".to_owned());
        }
        let new_app = find_single_dot_app(&stage)?;

        stop_daemon();
        stopped = true;
        let backup = parent.join(format!(".yerd-backup-{uniq}.app"));
        swap_bundle(&bundle, &new_app, &backup).map_err(|e| format!("swapping bundle: {e}"))?;
        let _ = std::fs::remove_dir_all(&backup);
        Ok(())
    })();

    let _ = std::fs::remove_dir_all(&stage);
    match result {
        Ok(()) => {
            restart_services(relaunch_gui);
            Ok(())
        }
        Err(e) => {
            if stopped {
                restart_services(false);
            }
            Err(e)
        }
    }
}

/// A per-invocation unique suffix (pid + nanoseconds) for staging paths.
#[cfg(any(target_os = "macos", target_os = "linux"))]
fn unique_suffix() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    format!("{}-{nanos}", std::process::id())
}

/// Stop the daemon (best-effort) so it releases its executable inode.
#[cfg(target_os = "macos")]
fn stop_daemon() {
    if let Some(yerdd) = sibling_yerdd() {
        yerd_service_ctl::ServiceCtl::new(yerdd).stop();
    }
}

/// Replace `target` with `new_app`, keeping `target`'s old contents at `backup`.
///
/// rename-aside (`target` → `backup`) then rename-in (`new_app` → `target`). On
/// the second rename failing, the original is restored from `backup`. All renames
/// must be on one filesystem (the caller guarantees same-volume staging).
pub fn swap_bundle(target: &Path, new_app: &Path, backup: &Path) -> std::io::Result<()> {
    if target.exists() {
        std::fs::rename(target, backup)?;
    }
    match std::fs::rename(new_app, target) {
        Ok(()) => Ok(()),
        Err(e) => {
            if backup.exists() {
                let _ = std::fs::rename(backup, target);
            }
            Err(e)
        }
    }
}

/// Find the *single* `*.app` directory directly inside `dir`. Errors if there
/// are zero or more than one (a multi-`.app` archive could mean a planted bundle).
#[cfg(target_os = "macos")]
fn find_single_dot_app(dir: &Path) -> Result<PathBuf, String> {
    let mut found: Option<PathBuf> = None;
    for entry in std::fs::read_dir(dir)
        .map_err(|e| format!("reading staging dir: {e}"))?
        .flatten()
    {
        let p = entry.path();
        if p.extension().is_some_and(|x| x == "app") {
            if found.is_some() {
                return Err("update archive contained more than one .app bundle".to_owned());
            }
            found = Some(p);
        }
    }
    found.ok_or_else(|| "the update archive contained no .app bundle".to_owned())
}

/// True if `dir` is writable by the current process (rename needs dir write).
#[cfg(target_os = "macos")]
fn dir_is_writable(dir: &Path) -> bool {
    let probe = dir.join(".yerd-write-probe");
    match std::fs::File::create(&probe) {
        Ok(_) => {
            let _ = std::fs::remove_file(&probe);
            true
        }
        Err(_) => false,
    }
}

/// Error unless `a` and `b` are on the same filesystem (so a rename is atomic).
#[cfg(target_os = "macos")]
fn same_volume(a: &Path, b: &Path) -> Result<(), String> {
    use std::os::unix::fs::MetadataExt as _;
    let da = a
        .metadata()
        .map_err(|e| format!("stat {}: {e}", a.display()))?
        .dev();
    let db = b
        .metadata()
        .map_err(|e| format!("stat {}: {e}", b.display()))?
        .dev();
    if da == db {
        Ok(())
    } else {
        Err("staging directory is on a different volume than the app".to_owned())
    }
}

/// Relaunch the Yerd GUI by bundle identifier (survives a path swap).
#[cfg(target_os = "macos")]
fn relaunch_gui_app() {
    let _ = Command::new("open").args(["-b", "dev.yerd.gui"]).status();
}

// ── Linux: reinstall the .deb ────────────────────────────────────────────────

#[cfg(target_os = "linux")]
fn apply_linux(staged: &Path, relaunch_gui: bool) -> Result<(), String> {
    if nix::unistd::geteuid().is_root() {
        return elevated_install_deb(staged);
    }
    let exe = std::env::current_exe().map_err(|e| format!("current_exe: {e}"))?;
    let status = Command::new("pkexec")
        .arg(&exe)
        .arg(INSTALL_DEB_ARG)
        .arg(staged)
        .status()
        .map_err(|e| format!("spawning pkexec: {e}"))?;
    if !status.success() {
        return Err("privileged install (pkexec) failed or was cancelled".to_owned());
    }
    restart_services(relaunch_gui);
    Ok(())
}

/// Elevated installer (runs as root via the `pkexec` re-exec). Reads + verifies
/// the staged `.deb` **once**, copies the verified bytes into a root-owned 0700
/// dir, and `dpkg -i`s that copy - closing the verify→re-read TOCTOU on the
/// user-writable staged path. `dpkg`'s postinst reapplies setcap + `/usr/bin`
/// symlinks.
#[cfg(target_os = "linux")]
fn elevated_install_deb(staged: &Path) -> Result<(), String> {
    use std::os::unix::fs::{DirBuilderExt as _, PermissionsExt as _};

    if !nix::unistd::geteuid().is_root() {
        return Err("the elevated installer must run as root".to_owned());
    }
    let bytes = std::fs::read(staged).map_err(|e| format!("reading staged .deb: {e}"))?;
    let sig_path = sibling_sig(staged);
    let sig = std::fs::read_to_string(&sig_path)
        .map_err(|e| format!("reading signature {}: {e}", sig_path.display()))?;
    verify_minisign(UPDATE_PUBLIC_KEY, &sig, &bytes).map_err(|e| e.to_string())?;

    let dir = std::env::temp_dir().join(format!("yerd-update-{}", unique_suffix()));
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(&dir)
        .map_err(|e| format!("creating secure install dir: {e}"))?;
    let pkg = dir.join("update.deb");
    let install = (|| -> Result<(), String> {
        std::fs::write(&pkg, &bytes).map_err(|e| format!("writing verified .deb: {e}"))?;
        let _ = std::fs::set_permissions(&pkg, std::fs::Permissions::from_mode(0o600));
        let status = Command::new("dpkg")
            .arg("-i")
            .arg(&pkg)
            .status()
            .map_err(|e| format!("spawning dpkg: {e}"))?;
        if status.success() {
            Ok(())
        } else {
            Err("dpkg failed to install the new package".to_owned())
        }
    })();
    let _ = std::fs::remove_dir_all(&dir);
    install
}

// ── Linux (Arch): reinstall the .pkg.tar.zst ─────────────────────────────────

#[cfg(target_os = "linux")]
fn apply_linux_pacman(staged: &Path, relaunch_gui: bool) -> Result<(), String> {
    if nix::unistd::geteuid().is_root() {
        return elevated_install_pacman(staged);
    }
    let exe = std::env::current_exe().map_err(|e| format!("current_exe: {e}"))?;
    let status = Command::new("pkexec")
        .arg(&exe)
        .arg(INSTALL_PACMAN_ARG)
        .arg(staged)
        .status()
        .map_err(|e| format!("spawning pkexec: {e}"))?;
    if !status.success() {
        return Err("privileged install (pkexec) failed or was cancelled".to_owned());
    }
    restart_services(relaunch_gui);
    Ok(())
}

/// Elevated installer (runs as root via the `pkexec` re-exec). Mirror of
/// [`elevated_install_deb`] for the Arch package: reads + verifies the staged
/// `.pkg.tar.zst` **once**, copies the verified bytes into a root-owned 0700 dir,
/// and `pacman -U`s that copy - closing the verify→re-read TOCTOU on the
/// user-writable staged path. The package's `.install` scriptlet reapplies setcap.
///
/// `pacman -U` installs the file regardless of version (so an edge-to-stable
/// downgrade works, like `dpkg -i`); `--noconfirm` because the re-exec is
/// non-interactive. It is a partial upgrade, so on a host behind on `pacman -Syu`
/// a newer library soname can make it abort; pacman's output (stderr, then stdout)
/// is surfaced in the error so db-lock / unresolved-dep / `SigLevel` failures are
/// legible rather than a generic "failed". The copy keeps a `.pkg.tar.zst` suffix
/// for clarity; the package name itself comes from the embedded `.PKGINFO`.
#[cfg(target_os = "linux")]
fn elevated_install_pacman(staged: &Path) -> Result<(), String> {
    use std::os::unix::fs::{DirBuilderExt as _, PermissionsExt as _};

    if !nix::unistd::geteuid().is_root() {
        return Err("the elevated installer must run as root".to_owned());
    }
    let bytes = std::fs::read(staged).map_err(|e| format!("reading staged .pkg.tar.zst: {e}"))?;
    let sig_path = sibling_sig(staged);
    let sig = std::fs::read_to_string(&sig_path)
        .map_err(|e| format!("reading signature {}: {e}", sig_path.display()))?;
    verify_minisign(UPDATE_PUBLIC_KEY, &sig, &bytes).map_err(|e| e.to_string())?;

    let dir = std::env::temp_dir().join(format!("yerd-update-{}", unique_suffix()));
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(&dir)
        .map_err(|e| format!("creating secure install dir: {e}"))?;
    let pkg = dir.join("update.pkg.tar.zst");
    let install = (|| -> Result<(), String> {
        std::fs::write(&pkg, &bytes).map_err(|e| format!("writing verified .pkg.tar.zst: {e}"))?;
        let _ = std::fs::set_permissions(&pkg, std::fs::Permissions::from_mode(0o600));
        let out = Command::new("pacman")
            .arg("-U")
            .arg("--noconfirm")
            .arg(&pkg)
            .output()
            .map_err(|e| format!("spawning pacman: {e}"))?;
        if out.status.success() {
            Ok(())
        } else {
            let stderr = String::from_utf8_lossy(&out.stderr);
            let detail = if stderr.trim().is_empty() {
                String::from_utf8_lossy(&out.stdout).trim().to_owned()
            } else {
                stderr.trim().to_owned()
            };
            if detail.is_empty() {
                Err("pacman failed to install the new package".to_owned())
            } else {
                Err(format!(
                    "pacman failed to install the new package: {detail}"
                ))
            }
        }
    })();
    let _ = std::fs::remove_dir_all(&dir);
    install
}

// ── Linux (Fedora): reinstall the .rpm ───────────────────────────────────────

#[cfg(target_os = "linux")]
fn apply_linux_rpm(staged: &Path, relaunch_gui: bool) -> Result<(), String> {
    if nix::unistd::geteuid().is_root() {
        return elevated_install_rpm(staged);
    }
    let exe = std::env::current_exe().map_err(|e| format!("current_exe: {e}"))?;
    let status = Command::new("pkexec")
        .arg(&exe)
        .arg(INSTALL_RPM_ARG)
        .arg(staged)
        .status()
        .map_err(|e| format!("spawning pkexec: {e}"))?;
    if !status.success() {
        return Err("privileged install (pkexec) failed or was cancelled".to_owned());
    }
    restart_services(relaunch_gui);
    Ok(())
}

/// Elevated installer (runs as root via the `pkexec` re-exec). Mirror of
/// [`elevated_install_deb`] for the Fedora package: reads + verifies the staged
/// `.rpm` **once**, copies the verified bytes into a root-owned 0700 dir, and
/// `rpm -U`s that copy - closing the verify→re-read TOCTOU on the user-writable
/// staged path. The package's `%post` scriptlet reapplies setcap.
///
/// `rpm -U --oldpackage` installs the file regardless of version (so an
/// edge-to-stable downgrade works, like `dpkg -i`); `rpm` is non-interactive by
/// default. Unlike `dnf`, `rpm -U` does not *resolve* dependencies, but it does
/// *check* them: it aborts on an unmet `Requires`, so the packaged `depends` list
/// must not gain a new entry between releases (an existing install would have the
/// old deps only). It also fails if PackageKit/dnf holds the rpmdb lock. rpm's
/// output (stderr, then stdout) is surfaced in the error so unmet-dep / db-lock
/// failures are legible rather than a generic "failed". The copy keeps a `.rpm`
/// suffix for clarity; the package name itself comes from the embedded header.
#[cfg(target_os = "linux")]
fn elevated_install_rpm(staged: &Path) -> Result<(), String> {
    use std::os::unix::fs::{DirBuilderExt as _, PermissionsExt as _};

    if !nix::unistd::geteuid().is_root() {
        return Err("the elevated installer must run as root".to_owned());
    }
    let bytes = std::fs::read(staged).map_err(|e| format!("reading staged .rpm: {e}"))?;
    let sig_path = sibling_sig(staged);
    let sig = std::fs::read_to_string(&sig_path)
        .map_err(|e| format!("reading signature {}: {e}", sig_path.display()))?;
    verify_minisign(UPDATE_PUBLIC_KEY, &sig, &bytes).map_err(|e| e.to_string())?;

    let dir = std::env::temp_dir().join(format!("yerd-update-{}", unique_suffix()));
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(&dir)
        .map_err(|e| format!("creating secure install dir: {e}"))?;
    let pkg = dir.join("update.rpm");
    let install = (|| -> Result<(), String> {
        std::fs::write(&pkg, &bytes).map_err(|e| format!("writing verified .rpm: {e}"))?;
        let _ = std::fs::set_permissions(&pkg, std::fs::Permissions::from_mode(0o600));
        let out = Command::new("rpm")
            .arg("-U")
            .arg("--oldpackage")
            .arg(&pkg)
            .output()
            .map_err(|e| format!("spawning rpm: {e}"))?;
        if out.status.success() {
            Ok(())
        } else {
            let stderr = String::from_utf8_lossy(&out.stderr);
            let detail = if stderr.trim().is_empty() {
                String::from_utf8_lossy(&out.stdout).trim().to_owned()
            } else {
                stderr.trim().to_owned()
            };
            if detail.is_empty() {
                Err("rpm failed to install the new package".to_owned())
            } else {
                Err(format!("rpm failed to install the new package: {detail}"))
            }
        }
    })();
    let _ = std::fs::remove_dir_all(&dir);
    install
}

#[cfg(target_os = "linux")]
fn relaunch_gui_app() {
    use std::os::unix::process::CommandExt as _;
    if let Some(gui) = sibling_gui() {
        let _ = Command::new(gui).process_group(0).spawn();
    }
}

#[cfg(target_os = "linux")]
fn sibling_gui() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let dir = exe.parent()?;
    ["yerd-gui", "Yerd"]
        .into_iter()
        .map(|name| dir.join(name))
        .find(|c| c.exists())
}

// ── cross-platform stubs ─────────────────────────────────────────────────────
// `run`'s match references both installers regardless of target, so each needs a
// definition on the *other* OS. The daemon only ever stages the artifact kind
// matching the running platform, so these stubs are defence-in-depth.

#[cfg(not(target_os = "macos"))]
fn apply_macos(_staged: &Path, _relaunch_gui: bool) -> Result<(), String> {
    Err("a macOS .app bundle cannot be installed on this platform".to_owned())
}

#[cfg(not(target_os = "linux"))]
fn apply_linux(_staged: &Path, _relaunch_gui: bool) -> Result<(), String> {
    Err("a .deb package cannot be installed on this platform".to_owned())
}

#[cfg(not(target_os = "linux"))]
fn apply_linux_pacman(_staged: &Path, _relaunch_gui: bool) -> Result<(), String> {
    Err("an Arch .pkg.tar.zst cannot be installed on this platform".to_owned())
}

#[cfg(not(target_os = "linux"))]
fn apply_linux_rpm(_staged: &Path, _relaunch_gui: bool) -> Result<(), String> {
    Err("a Fedora .rpm cannot be installed on this platform".to_owned())
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn relaunch_gui_app() {}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    fn write_bundle(dir: &Path, marker: &str) {
        std::fs::create_dir_all(dir.join("Contents/MacOS")).unwrap();
        std::fs::write(dir.join("Contents/Info.plist"), marker).unwrap();
    }

    #[test]
    fn swap_bundle_replaces_target_and_preserves_via_backup() {
        let tmp = tempfile::tempdir().unwrap();
        let target = tmp.path().join("Yerd.app");
        let staged = tmp.path().join(".stage/Yerd.app");
        let backup = tmp.path().join(".backup.app");
        write_bundle(&target, "OLD");
        write_bundle(&staged, "NEW");

        swap_bundle(&target, &staged, &backup).unwrap();

        assert_eq!(
            std::fs::read_to_string(target.join("Contents/Info.plist")).unwrap(),
            "NEW"
        );
        assert!(!staged.exists());
        assert_eq!(
            std::fs::read_to_string(backup.join("Contents/Info.plist")).unwrap(),
            "OLD"
        );
    }

    #[test]
    fn swap_bundle_into_empty_target_works() {
        let tmp = tempfile::tempdir().unwrap();
        let target = tmp.path().join("Yerd.app");
        let staged = tmp.path().join("Yerd.app.new");
        let backup = tmp.path().join(".backup.app");
        write_bundle(&staged, "NEW");
        swap_bundle(&target, &staged, &backup).unwrap();
        assert!(target.join("Contents/Info.plist").exists());
    }

    #[test]
    fn swap_bundle_rolls_back_when_rename_in_fails() {
        let tmp = tempfile::tempdir().unwrap();
        let target = tmp.path().join("Yerd.app");
        let missing = tmp.path().join("does-not-exist.app");
        let backup = tmp.path().join(".backup.app");
        write_bundle(&target, "OLD");

        let err = swap_bundle(&target, &missing, &backup);
        assert!(err.is_err(), "swap should report the rename-in failure");
        assert_eq!(
            std::fs::read_to_string(target.join("Contents/Info.plist")).unwrap(),
            "OLD"
        );
        assert!(
            !backup.exists(),
            "backup should have been renamed back to target"
        );
    }

    #[test]
    fn sibling_sig_appends_sig_extension() {
        let p = Path::new("/cache/update/Yerd_MacOS_AppleSilicon_v2.app.tar.gz");
        assert_eq!(
            sibling_sig(p),
            Path::new("/cache/update/Yerd_MacOS_AppleSilicon_v2.app.tar.gz.sig")
        );
    }

    #[test]
    fn sibling_sig_handles_bare_filename() {
        assert_eq!(
            sibling_sig(Path::new("update.deb")),
            Path::new("update.deb.sig")
        );
    }

    /// Two reads differ because the nanosecond clock advances, and the value
    /// carries this process's pid so concurrent stagers can't collide.
    #[test]
    fn unique_suffix_is_per_call_distinct() {
        let a = unique_suffix();
        let b = unique_suffix();
        assert_ne!(a, b);
        assert!(a.starts_with(&format!("{}-", std::process::id())));
    }

    #[test]
    fn reverify_errors_when_artifact_missing() {
        let tmp = tempfile::tempdir().unwrap();
        let missing = tmp.path().join("nope.tar.gz");
        let err = reverify(&missing).unwrap_err();
        assert!(err.contains("reading staged artifact"), "{err}");
    }

    #[test]
    fn reverify_errors_when_signature_missing() {
        let tmp = tempfile::tempdir().unwrap();
        let staged = tmp.path().join("Yerd.app.tar.gz");
        std::fs::write(&staged, b"payload").unwrap();
        let err = reverify(&staged).unwrap_err();
        assert!(err.contains("reading signature"), "{err}");
    }
}
