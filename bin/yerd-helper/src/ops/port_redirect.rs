//! `install-port-redirect` / `uninstall-port-redirect`. macOS only.
//!
//! macOS has no `setcap`, so the unprivileged daemon can't bind 80/443. Instead
//! we install a pf `rdr` redirect (validated by the plan's Step 0 spike) that
//! forwards inbound 80/443 to the daemon's rootless ports, plus a `LaunchDaemon`
//! that re-applies it at boot. See `yerd_platform::pure::pf_anchor` for the
//! exact rule text and the `/etc/pf.conf` editing strategy (we edit the
//! canonical file and reload it rather than load a self-contained ruleset,
//! which would flush Apple's default anchors).

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
pub fn install_port_redirect(
    http_from: u16,
    http_to: u16,
    https_from: u16,
    https_to: u16,
) -> Result<(), HelperError> {
    require_nonzero(http_from, "--http-from")?;
    require_nonzero(http_to, "--http-to")?;
    require_nonzero(https_from, "--https-from")?;
    require_nonzero(https_to, "--https-to")?;

    // 1. Write the anchor rules file (inert until referenced from pf.conf).
    let rules = pf_anchor::compose_anchor_rules(http_from, http_to, https_from, https_to);
    atomic_write(Path::new(pf_anchor::ANCHOR_PATH), rules.as_bytes(), true)?;

    // 2. Hook the anchor into /etc/pf.conf (idempotent, preserving its content).
    //    Keep the original so we can roll back if the live load rejects it.
    let original =
        std::fs::read_to_string(pf_anchor::PF_CONF_PATH).map_err(|source| HelperError::Io {
            path: pf_anchor::PF_CONF_PATH.into(),
            source,
        })?;
    let updated = pf_anchor::insert_anchor_refs(&original);
    if updated != original {
        atomic_write(Path::new(pf_anchor::PF_CONF_PATH), updated.as_bytes(), true)?;
    }

    // 3. Load + enable pf now. `pfctl -f` both applies and validates the
    //    ruleset; if it rejects the edited config, roll the pf.conf edit back
    //    before surfacing the error so we never leave (or later persist) a
    //    broken canonical config.
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
    // `pfctl -e` exits non-zero with "pf already enabled" when pf is already
    // on — that's success for our purposes, so ignore a non-zero exit here.
    ignore_command_failure(run_command("pfctl", "/sbin/pfctl", ["-e"]));

    // 4. Only once the ruleset loads cleanly, install boot persistence: the
    //    `LaunchDaemon` re-runs `pfctl -E -f /etc/pf.conf` at every boot, so it
    //    must never be installed against a config that doesn't load. Plist must
    //    be root:wheel, ≤0644 or launchd refuses it.
    let plist = pf_anchor::compose_launchdaemon_plist();
    atomic_write(Path::new(pf_anchor::PLIST_PATH), plist.as_bytes(), true)?;
    run_command(
        "chown",
        "/usr/sbin/chown",
        ["root:wheel", pf_anchor::PLIST_PATH],
    )?;
    // bootout first for idempotency (ignore "not loaded"), then bootstrap.
    ignore_command_failure(run_command(
        "launchctl",
        "/bin/launchctl",
        ["bootout", "system", pf_anchor::PLIST_PATH],
    ));
    run_command(
        "launchctl",
        "/bin/launchctl",
        ["bootstrap", "system", pf_anchor::PLIST_PATH],
    )?;
    Ok(())
}

// ---- uninstall -----------------------------------------------------------

#[cfg(target_os = "macos")]
pub fn uninstall_port_redirect() -> Result<(), HelperError> {
    // 1. Tear down the boot `LaunchDaemon` (idempotent).
    ignore_command_failure(run_command(
        "launchctl",
        "/bin/launchctl",
        ["bootout", "system", pf_anchor::PLIST_PATH],
    ));

    // 2. Remove our hook lines from /etc/pf.conf and reload, leaving pf's
    //    enabled state as-is (an Apple-default ruleset is harmless; we never
    //    force-disable a pf the user/system may rely on). Do this BEFORE
    //    deleting the anchor file so pf.conf never references a missing anchor.
    //    A read failure is fatal (skipping the cleanup would leave a dangling
    //    `load anchor`); a missing pf.conf means there's nothing to clean.
    match std::fs::read_to_string(pf_anchor::PF_CONF_PATH) {
        Ok(pf_conf) => {
            let cleaned = pf_anchor::remove_anchor_refs(&pf_conf);
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

    // 3. Now that pf.conf no longer references it, remove the anchor + plist
    //    files (idempotent: absent → Ok).
    remove_if_present(pf_anchor::ANCHOR_PATH)?;
    remove_if_present(pf_anchor::PLIST_PATH)?;
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

/// pf/launchctl steps that are "best effort" — already-enabled pf, an
/// already-unloaded `LaunchDaemon` — must not fail the whole operation.
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
pub fn install_port_redirect(_: u16, _: u16, _: u16, _: u16) -> Result<(), HelperError> {
    Err(HelperError::Unsupported {
        operation: yerd_platform::error::ops::INSTALL_PORT_REDIRECT,
    })
}

#[cfg(not(target_os = "macos"))]
pub fn uninstall_port_redirect() -> Result<(), HelperError> {
    Err(HelperError::Unsupported {
        operation: yerd_platform::error::ops::UNINSTALL_PORT_REDIRECT,
    })
}
