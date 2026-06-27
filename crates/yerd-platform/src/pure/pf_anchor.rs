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

/// The `rdr-anchor` line we insert into the translation section.
fn rdr_anchor_line() -> String {
    format!("rdr-anchor \"{ANCHOR_NAME}\" {MARKER}")
}

/// The `load anchor` line we append.
fn load_anchor_line() -> String {
    format!("load anchor \"{ANCHOR_NAME}\" from \"{ANCHOR_PATH}\" {MARKER}")
}

/// True if `pf_conf` already carries our managed lines.
#[must_use]
pub fn is_installed(pf_conf: &str) -> bool {
    pf_conf
        .lines()
        .any(|l| l.contains(MARKER) && l.contains(&format!("\"{ANCHOR_NAME}\"")))
}

/// Insert our `rdr-anchor` + `load anchor` lines into `pf_conf`, returning the
/// new file text. Idempotent: if already present, returns `pf_conf` unchanged.
///
/// The `rdr-anchor` line is placed immediately after the last existing
/// `rdr-anchor`/`nat-anchor` declaration (the translation section, which pf
/// requires to precede filter rules); if the file has neither, it is prepended.
/// The `load anchor` line is appended (load statements may appear anywhere).
#[must_use]
pub fn insert_anchor_refs(pf_conf: &str) -> String {
    if is_installed(pf_conf) {
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
    lines.insert(insert_at, rdr_anchor_line());

    lines.push(load_anchor_line());

    let mut out = lines.join("\n");
    out.push('\n');
    out
}

/// Remove our managed lines from `pf_conf`, returning the new file text.
/// Idempotent: a file without our lines is returned (re-normalised) unchanged
/// in content.
#[must_use]
pub fn remove_anchor_refs(pf_conf: &str) -> String {
    let kept: Vec<&str> = pf_conf.lines().filter(|l| !l.contains(MARKER)).collect();
    let mut out = kept.join("\n");
    if !out.is_empty() {
        out.push('\n');
    }
    out
}

/// Compose the `LaunchDaemon` plist that re-applies the redirect at boot by
/// reloading (and enabling) pf from the canonical config. One-shot:
/// `RunAtLoad` with no `KeepAlive` (launchd would otherwise respawn a process
/// that exits 0 in a tight loop).
#[must_use]
pub fn compose_launchdaemon_plist() -> String {
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
         <plist version=\"1.0\">\n\
         <dict>\n\
         \t<key>Label</key>\n\
         \t<string>{ANCHOR_NAME}.pf</string>\n\
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
}
