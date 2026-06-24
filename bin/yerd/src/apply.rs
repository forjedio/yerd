//! Self-update applier: install a staged, verified artifact.
//!
//! Two invocation modes, both ending in [`run`]:
//! - **CLI** (`yerd update --yes`): calls [`run`] **in-process** (via
//!   `spawn_blocking`). The CLI is short-lived — it swaps the bundle off its own
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
//! exercisable in CI — they are the Phase B hardware-spike preconditions.
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
/// spawner) is what makes this a hidden, non-discoverable entry point — there is
/// no clap subcommand, so it never appears in `--help` or shell completions.
pub const APPLY_ENV: &str = "YERD_APPLY_UPDATE";
/// Env var carrying the staged artifact path.
pub const APPLY_PATH_ENV: &str = "YERD_APPLY_PATH";
/// Env var carrying the artifact kind (`"deb"` / anything else = app tarball).
pub const APPLY_KIND_ENV: &str = "YERD_APPLY_KIND";
/// Env var: `"1"` to relaunch the GUI after the install.
pub const APPLY_RELAUNCH_GUI_ENV: &str = "YERD_APPLY_RELAUNCH_GUI";
/// argv sentinel for the elevated Linux deb-install re-exec. `pkexec` strips the
/// environment, so the staged path is passed positionally. Internal; not a clap
/// subcommand, so it never appears in help/completions.
pub const INSTALL_DEB_ARG: &str = "__yerd-install-deb";

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

/// If invoked in applier mode (the [`APPLY_ENV`] var is set), run the apply and
/// return its exit code; otherwise `None` (normal CLI dispatch proceeds). All
/// inputs travel via env vars so nothing leaks into the argv-driven help.
#[must_use]
pub fn run_from_env() -> Option<ExitCode> {
    // Not in apply mode → let normal CLI dispatch proceed.
    std::env::var_os(APPLY_ENV)?;
    let Some(path) = std::env::var_os(APPLY_PATH_ENV) else {
        eprintln!("yerd: {APPLY_PATH_ENV} is required in apply mode");
        return Some(ExitCode::from(2));
    };
    // Fail closed on an unknown/typoed kind rather than silently picking the
    // macOS installer — the value is set by our own GUI (`commands.rs`).
    let kind = match std::env::var(APPLY_KIND_ENV).as_deref() {
        Ok("deb") => StagedArtifact::Deb,
        Ok("app_tar_gz") => StagedArtifact::AppTarGz,
        other => {
            eprintln!(
                "yerd: invalid {APPLY_KIND_ENV}={other:?} (expected \"deb\" or \"app_tar_gz\")"
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
        // `StagedArtifact` is `#[non_exhaustive]`: a newer daemon could stage a
        // kind this binary doesn't know how to install.
        _ => Err("unknown staged artifact kind from the daemon".to_owned()),
    };
    match result {
        Ok(()) => {
            println!("yerd: update applied; Yerd is restarting");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("yerd: update failed: {e}");
            // The GUI quits before spawning us; on failure (bundle rolled back),
            // bring it back so a failed update doesn't strand the user appless.
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
    // Resolve the running bundle: current_exe is <Bundle>.app/Contents/MacOS/yerd.
    let exe = std::env::current_exe().map_err(|e| format!("current_exe: {e}"))?;
    let bundle = exe
        .ancestors()
        .nth(3)
        .filter(|p| p.extension().is_some_and(|x| x == "app"))
        .ok_or_else(|| "not running from an .app bundle (dev build?)".to_owned())?
        .to_path_buf();

    // Guard: only update an app installed in /Applications (App Translocation /
    // dev runs make in-place replacement impossible or unsafe).
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
    // Directory write access is what a rename needs (the parent dir, not the
    // bundle). Elevation for the swap is a follow-up; for now require it writable.
    if !dir_is_writable(&parent) {
        return Err(format!(
            "{} is not writable by you; elevated self-update is not yet wired — \
             reinstall from the .dmg",
            parent.display()
        ));
    }

    // Unique, *exclusively-created* staging dir on the same volume as the bundle.
    // A fixed name (`.yerd-staging`) was a local-attacker planting risk — a unique
    // name + `create_dir` (fails if the path exists) means we never extract into
    // or swap from a directory someone else pre-created.
    let uniq = unique_suffix();
    let stage = parent.join(format!(".yerd-staging-{uniq}"));
    std::fs::create_dir(&stage)
        .map_err(|e| format!("creating staging dir {}: {e}", stage.display()))?;

    // Everything that can fail after the daemon is stopped runs in this closure so
    // a single cleanup (remove staging) + daemon-restart-on-failure covers every
    // early return.
    let mut stopped = false;
    let result = (|| -> Result<(), String> {
        same_volume(&parent, &stage)?;
        // Extract via system `tar` (bsdtar) so the notarization staple's xattrs
        // survive — the Rust `tar` crate drops xattrs.
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
        // Reject anything but exactly one bundle — defends against a planted .app.
        let new_app = find_single_dot_app(&stage)?;

        // Stop the daemon so it releases its executable inode, then swap.
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
            // If we got as far as stopping the daemon, the swap rolled the bundle
            // back — restart the (now-original) daemon so we don't leave it down.
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
            // Roll back: put the original back so we never leave the app missing.
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
    // A probe create/remove is the most reliable cross-config check.
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
        // Already root (unusual direct invocation): install in place. A direct
        // root run can't reach the user session to restart the daemon, so that
        // path relies on the systemd unit / next login — acceptable for it.
        return elevated_install_deb(staged);
    }
    // Elevate ONLY the verify+install, by re-exec'ing ourselves under pkexec. The
    // staged path travels as argv (pkexec sanitizes the environment), and the
    // elevated process reads + verifies + installs the bytes *once under root*
    // from root-owned storage — so a same-uid attacker can't swap the
    // user-writable staged file between verification and dpkg's read.
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
    // Restart the daemon in the user session (NOT as root) and relaunch the GUI.
    restart_services(relaunch_gui);
    Ok(())
}

/// Elevated installer (runs as root via the `pkexec` re-exec). Reads + verifies
/// the staged `.deb` **once**, copies the verified bytes into a root-owned 0700
/// dir, and `dpkg -i`s that copy — closing the verify→re-read TOCTOU on the
/// user-writable staged path. `dpkg`'s postinst reapplies setcap + `/usr/bin`
/// symlinks.
#[cfg(target_os = "linux")]
fn elevated_install_deb(staged: &Path) -> Result<(), String> {
    use std::os::unix::fs::{DirBuilderExt as _, PermissionsExt as _};

    if !nix::unistd::geteuid().is_root() {
        return Err("the elevated installer must run as root".to_owned());
    }
    // Read the artifact + signature once, verify, then never re-read the
    // user-writable path.
    let bytes = std::fs::read(staged).map_err(|e| format!("reading staged .deb: {e}"))?;
    let sig_path = sibling_sig(staged);
    let sig = std::fs::read_to_string(&sig_path)
        .map_err(|e| format!("reading signature {}: {e}", sig_path.display()))?;
    verify_minisign(UPDATE_PUBLIC_KEY, &sig, &bytes).map_err(|e| e.to_string())?;

    // Root-owned, 0700, uniquely-named dir: a non-root attacker can neither enter
    // it nor replace the root-owned file inside (sticky /tmp).
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

#[cfg(target_os = "linux")]
fn relaunch_gui_app() {
    use std::os::unix::process::CommandExt as _;
    if let Some(gui) = sibling_gui() {
        // Detached (own process group) so it outlives this applier.
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

        // Target now holds the NEW bundle; the staged path is consumed.
        assert_eq!(
            std::fs::read_to_string(target.join("Contents/Info.plist")).unwrap(),
            "NEW"
        );
        assert!(!staged.exists());
        // The OLD bundle was preserved at the backup path.
        assert_eq!(
            std::fs::read_to_string(backup.join("Contents/Info.plist")).unwrap(),
            "OLD"
        );
    }

    #[test]
    fn swap_bundle_into_empty_target_works() {
        // First-ever placement (no existing target) must still install.
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
        // Force the second rename (new_app → target) to fail by pointing at a
        // non-existent staged bundle. The OLD bundle must be restored at target,
        // never left missing.
        let tmp = tempfile::tempdir().unwrap();
        let target = tmp.path().join("Yerd.app");
        let missing = tmp.path().join("does-not-exist.app");
        let backup = tmp.path().join(".backup.app");
        write_bundle(&target, "OLD");

        let err = swap_bundle(&target, &missing, &backup);
        assert!(err.is_err(), "swap should report the rename-in failure");
        // Rolled back: the original OLD bundle is back at target, not lost.
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
}
