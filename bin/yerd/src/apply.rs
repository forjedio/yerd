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
//! ## Single owner of the daemon restart (macOS)
//!
//! After an in-place bundle swap the macOS `SMAppService` registration must be
//! refreshed to re-point BTM at the new generation, and only the GUI can do that
//! (`bin/yerd` has no objc bindings). So when the GUI drives the update and
//! manages the daemon via `SMAppService`, it sets [`APPLY_GUI_OWNS_DAEMON_ENV`]
//! and the applier **does not** restart the daemon - it stops + swaps + relaunches
//! the GUI, and the GUI's re-registration is the sole restarter. A second
//! `launchctl kickstart -k` here would race the GUI's `unregister`/`register` and
//! trip a phantom/EINVAL restart. If the GUI relaunch fails to launch, the applier
//! falls back to restarting the daemon itself (nothing else would). Everywhere
//! else (macOS CLI `yerd update --yes`, the loose-launchd fallback, all Linux) the
//! applier remains the restart owner.
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
/// Env var: `"1"` when the relaunched GUI owns the daemon's launchd
/// re-registration (macOS `SMAppService` path), so the applier must **not**
/// restart the daemon itself - a second `launchctl kickstart -k` would race the
/// GUI's `unregister`/`register` and trip the phantom/EINVAL restart. Set by the
/// GUI in `apps/yerd-gui/src-tauri/src/commands.rs::spawn_applier`.
pub const APPLY_GUI_OWNS_DAEMON_ENV: &str = "YERD_APPLY_GUI_OWNS_DAEMON";
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
    let gui_owns_daemon =
        gui_owns_daemon_flag(std::env::var(APPLY_GUI_OWNS_DAEMON_ENV).ok().as_deref());
    Some(run(Path::new(&path), kind, relaunch_gui, gui_owns_daemon))
}

/// Whether the `YERD_APPLY_GUI_OWNS_DAEMON` env value means "the GUI owns the
/// daemon restart". Pure, so the presence/`"1"` contract is unit-tested.
fn gui_owns_daemon_flag(val: Option<&str>) -> bool {
    val == Some("1")
}

/// Entry point for the applier subprocess. `staged` is the verified artifact the
/// daemon downloaded; `kind` selects the install method; `relaunch_gui` asks for
/// the GUI to be reopened after the daemon restarts. `gui_owns_daemon` (macOS
/// only) defers the daemon restart to the relaunched GUI's `SMAppService`
/// re-registration (see [`APPLY_GUI_OWNS_DAEMON_ENV`]). It is threaded as a
/// parameter (parsed once in [`run_from_env`]) rather than read from the env
/// here, so the in-process CLI (`yerd update --yes`) path passes `false` and
/// stays immune to a stray exported `YERD_APPLY_GUI_OWNS_DAEMON`.
#[must_use]
pub fn run(
    staged: &Path,
    kind: StagedArtifact,
    relaunch_gui: bool,
    gui_owns_daemon: bool,
) -> ExitCode {
    if let Err(e) = reverify(staged) {
        eprintln!("yerd: update verification failed: {e}");
        return ExitCode::from(1);
    }
    let result = match kind {
        StagedArtifact::AppTarGz => apply_macos(staged, relaunch_gui, gui_owns_daemon),
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
                let _ = relaunch_gui_app();
            }
            ExitCode::from(1)
        }
    }
}

/// Re-verify the staged artifact against its sibling `.minisig` and the embedded
/// key.
fn reverify(staged: &Path) -> Result<(), String> {
    reverify_with_key(staged, UPDATE_PUBLIC_KEY)
}

/// [`reverify`] against an explicit public key, so tests can exercise the sibling
/// path + verification against a throwaway keypair (production passes
/// [`UPDATE_PUBLIC_KEY`]).
fn reverify_with_key(staged: &Path, public_key: &str) -> Result<(), String> {
    let bytes = std::fs::read(staged).map_err(|e| format!("reading staged artifact: {e}"))?;
    let sig_path = sibling_minisig(staged);
    let sig = std::fs::read_to_string(&sig_path)
        .map_err(|e| format!("reading signature {}: {e}", sig_path.display()))?;
    verify_minisign(public_key, &sig, &bytes).map_err(|e| e.to_string())
}

/// `<artifact>.minisig` beside the staged artifact. `.minisig` (not `.sig`)
/// because pacman reserves `<pkg>.sig` for `OpenPGP`; see issue #157.
fn sibling_minisig(staged: &Path) -> PathBuf {
    let mut name = staged.file_name().unwrap_or_default().to_os_string();
    name.push(".minisig");
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
    restart_daemon();
    if relaunch_gui {
        let _ = relaunch_gui_app();
    }
}

/// Restart the daemon so it picks up the freshly-swapped binary. Best-effort: a
/// failure is logged, and launchd's `KeepAlive`/`RunAtLoad` may still bring it up.
fn restart_daemon() {
    if let Some(yerdd) = sibling_yerdd() {
        let ctl = yerd_service_ctl::ServiceCtl::new(yerdd);
        if let Err(e) = ctl.restart() {
            eprintln!("yerd: daemon restart reported: {e} (it may auto-start)");
        }
    }
}

/// Finish a GUI-owned macOS update: the relaunched GUI re-registers (and
/// restarts) the daemon via `SMAppService` as the single owner of the launchd
/// lifecycle, so the applier only relaunches the GUI - a racing `kickstart -k`
/// here is what trips the phantom/EINVAL restart. A successful `open` only means
/// the launch was *dispatched*, not that the GUI finished starting and brought
/// the daemon back, so we then poll the daemon's IPC socket for a bounded window.
/// Falls back to restarting the daemon itself only when the GUI did not launch or
/// the daemon never answered in time (then nothing else would, and the job is
/// still loaded from the earlier `stop_daemon` so the restart works).
#[cfg(target_os = "macos")]
fn finish_gui_owned_update() {
    let daemon_up = relaunch_gui_app()
        && daemon_ready_within(GUI_DAEMON_READY_ATTEMPTS, GUI_DAEMON_POLL_INTERVAL);
    if !daemon_up {
        restart_daemon();
    }
}

/// Attempts (each followed by [`GUI_DAEMON_POLL_INTERVAL`]) to give the
/// relaunched GUI to re-register and start the daemon before the applier stops
/// trusting the single-owner path. 40 × 500 ms ≈ 20 s covers a cold GUI launch
/// plus first-run Gatekeeper verification of the freshly-swapped bundle.
#[cfg(target_os = "macos")]
const GUI_DAEMON_READY_ATTEMPTS: u32 = 40;

/// Delay between daemon-readiness probes in [`finish_gui_owned_update`].
#[cfg(target_os = "macos")]
const GUI_DAEMON_POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(500);

/// Poll the daemon's IPC socket until a `Ping` round-trips or `attempts` are
/// exhausted. Confirms the relaunched GUI actually brought the daemon back (as
/// opposed to `open` merely dispatching the launch) before the single-owner path
/// is trusted. A failure to build the probe runtime is treated as "not ready" so
/// the caller falls back to restarting the daemon itself.
#[cfg(target_os = "macos")]
fn daemon_ready_within(attempts: u32, interval: std::time::Duration) -> bool {
    let Ok(rt) = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    else {
        return false;
    };
    poll_until(
        || {
            rt.block_on(crate::transport::exchange(&yerd_ipc::Request::Ping))
                .is_ok()
        },
        attempts,
        interval,
    )
}

/// Call `probe` up to `attempts` times, returning `true` as soon as one does and
/// sleeping `interval` between the remaining tries; `false` if all fail. Pure but
/// for the injected probe and sleep, so the bounded-retry logic is unit-tested
/// with a counting fake.
#[cfg(target_os = "macos")]
fn poll_until<F: FnMut() -> bool>(
    mut probe: F,
    attempts: u32,
    interval: std::time::Duration,
) -> bool {
    for i in 0..attempts {
        if probe() {
            return true;
        }
        if i + 1 < attempts {
            std::thread::sleep(interval);
        }
    }
    false
}

// ── macOS: swap the .app bundle ──────────────────────────────────────────────

#[cfg(target_os = "macos")]
fn apply_macos(staged: &Path, relaunch_gui: bool, gui_owns_daemon: bool) -> Result<(), String> {
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
            if gui_owns_daemon {
                finish_gui_owned_update();
            } else {
                restart_services(relaunch_gui);
            }
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

/// A per-invocation unique suffix for staging paths: pid + nanoseconds + a
/// process-lifetime counter. The pid keeps concurrent processes from colliding;
/// the counter guarantees successive calls within one process differ even when
/// the nanosecond clock has coarse resolution and returns the same instant.
#[cfg(any(target_os = "macos", target_os = "linux"))]
fn unique_suffix() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    let seq = SEQ.fetch_add(1, Ordering::Relaxed);
    format!("{}-{nanos}-{seq}", std::process::id())
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

/// Relaunch the Yerd GUI by bundle identifier (survives a path swap). Returns
/// whether `open` reported a successful launch - the GUI-owned daemon path uses
/// this to decide whether to fall back to restarting the daemon itself.
#[cfg(target_os = "macos")]
fn relaunch_gui_app() -> bool {
    Command::new("open")
        .args(["-b", "dev.yerd.gui"])
        .status()
        .is_ok_and(|s| s.success())
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
    let sig_path = sibling_minisig(staged);
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
    let sig_path = sibling_minisig(staged);
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
/// `rpm -U --oldpackage --replacepkgs` installs the file regardless of version (so
/// an edge-to-stable downgrade works, like `dpkg -i`); `--replacepkgs` also lets a
/// same-version reinstall succeed, so a retry after a partial/interrupted attempt
/// is idempotent (plain `rpm -U` would abort with "already installed"). `rpm` is
/// non-interactive by default. Unlike `dnf`, `rpm -U` does not *resolve*
/// dependencies, but it does
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
    let sig_path = sibling_minisig(staged);
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
            .arg("--replacepkgs")
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
fn relaunch_gui_app() -> bool {
    use std::os::unix::process::CommandExt as _;
    sibling_gui().is_some_and(|gui| Command::new(gui).process_group(0).spawn().is_ok())
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
fn apply_macos(_staged: &Path, _relaunch_gui: bool, _gui_owns_daemon: bool) -> Result<(), String> {
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
fn relaunch_gui_app() -> bool {
    false
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    fn write_bundle(dir: &Path, marker: &str) {
        std::fs::create_dir_all(dir.join("Contents/MacOS")).unwrap();
        std::fs::write(dir.join("Contents/Info.plist"), marker).unwrap();
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn poll_until_returns_true_on_first_success() {
        assert!(poll_until(|| true, 3, std::time::Duration::ZERO));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn poll_until_succeeds_before_exhausting_attempts() {
        let mut calls = 0;
        let ready = poll_until(
            || {
                calls += 1;
                calls == 2
            },
            5,
            std::time::Duration::ZERO,
        );
        assert!(ready);
        assert_eq!(calls, 2);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn poll_until_gives_up_after_all_attempts() {
        let mut calls = 0;
        let ready = poll_until(
            || {
                calls += 1;
                false
            },
            3,
            std::time::Duration::ZERO,
        );
        assert!(!ready);
        assert_eq!(calls, 3);
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
    fn sibling_minisig_appends_minisig_extension() {
        let p = Path::new("/cache/update/Yerd_MacOS_AppleSilicon_v2.app.tar.gz");
        assert_eq!(
            sibling_minisig(p),
            Path::new("/cache/update/Yerd_MacOS_AppleSilicon_v2.app.tar.gz.minisig")
        );
    }

    #[test]
    fn sibling_minisig_handles_bare_filename() {
        assert_eq!(
            sibling_minisig(Path::new("update.deb")),
            Path::new("update.deb.minisig")
        );
    }

    /// Two reads differ because of the process-lifetime counter (not the clock,
    /// which may be coarse), and the value carries this process's pid so
    /// concurrent stagers can't collide.
    #[test]
    fn unique_suffix_is_per_call_distinct() {
        let a = unique_suffix();
        let b = unique_suffix();
        assert_ne!(a, b);
        assert!(a.starts_with(&format!("{}-", std::process::id())));
    }

    #[test]
    fn gui_owns_daemon_flag_only_true_for_one() {
        for (val, expected) in [
            (None, false),
            (Some(""), false),
            (Some("0"), false),
            (Some("true"), false),
            (Some("1"), true),
        ] {
            assert_eq!(gui_owns_daemon_flag(val), expected, "val={val:?}");
        }
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

    /// Locks the reader side of the `.minisig` contract: a valid `.minisig`
    /// sibling (named as `self_update::stage_update` writes it) reverifies against
    /// the signing key, so a change to the reader's `.minisig` literal fails here.
    /// The writer's literal is guarded by the ipc-server stage test, which asserts
    /// the sibling the real `stage_update` writes is named `<artifact>.minisig`.
    #[test]
    fn reverify_succeeds_with_valid_minisig_sibling() {
        let tmp = tempfile::tempdir().unwrap();
        let staged = tmp.path().join("Yerd_Linux_x86_64_v9.pkg.tar.zst");
        let bytes: &[u8] = b"verified artifact bytes";
        std::fs::write(&staged, bytes).unwrap();

        let kp = minisign::KeyPair::generate_unencrypted_keypair().unwrap();
        let sig = minisign::sign(
            Some(&kp.pk),
            &kp.sk,
            std::io::Cursor::new(bytes),
            Some("test artifact"),
            Some("yerd test"),
        )
        .unwrap()
        .into_string();

        let name = staged.file_name().unwrap().to_string_lossy().into_owned();
        let sibling = staged.with_file_name(format!("{name}.minisig"));
        std::fs::write(&sibling, sig.as_bytes()).unwrap();

        reverify_with_key(&staged, &kp.pk.to_base64()).unwrap();
    }
}
