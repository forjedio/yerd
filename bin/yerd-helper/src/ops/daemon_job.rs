//! `install-daemon-job` / `uninstall-daemon-job`. macOS only.
//!
//! Registers the headless daemon as a root-owned system-domain launchd job
//! (`/Library/LaunchDaemons/dev.yerd.daemon.plist`) that runs the `yerdd` beside
//! this helper as the invoking user from boot, with nobody logged in. See
//! `yerd_platform::pure::launchd_daemon` for the plist shape.
//!
//! The job is only ever written for a headless install: the helper's own
//! directory must carry a `headless` install receipt, so an app-bundle install
//! never gets a system job fighting its per-user launchd agent. The plist is
//! root:wheel 0644, so only root can change `UserName` or `ProgramArguments`;
//! launchd runs `yerdd` as that user, never as root.

use crate::error::HelperError;

#[cfg(target_os = "macos")]
use std::os::unix::fs::PermissionsExt as _;
#[cfg(target_os = "macos")]
use std::path::{Path, PathBuf};

#[cfg(target_os = "macos")]
use yerd_platform::install_mode::read_receipt_in;
#[cfg(target_os = "macos")]
use yerd_platform::pure::install_receipt::{self, InstallMode};
#[cfg(target_os = "macos")]
use yerd_platform::pure::launchd_daemon::{self, DAEMON_LABEL, DAEMON_PLIST};
#[cfg(target_os = "macos")]
use yerd_platform::pure::launchd_ports::is_persistent_forwarder_path;

#[cfg(target_os = "macos")]
use crate::error::ValidationReason;
#[cfg(target_os = "macos")]
use crate::ops::{
    atomic_write, bootout_system, bootout_system_checked, bootstrap_system, invalid,
    remove_if_present, resolve_user, run_command,
};

#[cfg(target_os = "macos")]
const DAEMON_BIN: &str = "yerdd";

/// Write and load the daemon job for `uid`, replacing any existing one.
///
/// Validates the user, the sibling `yerdd`, and the install receipt before
/// touching the system. If the new job can't be loaded, the previous plist (if
/// any) is put back and reloaded, so a failed re-install never leaves a server
/// without its daemon at the next boot; with no previous plist the new one is
/// removed so `yerd-service-ctl` never targets a job that isn't there.
#[cfg(target_os = "macos")]
pub fn install(uid: u32) -> Result<(), HelperError> {
    let user = resolve_user(uid)?;
    let dir = helper_dir()?;
    require_headless(&dir)?;
    let yerdd = locate_daemon(&dir)?;
    let plist = launchd_daemon::compose_daemon_plist(&yerdd, &user.name, &user.dir);
    let previous = read_previous_plist()?;
    atomic_write(Path::new(DAEMON_PLIST), plist.as_bytes(), true)?;
    let result = load_plist();
    if result.is_err() {
        bootout_system(DAEMON_LABEL);
        match previous {
            Some(old) => {
                let _ =
                    atomic_write(Path::new(DAEMON_PLIST), &old, true).and_then(|()| load_plist());
            }
            None => {
                let _ = remove_if_present(DAEMON_PLIST);
            }
        }
    }
    result
}

/// Unload the daemon job and delete its plist (absent is fine). The plist is
/// only deleted once the job is confirmed unloaded.
#[cfg(target_os = "macos")]
pub fn uninstall() -> Result<(), HelperError> {
    bootout_system_checked(DAEMON_LABEL)?;
    remove_if_present(DAEMON_PLIST)
}

/// Make the written plist root-owned and (re)load the job from it.
#[cfg(target_os = "macos")]
fn load_plist() -> Result<(), HelperError> {
    run_command("chown", "/usr/sbin/chown", ["root:wheel", DAEMON_PLIST])?;
    bootout_system(DAEMON_LABEL);
    bootstrap_system(DAEMON_PLIST)
}

/// The current daemon plist's bytes, if one is registered.
#[cfg(target_os = "macos")]
fn read_previous_plist() -> Result<Option<Vec<u8>>, HelperError> {
    match std::fs::read(DAEMON_PLIST) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(source) => Err(HelperError::Io {
            path: PathBuf::from(DAEMON_PLIST),
            source,
        }),
    }
}

#[cfg(target_os = "macos")]
fn helper_dir() -> Result<PathBuf, HelperError> {
    let exe = std::env::current_exe()
        .and_then(std::fs::canonicalize)
        .map_err(|source| HelperError::Io {
            path: PathBuf::from("current_exe"),
            source,
        })?;
    exe.parent()
        .map(Path::to_path_buf)
        .ok_or_else(|| invalid(ValidationReason::DaemonBinaryInvalid(exe.clone())))
}

#[cfg(target_os = "macos")]
fn require_headless(dir: &Path) -> Result<(), HelperError> {
    match install_receipt::classify(read_receipt_in(dir)) {
        Ok(InstallMode::Headless) => Ok(()),
        Ok(InstallMode::Package) | Err(_) => Err(invalid(ValidationReason::NotHeadlessInstall)),
    }
}

/// `yerdd` in `dir`: a regular file that only its owner can write, on a path
/// that survives reboot.
#[cfg(target_os = "macos")]
fn locate_daemon(dir: &Path) -> Result<PathBuf, HelperError> {
    let yerdd = dir.join(DAEMON_BIN);
    let acceptable = std::fs::symlink_metadata(&yerdd)
        .is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o022 == 0)
        && is_persistent_forwarder_path(&yerdd);
    if acceptable {
        Ok(yerdd)
    } else {
        Err(invalid(ValidationReason::DaemonBinaryInvalid(yerdd)))
    }
}

/// Unsupported off macOS.
#[cfg(not(target_os = "macos"))]
pub fn install(_uid: u32) -> Result<(), HelperError> {
    Err(HelperError::Unsupported {
        operation: yerd_platform::error::ops::INSTALL_DAEMON_JOB,
    })
}

/// Unsupported off macOS.
#[cfg(not(target_os = "macos"))]
pub fn uninstall() -> Result<(), HelperError> {
    Err(HelperError::Unsupported {
        operation: yerd_platform::error::ops::UNINSTALL_DAEMON_JOB,
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[cfg(target_os = "macos")]
    #[test]
    fn rejects_root_uid_before_any_io() {
        assert!(matches!(
            install(0),
            Err(HelperError::Validation {
                reason: ValidationReason::UidInvalid
            })
        ));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn require_headless_needs_a_headless_receipt() {
        let tmp = tempfile::tempdir().unwrap();
        let not_headless = |dir: &Path| {
            matches!(
                require_headless(dir),
                Err(HelperError::Validation {
                    reason: ValidationReason::NotHeadlessInstall
                })
            )
        };
        assert!(not_headless(tmp.path()));
        let receipt = tmp.path().join(install_receipt::RECEIPT_FILE);
        std::fs::write(&receipt, "gui\n").unwrap();
        assert!(not_headless(tmp.path()));
        std::fs::write(&receipt, "headless\n").unwrap();
        assert!(require_headless(tmp.path()).is_ok());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn locate_daemon_rejects_missing_writable_and_symlinked_binaries() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().canonicalize().unwrap();
        let rejected = |dir: &Path| {
            matches!(
                locate_daemon(dir),
                Err(HelperError::Validation {
                    reason: ValidationReason::DaemonBinaryInvalid(_)
                })
            )
        };
        assert!(rejected(&dir));
        let yerdd = dir.join(DAEMON_BIN);
        std::fs::write(&yerdd, b"bin").unwrap();
        std::fs::set_permissions(&yerdd, std::fs::Permissions::from_mode(0o775)).unwrap();
        assert!(rejected(&dir));
        std::fs::set_permissions(&yerdd, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(locate_daemon(&dir).unwrap(), yerdd);

        let linked = tempfile::tempdir().unwrap();
        let linked_dir = linked.path().canonicalize().unwrap();
        std::os::unix::fs::symlink(&yerdd, linked_dir.join(DAEMON_BIN)).unwrap();
        assert!(rejected(&linked_dir));
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn unsupported_off_macos() {
        assert!(matches!(install(501), Err(HelperError::Unsupported { .. })));
        assert!(matches!(uninstall(), Err(HelperError::Unsupported { .. })));
    }
}
