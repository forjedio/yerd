//! Compose and parse macOS `/etc/resolver/<tld>` files.
//!
//! The file format is documented by `resolver(5)`. Yerd uses the two
//! directives it actually relies on:
//!
//! ```text
//! nameserver <ip>
//! port <number>
//! ```
//!
//! Parsing is tolerant: comments, blank lines, additional directives, and
//! arbitrary whitespace between tokens are all ignored. Structural compare
//! against a freshly-composed file is the basis of macOS `is_installed`.

use std::net::SocketAddr;

/// The two fields Yerd writes into `/etc/resolver/<tld>`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResolverFile {
    /// `nameserver` directive — the IP address only.
    pub nameserver: std::net::IpAddr,
    /// `port` directive (defaults to 53 in `resolver(5)` but Yerd always
    /// writes it explicitly).
    pub port: u16,
}

impl ResolverFile {
    /// Convenience constructor from a `SocketAddr`.
    #[must_use]
    pub fn from_addr(addr: SocketAddr) -> Self {
        Self {
            nameserver: addr.ip(),
            port: addr.port(),
        }
    }
}

/// Compose the file content Yerd writes for `/etc/resolver/<tld>`.
///
/// The result ends in a trailing newline.
#[must_use]
pub fn compose(addr: SocketAddr) -> String {
    format!("nameserver {}\nport {}\n", addr.ip(), addr.port())
}

/// Parse a `/etc/resolver/<tld>` file, returning the structural fields.
///
/// Returns `None` if the file lacks a `nameserver` line. A missing or
/// invalid `port` line resolves to `53` per `resolver(5)`.
#[must_use]
pub fn parse(text: &str) -> Option<ResolverFile> {
    let mut nameserver: Option<std::net::IpAddr> = None;
    let mut port: Option<u16> = None;
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut tokens = line.split_whitespace();
        let directive = tokens.next().unwrap_or("");
        let value = tokens.next().unwrap_or("");
        match directive {
            "nameserver" => {
                if let Ok(ip) = value.parse::<std::net::IpAddr>() {
                    nameserver = Some(ip);
                }
            }
            "port" => {
                if let Ok(p) = value.parse::<u16>() {
                    port = Some(p);
                }
            }
            _ => {}
        }
    }
    Some(ResolverFile {
        nameserver: nameserver?,
        port: port.unwrap_or(53),
    })
}

/// True iff `text` parses to a `ResolverFile` equal to `expected`. Used by
/// macOS `is_installed` so the probe ignores comments and ordering.
#[must_use]
pub fn matches(text: &str, expected: SocketAddr) -> bool {
    parse(text) == Some(ResolverFile::from_addr(expected))
}

/// Whether `text` is safe to restore as an `/etc/resolver/<tld>` file: it parses
/// to a valid `nameserver`/`port`. Guards the macOS unelevate restore path from
/// writing an empty or garbage backup back as the system resolver config — when
/// this is false the helper deletes Yerd's file instead of restoring junk.
#[must_use]
pub fn restorable(text: &str) -> bool {
    parse(text).is_some()
}

// ── backups of a replaced `/etc/resolver/<tld>` (macOS) ──────────────────────
//
// When the helper overwrites a pre-existing resolver file (e.g. a Valet/Herd
// leftover), it first copies the old content here so it can be restored. The
// helper (root) writes the dir; the user's daemon reads it back to report the
// backup in `doctor`. Filenames are `"<tld>-<unixsecs>.conf"` — the path/format
// logic is pure and lives here; the I/O lives in the helper and daemon.

/// The system-level directory Yerd stores replaced-resolver backups in.
///
/// System (not user) Application Support so the root helper writes it and any
/// user's daemon can read it back.
#[cfg(target_os = "macos")]
#[must_use]
pub fn macos_backup_dir() -> std::path::PathBuf {
    std::path::PathBuf::from("/Library/Application Support/io.yerd.Yerd/resolver-backups")
}

/// Backup filename for `tld` captured at unix time `secs`: `"<tld>-<secs>.conf"`.
#[must_use]
pub fn backup_filename(tld: &str, secs: u64) -> String {
    format!("{tld}-{secs}.conf")
}

/// Parse the unix-seconds stamp out of a backup `name` for `tld`, i.e. the
/// inverse of [`backup_filename`]. Requires the exact `"<tld>-<digits>.conf"`
/// shape, so foreign files (`other-123.conf`, `testextra-123.conf`) and any
/// non-numeric stamp yield `None`; a hyphenated `tld` (`my-test`) still works
/// because the prefix is matched whole.
#[must_use]
pub fn parse_backup_secs(name: &str, tld: &str) -> Option<u64> {
    name.strip_prefix(&format!("{tld}-"))?
        .strip_suffix(".conf")?
        .parse::<u64>()
        .ok()
}

/// From a directory listing, return the most recent backup filename for `tld`
/// (the entry with the **numerically** largest stamp — `1000` beats `999`,
/// which a lexicographic compare would get wrong). `None` if none match. Pure;
/// the OS layer supplies the listing.
#[must_use]
pub fn latest_backup<'a>(filenames: &'a [String], tld: &str) -> Option<&'a str> {
    filenames
        .iter()
        .filter_map(|name| Some((parse_backup_secs(name, tld)?, name.as_str())))
        .max_by_key(|(secs, _)| *secs)
        .map(|(_, name)| name)
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
    use std::net::{IpAddr, Ipv4Addr, SocketAddr};

    fn loopback(port: u16) -> SocketAddr {
        SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port)
    }

    #[test]
    fn compose_emits_two_lines_with_trailing_newline() {
        let s = compose(loopback(53));
        assert_eq!(s, "nameserver 127.0.0.1\nport 53\n");
    }

    #[test]
    fn parse_roundtrips_compose() {
        let addr = loopback(5353);
        let s = compose(addr);
        assert_eq!(parse(&s), Some(ResolverFile::from_addr(addr)));
    }

    #[test]
    fn parse_tolerates_extra_whitespace_and_comments() {
        let text = "# yerd-managed\n\n   nameserver    127.0.0.1   \nport\t53\n";
        let r = parse(text).unwrap();
        assert_eq!(r.nameserver, IpAddr::V4(Ipv4Addr::LOCALHOST));
        assert_eq!(r.port, 53);
    }

    #[test]
    fn parse_missing_port_defaults_to_53() {
        let text = "nameserver 127.0.0.1\n";
        let r = parse(text).unwrap();
        assert_eq!(r.port, 53);
    }

    #[test]
    fn parse_missing_nameserver_returns_none() {
        let text = "port 53\n";
        assert!(parse(text).is_none());
    }

    #[test]
    fn parse_invalid_nameserver_returns_none() {
        let text = "nameserver bogus\n";
        assert!(parse(text).is_none());
    }

    #[test]
    fn restorable_accepts_valid_and_rejects_junk() {
        assert!(restorable("nameserver 192.168.1.1\nport 53\n"));
        assert!(restorable("nameserver 127.0.0.1\n")); // port defaults to 53
        assert!(!restorable("")); // empty backup → don't restore
        assert!(!restorable("port 53\n")); // no nameserver
        assert!(!restorable("garbage not a resolver file"));
    }

    #[test]
    fn matches_ignores_comments() {
        let composed = compose(loopback(53));
        let with_comment = format!("# yerd\n{composed}# trailing\n");
        assert!(matches(&with_comment, loopback(53)));
    }

    #[test]
    fn matches_false_on_port_mismatch() {
        let composed = compose(loopback(53));
        assert!(!matches(&composed, loopback(5353)));
    }

    #[test]
    fn matches_false_on_ip_mismatch() {
        let composed = compose(loopback(53));
        let other: SocketAddr = "192.168.1.1:53".parse().unwrap();
        assert!(!matches(&composed, other));
    }

    #[test]
    fn backup_filename_roundtrips_parse() {
        let name = backup_filename("test", 1_717_142_400);
        assert_eq!(name, "test-1717142400.conf");
        assert_eq!(parse_backup_secs(&name, "test"), Some(1_717_142_400));
    }

    #[test]
    fn parse_backup_secs_rejects_foreign_and_malformed() {
        // Different tld, tld as a strict prefix, and a non-numeric stamp.
        assert_eq!(parse_backup_secs("other-123.conf", "test"), None);
        assert_eq!(parse_backup_secs("testextra-123.conf", "test"), None);
        assert_eq!(parse_backup_secs("test-nope.conf", "test"), None);
        assert_eq!(parse_backup_secs("test-123.txt", "test"), None);
    }

    #[test]
    fn parse_backup_secs_handles_hyphenated_tld() {
        assert_eq!(parse_backup_secs("my-test-99.conf", "my-test"), Some(99));
        // The middle hyphen must not be mistaken for the stamp separator.
        assert_eq!(parse_backup_secs("my-test-99.conf", "my"), None);
    }

    #[test]
    fn latest_backup_picks_numeric_max_not_lexicographic() {
        let names = vec![
            "test-999.conf".to_owned(),
            "test-1000.conf".to_owned(),
            "other-5000.conf".to_owned(), // foreign tld — ignored
            "test-123.conf".to_owned(),
        ];
        // 1000 > 999 numerically (lexicographically "999" > "1000").
        assert_eq!(latest_backup(&names, "test"), Some("test-1000.conf"));
    }

    #[test]
    fn latest_backup_empty_or_no_match_is_none() {
        assert_eq!(latest_backup(&[], "test"), None);
        assert_eq!(latest_backup(&["other-1.conf".to_owned()], "test"), None);
    }
}
