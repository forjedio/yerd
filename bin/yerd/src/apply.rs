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
//! isn't user-writable). A headless install never elevates: its binaries live
//! in a user-owned dir and are swapped in place.
//!
//! Flow:
//! 1. **Check the install receipt** beside this `yerd` against the staged kind,
//!    so a desktop-app `yerd` never applies a headless tarball and vice versa.
//! 2. **Re-verify** the staged artifact's minisign signature (closes the
//!    daemon-verify → swap TOCTOU window).
//! 3. Stop the daemon, install the new bundle/package, restart the daemon, and
//!    optionally relaunch the GUI. The headless path instead swaps each binary
//!    with a hard-link backup and an atomic rename, then restarts the daemon.
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
use yerd_platform::pure::install_receipt::{InstallMode, ReceiptError};
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
    if let Err(e) = preflight(kind, yerd_platform::install_mode::detect()) {
        eprintln!("yerd: {e}");
        return ExitCode::from(1);
    }
    if let Err(e) = reverify(staged) {
        eprintln!("yerd: update verification failed: {e}");
        return ExitCode::from(1);
    }
    if kind == StagedArtifact::HeadlessTarGz {
        return finish_headless(apply_headless(staged));
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

/// Refuse before touching anything when this `yerd`'s install mode is unknown
/// or does not match the staged artifact kind.
fn preflight(kind: StagedArtifact, mode: Result<InstallMode, ReceiptError>) -> Result<(), String> {
    let mode = mode.map_err(|e| e.to_string())?;
    check_kind(kind, mode)
}

/// Whether an install of `mode` may apply a staged artifact of `kind`. A
/// mismatch means the CLI and the daemon belong to different installs.
fn check_kind(kind: StagedArtifact, mode: InstallMode) -> Result<(), String> {
    match (kind, mode) {
        (StagedArtifact::HeadlessTarGz, InstallMode::Package) => Err(
            "this yerd belongs to a different install than the daemon (a headless install); \
             run the headless install's `yerd update --yes` instead"
                .to_owned(),
        ),
        (
            StagedArtifact::AppTarGz
            | StagedArtifact::Deb
            | StagedArtifact::Pacman
            | StagedArtifact::Rpm,
            InstallMode::Headless,
        ) => Err(
            "this yerd is a headless install but the daemon belongs to the desktop app; \
             use the desktop app's updater instead"
                .to_owned(),
        ),
        _ => Ok(()),
    }
}

/// Whether `exe` (a canonical path) is where a distro package puts `yerd`:
/// `/usr/bin/<file>`, or the deb postinst's fallback `/usr/lib/<dir>/<file>`.
/// Guards the Linux package paths so a tarball install that lost its receipt
/// can never `pkexec` the desktop package.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn is_package_location(exe: &Path) -> bool {
    let normal_count = |rest: &Path| {
        rest.components()
            .all(|c| matches!(c, std::path::Component::Normal(_)))
            .then(|| rest.components().count())
    };
    if let Ok(rest) = exe.strip_prefix("/usr/bin") {
        return normal_count(rest) == Some(1);
    }
    if let Ok(rest) = exe.strip_prefix("/usr/lib") {
        return normal_count(rest) == Some(2);
    }
    false
}

/// Refuse a Linux package install unless this `yerd` lives where the package
/// put it (see [`is_package_location`]).
#[cfg(target_os = "linux")]
fn ensure_package_location() -> Result<(), String> {
    let exe = std::env::current_exe()
        .and_then(std::fs::canonicalize)
        .map_err(|e| format!("resolving the running yerd: {e}"))?;
    if is_package_location(&exe) {
        Ok(())
    } else {
        Err(format!(
            "{} is not a package install (expected under /usr/bin); refusing to install a \
             system package from here. If this is a headless install, reinstall it with the \
             install script",
            exe.display()
        ))
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
    ensure_package_location()?;
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
    ensure_package_location()?;
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
    ensure_package_location()?;
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

// ── headless: swap the binaries in place ─────────────────────────────────────

/// The top-level directory every headless tarball extracts to.
const HEADLESS_TOP_DIR: &str = "yerd";

/// The files every headless tarball must carry, in swap order: the receipt and
/// helpers first, then `yerdd`, then `yerd` last so the CLI is replaced only
/// once everything it drives is in place.
fn required_files(macos: bool) -> &'static [&'static str] {
    if macos {
        &[
            yerd_platform::pure::install_receipt::RECEIPT_FILE,
            "yerd-helper",
            "yerd-forward",
            "yerdd",
            "yerd",
        ]
    } else {
        &[
            yerd_platform::pure::install_receipt::RECEIPT_FILE,
            "yerd-helper",
            "yerdd",
            "yerd",
        ]
    }
}

/// Swapped after everything else, in this order.
const SWAPPED_LAST: [&str; 2] = ["yerdd", "yerd"];

/// Prefix every extra file in a headless tarball must carry; shared with
/// `yerd uninstall`, which removes such files from a receipt-marked dir.
pub(crate) const EXTRA_PREFIX: &str = "yerd-";

/// What a path inside the extracted tarball is, read without following links.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EntryKind {
    File,
    Dir,
    Other,
}

/// A name a newer release may add beside the required files: a `yerd-`
/// prefixed basename of `[A-Za-z0-9._-]` that can't be mistaken for a swap
/// backup or the update lock. The prefix is what lets `yerd uninstall` remove
/// extras from a shared install dir without touching the user's own files.
fn is_safe_extra_name(name: &str) -> bool {
    name != UPDATE_LOCK
        && name.len() > EXTRA_PREFIX.len()
        && name.starts_with(EXTRA_PREFIX)
        && !name.contains(".yerd-old-")
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
}

/// Require the extracted tarball to be exactly `yerd/` holding every
/// `required` file plus any extra files a newer release adds, all regular
/// files directly under `yerd/` (no symlinks, dirs or nested paths). `entries`
/// are paths relative to the staging dir, `/`-separated.
///
/// Returns the swap order: the required files except [`SWAPPED_LAST`], then
/// the extras (sorted), then `yerdd` and `yerd`.
fn validate_payload(
    entries: &[(String, EntryKind)],
    required: &[&str],
) -> Result<Vec<String>, String> {
    let mut seen_top = false;
    let mut seen = std::collections::BTreeSet::new();
    let mut extras = std::collections::BTreeSet::new();
    for (path, kind) in entries {
        if path == HEADLESS_TOP_DIR {
            if *kind != EntryKind::Dir {
                return Err(format!("update archive: {path} is not a directory"));
            }
            seen_top = true;
            continue;
        }
        let name = path
            .strip_prefix(HEADLESS_TOP_DIR)
            .and_then(|rest| rest.strip_prefix('/'))
            .filter(|name| required.contains(name) || is_safe_extra_name(name))
            .ok_or_else(|| format!("update archive has an unexpected entry: {path}"))?;
        if *kind != EntryKind::File {
            return Err(format!("update archive: {path} is not a regular file"));
        }
        if required.contains(&name) {
            seen.insert(name);
        } else {
            extras.insert(name.to_owned());
        }
    }
    if !seen_top {
        return Err(format!(
            "update archive has no top-level {HEADLESS_TOP_DIR}/ directory"
        ));
    }
    if let Some(missing) = required.iter().find(|name| !seen.contains(*name)) {
        return Err(format!("update archive is missing {missing}"));
    }
    let first = required
        .iter()
        .filter(|name| !SWAPPED_LAST.contains(name))
        .map(|name| (*name).to_owned());
    let last = SWAPPED_LAST
        .iter()
        .filter(|name| required.contains(name))
        .map(|name| (*name).to_owned());
    Ok(first.chain(extras).chain(last).collect())
}

/// Every path under `root`, relative and `/`-separated, with its kind. Symlinks
/// are reported as [`EntryKind::Other`] and never followed.
#[cfg(any(target_os = "macos", target_os = "linux"))]
fn list_payload(root: &Path) -> Result<Vec<(String, EntryKind)>, String> {
    let mut out = Vec::new();
    let mut pending = vec![(root.to_path_buf(), String::new())];
    while let Some((dir, prefix)) = pending.pop() {
        let read =
            std::fs::read_dir(&dir).map_err(|e| format!("reading {}: {e}", dir.display()))?;
        for entry in read {
            let entry = entry.map_err(|e| format!("reading {}: {e}", dir.display()))?;
            let name = entry.file_name().to_string_lossy().into_owned();
            let rel = if prefix.is_empty() {
                name
            } else {
                format!("{prefix}/{name}")
            };
            let meta = std::fs::symlink_metadata(entry.path())
                .map_err(|e| format!("stat {}: {e}", entry.path().display()))?;
            let kind = if meta.is_dir() {
                pending.push((entry.path(), rel.clone()));
                EntryKind::Dir
            } else if meta.is_file() {
                EntryKind::File
            } else {
                EntryKind::Other
            };
            out.push((rel, kind));
        }
    }
    Ok(out)
}

/// How a swapped file is undone on rollback.
#[cfg(any(target_os = "macos", target_os = "linux"))]
enum Swapped {
    /// An existing file was replaced; its original is at `backup`.
    Replaced { target: PathBuf, backup: PathBuf },
    /// The file is new (an extra from a newer release); rollback removes it.
    Created { target: PathBuf },
}

/// Replace each of `names` in `dir` with the same-named file from `payload`.
///
/// Each existing target is first hard-linked to `<name>.yerd-old-<suffix>` as
/// a backup, the new file gets its mode, then one atomic `rename` puts it over
/// the target, so the target path never disappears. A name with no existing
/// target (an extra file a newer release added) is simply renamed in. On any
/// failure every already-swapped file is restored from its backup and every
/// newly created one removed. Backups are removed on success. All paths must
/// be on one filesystem.
#[cfg(any(target_os = "macos", target_os = "linux"))]
fn swap_files<S: AsRef<str>>(
    dir: &Path,
    payload: &Path,
    names: &[S],
    suffix: &str,
) -> Result<(), String> {
    let mut swapped: Vec<Swapped> = Vec::new();
    for name in names {
        let name = name.as_ref();
        let target = dir.join(name);
        let backup = dir.join(format!("{name}.yerd-old-{suffix}"));
        let existed = std::fs::symlink_metadata(&target).is_ok();
        if let Err(e) = swap_one(
            &target,
            &payload.join(name),
            existed.then_some(&*backup),
            name,
        ) {
            let failures = roll_back(&swapped);
            if failures.is_empty() {
                return Err(e);
            }
            return Err(format!(
                "{e}; rolling back also failed, so {} may hold a mix of old and new files: {}",
                dir.display(),
                failures.join("; ")
            ));
        }
        swapped.push(if existed {
            Swapped::Replaced { target, backup }
        } else {
            Swapped::Created { target }
        });
    }
    for entry in &swapped {
        if let Swapped::Replaced { backup, .. } = entry {
            let _ = std::fs::remove_file(backup);
        }
    }
    Ok(())
}

/// Undo `swapped` in reverse order, attempting every step. Returns a message
/// per step that failed, naming any backup still on disk.
#[cfg(any(target_os = "macos", target_os = "linux"))]
fn roll_back(swapped: &[Swapped]) -> Vec<String> {
    let mut failures = Vec::new();
    for entry in swapped.iter().rev() {
        match entry {
            Swapped::Replaced { target, backup } => {
                if let Err(e) = std::fs::rename(backup, target) {
                    failures.push(format!(
                        "restoring {}: {e} (the old file is still at {})",
                        target.display(),
                        backup.display()
                    ));
                }
            }
            Swapped::Created { target } => {
                if let Err(e) = std::fs::remove_file(target) {
                    failures.push(format!("removing {}: {e}", target.display()));
                }
            }
        }
    }
    failures
}

/// The installed mode: 0644 for the receipt, 0755 for the required binaries,
/// and for an extra file 0755 only when the archive marked it executable.
#[cfg(any(target_os = "macos", target_os = "linux"))]
fn install_mode_for(name: &str, payload_mode: u32) -> u32 {
    if name == yerd_platform::pure::install_receipt::RECEIPT_FILE {
        0o644
    } else if required_files(cfg!(target_os = "macos")).contains(&name) || payload_mode & 0o111 != 0
    {
        0o755
    } else {
        0o644
    }
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn swap_one(target: &Path, new: &Path, backup: Option<&Path>, name: &str) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt as _;
    if let Some(backup) = backup {
        std::fs::hard_link(target, backup)
            .map_err(|e| format!("backing up {}: {e}", target.display()))?;
    }
    let installed = std::fs::metadata(new)
        .and_then(|meta| {
            let mode = install_mode_for(name, meta.permissions().mode());
            std::fs::set_permissions(new, std::fs::Permissions::from_mode(mode))
        })
        .and_then(|()| std::fs::rename(new, target));
    installed.map_err(|e| {
        if let Some(backup) = backup {
            let _ = std::fs::remove_file(backup);
        }
        format!("replacing {}: {e}", target.display())
    })
}

/// How the daemon came back after a headless swap. The files are installed in
/// every case; only the restart differs.
#[cfg_attr(not(any(target_os = "macos", target_os = "linux")), allow(dead_code))]
#[derive(Debug, PartialEq, Eq)]
enum HeadlessRestart {
    /// The daemon restarted onto the new binary.
    Restarted,
    /// No daemon was running; it picks up the new binary at its next start.
    /// Only detected on macOS, where the restart goes over IPC.
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    NotRunning,
    /// The daemon could not be restarted.
    Failed(String),
}

/// Lock file in the install dir that serializes headless updates. Named with
/// the `yerd-` prefix so `yerd uninstall` sweeps it up with the other files,
/// and reserved: no update payload may carry a file of this name.
const UPDATE_LOCK: &str = "yerd-update.lock";

/// Report a headless apply. A failed restart after a committed swap says the
/// new files are in place (nothing was rolled back) and how to recover.
fn finish_headless(result: Result<HeadlessRestart, String>) -> ExitCode {
    match result {
        Ok(HeadlessRestart::Restarted) => {
            println!("yerd: update applied; the daemon restarted on the new version");
            ExitCode::SUCCESS
        }
        Ok(HeadlessRestart::NotRunning) => {
            println!(
                "yerd: update applied; the daemon isn't running and will use the new version \
                 when it next starts"
            );
            ExitCode::SUCCESS
        }
        Ok(HeadlessRestart::Failed(e)) => {
            eprintln!(
                "yerd: the update is installed, but restarting the daemon failed: {e}. \
                 Run `yerd restart daemon` to load the new version"
            );
            ExitCode::from(1)
        }
        Err(e) => {
            eprintln!("yerd: update failed: {e}");
            ExitCode::from(1)
        }
    }
}

/// Install a staged headless tarball over the running binaries and restart the
/// daemon. No elevation: the install dir is the user's. Holds the install
/// dir's update lock from before the first replacement until the restart, so
/// two updates never interleave their swaps or rollbacks.
#[cfg(any(target_os = "macos", target_os = "linux"))]
fn apply_headless(staged: &Path) -> Result<HeadlessRestart, String> {
    let dir = yerd_platform::install_mode::current_exe_dir()
        .ok_or_else(|| ReceiptError::ExeUnresolved.to_string())?;
    let _lock = lock_install_dir(&dir)?;
    check_registered_dir(registered_service_yerdd().as_deref(), &dir)?;
    let had_port_cap = yerdd_had_port_capability(&dir.join("yerdd"));
    install_headless(staged, &dir, &unique_suffix(), Some(UPDATE_PUBLIC_KEY))?;
    Ok(restart_after_headless_swap(&dir, had_port_cap))
}

#[cfg_attr(not(any(target_os = "macos", target_os = "linux")), allow(dead_code))]
/// Refuse when the registered daemon service runs a `yerdd` from another
/// directory: swapping this install's files would leave that daemon on its
/// old version. No registration (a hand-started daemon) passes.
fn check_registered_dir(registered: Option<&Path>, dir: &Path) -> Result<(), String> {
    let Some(yerdd) = registered else {
        return Ok(());
    };
    let registered_dir = yerdd.parent().unwrap_or(yerdd);
    let same = registered_dir == dir
        || std::fs::canonicalize(registered_dir).is_ok_and(|canon| canon == dir);
    if same {
        Ok(())
    } else {
        Err(format!(
            "the daemon service runs {}, from a different install than this yerd ({}); run \
             that install's `yerd update --yes` instead. Nothing was changed",
            yerdd.display(),
            dir.display()
        ))
    }
}

/// The `yerdd` the registered daemon service runs: the systemd user unit's
/// `ExecStart=` on Linux, the program of the system launchd daemon on macOS.
#[cfg(target_os = "linux")]
fn registered_service_yerdd() -> Option<PathBuf> {
    let unit = std::fs::read_to_string(yerd_service_ctl::systemd_unit_path()?).ok()?;
    yerd_service_ctl::unit_exec_start(&unit).map(PathBuf::from)
}

#[cfg(target_os = "macos")]
fn registered_service_yerdd() -> Option<PathBuf> {
    let plist = std::fs::read_to_string(yerd_platform::pure::launchd_daemon::DAEMON_PLIST).ok()?;
    yerd_platform::pure::launchd_daemon::parse_daemon_plist(&plist)
        .map(|job| PathBuf::from(job.yerdd))
}

/// Take the install dir's exclusive update lock, or refuse without touching
/// anything when another update holds it. Released when the file drops.
#[cfg(any(target_os = "macos", target_os = "linux"))]
fn lock_install_dir(dir: &Path) -> Result<std::fs::File, String> {
    use fs4::fs_std::FileExt as _;
    let path = dir.join(UPDATE_LOCK);
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&path)
        .map_err(|e| format!("opening {}: {e}", path.display()))?;
    match file.try_lock_exclusive() {
        Ok(true) => Ok(file),
        Ok(false) => Err(format!(
            "another update of {} is in progress; nothing was changed",
            dir.display()
        )),
        Err(e) => Err(format!("locking {}: {e}", path.display())),
    }
}

/// Whether `getcap` output shows `cap_net_bind_service` on the file. `None`
/// (getcap missing or failed) counts as yes, so the reset note still prints.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn port_capability_from_getcap(output: Option<&str>) -> bool {
    output.map_or(true, |out| out.contains("cap_net_bind_service"))
}

/// Whether `yerdd` carries the port capability before the swap drops it,
/// read with `getcap` (best-effort; it often lives in an sbin dir).
#[cfg(target_os = "linux")]
fn yerdd_had_port_capability(yerdd: &Path) -> bool {
    let output = ["getcap", "/usr/sbin/getcap", "/sbin/getcap"]
        .into_iter()
        .find_map(|tool| {
            Command::new(tool)
                .arg(yerdd)
                .output()
                .ok()
                .filter(|o| o.status.success())
        })
        .map(|o| String::from_utf8_lossy(&o.stdout).into_owned());
    port_capability_from_getcap(output.as_deref())
}

/// macOS binds 80/443 through launchd sockets, not a file capability.
#[cfg(target_os = "macos")]
fn yerdd_had_port_capability(_yerdd: &Path) -> bool {
    false
}

/// Snapshot `staged` into a staging dir inside `dir`, verify the snapshot's
/// signature against `public_key` (when given), extract the snapshot, validate
/// the file set and its receipt, then swap every binary in `dir`. Extracting
/// the verified copy means a concurrent re-stage of `staged` can't change what
/// gets installed. Nothing in `dir` changes unless validation passes; the
/// staging dir is always removed.
#[cfg(any(target_os = "macos", target_os = "linux"))]
fn install_headless(
    staged: &Path,
    dir: &Path,
    suffix: &str,
    public_key: Option<&str>,
) -> Result<(), String> {
    let staging = dir.join(format!(".yerd-staging-{suffix}"));
    std::fs::create_dir(&staging)
        .map_err(|e| format!("creating staging dir {}: {e}", staging.display()))?;
    let result = (|| -> Result<(), String> {
        let bytes = std::fs::read(staged).map_err(|e| format!("reading staged artifact: {e}"))?;
        if let Some(key) = public_key {
            let sig_path = sibling_minisig(staged);
            let sig = std::fs::read_to_string(&sig_path)
                .map_err(|e| format!("reading signature {}: {e}", sig_path.display()))?;
            verify_minisign(key, &sig, &bytes)
                .map_err(|e| format!("update verification failed: {e}"))?;
        }
        let archive = staging.join("update.tar.gz");
        std::fs::write(&archive, &bytes)
            .map_err(|e| format!("writing {}: {e}", archive.display()))?;
        let extracted = staging.join("x");
        std::fs::create_dir(&extracted)
            .map_err(|e| format!("creating {}: {e}", extracted.display()))?;
        let status = Command::new("tar")
            .arg("-xzf")
            .arg(&archive)
            .arg("-C")
            .arg(&extracted)
            .status()
            .map_err(|e| format!("spawning tar: {e}"))?;
        if !status.success() {
            return Err("extracting the update archive failed".to_owned());
        }
        let names = validate_payload(
            &list_payload(&extracted)?,
            required_files(cfg!(target_os = "macos")),
        )?;
        let payload = extracted.join(HEADLESS_TOP_DIR);
        let receipt = std::fs::read_to_string(
            payload.join(yerd_platform::pure::install_receipt::RECEIPT_FILE),
        )
        .map_err(|e| format!("reading the update's install receipt: {e}"))?;
        if yerd_platform::pure::install_receipt::parse(&receipt) != Ok(InstallMode::Headless) {
            return Err("the update archive is not a headless build".to_owned());
        }
        swap_files(dir, &payload, &names, suffix)
    })();
    let _ = std::fs::remove_dir_all(&staging);
    result
}

/// Restart the daemon onto the swapped binary. Linux re-exec of a renamed-over
/// binary resolves to `"<path> (deleted)"`, so it goes through the service
/// manager instead. The swap also drops any `cap_net_bind_service` on `yerdd`,
/// so when it had one, say how to grant it again.
#[cfg(target_os = "linux")]
fn restart_after_headless_swap(dir: &Path, had_port_cap: bool) -> HeadlessRestart {
    let outcome = match yerd_service_ctl::ServiceCtl::new(dir.join("yerdd")).restart() {
        Ok(()) => HeadlessRestart::Restarted,
        Err(e) => HeadlessRestart::Failed(e.to_string()),
    };
    if had_port_cap {
        println!(
            "yerd: the update reset the port capability on yerdd; to bind 80/443 again, run \
             `sudo {0}/yerd elevate ports`, then `{0}/yerd restart daemon`",
            dir.display()
        );
    }
    outcome
}

/// Restart the daemon onto the swapped binary. A system-domain launchd job
/// can't be kickstarted without root, so this uses the daemon's own in-place
/// re-exec (which loads the new binary from its launch path) and waits for the
/// boot id to change. A daemon that isn't running picks the new binary up at
/// its next start.
#[cfg(target_os = "macos")]
fn restart_after_headless_swap(_dir: &Path, _had_port_cap: bool) -> HeadlessRestart {
    let rt = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => return HeadlessRestart::Failed(format!("starting a runtime: {e}")),
    };
    match rt.block_on(crate::transport::exchange(&yerd_ipc::Request::Ping)) {
        Ok(_) => {}
        Err(e) if e.is_daemon_down() => return HeadlessRestart::NotRunning,
        Err(e) => return HeadlessRestart::Failed(e.to_string()),
    }
    let before = match rt.block_on(crate::fetch_boot_id()) {
        Ok(Some(id)) => id,
        Ok(None) => {
            return HeadlessRestart::Failed(
                "the daemon did not report a boot id, so the restart can't be confirmed".to_owned(),
            )
        }
        Err(e) => return HeadlessRestart::Failed(format!("reading the daemon's boot id: {e}")),
    };
    match rt.block_on(crate::restart_and_await_boot_change(Some(before))) {
        Ok(()) => HeadlessRestart::Restarted,
        Err(e) => HeadlessRestart::Failed(e.to_string()),
    }
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

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn apply_headless(_staged: &Path) -> Result<HeadlessRestart, String> {
    Err("a headless tarball cannot be installed on this platform".to_owned())
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

    #[test]
    fn check_kind_covers_every_kind_and_mode() {
        let package_kinds = [
            StagedArtifact::AppTarGz,
            StagedArtifact::Deb,
            StagedArtifact::Pacman,
            StagedArtifact::Rpm,
        ];
        for kind in package_kinds {
            assert_eq!(check_kind(kind, InstallMode::Package), Ok(()), "{kind:?}");
            let err = check_kind(kind, InstallMode::Headless).unwrap_err();
            assert!(err.contains("desktop app's updater"), "{kind:?}: {err}");
        }
        assert_eq!(
            check_kind(StagedArtifact::HeadlessTarGz, InstallMode::Headless),
            Ok(())
        );
        let err = check_kind(StagedArtifact::HeadlessTarGz, InstallMode::Package).unwrap_err();
        assert!(
            err.contains("headless install's `yerd update --yes`"),
            "{err}"
        );
    }

    #[test]
    fn preflight_refuses_an_unusable_receipt() {
        let cases = [
            (
                ReceiptError::Unrecognised,
                "reinstall with the install script",
            ),
            (ReceiptError::ExeUnresolved, "restart it"),
        ];
        for kind in [StagedArtifact::HeadlessTarGz, StagedArtifact::Deb] {
            for (err, hint) in cases {
                let msg = preflight(kind, Err(err)).unwrap_err();
                assert!(msg.contains(hint), "{kind:?}/{err:?}: {msg}");
            }
        }
        assert_eq!(
            preflight(StagedArtifact::Deb, Ok(InstallMode::Package)),
            Ok(())
        );
    }

    #[test]
    fn is_package_location_accepts_only_package_layouts() {
        for (path, expected) in [
            ("/usr/bin/yerd", true),
            ("/usr/lib/yerd/yerd", true),
            ("/usr/lib/a/b/yerd", false),
            ("/usr/lib/yerd", false),
            ("/usr/bin/sub/yerd", false),
            ("/usr/local/bin/yerd", false),
            ("/home/u/.yerd/bin/yerd", false),
        ] {
            assert_eq!(is_package_location(Path::new(path)), expected, "{path}");
        }
    }

    fn payload_entries(files: &[&str]) -> Vec<(String, EntryKind)> {
        let mut out = vec![("yerd".to_owned(), EntryKind::Dir)];
        out.extend(files.iter().map(|f| (format!("yerd/{f}"), EntryKind::File)));
        out
    }

    #[test]
    fn validate_payload_accepts_the_required_set_in_swap_order() {
        for macos in [true, false] {
            let required = required_files(macos);
            let order = validate_payload(&payload_entries(required), required).unwrap();
            assert_eq!(order, required.to_vec(), "macos={macos}");
        }
    }

    #[test]
    fn validate_payload_swaps_extras_before_yerdd_and_yerd() {
        let required = required_files(false);
        let mut entries = payload_entries(required);
        entries.push(("yerd/yerd-zz-tool".to_owned(), EntryKind::File));
        entries.push(("yerd/yerd-aa-notes.txt".to_owned(), EntryKind::File));
        assert_eq!(
            validate_payload(&entries, required).unwrap(),
            vec![
                "yerd-install-receipt",
                "yerd-helper",
                "yerd-aa-notes.txt",
                "yerd-zz-tool",
                "yerdd",
                "yerd",
            ]
        );
    }

    #[test]
    fn check_registered_dir_refuses_another_installs_daemon() {
        let dir = Path::new("/nonexistent-yerd/home/u/.yerd/bin");
        assert_eq!(check_registered_dir(None, dir), Ok(()));
        assert_eq!(check_registered_dir(Some(&dir.join("yerdd")), dir), Ok(()));
        let err = check_registered_dir(Some(Path::new("/nonexistent-yerd/opt/yerd/yerdd")), dir)
            .unwrap_err();
        assert!(err.contains("/nonexistent-yerd/opt/yerd/yerdd"), "{err}");
        assert!(err.contains("Nothing was changed"), "{err}");
    }

    #[test]
    fn safe_extra_name_table() {
        let cases = [
            ("yerd-mcp", true),
            ("yerd-tool_2.0", true),
            ("yerd-", false),
            ("README.md", false),
            ("tool_2.0", false),
            ("yerdx", false),
            ("", false),
            (".hidden", false),
            ("..", false),
            (".", false),
            ("a/b", false),
            ("a b", false),
            ("yerd.yerd-old-123", false),
            ("yerd-update.lock", false),
            ("caf\u{e9}", false),
            ("x\0y", false),
        ];
        for (name, want) in cases {
            assert_eq!(is_safe_extra_name(name), want, "{name:?}");
        }
    }

    #[test]
    fn validate_payload_rejects_malformed_archives() {
        let expected = required_files(false);
        let mut hidden = payload_entries(expected);
        hidden.push(("yerd/.evil".to_owned(), EntryKind::File));
        let mut extra_link = payload_entries(expected);
        extra_link.push(("yerd/yerd-tool".to_owned(), EntryKind::Other));
        let mut unprefixed = payload_entries(expected);
        unprefixed.push(("yerd/README.md".to_owned(), EntryKind::File));
        let missing = payload_entries(&["yerd-install-receipt", "yerd-helper", "yerd"]);
        let mut symlink = payload_entries(&["yerd-install-receipt", "yerd-helper", "yerd"]);
        symlink.push(("yerd/yerdd".to_owned(), EntryKind::Other));
        let no_top: Vec<(String, EntryKind)> = payload_entries(expected)
            .into_iter()
            .filter(|(p, _)| p != "yerd")
            .collect();
        let mut top_is_link = no_top.clone();
        top_is_link.push(("yerd".to_owned(), EntryKind::Other));
        let mut stray_root = payload_entries(expected);
        stray_root.push(("other".to_owned(), EntryKind::File));
        let mut nested = payload_entries(expected);
        nested.push(("yerd/yerd-sub".to_owned(), EntryKind::Dir));
        nested.push(("yerd/yerd-sub/file".to_owned(), EntryKind::File));
        let mut dotdot = payload_entries(expected);
        dotdot.push(("yerd/..".to_owned(), EntryKind::Dir));
        let cases = [
            (hidden, "unexpected entry: yerd/.evil"),
            (extra_link, "yerd/yerd-tool is not a regular file"),
            (unprefixed, "unexpected entry: yerd/README.md"),
            (missing, "missing yerdd"),
            (symlink, "yerd/yerdd is not a regular file"),
            (no_top, "no top-level yerd/"),
            (top_is_link, "yerd is not a directory"),
            (stray_root, "unexpected entry: other"),
            (nested, "yerd/yerd-sub is not a regular file"),
            (dotdot, "unexpected entry: yerd/.."),
        ];
        for (entries, needle) in cases {
            let err = validate_payload(&entries, expected).unwrap_err();
            assert!(err.contains(needle), "expected {needle:?} in {err:?}");
        }
    }

    #[test]
    fn port_capability_from_getcap_table() {
        let cases = [
            (None, true),
            (Some(""), false),
            (
                Some("/home/u/.yerd/bin/yerdd cap_net_bind_service=ep\n"),
                true,
            ),
            (
                Some("/home/u/.yerd/bin/yerdd = cap_net_bind_service+ep\n"),
                true,
            ),
            (Some("/home/u/.yerd/bin/yerdd cap_sys_nice=ep\n"), false),
        ];
        for (out, want) in cases {
            assert_eq!(port_capability_from_getcap(out), want, "{out:?}");
        }
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    mod headless_swap {
        use std::os::unix::fs::PermissionsExt as _;

        use super::*;

        const RECEIPT: &str = yerd_platform::pure::install_receipt::RECEIPT_FILE;

        fn names() -> &'static [&'static str] {
            required_files(cfg!(target_os = "macos"))
        }

        fn install_dir(tmp: &Path) -> PathBuf {
            let dir = tmp.join("bin");
            std::fs::create_dir(&dir).unwrap();
            for name in names() {
                std::fs::write(dir.join(name), format!("old {name}")).unwrap();
            }
            dir
        }

        fn build_tarball(
            tmp: &Path,
            receipt: &str,
            extra: Option<&str>,
            skip: Option<&str>,
        ) -> PathBuf {
            let src = tmp.join("src");
            let top = src.join("yerd");
            std::fs::create_dir_all(&top).unwrap();
            for name in names().iter().filter(|n| Some(**n) != skip) {
                let body = if *name == RECEIPT {
                    receipt.to_owned()
                } else {
                    format!("new {name}")
                };
                std::fs::write(top.join(name), body).unwrap();
            }
            if let Some(extra) = extra {
                std::fs::write(top.join(extra), "extra").unwrap();
            }
            let out = tmp.join("headless.tar.gz");
            let status = Command::new("tar")
                .env("COPYFILE_DISABLE", "1")
                .arg("-czf")
                .arg(&out)
                .arg("-C")
                .arg(&src)
                .arg("yerd")
                .status()
                .unwrap();
            assert!(status.success());
            out
        }

        fn assert_untouched(dir: &Path) {
            for name in names() {
                assert_eq!(
                    std::fs::read_to_string(dir.join(name)).unwrap(),
                    format!("old {name}"),
                    "{name} must be left as it was"
                );
            }
        }

        fn leftovers(dir: &Path) -> Vec<String> {
            std::fs::read_dir(dir)
                .unwrap()
                .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
                .filter(|n| !names().contains(&n.as_str()))
                .collect()
        }

        #[test]
        fn well_formed_payload_swaps_every_file_with_modes() {
            let tmp = tempfile::tempdir().unwrap();
            let dir = install_dir(tmp.path());
            let tarball = build_tarball(tmp.path(), "headless\n", None, None);

            install_headless(&tarball, &dir, "t1", None).unwrap();

            for name in names() {
                let path = dir.join(name);
                let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
                if *name == RECEIPT {
                    assert_eq!(std::fs::read_to_string(&path).unwrap(), "headless\n");
                    assert_eq!(mode, 0o644, "{name}");
                } else {
                    assert_eq!(
                        std::fs::read_to_string(&path).unwrap(),
                        format!("new {name}")
                    );
                    assert_eq!(mode, 0o755, "{name}");
                }
            }
            assert!(leftovers(&dir).is_empty(), "{:?}", leftovers(&dir));
        }

        #[test]
        fn extra_payload_files_are_installed_alongside() {
            let tmp = tempfile::tempdir().unwrap();
            let dir = install_dir(tmp.path());
            let tarball = build_tarball(tmp.path(), "headless\n", Some("yerd-extra"), None);

            install_headless(&tarball, &dir, "t5", None).unwrap();

            assert_eq!(
                std::fs::read_to_string(dir.join("yerd-extra")).unwrap(),
                "extra"
            );
            assert_eq!(
                std::fs::read_to_string(dir.join("yerdd")).unwrap(),
                "new yerdd"
            );
            assert_eq!(leftovers(&dir), vec!["yerd-extra".to_owned()]);
        }

        #[test]
        fn existing_extra_target_is_replaced() {
            let tmp = tempfile::tempdir().unwrap();
            let dir = install_dir(tmp.path());
            std::fs::write(dir.join("yerd-extra"), "old extra").unwrap();
            let tarball = build_tarball(tmp.path(), "headless\n", Some("yerd-extra"), None);

            install_headless(&tarball, &dir, "t6", None).unwrap();

            assert_eq!(
                std::fs::read_to_string(dir.join("yerd-extra")).unwrap(),
                "extra"
            );
            assert_eq!(leftovers(&dir), vec!["yerd-extra".to_owned()]);
        }

        #[test]
        fn malformed_payloads_are_rejected_before_any_swap() {
            let cases: [(&str, Option<&str>, Option<&str>, &str); 3] = [
                ("headless", Some(".evil"), None, "unexpected entry"),
                ("headless", None, Some("yerdd"), "missing yerdd"),
                ("gui", None, None, "not a headless build"),
            ];
            for (receipt, extra, skip, needle) in cases {
                let tmp = tempfile::tempdir().unwrap();
                let dir = install_dir(tmp.path());
                let tarball = build_tarball(tmp.path(), receipt, extra, skip);
                let err = install_headless(&tarball, &dir, "t2", None).unwrap_err();
                assert!(err.contains(needle), "expected {needle:?} in {err:?}");
                assert_untouched(&dir);
                assert!(leftovers(&dir).is_empty(), "{:?}", leftovers(&dir));
            }
        }

        #[test]
        fn symlink_in_payload_is_rejected_before_any_swap() {
            let tmp = tempfile::tempdir().unwrap();
            let dir = install_dir(tmp.path());
            let top = tmp.path().join("src/yerd");
            std::fs::create_dir_all(&top).unwrap();
            for name in names() {
                if *name == "yerdd" {
                    std::os::unix::fs::symlink("/bin/sh", top.join(name)).unwrap();
                } else {
                    std::fs::write(top.join(name), "headless").unwrap();
                }
            }
            let tarball = tmp.path().join("headless.tar.gz");
            let status = Command::new("tar")
                .env("COPYFILE_DISABLE", "1")
                .arg("-czf")
                .arg(&tarball)
                .arg("-C")
                .arg(tmp.path().join("src"))
                .arg("yerd")
                .status()
                .unwrap();
            assert!(status.success());

            let err = install_headless(&tarball, &dir, "t3", None).unwrap_err();
            assert!(err.contains("yerd/yerdd is not a regular file"), "{err}");
            assert_untouched(&dir);
        }

        #[test]
        fn failure_mid_swap_restores_the_originals() {
            let tmp = tempfile::tempdir().unwrap();
            let dir = install_dir(tmp.path());
            let payload = tmp.path().join("payload");
            std::fs::create_dir(&payload).unwrap();
            for name in names().iter().filter(|n| **n != "yerd") {
                std::fs::write(payload.join(name), format!("new {name}")).unwrap();
            }

            let err = swap_files(&dir, &payload, names(), "t4").unwrap_err();
            assert!(err.contains("replacing"), "{err}");
            assert_untouched(&dir);
            assert!(leftovers(&dir).is_empty(), "{:?}", leftovers(&dir));
        }

        #[test]
        fn failure_mid_swap_removes_a_newly_created_extra() {
            let tmp = tempfile::tempdir().unwrap();
            let dir = install_dir(tmp.path());
            std::fs::write(dir.join("old-extra"), "old extra").unwrap();
            let payload = tmp.path().join("payload");
            std::fs::create_dir(&payload).unwrap();
            for name in names().iter().filter(|n| **n != "yerd") {
                std::fs::write(payload.join(name), format!("new {name}")).unwrap();
            }
            std::fs::write(payload.join("new-extra"), "new").unwrap();
            std::fs::write(payload.join("old-extra"), "new").unwrap();
            let mut order: Vec<&str> = names()
                .iter()
                .copied()
                .filter(|n| !SWAPPED_LAST.contains(n))
                .collect();
            order.extend(["new-extra", "old-extra", "yerdd", "yerd"]);

            let err = swap_files(&dir, &payload, &order, "t7").unwrap_err();
            assert!(err.contains("replacing"), "{err}");
            assert_untouched(&dir);
            assert!(!dir.join("new-extra").exists());
            assert_eq!(
                std::fs::read_to_string(dir.join("old-extra")).unwrap(),
                "old extra"
            );
            assert_eq!(leftovers(&dir), vec!["old-extra".to_owned()]);
        }
    }
}
