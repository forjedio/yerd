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
