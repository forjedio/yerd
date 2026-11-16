//! Legacy pf state from pre-launchd Yerd, kept only to detect and remove it.
//!
//! Older Yerd versions served 80/443 on macOS through pf `rdr` rules in their
//! own anchors (`dev.yerd` for loopback, `dev.yerd.lan` for LAN), hooked into
//! `/etc/pf.conf` by marker-tagged lines and re-applied at boot by a launchd
//! daemon running `pfctl -E -f /etc/pf.conf`. Privileged ports now use launchd
//! socket activation (see [`super::launchd_ports`]), and every elevate or
//! unelevate removes this legacy state for both anchors.
//!
//! Removal must not switch off pf for other software. VPN kill switches and
//! firewall tools also enable pf through `pfctl`, so their enable references
//! look identical to Yerd's. [`only_apple_and_yerd_pf_state`] is the fail-closed
//! gate that decides whether releasing `pfctl`-owned references is safe.
//!
//! Every function here is pure and table-tested; the helper does the I/O.

#![allow(clippy::similar_names)]

/// Legacy loopback pf anchor name.
pub const ANCHOR_NAME: &str = "dev.yerd";
/// Legacy loopback anchor rules file.
pub const ANCHOR_PATH: &str = "/etc/pf.anchors/dev.yerd";
/// Legacy loopback boot daemon plist.
pub const PLIST_PATH: &str = "/Library/LaunchDaemons/dev.yerd.pf.plist";
/// Legacy loopback boot daemon label.
pub const PLIST_LABEL: &str = "dev.yerd.pf";
/// The canonical pf config the system loads at boot.
pub const PF_CONF_PATH: &str = "/etc/pf.conf";

/// Trailing marker on the loopback lines older Yerd inserted into
/// `/etc/pf.conf`.
const MARKER: &str = "# yerd-managed";

/// Legacy LAN pf anchor name.
pub const LAN_ANCHOR_NAME: &str = "dev.yerd.lan";
/// Legacy LAN anchor rules file.
pub const LAN_ANCHOR_PATH: &str = "/etc/pf.anchors/dev.yerd.lan";
/// Legacy LAN boot daemon plist.
pub const LAN_PLIST_PATH: &str = "/Library/LaunchDaemons/dev.yerd.lan.pf.plist";
/// Legacy LAN boot daemon label.
pub const LAN_PLIST_LABEL: &str = "dev.yerd.lan.pf";

/// Marker on the LAN lines in `/etc/pf.conf`. `"# yerd-managed"` is not a
/// substring of it, so the two never strip each other's lines.
const LAN_MARKER: &str = "# yerd-lan-managed";

/// Apple's own anchors and sub-anchors as listed by `pfctl -s Anchors -v` on a
/// clean macOS install, plus the static sub-anchors declared in
/// `/etc/pf.anchors/com.apple`. Exact names only: anything else, including an
/// unknown `com.apple/<n>.<name>` sub-anchor where a third-party tool could
/// hide its rules, makes [`only_apple_and_yerd_pf_state`] fail closed. The
/// `com.apple.internet-sharing` anchors are deliberately left out: Internet
/// Sharing relies on pf being enabled, and its reference can't be told apart
/// from Yerd's, so releasing while it's loaded could switch off its NAT.
const APPLE_ANCHORS: &[&str] = &[
    "com.apple",
    "com.apple/200.AirDrop",
    "com.apple/250.ApplicationFirewall",
];

/// Parse the destination ports of a legacy `dev.yerd` or `dev.yerd.lan`
/// anchor file, returning `(http_to, https_to)`.
///
/// Handles both the loopback shape (`... to any port 80 -> 127.0.0.1 port 8080`)
/// and the LAN shape (`... to <lan_ip> port 80 -> <lan_ip> port 8080`). For each
/// `rdr` line the from-port is the number after the last `port` before `->` and
/// the to-port the number after the last `port` after `->`; the line whose
/// from-port is 80 gives `http_to` and 443 gives `https_to`. Returns `None`
/// unless both are found (a partial or hand-edited file is treated as unknown).
#[must_use]
pub fn parse_anchor_targets(rules: &str) -> Option<(u16, u16)> {
    let mut http_to: Option<u16> = None;
    let mut https_to: Option<u16> = None;
    for line in rules.lines() {
        let trimmed = line.trim_start();
        if !trimmed.starts_with("rdr ") {
            continue;
        }
        let Some((lhs, rhs)) = trimmed.split_once("->") else {
            continue;
        };
        let Some(from) = last_port_token(lhs) else {
            continue;
        };
        let Some(to) = last_port_token(rhs) else {
            continue;
        };
        match from {
            80 => http_to = Some(to),
            443 => https_to = Some(to),
            _ => {}
        }
    }
    Some((http_to?, https_to?))
}

fn last_port_token(segment: &str) -> Option<u16> {
    let mut tokens = segment.split_whitespace().peekable();
    let mut found: Option<u16> = None;
    while let Some(tok) = tokens.next() {
        if tok == "port" {
            if let Some(value) = tokens.peek().and_then(|v| v.parse::<u16>().ok()) {
                found = Some(value);
            }
        }
    }
    found
}

fn is_installed_for(pf_conf: &str, name: &str, marker: &str) -> bool {
    let quoted = format!("\"{name}\"");
    pf_conf
        .lines()
        .any(|l| l.contains(marker) && l.contains(&quoted))
}

fn remove_refs_for(pf_conf: &str, marker: &str) -> String {
    let kept: Vec<&str> = pf_conf.lines().filter(|l| !l.contains(marker)).collect();
    let mut out = kept.join("\n");
    if !out.is_empty() {
        out.push('\n');
    }
    out
}

/// True if `pf_conf` carries the legacy loopback (`dev.yerd`) lines.
#[must_use]
pub fn is_installed(pf_conf: &str) -> bool {
    is_installed_for(pf_conf, ANCHOR_NAME, MARKER)
}

/// Remove the legacy loopback lines from `pf_conf`. Idempotent, and never
/// touches the LAN lines.
#[must_use]
pub fn remove_anchor_refs(pf_conf: &str) -> String {
    remove_refs_for(pf_conf, MARKER)
}

/// True if `pf_conf` carries the legacy LAN (`dev.yerd.lan`) lines.
#[must_use]
pub fn is_lan_installed(pf_conf: &str) -> bool {
    is_installed_for(pf_conf, LAN_ANCHOR_NAME, LAN_MARKER)
}

/// Remove the legacy LAN lines from `pf_conf`. Idempotent, and never touches
/// the loopback lines.
#[must_use]
pub fn remove_lan_anchor_refs(pf_conf: &str) -> String {
    remove_refs_for(pf_conf, LAN_MARKER)
}

/// Remove both anchors' legacy lines from `pf_conf`.
#[must_use]
pub fn remove_all_refs(pf_conf: &str) -> String {
    remove_lan_anchor_refs(&remove_anchor_refs(pf_conf))
}

/// One pf enable reference from `pfctl -s References`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PfReference {
    /// Name of the process that took the reference.
    pub process: String,
    /// The reference token, as printed.
    pub token: String,
}

/// Parse `pfctl -s References`. Data rows start with a numeric pid followed by
/// the process name and the token; headers, blank lines and anything else are
/// skipped.
#[must_use]
pub fn parse_pf_references(text: &str) -> Vec<PfReference> {
    text.lines()
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            let pid = fields.next()?;
            if !is_digits(pid) {
                return None;
            }
            let process = fields.next()?.to_owned();
            let token = fields.next()?.to_owned();
            Some(PfReference { process, token })
        })
        .collect()
}

/// The tokens of references taken by a process named exactly `pfctl`, which
/// is how older Yerd enabled pf. `None` when any such token is not a plain
/// decimal number, so nothing unvalidated reaches `pfctl -X`.
#[must_use]
pub fn yerd_owned_tokens(refs: &[PfReference]) -> Option<Vec<&str>> {
    refs.iter()
        .filter(|r| r.process == "pfctl")
        .map(|r| is_digits(&r.token).then_some(r.token.as_str()))
        .collect()
}

/// True when a `pfctl -a <anchor> -s nat` listing contains any rule.
#[must_use]
pub fn anchor_has_rules(listing: &str) -> bool {
    listing
        .lines()
        .map(str::trim)
        .any(|l| !l.is_empty() && !l.starts_with('#'))
}

/// True only when the loaded pf state is provably just Apple's defaults plus
/// Yerd's legacy anchors, so releasing `pfctl`-owned references can't disable
/// another tool's pf.
///
/// `anchors` is `pfctl -s Anchors -v` (recursive); every listed anchor must be
/// in the exact Apple allowlist or be a Yerd anchor. `nat` and `rules` are
/// `pfctl -s nat` and `pfctl -s rules`; every main-ruleset line must be an
/// anchor hook for `com.apple/*` or a Yerd anchor. Anything unrecognised
/// returns false. Unknown Apple names (other macOS versions, dynamic
/// sub-anchors) also fail closed; the cost is only that pf stays enabled until
/// the next reboot.
#[must_use]
pub fn only_apple_and_yerd_pf_state(anchors: &str, nat: &str, rules: &str) -> bool {
    let anchors_ok = anchors
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .all(|name| {
            APPLE_ANCHORS.contains(&name) || name == ANCHOR_NAME || name == LAN_ANCHOR_NAME
        });
    anchors_ok && only_known_hooks(nat) && only_known_hooks(rules)
}

fn only_known_hooks(listing: &str) -> bool {
    listing
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .all(is_known_hook)
}

fn is_known_hook(line: &str) -> bool {
    let mut fields = line.split_whitespace();
    let (Some(kind), Some(target)) = (fields.next(), fields.next()) else {
        return false;
    };
    let hook_kind = matches!(
        kind,
        "anchor"
            | "nat-anchor"
            | "rdr-anchor"
            | "binat-anchor"
            | "scrub-anchor"
            | "dummynet-anchor"
    );
    let known_target = matches!(
        target,
        "\"com.apple/*\"" | "\"dev.yerd\"" | "\"dev.yerd.lan\""
    );
    hook_kind && known_target
}

fn is_digits(value: &str) -> bool {
    !value.is_empty() && value.bytes().all(|b| b.is_ascii_digit())
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

    const LEGACY_BOTH: &str = "\
scrub-anchor \"com.apple/*\"
nat-anchor \"com.apple/*\"
rdr-anchor \"com.apple/*\"
rdr-anchor \"dev.yerd\" # yerd-managed
rdr-anchor \"dev.yerd.lan\" # yerd-lan-managed
dummynet-anchor \"com.apple/*\"
anchor \"com.apple/*\"
load anchor \"com.apple\" from \"/etc/pf.anchors/com.apple\"
load anchor \"dev.yerd\" from \"/etc/pf.anchors/dev.yerd\" # yerd-managed
load anchor \"dev.yerd.lan\" from \"/etc/pf.anchors/dev.yerd.lan\" # yerd-lan-managed
";

    const LEGACY_LOOPBACK_ANCHOR: &str = "\
rdr pass on lo0 inet proto tcp from any to any port 80 -> 127.0.0.1 port 8080
rdr pass on lo0 inet proto tcp from any to any port 443 -> 127.0.0.1 port 8443
";

    const LEGACY_LAN_ANCHOR: &str = "\
rdr pass inet proto tcp from { 10.0.0.0/8, 172.16.0.0/12, 192.168.0.0/16, 169.254.0.0/16 } to 192.168.1.42 port 80 -> 192.168.1.42 port 8080
rdr pass inet proto tcp from { 10.0.0.0/8, 172.16.0.0/12, 192.168.0.0/16, 169.254.0.0/16 } to 192.168.1.42 port 443 -> 192.168.1.42 port 8443
";

    #[test]
    fn detects_and_removes_each_scope_independently() {
        assert!(is_installed(LEGACY_BOTH));
        assert!(is_lan_installed(LEGACY_BOTH));
        let lan_gone = remove_lan_anchor_refs(LEGACY_BOTH);
        assert!(is_installed(&lan_gone));
        assert!(!is_lan_installed(&lan_gone));
        let loop_gone = remove_anchor_refs(LEGACY_BOTH);
        assert!(!is_installed(&loop_gone));
        assert!(is_lan_installed(&loop_gone));
    }

    #[test]
    fn remove_all_restores_apple_default() {
        assert_eq!(remove_all_refs(LEGACY_BOTH), APPLE_DEFAULT);
        assert_eq!(remove_all_refs(APPLE_DEFAULT), APPLE_DEFAULT);
        assert!(!is_installed(APPLE_DEFAULT));
        assert!(!is_lan_installed(APPLE_DEFAULT));
    }

    #[test]
    fn parse_anchor_targets_table() {
        let cases = [
            (LEGACY_LOOPBACK_ANCHOR, Some((8080, 8443))),
            (LEGACY_LAN_ANCHOR, Some((8080, 8443))),
            (
                "rdr pass on lo0 inet proto tcp from any to any port 80 -> 127.0.0.1 port 9080\n\
                 rdr pass on lo0 inet proto tcp from any to any port 443 -> 127.0.0.1 port 9443\n",
                Some((9080, 9443)),
            ),
            (
                "rdr pass on lo0 inet proto tcp from any to any port 80 -> 127.0.0.1 port 8080\n",
                None,
            ),
            ("", None),
            ("not a rule at all\n# comment\n", None),
            (
                "# rdr pass on lo0 inet proto tcp from any to any port 80 -> 127.0.0.1 port 9999\n\
                 rdr pass on lo0 inet proto tcp from any to any port 80 -> 127.0.0.1 port 8080\n\
                 rdr pass on lo0 inet proto tcp from any to any port 443 -> 127.0.0.1 port 8443\n\
                 pass in quick proto tcp from any to any port 80 -> 127.0.0.1 port 5555\n",
                Some((8080, 8443)),
            ),
        ];
        for (text, want) in cases {
            assert_eq!(parse_anchor_targets(text), want, "{text:?}");
        }
    }

    const REFERENCES: &str = "\
TOKENS:
PID      Process Name                TOKEN                     TIMESTAMP
412      socketfilterfw              18446744073709551615      3 days 2:01:10
1203     pfctl                       18446742974203543055      1 days 1:00:00
2207     pfctl                       18446742974203611111      0 days 0:10:00
";

    #[test]
    fn parses_references_and_picks_pfctl_tokens() {
        let refs = parse_pf_references(REFERENCES);
        assert_eq!(refs.len(), 3);
        assert_eq!(refs[0].process, "socketfilterfw");
        assert_eq!(
            yerd_owned_tokens(&refs),
            Some(vec!["18446742974203543055", "18446742974203611111"])
        );
    }

    #[test]
    fn references_edge_cases() {
        assert!(parse_pf_references("").is_empty());
        assert!(parse_pf_references("TOKENS:\nPID  Process Name  TOKEN\n").is_empty());
        let other = parse_pf_references("77 someapp 123 now\n");
        assert_eq!(yerd_owned_tokens(&other), Some(vec![]));
        let tainted = parse_pf_references("77 pfctl 12a;-F all\n");
        assert_eq!(yerd_owned_tokens(&tainted), None);
    }

    #[test]
    fn anchor_has_rules_table() {
        let cases = [
            ("", false),
            ("\n  \n", false),
            ("# nothing here\n", false),
            (
                "rdr pass on lo0 inet proto tcp from any to any port 80 -> 127.0.0.1 port 8080\n",
                true,
            ),
        ];
        for (text, want) in cases {
            assert_eq!(anchor_has_rules(text), want, "{text:?}");
        }
    }

    const CLEAN_ANCHORS: &str =
        "  com.apple\n  com.apple/200.AirDrop\n  com.apple/250.ApplicationFirewall\n";
    const CLEAN_NAT: &str = "nat-anchor \"com.apple/*\" all\nrdr-anchor \"com.apple/*\" all\n";
    const CLEAN_RULES: &str =
        "scrub-anchor \"com.apple/*\" all fragment reassemble\nanchor \"com.apple/*\" all\n";

    const INTERNET_SHARING_ANCHORS: &str = "  com.apple\n  com.apple/200.AirDrop\n  \
        com.apple/250.ApplicationFirewall\n  com.apple.internet-sharing\n  \
        com.apple.internet-sharing/network_isolation\n  dev.yerd\n  dev.yerd.lan\n";
    const LEGACY_NAT: &str = "nat-anchor \"com.apple/*\" all\nrdr-anchor \"com.apple/*\" all\n\
        rdr-anchor \"dev.yerd\" all\nrdr-anchor \"dev.yerd.lan\" all\n";

    #[test]
    fn release_gate_table() {
        let cases = [
            (CLEAN_ANCHORS, CLEAN_NAT, CLEAN_RULES, true),
            ("  com.apple\n", "", "", true),
            (INTERNET_SHARING_ANCHORS, LEGACY_NAT, CLEAN_RULES, false),
            (
                "  com.apple\n  com.apple.internet-sharing/999.test\n",
                CLEAN_NAT,
                CLEAN_RULES,
                false,
            ),
            (
                "  com.apple\n  dev.yerd\n  dev.yerd.lan\n",
                "rdr-anchor \"dev.yerd\" all\n",
                CLEAN_RULES,
                true,
            ),
            (
                "  com.apple\n  com.example.test\n",
                CLEAN_NAT,
                CLEAN_RULES,
                false,
            ),
            (
                "  com.apple\n  com.apple/999.test\n",
                CLEAN_NAT,
                CLEAN_RULES,
                false,
            ),
            (
                CLEAN_ANCHORS,
                "rdr pass on lo0 inet proto tcp from any to any port 80 -> 127.0.0.1 port 9000\n",
                CLEAN_RULES,
                false,
            ),
            (
                CLEAN_ANCHORS,
                CLEAN_NAT,
                "pass quick on lo0 all\nanchor \"com.apple/*\" all\n",
                false,
            ),
            (
                CLEAN_ANCHORS,
                CLEAN_NAT,
                "anchor \"vpn.killswitch\" all\n",
                false,
            ),
            ("garbage that is not an anchor list\n", "", "", false),
        ];
        for (anchors, nat, rules, want) in cases {
            assert_eq!(
                only_apple_and_yerd_pf_state(anchors, nat, rules),
                want,
                "anchors={anchors:?} nat={nat:?} rules={rules:?}"
            );
        }
    }
}
