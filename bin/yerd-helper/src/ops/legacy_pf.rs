//! One-shot removal of the pf redirects older Yerd versions installed. macOS
//! only.
//!
//! Every forwarder install and uninstall calls [`remove_all_legacy`] for both
//! legacy anchors (`dev.yerd` and `dev.yerd.lan`), whichever scope the user is
//! acting on. Older uninstalls stripped `/etc/pf.conf` but left the anchors'
//! rules loaded in the kernel, which is what kept iCloud Private Relay broken
//! (#247); flushing them is the real fix. pf is never disabled here: Yerd's
//! `pfctl`-owned enable references are released only when the gate in
//! `yerd_platform::pure::pf_anchor` proves no other software relies on pf.

use std::path::Path;

use yerd_platform::pure::pf_anchor;

use crate::error::HelperError;
use crate::ops::{atomic_write, ignore_command_failure, remove_if_present, run_command};

const PFCTL: &str = "/sbin/pfctl";

/// Remove all legacy pf state for both anchors, then release Yerd's pf enable
/// references when that's provably safe. Does nothing beyond the evidence probe
/// when no legacy state is found. Only a failure to read or rewrite
/// `/etc/pf.conf` or delete a legacy file is an error; pf and launchctl steps
/// are best effort.
pub fn remove_all_legacy() -> Result<(), HelperError> {
    let pf_conf = read_pf_conf()?;
    let found = legacy_evidence(
        pf_anchor::ANCHOR_NAME,
        pf_anchor::ANCHOR_PATH,
        pf_anchor::PLIST_PATH,
        pf_conf.as_deref().is_some_and(pf_anchor::is_installed),
    ) | legacy_evidence(
        pf_anchor::LAN_ANCHOR_NAME,
        pf_anchor::LAN_ANCHOR_PATH,
        pf_anchor::LAN_PLIST_PATH,
        pf_conf.as_deref().is_some_and(pf_anchor::is_lan_installed),
    );
    if !found {
        return Ok(());
    }

    for label in [pf_anchor::PLIST_LABEL, pf_anchor::LAN_PLIST_LABEL] {
        ignore_command_failure(run_command(
            "launchctl",
            "/bin/launchctl",
            ["bootout", &format!("system/{label}")],
        ));
    }

    if let Some(original) = pf_conf {
        let cleaned = pf_anchor::remove_all_refs(&original);
        if cleaned != original {
            atomic_write(Path::new(pf_anchor::PF_CONF_PATH), cleaned.as_bytes(), true)?;
            ignore_command_failure(run_command("pfctl", PFCTL, ["-f", pf_anchor::PF_CONF_PATH]));
        }
    }

    for anchor in [pf_anchor::ANCHOR_NAME, pf_anchor::LAN_ANCHOR_NAME] {
        ignore_command_failure(run_command("pfctl", PFCTL, ["-a", anchor, "-F", "all"]));
    }

    for path in [
        pf_anchor::ANCHOR_PATH,
        pf_anchor::LAN_ANCHOR_PATH,
        pf_anchor::PLIST_PATH,
        pf_anchor::LAN_PLIST_PATH,
    ] {
        remove_if_present(path)?;
    }

    release_references();
    Ok(())
}

/// `None` when pf.conf is absent (nothing to clean).
fn read_pf_conf() -> Result<Option<String>, HelperError> {
    match std::fs::read_to_string(pf_anchor::PF_CONF_PATH) {
        Ok(text) => Ok(Some(text)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(source) => Err(HelperError::Io {
            path: pf_anchor::PF_CONF_PATH.into(),
            source,
        }),
    }
}

fn legacy_evidence(anchor: &str, anchor_path: &str, plist: &str, hooked: bool) -> bool {
    hooked
        || Path::new(anchor_path).exists()
        || Path::new(plist).exists()
        || pfctl_stdout(&["-a", anchor, "-s", "nat"])
            .is_some_and(|nat| pf_anchor::anchor_has_rules(&nat))
}

/// Release `pfctl`-owned enable references when only Apple's and Yerd's pf
/// state remains. Otherwise leave them and tell the user why pf may stay on.
fn release_references() {
    let tokens = gate_inputs()
        .filter(|(anchors, nat, rules)| {
            pf_anchor::only_apple_and_yerd_pf_state(anchors, nat, rules)
        })
        .and_then(|_| pfctl_stdout(&["-s", "References"]))
        .and_then(|refs| {
            pf_anchor::yerd_owned_tokens(&pf_anchor::parse_pf_references(&refs))
                .map(|tokens| tokens.into_iter().map(str::to_owned).collect::<Vec<_>>())
        });
    let Some(tokens) = tokens else {
        eprintln!(
            "yerd-helper: note: pf may stay enabled until the next reboot \
             (pf state Yerd doesn't recognise is present)"
        );
        return;
    };
    for token in tokens {
        ignore_command_failure(run_command("pfctl", PFCTL, ["-X", token.as_str()]));
    }
}

fn gate_inputs() -> Option<(String, String, String)> {
    Some((
        pfctl_stdout(&["-s", "Anchors", "-v"])?,
        pfctl_stdout(&["-s", "nat"])?,
        pfctl_stdout(&["-s", "rules"])?,
    ))
}

fn pfctl_stdout(args: &[&str]) -> Option<String> {
    run_command("pfctl", PFCTL, args)
        .ok()
        .map(|out| String::from_utf8_lossy(&out.stdout).into_owned())
}
