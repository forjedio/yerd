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
    let runtime_dir_exists = std::fs::metadata("/run/systemd/resolve")
        .is_ok_and(|m| m.is_dir());
    if !resolv_conf::detect_systemd_resolved(&resolv, runtime_dir_exists) {
        // No safe automatic edit path on Phase 1. /etc/resolv.conf is
        // rewritten by NetworkManager / resolvconf / cloud-init on
        // many distros; we refuse rather than do something fragile.
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
    atomic_write(&dest, body.as_bytes(), true)?;
    // macOS picks up new /etc/resolver/<tld> at the next query — no
    // reload needed.
    Ok(())
}

// ---- uninstall-resolver --------------------------------------------

#[cfg(target_os = "linux")]
pub fn uninstall_resolver(tld: &str) -> Result<(), HelperError> {
    let tld_obj = validate::require_valid_tld(tld)?;
    let path = drop_in_path(tld_obj.as_str());
    match std::fs::remove_file(&path) {
        Ok(()) => {
            // Reload so resolved picks up the removal. If the unit is
            // inactive (e.g. resolved isn't even installed) systemctl
            // will surface an error; we accept that since the drop-in
            // is already gone.
            let _ = run_command(
                "systemctl",
                "systemctl",
                ["reload-or-restart", "systemd-resolved"],
            );
            Ok(())
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()), // idempotent
        Err(source) => Err(HelperError::Io { path, source }),
    }
}

#[cfg(target_os = "macos")]
pub fn uninstall_resolver(tld: &str) -> Result<(), HelperError> {
    let tld_obj = validate::require_valid_tld(tld)?;
    let path = resolver_file_path(tld_obj.as_str());
    match std::fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()), // idempotent
        Err(source) => Err(HelperError::Io { path, source }),
    }
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
        assert_eq!(resolver_file_path("test"), Path::new("/etc/resolver/test"));
    }
}
