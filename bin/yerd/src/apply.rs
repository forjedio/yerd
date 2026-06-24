//! Self-update applier: install a staged, verified artifact.
//!
//! Invoked as a **detached** `yerd` subprocess (gated by the `YERD_APPLY_UPDATE`
//! env var, so it never shows in help or completions) by `yerd update --yes` and
//! the GUI Update button. It runs **unprivileged in the user session**; only the
//! minimal privileged step is elevated (Linux `dpkg` via `pkexec`; macOS only if
//! `/Applications` isn't user-writable).
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
    let kind = match std::env::var(APPLY_KIND_ENV).as_deref() {
        Ok("deb") => StagedArtifact::Deb,
        _ => StagedArtifact::AppTarGz,
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

    // Stage on the SAME volume as the bundle so the rename is atomic.
    let stage = parent.join(".yerd-staging");
    let _ = std::fs::remove_dir_all(&stage);
    std::fs::create_dir_all(&stage).map_err(|e| format!("creating staging dir: {e}"))?;
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
        let _ = std::fs::remove_dir_all(&stage);
        return Err("extracting the update archive failed".to_owned());
    }
    let new_app = find_dot_app(&stage)?;

    // Stop the daemon so it releases its executable inode, then swap.
    if let Some(yerdd) = sibling_yerdd() {
        yerd_service_ctl::ServiceCtl::new(yerdd).stop();
    }
    let backup = parent.join(".yerd-backup.app");
    let _ = std::fs::remove_dir_all(&backup);
    swap_bundle(&bundle, &new_app, &backup).map_err(|e| format!("swapping bundle: {e}"))?;
    let _ = std::fs::remove_dir_all(&backup);
    let _ = std::fs::remove_dir_all(&stage);

    restart_services(relaunch_gui);
    Ok(())
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

/// Find the single `*.app` directory directly inside `dir`.
#[cfg(target_os = "macos")]
fn find_dot_app(dir: &Path) -> Result<PathBuf, String> {
    let entries = std::fs::read_dir(dir).map_err(|e| format!("reading staging dir: {e}"))?;
    for entry in entries.flatten() {
        let p = entry.path();
        if p.extension().is_some_and(|x| x == "app") {
            return Ok(p);
        }
    }
    Err("the update archive contained no .app bundle".to_owned())
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
    // The privileged step is just `dpkg -i`. Elevate via pkexec unless already
    // root. dpkg's postinst reapplies setcap + /usr/bin symlinks.
    let is_root = nix::unistd::geteuid().is_root();
    let status = if is_root {
        Command::new("dpkg").arg("-i").arg(staged).status()
    } else {
        Command::new("pkexec")
            .arg("dpkg")
            .arg("-i")
            .arg(staged)
            .status()
    }
    .map_err(|e| format!("spawning the installer: {e}"))?;
    if !status.success() {
        return Err("dpkg failed to install the new package".to_owned());
    }
    // Restart the daemon in the user session (NOT as root) and relaunch the GUI.
    restart_services(relaunch_gui);
    Ok(())
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
    fn sibling_sig_appends_sig_extension() {
        let p = Path::new("/cache/update/Yerd_MacOS_AppleSilicon_v2.app.tar.gz");
        assert_eq!(
            sibling_sig(p),
            Path::new("/cache/update/Yerd_MacOS_AppleSilicon_v2.app.tar.gz.sig")
        );
    }
}
