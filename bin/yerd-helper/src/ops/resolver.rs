//! `install-resolver` and `uninstall-resolver` for Linux + macOS.

use std::net::SocketAddr;
use std::path::PathBuf;

#[cfg(target_os = "macos")]
use yerd_platform::pure::resolver_file;
#[cfg(target_os = "linux")]
use yerd_platform::pure::{networkmanager_dnsmasq, resolv_conf, resolved_drop_in};

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

#[cfg(target_os = "linux")]
fn networkmanager_path() -> PathBuf {
    PathBuf::from("/etc/NetworkManager/conf.d/yerd-dnsmasq.conf")
}

#[cfg(target_os = "linux")]
fn dnsmasq_path(tld: &str) -> PathBuf {
    PathBuf::from(format!("/etc/NetworkManager/dnsmasq.d/yerd-{tld}.conf"))
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
    let resolved_runtime = std::fs::metadata("/run/systemd/resolve").is_ok_and(|m| m.is_dir());
    match resolv_conf::detect_linux_backend(&resolv, resolved_runtime) {
        resolv_conf::LinuxResolverBackend::SystemdResolved => {
            let dest = drop_in_path(tld_obj.as_str());
            let body = resolved_drop_in::compose(tld_obj.as_str(), addr);
            if std::fs::read_to_string(&dest).is_ok_and(|text| text == body) {
                return Ok(());
            }
            atomic_write(&dest, body.as_bytes(), true)?;
            run_command(
                "systemctl",
                "systemctl",
                ["reload-or-restart", "systemd-resolved"],
            )?;
            Ok(())
        }
        resolv_conf::LinuxResolverBackend::NetworkManager => {
            install_networkmanager(tld_obj.as_str(), addr)
        }
        resolv_conf::LinuxResolverBackend::Unsupported => Err(HelperError::Unsupported {
            operation: yerd_platform::error::ops::INSTALL_RESOLVER,
        }),
    }
}

/// Install through `NetworkManager`, preflighting dependencies before any
/// write, then polling until its dnsmasq listener answers a Yerd-domain query.
#[cfg(target_os = "linux")]
fn install_networkmanager(tld: &str, addr: SocketAddr) -> Result<(), HelperError> {
    run_command("dnsmasq", "dnsmasq", ["--version"])?;
    run_command("nmcli", "nmcli", ["--version"])?;

    let nm_path = networkmanager_path();
    let dns_path = dnsmasq_path(tld);
    let nm_body = networkmanager_dnsmasq::compose_networkmanager();
    let dns_body = networkmanager_dnsmasq::compose_dnsmasq(tld, addr);
    let files_match = std::fs::read_to_string(&nm_path)
        .is_ok_and(|text| networkmanager_dnsmasq::matches_networkmanager(&text))
        && std::fs::read_to_string(&dns_path)
            .is_ok_and(|text| networkmanager_dnsmasq::matches_dnsmasq(&text, tld, addr));
    if files_match && networkmanager_ready(tld) {
        return Ok(());
    }

    let old_nm = std::fs::read(&nm_path).ok();
    let old_dns = std::fs::read(&dns_path).ok();
    if let Err(error) = atomic_write(&nm_path, nm_body.as_bytes(), true)
        .and_then(|()| atomic_write(&dns_path, dns_body.as_bytes(), true))
    {
        restore_file(&nm_path, old_nm.as_deref());
        restore_file(&dns_path, old_dns.as_deref());
        return Err(error);
    }

    let applied = reload_networkmanager().and_then(|()| {
        if wait_for_networkmanager(tld) {
            Ok(())
        } else {
            Err(HelperError::ResolverPostcondition {
                reason:
                    "NetworkManager dnsmasq did not answer through the active 127.0.0.1 resolver",
            })
        }
    });
    if let Err(error) = applied {
        restore_file(&nm_path, old_nm.as_deref());
        restore_file(&dns_path, old_dns.as_deref());
        if let Err(rollback_error) = reload_networkmanager() {
            eprintln!("    warning: resolver rollback reload failed: {rollback_error}");
        }
        return Err(error);
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn wait_for_networkmanager(tld: &str) -> bool {
    for _ in 0..20 {
        if networkmanager_ready(tld) {
            return true;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    false
}

#[cfg(target_os = "linux")]
fn networkmanager_ready(tld: &str) -> bool {
    let resolv = std::fs::read_to_string("/etc/resolv.conf").unwrap_or_default();
    resolv_conf::networkmanager_dnsmasq_is_active(&resolv) && probe_dnsmasq(tld)
}

#[cfg(target_os = "linux")]
fn probe_dnsmasq(tld: &str) -> bool {
    use std::net::UdpSocket;
    use std::time::Duration;

    fn query(tld: &str) -> Option<Vec<u8>> {
        let mut packet = vec![0x59, 0x44, 0x01, 0x00, 0x00, 0x01, 0, 0, 0, 0, 0, 0];
        for label in ["yerd-resolver-probe", tld] {
            let len = u8::try_from(label.len()).ok()?;
            packet.push(len);
            packet.extend_from_slice(label.as_bytes());
        }
        packet.extend_from_slice(&[0, 0, 1, 0, 1]);
        Some(packet)
    }

    let Some(query) = query(tld) else {
        return false;
    };
    let socket = match UdpSocket::bind("127.0.0.1:0") {
        Ok(socket) => socket,
        Err(_) => return false,
    };
    if socket
        .set_read_timeout(Some(Duration::from_millis(150)))
        .is_err()
        || socket.connect("127.0.0.1:53").is_err()
        || socket.send(&query).is_err()
    {
        return false;
    }
    let mut response = [0_u8; 512];
    let size = match socket.recv(&mut response) {
        Ok(size) => size,
        Err(_) => return false,
    };
    dns_response_has_loopback_a(&response[..size])
}

#[cfg(target_os = "linux")]
fn dns_response_has_loopback_a(packet: &[u8]) -> bool {
    packet.len() >= 12
        && packet.starts_with(&[0x59, 0x44])
        && packet.get(2).is_some_and(|flags| flags & 0x80 != 0)
        && packet.get(3).is_some_and(|flags| flags & 0x0f == 0)
        && packet.windows(14).any(|window| {
            window.starts_with(&[0, 1, 0, 1])
                && window.get(8..10) == Some([0, 4].as_slice())
                && window.ends_with(&[127, 0, 0, 1])
        })
}

#[cfg(target_os = "linux")]
fn reload_networkmanager() -> Result<(), HelperError> {
    run_command("nmcli", "nmcli", ["general", "reload", "conf", "dns-full"]).map(|_| ())
}

#[cfg(target_os = "linux")]
fn restore_file(path: &std::path::Path, previous: Option<&[u8]>) {
    let result = match previous {
        Some(bytes) => atomic_write(path, bytes, true),
        None => remove_if_present(path).map(|_| ()),
    };
    if let Err(error) = result {
        eprintln!(
            "    warning: could not restore resolver file {}: {error}",
            path.display()
        );
    }
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
    let resolved_removed = remove_if_present(&drop_in_path(tld_obj.as_str()))?;
    let nm_removed = remove_if_present(&networkmanager_path())?
        | remove_if_present(&dnsmasq_path(tld_obj.as_str()))?;
    if resolved_removed {
        let _ = run_command(
            "systemctl",
            "systemctl",
            ["reload-or-restart", "systemd-resolved"],
        );
    }
    if nm_removed {
        let _ = reload_networkmanager();
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn remove_if_present(path: &std::path::Path) -> Result<bool, HelperError> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(source) => Err(HelperError::Io {
            path: path.to_path_buf(),
            source,
        }),
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

    #[cfg(target_os = "linux")]
    #[test]
    fn networkmanager_paths_have_expected_shape() {
        assert_eq!(
            networkmanager_path(),
            PathBuf::from("/etc/NetworkManager/conf.d/yerd-dnsmasq.conf")
        );
        assert_eq!(
            dnsmasq_path("test"),
            PathBuf::from("/etc/NetworkManager/dnsmasq.d/yerd-test.conf")
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn dns_probe_response_requires_successful_loopback_a_answer() {
        let mut response = vec![0x59, 0x44, 0x81, 0x80, 0, 1, 0, 1, 0, 0, 0, 0];
        response.extend_from_slice(&[0xc0, 0x0c, 0, 1, 0, 1, 0, 0, 0, 60, 0, 4, 127, 0, 0, 1]);
        assert!(dns_response_has_loopback_a(&response));
        response[3] = 0x83;
        assert!(!dns_response_has_loopback_a(&response));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn resolver_file_path_shape() {
        use std::path::Path;
        assert_eq!(resolver_file_path("test"), Path::new("/etc/resolver/test"));
    }
}
