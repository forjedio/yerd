//! launchd socket-activated port forwarders (macOS privileged ports).
//!
//! `yerd elevate ports` / `yerd elevate lan` install one root-owned launchd
//! daemon per privileged port. launchd binds the socket itself (`Sockets`) and,
//! on the first connection, spawns `yerd-forward` as the invoking user
//! (`UserName`) in `inetdCompatibility` `Wait` mode, which hands it the
//! listening socket on stdin. The forwarder relays to the daemon's rootless
//! port. No pf is involved, so other pf clients such as Internet Sharing
//! can't break it and iCloud Private Relay keeps working.
//!
//! The plist lives in root-owned `/Library/LaunchDaemons` (root:wheel 0644), so
//! only root can change `UserName` or `ProgramArguments`. The binary it names
//! sits in the app bundle and may be user-writable, but launchd runs it as
//! `UserName`, never as root, so replacing it grants nothing that user lacks.
//!
//! Pure: composes and parses plist text only. The helper does the writes and
//! `launchctl` calls; the macOS port redirector reads the plists back to learn
//! the forward targets.

use std::fmt::Write as _;
use std::path::{Component, Path};

/// File name of the forwarder binary, a sibling of `yerd-helper`.
pub const FORWARDER_BIN: &str = "yerd-forward";
/// Forwarder flag carrying the rootless target port.
pub const FORWARDER_TO_FLAG: &str = "--to";
/// Forwarder flag carrying the listener scope.
pub const FORWARDER_SCOPE_FLAG: &str = "--scope";

/// Directory holding system-domain launchd daemons.
pub const LAUNCH_DAEMONS_DIR: &str = "/Library/LaunchDaemons";

/// Loopback HTTP job label.
pub const PORTS_HTTP_LABEL: &str = "dev.yerd.ports.http";
/// Loopback HTTPS job label.
pub const PORTS_HTTPS_LABEL: &str = "dev.yerd.ports.https";
/// LAN HTTP job label.
pub const LAN_PORTS_HTTP_LABEL: &str = "dev.yerd.lan.ports.http";
/// LAN HTTPS job label.
pub const LAN_PORTS_HTTPS_LABEL: &str = "dev.yerd.lan.ports.https";

/// Loopback HTTP job plist path.
pub const PORTS_HTTP_PLIST: &str = "/Library/LaunchDaemons/dev.yerd.ports.http.plist";
/// Loopback HTTPS job plist path.
pub const PORTS_HTTPS_PLIST: &str = "/Library/LaunchDaemons/dev.yerd.ports.https.plist";
/// LAN HTTP job plist path.
pub const LAN_PORTS_HTTP_PLIST: &str = "/Library/LaunchDaemons/dev.yerd.lan.ports.http.plist";
/// LAN HTTPS job plist path.
pub const LAN_PORTS_HTTPS_PLIST: &str = "/Library/LaunchDaemons/dev.yerd.lan.ports.https.plist";

/// Which listener set a job belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    /// `127.0.0.1`, installed by `elevate ports`.
    Loopback,
    /// `0.0.0.0`, installed by `elevate lan`.
    Lan,
}

/// Which privileged port a job carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    /// The HTTP port (80).
    Http,
    /// The HTTPS port (443).
    Https,
}

/// One forwarder job: its label and plist path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Job {
    /// launchd label.
    pub label: &'static str,
    /// Absolute plist path under [`LAUNCH_DAEMONS_DIR`].
    pub plist: &'static str,
    /// Port role.
    pub role: Role,
}

impl Scope {
    /// Address launchd binds for this scope. LAN uses the wildcard so the job
    /// loads at boot before DHCP and survives LAN IP changes.
    #[must_use]
    pub const fn node(self) -> &'static str {
        match self {
            Self::Loopback => "127.0.0.1",
            Self::Lan => "0.0.0.0",
        }
    }

    /// Value passed to the forwarder's [`FORWARDER_SCOPE_FLAG`].
    #[must_use]
    pub const fn arg(self) -> &'static str {
        match self {
            Self::Loopback => "loopback",
            Self::Lan => "lan",
        }
    }

    /// The scope's two jobs, HTTP first.
    #[must_use]
    pub const fn jobs(self) -> [Job; 2] {
        match self {
            Self::Loopback => [
                Job {
                    label: PORTS_HTTP_LABEL,
                    plist: PORTS_HTTP_PLIST,
                    role: Role::Http,
                },
                Job {
                    label: PORTS_HTTPS_LABEL,
                    plist: PORTS_HTTPS_PLIST,
                    role: Role::Https,
                },
            ],
            Self::Lan => [
                Job {
                    label: LAN_PORTS_HTTP_LABEL,
                    plist: LAN_PORTS_HTTP_PLIST,
                    role: Role::Http,
                },
                Job {
                    label: LAN_PORTS_HTTPS_LABEL,
                    plist: LAN_PORTS_HTTPS_PLIST,
                    role: Role::Https,
                },
            ],
        }
    }
}

/// Compose a forwarder job plist: launchd binds `scope.node():from` and spawns
/// `forwarder --scope <scope> --to <to>` as `user` with the listening socket
/// on stdin. Every interpolated value is XML-escaped.
///
/// There is deliberately no `RunAtLoad` or `KeepAlive`: launchd holds the
/// socket from load (and every boot) and spawns the forwarder on demand.
/// `ProcessType` `Interactive` keeps launchd from throttling a process that
/// sits on every request.
#[must_use]
pub fn compose_forwarder_plist(
    scope: Scope,
    label: &str,
    forwarder: &Path,
    user: &str,
    from: u16,
    to: u16,
) -> String {
    let mut out = String::new();
    let _ = write!(
        out,
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
         <plist version=\"1.0\">\n\
         <dict>\n\
         \t<key>Label</key>\n\
         \t<string>{label}</string>\n\
         \t<key>ProgramArguments</key>\n\
         \t<array>\n\
         \t\t<string>{forwarder}</string>\n\
         \t\t<string>{scope_flag}</string>\n\
         \t\t<string>{scope}</string>\n\
         \t\t<string>{to_flag}</string>\n\
         \t\t<string>{to}</string>\n\
         \t</array>\n\
         \t<key>UserName</key>\n\
         \t<string>{user}</string>\n\
         \t<key>ProcessType</key>\n\
         \t<string>Interactive</string>\n\
         \t<key>inetdCompatibility</key>\n\
         \t<dict>\n\
         \t\t<key>Wait</key>\n\
         \t\t<true/>\n\
         \t</dict>\n\
         \t<key>Sockets</key>\n\
         \t<dict>\n\
         \t\t<key>Listeners</key>\n\
         \t\t<dict>\n\
         \t\t\t<key>SockNodeName</key>\n\
         \t\t\t<string>{node}</string>\n\
         \t\t\t<key>SockServiceName</key>\n\
         \t\t\t<string>{from}</string>\n\
         \t\t\t<key>SockFamily</key>\n\
         \t\t\t<string>IPv4</string>\n\
         \t\t\t<key>SockType</key>\n\
         \t\t\t<string>stream</string>\n\
         \t\t</dict>\n\
         \t</dict>\n\
         </dict>\n\
         </plist>\n",
        label = xml_escape(label),
        forwarder = xml_escape(&forwarder.to_string_lossy()),
        scope_flag = FORWARDER_SCOPE_FLAG,
        scope = scope.arg(),
        to_flag = FORWARDER_TO_FLAG,
        user = xml_escape(user),
        node = scope.node(),
    );
    out
}

/// The `--to` port from a plist written by [`compose_forwarder_plist`], or
/// `None` when the text is not one.
#[must_use]
pub fn parse_forwarder_target(plist: &str) -> Option<u16> {
    let mut values = string_values(plist);
    while let Some(value) = values.next() {
        if value == FORWARDER_TO_FLAG {
            return values.next()?.parse::<u16>().ok().filter(|p| *p != 0);
        }
    }
    None
}

/// True for an absolute path that isn't under macOS App Translocation or a
/// mounted volume in `/Volumes`. A translocated app runs from a randomised
/// read-only mount that is gone after reboot, and an app run straight from its
/// disk image disappears on eject, so a plist pointing at either stops working
/// while launchd keeps holding the port.
#[must_use]
pub fn is_persistent_forwarder_path(path: &Path) -> bool {
    let mut normal = path.components().filter_map(|c| match c {
        Component::Normal(name) => Some(name),
        _ => None,
    });
    path.is_absolute()
        && normal.next().is_some_and(|first| first != "Volumes")
        && !normal.any(|name| name == "AppTranslocation")
}

fn string_values(plist: &str) -> impl Iterator<Item = &str> {
    plist
        .split("<string>")
        .skip(1)
        .filter_map(|chunk| chunk.split_once("</string>").map(|(value, _)| value.trim()))
}

pub(crate) fn xml_escape(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for c in value.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            other => out.push(other),
        }
    }
    out
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

    const FWD: &str = "/Applications/Yerd.app/Contents/MacOS/yerd-forward";

    fn compose(scope: Scope, to: u16) -> String {
        compose_forwarder_plist(
            scope,
            scope.jobs()[0].label,
            Path::new(FWD),
            "coding",
            80,
            to,
        )
    }

    #[test]
    fn loopback_plist_shape() {
        let p = compose(Scope::Loopback, 8080);
        for needle in [
            "<string>dev.yerd.ports.http</string>",
            "<key>UserName</key>\n\t<string>coding</string>",
            "<key>ProcessType</key>\n\t<string>Interactive</string>",
            "<key>inetdCompatibility</key>",
            "<key>Wait</key>\n\t\t<true/>",
            "<string>127.0.0.1</string>",
            "<key>SockServiceName</key>\n\t\t\t<string>80</string>",
            "<string>IPv4</string>",
            "<string>stream</string>",
            "<string>--scope</string>\n\t\t<string>loopback</string>",
            "<string>--to</string>\n\t\t<string>8080</string>",
        ] {
            assert!(p.contains(needle), "missing {needle:?} in\n{p}");
        }
        assert!(p.contains(&format!("<array>\n\t\t<string>{FWD}</string>")));
        assert!(!p.contains("RunAtLoad"));
        assert!(!p.contains("KeepAlive"));
    }

    #[test]
    fn lan_plist_binds_wildcard_with_lan_scope() {
        let p = compose(Scope::Lan, 8080);
        assert!(p.contains("<string>0.0.0.0</string>"));
        assert!(p.contains("<string>--scope</string>\n\t\t<string>lan</string>"));
        assert!(p.contains("<string>dev.yerd.lan.ports.http</string>"));
    }

    #[test]
    fn interpolated_values_are_escaped() {
        let p = compose_forwarder_plist(
            Scope::Loopback,
            PORTS_HTTP_LABEL,
            Path::new("/tmp/a&b<c>/yerd-forward"),
            "o'neil\"x",
            80,
            8080,
        );
        assert!(p.contains("/tmp/a&amp;b&lt;c&gt;/yerd-forward"));
        assert!(p.contains("o&apos;neil&quot;x"));
        assert!(!p.contains("a&b"));
    }

    #[test]
    fn target_round_trips() {
        for to in [8080, 9080, 8443] {
            for scope in [Scope::Loopback, Scope::Lan] {
                assert_eq!(parse_forwarder_target(&compose(scope, to)), Some(to));
            }
        }
    }

    #[test]
    fn parse_target_rejects_garbage() {
        let cases = [
            "",
            "not a plist",
            "<string>--scope</string><string>lan</string>",
            "<string>--to</string>",
            "<string>--to</string><string>http</string>",
            "<string>--to</string><string>0</string>",
        ];
        for text in cases {
            assert_eq!(parse_forwarder_target(text), None, "{text:?}");
        }
    }

    #[test]
    fn jobs_have_distinct_labels_and_matching_paths() {
        let all: Vec<Job> = [Scope::Loopback, Scope::Lan]
            .iter()
            .flat_map(|s| s.jobs())
            .collect();
        for job in &all {
            assert_eq!(
                job.plist,
                format!("{LAUNCH_DAEMONS_DIR}/{}.plist", job.label)
            );
        }
        let mut labels: Vec<&str> = all.iter().map(|j| j.label).collect();
        labels.sort_unstable();
        labels.dedup();
        assert_eq!(labels.len(), 4);
        assert_eq!(Scope::Loopback.jobs()[0].role, Role::Http);
        assert_eq!(Scope::Lan.jobs()[1].role, Role::Https);
    }

    #[test]
    fn persistent_path_table() {
        let cases = [
            (FWD, true),
            ("yerd-forward", false),
            ("./yerd-forward", false),
            (
                "/private/var/folders/x/T/AppTranslocation/ABC/d/Yerd.app/Contents/MacOS/yerd-forward",
                false,
            ),
            ("/Users/me/Code/yerd/target/debug/yerd-forward", true),
            ("/Volumes/Yerd/Yerd.app/Contents/MacOS/yerd-forward", false),
            ("/Applications/Volumes/yerd-forward", true),
            ("/", false),
        ];
        for (path, want) in cases {
            assert_eq!(
                is_persistent_forwarder_path(Path::new(path)),
                want,
                "{path}"
            );
        }
    }
}
