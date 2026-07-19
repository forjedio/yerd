//! Compose the macOS pf redirect that lets the unprivileged daemon serve
//! 80/443 while binding only rootless ports.
//!
//! macOS has no `setcap`; instead a privileged helper installs a pf `rdr`
//! redirect. The empirically-validated rule (see the plan's Step 0 spike) is:
//!
//! ```text
//! rdr pass on lo0 inet proto tcp from any to any port 80  -> 127.0.0.1 port <http_to>
//! rdr pass on lo0 inet proto tcp from any to any port 443 -> 127.0.0.1 port <https_to>
//! ```
//!
//! `on lo0` is load-bearing: pf `rdr` only fires for loopback-delivered
//! `127.0.0.1→127.0.0.1` traffic when the rule is anchored to `lo0`.
//!
//! These rules live in their own anchor file (`/etc/pf.anchors/dev.yerd`) and
//! are hooked into the main ruleset via two lines inserted into `/etc/pf.conf`:
//! an `rdr-anchor` reference (in the translation section, before any filter
//! rule) and a `load anchor` statement. We **edit** `/etc/pf.conf` rather than
//! load a self-contained ruleset because `pfctl -f <full-ruleset>` *replaces*
//! the running ruleset and would flush Apple's default `com.apple/*` anchors;
//! editing the canonical file and reloading it preserves everything.
//!
//! Every function here is pure and table-tested; the helper does the I/O.

#![allow(clippy::similar_names)]

/// pf anchor name (also the `LaunchDaemon` `Label` and plist filename stem).
pub const ANCHOR_NAME: &str = "dev.yerd";
/// Absolute path of the anchor rules file.
pub const ANCHOR_PATH: &str = "/etc/pf.anchors/dev.yerd";
/// Absolute path of the boot-persistence `LaunchDaemon` plist.
pub const PLIST_PATH: &str = "/Library/LaunchDaemons/dev.yerd.pf.plist";
/// The canonical pf config the system loads at boot.
pub const PF_CONF_PATH: &str = "/etc/pf.conf";

/// Trailing marker on every line we insert into `/etc/pf.conf`, so insertion
/// is idempotent and removal is unambiguous.
const MARKER: &str = "# yerd-managed";

/// LAN pf anchor name - a **separate** anchor from the loopback `dev.yerd` one
/// (installed by `yerd elevate ports`) so teardown of one never disturbs the
/// other. It carries the M2 `rdr` that redirects inbound LAN 80/443 to the
/// daemon's rootless ports on the host's LAN IP.
pub const LAN_ANCHOR_NAME: &str = "dev.yerd.lan";
/// Absolute path of the LAN anchor rules file.
pub const LAN_ANCHOR_PATH: &str = "/etc/pf.anchors/dev.yerd.lan";
/// Absolute path of the LAN boot-persistence `LaunchDaemon` plist.
pub const LAN_PLIST_PATH: &str = "/Library/LaunchDaemons/dev.yerd.lan.pf.plist";

/// Distinct marker on LAN-managed `/etc/pf.conf` lines. Deliberately different
/// from [`MARKER`] and non-overlapping (`"# yerd-managed"` is not a substring of
/// `"# yerd-lan-managed"`), so removing one anchor's refs never strips the
/// other's.
const LAN_MARKER: &str = "# yerd-lan-managed";

/// Compose the contents of `/etc/pf.anchors/dev.yerd`. Ends in a newline.
#[must_use]
pub fn compose_anchor_rules(
    http_from: u16,
    http_to: u16,
    https_from: u16,
    https_to: u16,
) -> String {
    format!(
        "rdr pass on lo0 inet proto tcp from any to any port {http_from} -> 127.0.0.1 port {http_to}\n\
         rdr pass on lo0 inet proto tcp from any to any port {https_from} -> 127.0.0.1 port {https_to}\n"
    )
}

/// The `rdr-anchor` line for `name`, tagged with `marker`.
fn rdr_anchor_line_for(name: &str, marker: &str) -> String {
    format!("rdr-anchor \"{name}\" {marker}")
}

/// The `load anchor` line for `name`/`path`, tagged with `marker`.
fn load_anchor_line_for(name: &str, path: &str, marker: &str) -> String {
    format!("load anchor \"{name}\" from \"{path}\" {marker}")
}

/// True if `pf_conf` already carries the refs for `name`/`marker`.
fn is_installed_for(pf_conf: &str, name: &str, marker: &str) -> bool {
    let quoted = format!("\"{name}\"");
    pf_conf
        .lines()
        .any(|l| l.contains(marker) && l.contains(&quoted))
}

/// Insert an anchor's `rdr-anchor` + `load anchor` refs into `pf_conf`.
fn insert_refs_for(pf_conf: &str, name: &str, path: &str, marker: &str) -> String {
    if is_installed_for(pf_conf, name, marker) {
        return pf_conf.to_owned();
    }

    let mut lines: Vec<String> = pf_conf.lines().map(str::to_owned).collect();

    let after = lines
        .iter()
        .enumerate()
        .filter(|(_, l)| {
            let t = l.trim_start();
            t.starts_with("rdr-anchor") || t.starts_with("nat-anchor")
        })
        .map(|(i, _)| i)
        .next_back();
    let insert_at = after.map_or(0, |i| i + 1);
    lines.insert(insert_at, rdr_anchor_line_for(name, marker));

    lines.push(load_anchor_line_for(name, path, marker));

    let mut out = lines.join("\n");
    out.push('\n');
    out
}

/// Remove the lines tagged with `marker` from `pf_conf`.
fn remove_refs_for(pf_conf: &str, marker: &str) -> String {
    let kept: Vec<&str> = pf_conf.lines().filter(|l| !l.contains(marker)).collect();
    let mut out = kept.join("\n");
    if !out.is_empty() {
        out.push('\n');
    }
    out
}

/// True if `pf_conf` already carries the loopback (`dev.yerd`) managed lines.
#[must_use]
pub fn is_installed(pf_conf: &str) -> bool {
    is_installed_for(pf_conf, ANCHOR_NAME, MARKER)
}

/// Insert the loopback `rdr-anchor` + `load anchor` lines into `pf_conf`,
/// returning the new file text. Idempotent.
///
/// The `rdr-anchor` line is placed immediately after the last existing
/// `rdr-anchor`/`nat-anchor` declaration (the translation section, which pf
/// requires to precede filter rules); if the file has neither, it is prepended.
/// The `load anchor` line is appended (load statements may appear anywhere).
#[must_use]
pub fn insert_anchor_refs(pf_conf: &str) -> String {
    insert_refs_for(pf_conf, ANCHOR_NAME, ANCHOR_PATH, MARKER)
}

/// Remove the loopback managed lines from `pf_conf` (scoped to [`MARKER`], so it
/// never strips the LAN anchor's refs). Idempotent.
#[must_use]
pub fn remove_anchor_refs(pf_conf: &str) -> String {
    remove_refs_for(pf_conf, MARKER)
}

/// True if `pf_conf` already carries the LAN (`dev.yerd.lan`) managed lines.
#[must_use]
pub fn is_lan_installed(pf_conf: &str) -> bool {
    is_installed_for(pf_conf, LAN_ANCHOR_NAME, LAN_MARKER)
}

/// Insert the LAN anchor refs into `pf_conf`. Idempotent; same placement rules
/// as [`insert_anchor_refs`], but tagged with [`LAN_MARKER`] so the two anchors
/// coexist independently.
#[must_use]
pub fn insert_lan_anchor_refs(pf_conf: &str) -> String {
    insert_refs_for(pf_conf, LAN_ANCHOR_NAME, LAN_ANCHOR_PATH, LAN_MARKER)
}

/// Remove the LAN managed lines from `pf_conf` (scoped to [`LAN_MARKER`], so it
/// never strips the loopback anchor's refs). Idempotent.
#[must_use]
pub fn remove_lan_anchor_refs(pf_conf: &str) -> String {
    remove_refs_for(pf_conf, LAN_MARKER)
}

/// Compose the LAN anchor rules file (`/etc/pf.anchors/dev.yerd.lan`), the M2
/// redirect. Ends in a newline.
///
/// Redirects inbound 80/443 to `<lan_ip>:<rootless>`, **source-scoped to the
/// RFC1918 + link-local ranges** (the LAN subset of [`yerd_core::is_lan_source`];
/// loopback is deliberately excluded here - loopback traffic is served by the
/// separate `dev.yerd` anchor and never has `<lan_ip>` as its destination) and
/// dest-scoped to `<lan_ip>`. No `on <iface>` qualifier: the destination is a
/// real routable local address, so no martian/loopback drop applies and no
/// interface name is needed.
#[must_use]
pub fn compose_lan_anchor_rules(
    lan_ip: std::net::Ipv4Addr,
    http_from: u16,
    http_to: u16,
    https_from: u16,
    https_to: u16,
) -> String {
    let src = "{ 10.0.0.0/8, 172.16.0.0/12, 192.168.0.0/16, 169.254.0.0/16 }";
    format!(
        "rdr pass inet proto tcp from {src} to {lan_ip} port {http_from} -> {lan_ip} port {http_to}\n\
         rdr pass inet proto tcp from {src} to {lan_ip} port {https_from} -> {lan_ip} port {https_to}\n"
    )
}

/// Compose the `LaunchDaemon` plist that re-applies the redirect at boot by
/// reloading (and enabling) pf from the canonical config. One-shot:
/// `RunAtLoad` with no `KeepAlive` (launchd would otherwise respawn a process
/// that exits 0 in a tight loop).
#[must_use]
pub fn compose_launchdaemon_plist() -> String {
    compose_launchdaemon_plist_labelled(ANCHOR_NAME)
}

/// LAN variant of [`compose_launchdaemon_plist`], labelled `dev.yerd.lan.pf`.
/// Reloads the same canonical `/etc/pf.conf` (which loads both anchors) at boot.
#[must_use]
pub fn compose_lan_launchdaemon_plist() -> String {
    compose_launchdaemon_plist_labelled(LAN_ANCHOR_NAME)
}

fn compose_launchdaemon_plist_labelled(label_stem: &str) -> String {
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
         <plist version=\"1.0\">\n\
         <dict>\n\
         \t<key>Label</key>\n\
         \t<string>{label_stem}.pf</string>\n\
         \t<key>RunAtLoad</key>\n\
         \t<true/>\n\
         \t<key>ProgramArguments</key>\n\
         \t<array>\n\
         \t\t<string>/sbin/pfctl</string>\n\
         \t\t<string>-E</string>\n\
         \t\t<string>-f</string>\n\
         \t\t<string>{PF_CONF_PATH}</string>\n\
         \t</array>\n\
         </dict>\n\
         </plist>\n"
    )
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

    const APPLE_DEFAULT: &str = "\
scrub-anchor \"com.apple/*\"
nat-anchor \"com.apple/*\"
rdr-anchor \"com.apple/*\"
dummynet-anchor \"com.apple/*\"
anchor \"com.apple/*\"
load anchor \"com.apple\" from \"/etc/pf.anchors/com.apple\"
";

    #[test]
    fn anchor_rules_have_on_lo0_and_both_ports() {
        let r = compose_anchor_rules(80, 8080, 443, 8443);
        assert!(r.contains(
            "rdr pass on lo0 inet proto tcp from any to any port 80 -> 127.0.0.1 port 8080"
        ));
        assert!(r.contains(
            "rdr pass on lo0 inet proto tcp from any to any port 443 -> 127.0.0.1 port 8443"
        ));
        assert!(r.ends_with('\n'));
    }

    #[test]
    fn insert_places_rdr_anchor_after_last_translation_anchor() {
        let out = insert_anchor_refs(APPLE_DEFAULT);
        let lines: Vec<&str> = out.lines().collect();
        let rdr_apple = lines
            .iter()
            .position(|l| l.contains("rdr-anchor \"com.apple/*\""))
            .unwrap();
        let rdr_yerd = lines
            .iter()
            .position(|l| l.starts_with("rdr-anchor \"dev.yerd\""))
            .unwrap();
        let dummynet = lines
            .iter()
            .position(|l| l.contains("dummynet-anchor"))
            .unwrap();
        assert!(rdr_yerd > rdr_apple && rdr_yerd < dummynet);
        assert!(lines
            .last()
            .unwrap()
            .starts_with("load anchor \"dev.yerd\""));
    }

    #[test]
    fn insert_is_idempotent() {
        let once = insert_anchor_refs(APPLE_DEFAULT);
        let twice = insert_anchor_refs(&once);
        assert_eq!(once, twice);
        assert!(is_installed(&once));
    }

    #[test]
    fn remove_reverses_insert() {
        let inserted = insert_anchor_refs(APPLE_DEFAULT);
        let removed = remove_anchor_refs(&inserted);
        assert_eq!(removed, APPLE_DEFAULT);
        assert!(!is_installed(&removed));
    }

    #[test]
    fn remove_is_idempotent_on_clean_file() {
        assert_eq!(remove_anchor_refs(APPLE_DEFAULT), APPLE_DEFAULT);
    }

    #[test]
    fn insert_into_empty_file_prepends_rdr_anchor() {
        let out = insert_anchor_refs("");
        let lines: Vec<&str> = out.lines().collect();
        assert!(lines[0].starts_with("rdr-anchor \"dev.yerd\""));
        assert!(lines
            .last()
            .unwrap()
            .starts_with("load anchor \"dev.yerd\""));
    }

    #[test]
    fn plist_is_one_shot_runatload() {
        let p = compose_launchdaemon_plist();
        assert!(p.contains("<string>dev.yerd.pf</string>"));
        assert!(p.contains("<key>RunAtLoad</key>"));
        assert!(!p.contains("KeepAlive"));
        assert!(p.contains("/sbin/pfctl"));
        assert!(p.contains(PF_CONF_PATH));
    }

    #[test]
    fn lan_rules_are_source_scoped_and_dest_scoped_no_iface() {
        let ip = std::net::Ipv4Addr::new(192, 168, 1, 42);
        let r = compose_lan_anchor_rules(ip, 80, 8080, 443, 8443);
        assert!(r.contains("to 192.168.1.42 port 80 -> 192.168.1.42 port 8080"));
        assert!(r.contains("to 192.168.1.42 port 443 -> 192.168.1.42 port 8443"));
        assert!(r.contains("10.0.0.0/8"));
        assert!(r.contains("192.168.0.0/16"));
        assert!(r.contains("169.254.0.0/16"));
        assert!(!r.contains("on lo0"));
        assert!(
            !r.contains(" on "),
            "M2 rule must carry no interface qualifier"
        );
        assert!(!r.contains("from any to any"));
    }

    #[test]
    fn lan_plist_uses_distinct_label() {
        let p = compose_lan_launchdaemon_plist();
        assert!(p.contains("<string>dev.yerd.lan.pf</string>"));
    }

    /// The two anchors coexist: installing both then removing one leaves the
    /// other's refs intact (distinct, non-overlapping markers).
    #[test]
    fn loopback_and_lan_anchors_coexist_and_remove_independently() {
        let both = insert_lan_anchor_refs(&insert_anchor_refs(APPLE_DEFAULT));
        assert!(is_installed(&both));
        assert!(is_lan_installed(&both));

        let lan_gone = remove_lan_anchor_refs(&both);
        assert!(
            is_installed(&lan_gone),
            "removing LAN leaves loopback anchor"
        );
        assert!(!is_lan_installed(&lan_gone));

        let loop_gone = remove_anchor_refs(&both);
        assert!(!is_installed(&loop_gone));
        assert!(
            is_lan_installed(&loop_gone),
            "removing loopback leaves LAN anchor"
        );

        assert_eq!(
            remove_anchor_refs(&lan_gone),
            APPLE_DEFAULT,
            "removing both returns the original"
        );
    }

    #[test]
    fn lan_insert_is_idempotent() {
        let once = insert_lan_anchor_refs(APPLE_DEFAULT);
        let twice = insert_lan_anchor_refs(&once);
        assert_eq!(once, twice);
    }
}
