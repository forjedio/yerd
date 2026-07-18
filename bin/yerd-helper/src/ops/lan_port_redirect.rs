//! `install-lan-port-redirect` / `uninstall-lan-port-redirect`. macOS only.
//!
//! The M2 LAN strategy: the daemon binds its rootless ports on `0.0.0.0`, and
//! this pf `rdr` forwards inbound LAN 80/443 (destined for the host's LAN IP) to
//! those rootless ports on the same LAN IP. It lives in a **separate** anchor
//! (`dev.yerd.lan`) from the loopback `elevate ports` redirect, so tearing one
//! down never disturbs the other. See `yerd_platform::pure::pf_anchor` for the
//! rule text and the `/etc/pf.conf` editing strategy.

#![allow(clippy::similar_names)]

#[cfg(target_os = "macos")]
use std::path::Path;

#[cfg(target_os = "macos")]
use yerd_platform::pure::pf_anchor;

use crate::error::HelperError;
#[cfg(target_os = "macos")]
use crate::error::ValidationReason;
#[cfg(target_os = "macos")]
use crate::ops::{atomic_write, run_command};

// ---- install -------------------------------------------------------------

#[cfg(target_os = "macos")]
pub fn install_lan_port_redirect(
    lan_ip: std::net::Ipv4Addr,
    http_from: u16,
    http_to: u16,
    https_from: u16,
    https_to: u16,
) -> Result<(), HelperError> {
    // Defence in depth: the helper validates its own inputs, independent of the
    // caller. A redirect target must be a real routable address - never loopback
    // or unspecified - so a forged/mistaken invocation can't install a bogus rule.
    if lan_ip.is_loopback() || lan_ip.is_unspecified() {
        return Err(HelperError::Validation {
            reason: ValidationReason::LanIpInvalid,
        });
    }
    require_nonzero(http_from, "--http-from")?;
    require_nonzero(http_to, "--http-to")?;
    require_nonzero(https_from, "--https-from")?;
    require_nonzero(https_to, "--https-to")?;

    let rules =
        pf_anchor::compose_lan_anchor_rules(lan_ip, http_from, http_to, https_from, https_to);
    atomic_write(
        Path::new(pf_anchor::LAN_ANCHOR_PATH),
        rules.as_bytes(),
        true,
    )?;

    let original =
        std::fs::read_to_string(pf_anchor::PF_CONF_PATH).map_err(|source| HelperError::Io {
            path: pf_anchor::PF_CONF_PATH.into(),
            source,
        })?;
    let updated = pf_anchor::insert_lan_anchor_refs(&original);
    if updated != original {
        atomic_write(Path::new(pf_anchor::PF_CONF_PATH), updated.as_bytes(), true)?;
    }

    // `pfctl -f` applies + validates; roll the pf.conf edit back on rejection.
    if let Err(e) = run_command("pfctl", "/sbin/pfctl", ["-f", pf_anchor::PF_CONF_PATH]) {
        if updated != original {
            let _ = atomic_write(
                Path::new(pf_anchor::PF_CONF_PATH),
                original.as_bytes(),
                true,
            );
        }
        return Err(e);
    }
    ignore_command_failure(run_command("pfctl", "/sbin/pfctl", ["-e"]));

    let plist = pf_anchor::compose_lan_launchdaemon_plist();
    atomic_write(Path::new(pf_anchor::LAN_PLIST_PATH), plist.as_bytes(), true)?;
    run_command(
        "chown",
        "/usr/sbin/chown",
        ["root:wheel", pf_anchor::LAN_PLIST_PATH],
    )?;
    ignore_command_failure(run_command(
        "launchctl",
        "/bin/launchctl",
        ["bootout", "system", pf_anchor::LAN_PLIST_PATH],
    ));
    run_command(
        "launchctl",
        "/bin/launchctl",
        ["bootstrap", "system", pf_anchor::LAN_PLIST_PATH],
    )?;
    Ok(())
}

// ---- uninstall -----------------------------------------------------------

#[cfg(target_os = "macos")]
pub fn uninstall_lan_port_redirect() -> Result<(), HelperError> {
    ignore_command_failure(run_command(
        "launchctl",
        "/bin/launchctl",
        ["bootout", "system", pf_anchor::LAN_PLIST_PATH],
    ));

    // Remove the LAN hook lines (scoped to the LAN marker, so the loopback
    // anchor's refs survive) BEFORE deleting the anchor file, so pf.conf never
    // references a missing anchor.
    match std::fs::read_to_string(pf_anchor::PF_CONF_PATH) {
        Ok(pf_conf) => {
            let cleaned = pf_anchor::remove_lan_anchor_refs(&pf_conf);
            if cleaned != pf_conf {
                atomic_write(Path::new(pf_anchor::PF_CONF_PATH), cleaned.as_bytes(), true)?;
                ignore_command_failure(run_command(
                    "pfctl",
                    "/sbin/pfctl",
                    ["-f", pf_anchor::PF_CONF_PATH],
                ));
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(source) => {
            return Err(HelperError::Io {
                path: pf_anchor::PF_CONF_PATH.into(),
                source,
            })
        }
    }

    remove_if_present(pf_anchor::LAN_ANCHOR_PATH)?;
    remove_if_present(pf_anchor::LAN_PLIST_PATH)?;
    Ok(())
}

#[cfg(target_os = "macos")]
fn require_nonzero(port: u16, flag: &'static str) -> Result<(), HelperError> {
    if port == 0 {
        return Err(HelperError::Validation {
            reason: ValidationReason::PortInvalid(flag),
        });
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn ignore_command_failure(result: Result<std::process::Output, HelperError>) {
    let _ = result;
}

#[cfg(target_os = "macos")]
fn remove_if_present(path: &str) -> Result<(), HelperError> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(source) => Err(HelperError::Io {
            path: path.into(),
            source,
        }),
    }
}

// ---- non-macOS -----------------------------------------------------------

#[cfg(not(target_os = "macos"))]
pub fn install_lan_port_redirect(
    _: std::net::Ipv4Addr,
    _: u16,
    _: u16,
    _: u16,
    _: u16,
) -> Result<(), HelperError> {
    Err(HelperError::Unsupported {
        operation: yerd_platform::error::ops::INSTALL_LAN_PORT_REDIRECT,
    })
}

#[cfg(not(target_os = "macos"))]
pub fn uninstall_lan_port_redirect() -> Result<(), HelperError> {
    Err(HelperError::Unsupported {
        operation: yerd_platform::error::ops::UNINSTALL_LAN_PORT_REDIRECT,
    })
}

#[cfg(all(test, target_os = "macos"))]
#[allow(clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;

    /// A loopback/unspecified `--lan-ip` is rejected up front, before any pf I/O
    /// (defence in depth on the privileged runtime path, not just `from_argv`).
    #[test]
    fn rejects_loopback_and_unspecified_lan_ip() {
        for ip in [
            std::net::Ipv4Addr::LOCALHOST,
            std::net::Ipv4Addr::UNSPECIFIED,
        ] {
            let err = install_lan_port_redirect(ip, 80, 8080, 443, 8443).unwrap_err();
            assert!(matches!(
                err,
                HelperError::Validation {
                    reason: ValidationReason::LanIpInvalid
                }
            ));
        }
    }
}
