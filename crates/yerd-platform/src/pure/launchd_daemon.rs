//! The system-domain launchd job that runs `yerdd` for a headless macOS install.
//!
//! `sudo yerd daemon install` has `yerd-helper` write a root-owned
//! `/Library/LaunchDaemons/dev.yerd.daemon.plist` that runs `yerdd serve` as the
//! invoking user (`UserName`) from boot, with nobody logged in. `HOME` is set
//! explicitly because launchd does not set it for a `UserName` job and the
//! daemon derives its data dir from it.
//!
//! `KeepAlive` with `SuccessfulExit` false restarts a crashed daemon but leaves
//! it down after a clean exit, and `yerdd serve` exits 0 on SIGTERM, so an
//! unprivileged `yerd daemon stop` keeps it stopped.
//!
//! Pure: composes plist text only. The helper does the write and the
//! `launchctl` calls.

use std::fmt::Write as _;
use std::path::Path;

use super::launchd_ports::xml_escape;

/// launchd label of the daemon job, shared with the desktop app's per-user
/// `LaunchAgent`.
pub const DAEMON_LABEL: &str = "dev.yerd.daemon";
/// Absolute path of the headless system-domain daemon plist.
pub const DAEMON_PLIST: &str = "/Library/LaunchDaemons/dev.yerd.daemon.plist";
/// The desktop app's loose per-user `LaunchAgent`, relative to the user's home.
pub const GUI_AGENT_RELPATH: &str = "Library/LaunchAgents/dev.yerd.daemon.plist";

/// Compose the daemon job plist running `yerdd serve` as `user` with `HOME`
/// set to `home`. Every interpolated value is XML-escaped.
#[must_use]
pub fn compose_daemon_plist(yerdd: &Path, user: &str, home: &Path) -> String {
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
         \t\t<string>{yerdd}</string>\n\
         \t\t<string>serve</string>\n\
         \t</array>\n\
         \t<key>UserName</key>\n\
         \t<string>{user}</string>\n\
         \t<key>EnvironmentVariables</key>\n\
         \t<dict>\n\
         \t\t<key>HOME</key>\n\
         \t\t<string>{home}</string>\n\
         \t</dict>\n\
         \t<key>RunAtLoad</key>\n\
         \t<true/>\n\
         \t<key>KeepAlive</key>\n\
         \t<dict>\n\
         \t\t<key>SuccessfulExit</key>\n\
         \t\t<false/>\n\
         \t</dict>\n\
         \t<key>ThrottleInterval</key>\n\
         \t<integer>5</integer>\n\
         </dict>\n\
         </plist>\n",
        label = DAEMON_LABEL,
        yerdd = xml_escape(&yerdd.to_string_lossy()),
        user = xml_escape(user),
        home = xml_escape(&home.to_string_lossy()),
    );
    out
}

/// Who a registered daemon job runs as and which `yerdd` it runs, read back
/// from a plist written by [`compose_daemon_plist`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DaemonJobIdentity {
    /// The `yerdd` path (first `ProgramArguments` entry).
    pub yerdd: String,
    /// The account the job runs as (`UserName`).
    pub user: String,
}

/// Parse the identity out of `plist`, or `None` if it isn't shaped like one
/// [`compose_daemon_plist`] wrote.
#[must_use]
pub fn parse_daemon_plist(plist: &str) -> Option<DaemonJobIdentity> {
    let (_, args) = plist.split_once("<key>ProgramArguments</key>")?;
    let (_, user) = plist.split_once("<key>UserName</key>")?;
    Some(DaemonJobIdentity {
        yerdd: xml_unescape(first_string(args)?),
        user: xml_unescape(first_string(user)?),
    })
}

fn first_string(text: &str) -> Option<&str> {
    let (_, rest) = text.split_once("<string>")?;
    let (value, _) = rest.split_once("</string>")?;
    Some(value)
}

fn xml_unescape(value: &str) -> String {
    value
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
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

    #[test]
    fn plist_contains_every_key() {
        let p = compose_daemon_plist(
            Path::new("/Users/me/.yerd/bin/yerdd"),
            "me",
            Path::new("/Users/me"),
        );
        for needle in [
            "<key>Label</key>\n\t<string>dev.yerd.daemon</string>",
            "<array>\n\t\t<string>/Users/me/.yerd/bin/yerdd</string>\n\t\t<string>serve</string>\n\t</array>",
            "<key>UserName</key>\n\t<string>me</string>",
            "<key>HOME</key>\n\t\t<string>/Users/me</string>",
            "<key>RunAtLoad</key>\n\t<true/>",
            "<key>SuccessfulExit</key>\n\t\t<false/>",
            "<key>ThrottleInterval</key>\n\t<integer>5</integer>",
        ] {
            assert!(p.contains(needle), "missing {needle:?} in\n{p}");
        }
    }

    #[test]
    fn interpolated_values_are_escaped() {
        let p = compose_daemon_plist(
            Path::new("/tmp/a&b<c>/yerdd"),
            "o'neil\"x",
            Path::new("/Users/a&b"),
        );
        assert!(p.contains("/tmp/a&amp;b&lt;c&gt;/yerdd"));
        assert!(p.contains("o&apos;neil&quot;x"));
        assert!(p.contains("/Users/a&amp;b"));
        assert!(!p.contains("a&b"));
    }

    #[test]
    fn parse_round_trips_the_composed_plist() {
        for (yerdd, user) in [
            ("/Users/me/.yerd/bin/yerdd", "me"),
            ("/tmp/a&b<c>/yerdd", "o'neil\"x"),
            ("/tmp/&amp;/yerdd", "me"),
        ] {
            let p = compose_daemon_plist(Path::new(yerdd), user, Path::new("/Users/me"));
            assert_eq!(
                parse_daemon_plist(&p),
                Some(DaemonJobIdentity {
                    yerdd: yerdd.to_owned(),
                    user: user.to_owned(),
                })
            );
        }
        assert_eq!(parse_daemon_plist(""), None);
        assert_eq!(
            parse_daemon_plist("<key>ProgramArguments</key><array></array>"),
            None
        );
    }

    #[test]
    fn plist_path_matches_label() {
        assert_eq!(
            DAEMON_PLIST,
            format!(
                "{}/{DAEMON_LABEL}.plist",
                super::super::launchd_ports::LAUNCH_DAEMONS_DIR
            )
        );
        assert!(GUI_AGENT_RELPATH.ends_with(&format!("{DAEMON_LABEL}.plist")));
    }
}
