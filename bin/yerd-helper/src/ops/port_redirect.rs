//! `install-port-redirect` / `uninstall-port-redirect`. macOS only.
//!
//! macOS has no `setcap`, so the unprivileged daemon can't bind 80/443. These
//! ops install launchd jobs that bind `127.0.0.1:80`/`:443` and hand each
//! socket to `yerd-forward`, running as the invoking user, which relays to the
//! daemon's rootless ports. See [`super::forwarder_job`].

#![allow(clippy::similar_names)]

use crate::error::HelperError;

/// Install the loopback port forwarder for `uid`.
#[cfg(target_os = "macos")]
pub fn install_port_redirect(
    http_from: u16,
    http_to: u16,
    https_from: u16,
    https_to: u16,
    uid: u32,
) -> Result<(), HelperError> {
    super::forwarder_job::install(
        yerd_platform::pure::launchd_ports::Scope::Loopback,
        http_from,
        http_to,
        https_from,
        https_to,
        uid,
    )
}

/// Remove the loopback port forwarder and any legacy pf redirect.
#[cfg(target_os = "macos")]
pub fn uninstall_port_redirect() -> Result<(), HelperError> {
    super::forwarder_job::uninstall(yerd_platform::pure::launchd_ports::Scope::Loopback)
}

/// Unsupported off macOS.
#[cfg(not(target_os = "macos"))]
pub fn install_port_redirect(_: u16, _: u16, _: u16, _: u16, _: u32) -> Result<(), HelperError> {
    Err(HelperError::Unsupported {
        operation: yerd_platform::error::ops::INSTALL_PORT_REDIRECT,
    })
}

/// Unsupported off macOS.
#[cfg(not(target_os = "macos"))]
pub fn uninstall_port_redirect() -> Result<(), HelperError> {
    Err(HelperError::Unsupported {
        operation: yerd_platform::error::ops::UNINSTALL_PORT_REDIRECT,
    })
}
