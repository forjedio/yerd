//! Pure derivation of the Windows named-pipe name, its SDDL security
//! descriptor, and the SID parse from `whoami` output.
//!
//! Compiled on every OS so Linux/macOS CI table-tests it too (the
//! "decisions in pure helpers" rule). The daemon and every client derive the
//! same pipe name from the same `(sid, runtime dir)` pair, so they agree without
//! a shared registry.

use std::fmt::Write as _;
use std::path::Path;

use sha2::{Digest, Sha256};

/// The deterministic pipe name for the daemon owned by `sid`, rooted at
/// `runtime`: `yerd-<sid>-<h>`, where `h` is the first 16 hex chars of
/// `SHA-256(runtime path bytes)`.
///
/// - **SID component** is the locked, per-user-unique key a session-0
///   service will key the DACL on. Named-pipe names are inherently global
///   (`\\.\pipe\` has no per-session namespace), so cross-session reachability is
///   controlled by the DACL, not a `Global\` prefix.
/// - **runtime-dir hash** preserves production determinism (daemon and clients
///   both resolve the same `%TEMP%\yerd`) while giving tempdir-rooted
///   integration tests collision-free names, so the lifecycle tests can run in
///   parallel and coexist with a real installed daemon.
#[must_use]
pub fn pipe_name(sid: &str, runtime: &Path) -> String {
    let mut hasher = Sha256::new();
    hasher.update(runtime.to_string_lossy().as_bytes());
    let digest = hasher.finalize();
    let mut hex = String::with_capacity(16);
    for byte in digest.iter().take(8) {
        let _ = write!(hex, "{byte:02x}");
    }
    format!("yerd-{sid}-{hex}")
}

/// The SDDL for the daemon pipe's DACL: a protected DACL (no inheritance) that
/// grants full access to SYSTEM (so a service account can own/serve
/// it) and to the named user, denying everyone else by absence.
#[must_use]
pub fn pipe_sddl(sid: &str) -> String {
    format!("D:P(A;;GA;;;SY)(A;;GA;;;{sid})")
}

/// Parse a user SID from `whoami /user /fo csv /nh` output. The last CSV field
/// is the SID (`"machine\user","S-1-5-21-..."`); strip quotes/whitespace and
/// validate the `S-1-` prefix and `[0-9-]` charset. Rejecting anything else also
/// guards the SDDL / pipe-name injection surface.
#[must_use]
pub fn parse_whoami_sid(stdout: &str) -> Option<String> {
    let line = stdout.lines().find(|l| !l.trim().is_empty())?;
    let last = line.rsplit(',').next()?;
    let sid = last.trim().trim_matches('"').trim();
    if !sid.starts_with("S-1-") {
        return None;
    }
    let rest = sid.get("S-1-".len()..)?;
    if rest.is_empty() || !rest.bytes().all(|b| b.is_ascii_digit() || b == b'-') {
        return None;
    }
    Some(sid.to_owned())
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
    use std::path::PathBuf;

    #[test]
    fn pipe_name_is_stable_and_golden() {
        let name = pipe_name("S-1-5-21-1-2-3-1001", &PathBuf::from("/tmp/yerd"));
        assert!(name.starts_with("yerd-S-1-5-21-1-2-3-1001-"));
        assert_eq!(
            name,
            pipe_name("S-1-5-21-1-2-3-1001", &PathBuf::from("/tmp/yerd")),
            "same inputs must yield the same name"
        );
        let (_, hex) = name.rsplit_once('-').unwrap();
        assert_eq!(hex.len(), 16);
        assert!(hex.bytes().all(|b| b.is_ascii_hexdigit()));
    }

    #[test]
    fn pipe_name_differs_by_runtime_dir() {
        let sid = "S-1-5-21-1-2-3-1001";
        assert_ne!(
            pipe_name(sid, &PathBuf::from("/tmp/yerd-a")),
            pipe_name(sid, &PathBuf::from("/tmp/yerd-b"))
        );
    }

    #[test]
    fn sddl_is_golden() {
        assert_eq!(
            pipe_sddl("S-1-5-21-1-2-3-1001"),
            "D:P(A;;GA;;;SY)(A;;GA;;;S-1-5-21-1-2-3-1001)"
        );
    }

    #[test]
    fn parse_sid_from_real_shaped_csv() {
        let out = "\"machine\\user\",\"S-1-5-21-1004336348-1177238915-682003330-512\"\n";
        assert_eq!(
            parse_whoami_sid(out).as_deref(),
            Some("S-1-5-21-1004336348-1177238915-682003330-512")
        );
    }

    #[test]
    fn parse_sid_tolerates_no_quotes_and_whitespace() {
        assert_eq!(
            parse_whoami_sid("machine\\user, S-1-5-18 \n").as_deref(),
            Some("S-1-5-18")
        );
    }

    #[test]
    fn parse_sid_rejects_junk() {
        assert_eq!(parse_whoami_sid(""), None);
        assert_eq!(parse_whoami_sid("no comma here"), None);
        assert_eq!(parse_whoami_sid("\"m\\u\",\"not-a-sid\"\n"), None);
        assert_eq!(parse_whoami_sid("\"m\\u\",\"S-1-\"\n"), None);
        assert_eq!(
            parse_whoami_sid("\"m\\u\",\"S-1-5-21-abc\"\n"),
            None,
            "non-digit tail must be rejected (injection guard)"
        );
    }
}
