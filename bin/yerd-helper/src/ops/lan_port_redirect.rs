//! `install-lan-port-redirect` / `uninstall-lan-port-redirect`. macOS only.
//!
//! The LAN counterpart of [`super::port_redirect`]: launchd binds `0.0.0.0:80`
//! and `:443` so LAN devices reach the daemon, which binds its rootless ports on
//! `0.0.0.0` in LAN mode. The wildcard bind needs no knowledge of the LAN IP,
//! so it loads at boot before DHCP and survives IP changes. `yerd-forward`
//! drops loopback-destined and non-LAN connections in this scope. Separate jobs
//! from the loopback forwarder, so removing one never disturbs the other.

#![allow(clippy::similar_names)]

use crate::error::HelperError;

/// Install the LAN port forwarder for `uid`.
#[cfg(target_os = "macos")]
pub fn install_lan_port_redirect(
    http_from: u16,
    http_to: u16,
    https_from: u16,
    https_to: u16,
    uid: u32,
) -> Result<(), HelperError> {
    super::forwarder_job::install(
        yerd_platform::pure::launchd_ports::Scope::Lan,
        http_from,
        http_to,
        https_from,
        https_to,
        uid,
    )
}

/// Remove the LAN port forwarder and any legacy pf redirect.
#[cfg(target_os = "macos")]
pub fn uninstall_lan_port_redirect() -> Result<(), HelperError> {
    super::forwarder_job::uninstall(yerd_platform::pure::launchd_ports::Scope::Lan)
}

/// Unsupported off macOS.
#[cfg(not(target_os = "macos"))]
pub fn install_lan_port_redirect(
    _: u16,
    _: u16,
    _: u16,
    _: u16,
    _: u32,
) -> Result<(), HelperError> {
    Err(HelperError::Unsupported {
        operation: yerd_platform::error::ops::INSTALL_LAN_PORT_REDIRECT,
    })
}

/// Unsupported off macOS.
#[cfg(not(target_os = "macos"))]
pub fn uninstall_lan_port_redirect() -> Result<(), HelperError> {
    Err(HelperError::Unsupported {
        operation: yerd_platform::error::ops::UNINSTALL_LAN_PORT_REDIRECT,
    })
}
