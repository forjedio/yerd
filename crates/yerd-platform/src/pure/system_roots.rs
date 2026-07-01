//! Pure decision helpers for locating the host's public CA-root bundle.
//!
//! The bundled PHP needs a PEM file of public roots (plus the Yerd CA) to
//! verify `.test` HTTPS. On Linux that bundle is a well-known file shipped by
//! the `ca-certificates` package; the candidate list and the "first readable
//! wins" selection live here so they are table-tested on every host, while the
//! actual `fs` read stays in the OS impl. macOS sources its roots from the
//! keychain instead (see `os::macos`), so it does not use these candidates.

use std::path::{Path, PathBuf};

/// Ordered Linux system CA-bundle candidates, most-common first. The
/// `ca-certificates` package guarantees at least one of these exists on
/// essentially every distro.
const LINUX_ROOT_CANDIDATES: &[&str] = &[
    "/etc/ssl/certs/ca-certificates.crt", // Debian/Ubuntu/Arch
    "/etc/pki/tls/certs/ca-bundle.crt",   // RHEL/Fedora
    "/etc/ssl/cert.pem",                  // Alpine/others
    "/etc/ssl/ca-bundle.pem",             // openSUSE
];

/// The [`LINUX_ROOT_CANDIDATES`] as owned paths.
#[must_use]
pub fn linux_root_candidates() -> Vec<PathBuf> {
    LINUX_ROOT_CANDIDATES.iter().map(PathBuf::from).collect()
}

/// Return the contents of the first candidate that `read` yields a non-empty
/// string for, in order. `read` returns `None` for a missing/unreadable/empty
/// file. Returns `None` when no candidate is usable.
pub fn pick_first_readable(
    candidates: &[PathBuf],
    read: impl Fn(&Path) -> Option<String>,
) -> Option<String> {
    candidates
        .iter()
        .filter_map(|c| read(c))
        .find(|s| !s.trim().is_empty())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn linux_candidates_are_absolute_and_ordered() {
        let c = linux_root_candidates();
        assert_eq!(
            c.first().unwrap(),
            Path::new("/etc/ssl/certs/ca-certificates.crt")
        );
        assert!(c.iter().all(|p| p.is_absolute()));
        assert_eq!(c.len(), 4);
    }

    #[test]
    fn pick_returns_first_readable() {
        let present: HashMap<PathBuf, String> = [
            (PathBuf::from("/b"), "roots-b".to_owned()),
            (PathBuf::from("/c"), "roots-c".to_owned()),
        ]
        .into_iter()
        .collect();
        let candidates = [
            PathBuf::from("/a"),
            PathBuf::from("/b"),
            PathBuf::from("/c"),
        ];
        let got = pick_first_readable(&candidates, |p| present.get(p).cloned());
        assert_eq!(got.as_deref(), Some("roots-b"));
    }

    #[test]
    fn pick_returns_none_when_all_absent() {
        let candidates = [PathBuf::from("/a"), PathBuf::from("/b")];
        assert!(pick_first_readable(&candidates, |_| None).is_none());
    }

    #[test]
    fn pick_skips_empty_and_whitespace_only() {
        let present: HashMap<PathBuf, String> = [
            (PathBuf::from("/a"), "   \n".to_owned()),
            (PathBuf::from("/b"), "real".to_owned()),
        ]
        .into_iter()
        .collect();
        let candidates = [PathBuf::from("/a"), PathBuf::from("/b")];
        let got = pick_first_readable(&candidates, |p| present.get(p).cloned());
        assert_eq!(got.as_deref(), Some("real"));
    }
}
