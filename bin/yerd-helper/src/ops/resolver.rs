//! `install-resolver` and `uninstall-resolver` for Linux + macOS.

use std::net::SocketAddr;
use std::path::PathBuf;

#[cfg(target_os = "macos")]
use yerd_platform::pure::resolver_file;
#[cfg(target_os = "linux")]
use yerd_platform::pure::{resolv_conf, resolved_drop_in};

use crate::error::HelperError;
#[cfg(target_os = "macos")]
use crate::ops::atomic_write;
#[cfg(target_os = "linux")]
use crate::ops::{atomic_write, run_command};
use crate::validate;

#[cfg(target_os = "linux")]
fn drop_in_path(tld: &str) -> PathBuf {
    PathBuf::from(format!("/etc/systemd/resolved.conf.d/yerd-{tld}.conf"))
}

#[cfg(target_os = "macos")]
fn resolver_file_path(tld: &str) -> PathBuf {
    PathBuf::from(format!("/etc/resolver/{tld}"))
}

// ---- install-resolver ----------------------------------------------

#[cfg(target_os = "linux")]
pub fn install_resolver(tld: &str, addr: SocketAddr) -> Result<(), HelperError> {
    let tld_obj = validate::require_valid_tld(tld)?;
    let resolv = std::fs::read_to_string("/etc/resolv.conf").unwrap_or_default();
    let runtime_dir_exists = std::fs::metadata("/run/systemd/resolve").is_ok_and(|m| m.is_dir());
    if !resolv_conf::detect_systemd_resolved(&resolv, runtime_dir_exists) {
        return Err(HelperError::Unsupported {
            operation: yerd_platform::error::ops::INSTALL_RESOLVER,
        });
    }
    let dest = drop_in_path(tld_obj.as_str());
    let body = resolved_drop_in::compose(tld_obj.as_str(), addr);
    atomic_write(&dest, body.as_bytes(), true)?;
    run_command(
        "systemctl",
        "systemctl",
        ["reload-or-restart", "systemd-resolved"],
    )
    .map(|_| ())
}

#[cfg(target_os = "macos")]
pub fn install_resolver(tld: &str, addr: SocketAddr) -> Result<(), HelperError> {
    let tld_obj = validate::require_valid_tld(tld)?;
    let dest = resolver_file_path(tld_obj.as_str());
    let body = resolver_file::compose(addr);
    if let Ok(existing) = std::fs::read_to_string(&dest) {
        if !resolver_file::matches(&existing, addr) {
            macos_back_up_existing(tld_obj.as_str(), &dest, existing.as_bytes());
        }
    }
    atomic_write(&dest, body.as_bytes(), true)?;
    Ok(())
}

/// Best-effort copy of the about-to-be-replaced `/etc/resolver/<tld>` into the
/// system backups dir as `<tld>-<unixsecs>.conf`, printing where it went so the
/// CLI `elevate resolver` output surfaces it. **Any failure is logged and
/// swallowed** - backing up must never fail the install.
///
/// The dir is created mode `0755` (umask-proof) so the unprivileged daemon can
/// later traverse + list it to report the backup in `doctor`.
#[cfg(target_os = "macos")]
fn macos_back_up_existing(tld: &str, dest: &std::path::Path, content: &[u8]) {
    use std::os::unix::fs::{DirBuilderExt, PermissionsExt};

    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let dir = resolver_file::macos_backup_dir();

    if let Err(e) = std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o755)
        .create(&dir)
    {
        eprintln!(
            "    note: could not create resolver backup dir {}: {e}",
            dir.display()
        );
        return;
    }
    let _ = std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755));

    let backup = dir.join(resolver_file::backup_filename(tld, secs));
    match atomic_write(&backup, content, true) {
        Ok(()) => println!(
            "    backed up existing {} → {}",
            dest.display(),
            backup.display()
        ),
        Err(e) => eprintln!("    note: could not back up {}: {e}", dest.display()),
    }
}

// ---- uninstall-resolver --------------------------------------------

#[cfg(target_os = "linux")]
pub fn uninstall_resolver(tld: &str) -> Result<(), HelperError> {
    let tld_obj = validate::require_valid_tld(tld)?;
    let path = drop_in_path(tld_obj.as_str());
    match std::fs::remove_file(&path) {
        Ok(()) => {
            let _ = run_command(
                "systemctl",
                "systemctl",
                ["reload-or-restart", "systemd-resolved"],
            );
            Ok(())
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(source) => Err(HelperError::Io { path, source }),
    }
}

#[cfg(target_os = "macos")]
pub fn uninstall_resolver(tld: &str) -> Result<(), HelperError> {
    let tld_obj = validate::require_valid_tld(tld)?;
    let path = resolver_file_path(tld_obj.as_str());
    if macos_try_restore_backup(tld_obj.as_str(), &path)? {
        return Ok(());
    }
    match std::fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(source) => Err(HelperError::Io { path, source }),
    }
}

/// Restore the most recent `/etc/resolver/<tld>` backup (if any) over `dest`,
/// then delete every backup for `tld`. Returns `Ok(true)` when a backup was
/// restored, `Ok(false)` when there's nothing safe to restore (caller then
/// removes Yerd's file).
///
/// Security - the helper runs as **root** and the backup dir is world-traversable
/// (`0755`), so before trusting any backup we require the dir and the chosen file
/// to be **root-owned**, the file a non-symlink **regular** file with **no
/// group/other write bit**, and its contents to **parse** as a real resolver
/// file. Any check failing falls back to `Ok(false)` (plain removal) rather than
/// installing attacker-plantable bytes as the system `*.test` resolver.
#[cfg(target_os = "macos")]
fn macos_try_restore_backup(tld: &str, dest: &std::path::Path) -> Result<bool, HelperError> {
    use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};

    let dir = resolver_file::macos_backup_dir();
    match std::fs::symlink_metadata(&dir) {
        Ok(m) if m.is_dir() && m.uid() == 0 => {}
        _ => return Ok(false),
    }

    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Ok(false);
    };
    let names: Vec<String> = entries
        .flatten()
        .filter_map(|e| e.file_name().into_string().ok())
        .collect();

    let Some(latest) = resolver_file::latest_backup(&names, tld) else {
        return Ok(false);
    };
    let Some(secs) = resolver_file::parse_backup_secs(latest, tld) else {
        return Ok(false);
    };
    let backup = dir.join(resolver_file::backup_filename(tld, secs));

    match std::fs::symlink_metadata(&backup) {
        Ok(m)
            if m.file_type().is_file() && m.uid() == 0 && (m.permissions().mode() & 0o022) == 0 => {
        }
        _ => return Ok(false),
    }

    let Ok(bytes) = std::fs::read(&backup) else {
        return Ok(false);
    };
    if !resolver_file::restorable(&String::from_utf8_lossy(&bytes)) {
        return Ok(false);
    }

    atomic_write(dest, &bytes, true)?;

    for name in &names {
        if resolver_file::parse_backup_secs(name, tld).is_some() {
            let _ = std::fs::remove_file(dir.join(name));
        }
    }
    println!(
        "    restored previous resolver {} → {}",
        backup.display(),
        dest.display()
    );
    Ok(true)
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod tests {
    use super::*;

    #[cfg(target_os = "linux")]
    #[test]
    fn drop_in_path_shape() {
        assert_eq!(
            drop_in_path("test"),
            PathBuf::from("/etc/systemd/resolved.conf.d/yerd-test.conf")
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn resolver_file_path_shape() {
        use std::path::Path;
        assert_eq!(resolver_file_path("test"), Path::new("/etc/resolver/test"));
    }
}
