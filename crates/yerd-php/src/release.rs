//! Pure resolution of prebuilt static-PHP download artifacts.
//!
//! Versions come from yerd's **own** signed manifest `php.json`, published on a
//! single rolling GitHub release of the `forjedio/yerd-php` build repo. Those
//! binaries link libcurl **without c-ares**, so PHP
//! resolves yerd's scoped `.test` resolver (issue #59); the previous upstream
//! `dl.static-php.dev` builds did not. The daemon fetches `php.json` +
//! `php.json.minisig`, verifies the minisign signature (at the I/O edge), then
//! hands the JSON body to [`resolve_from_listing`] / [`available_minors`] (both
//! pure). Each build carries a per-tarball SHA-256 (verified after download) and
//! a **revision** (`-N`) counter so a rebuild of an unchanged patch surfaces as
//! an available upgrade to existing installs.
//!
//! ## Manifest format (`php.json`)
//!
//! ```json
//! {
//!   "schema": 1,
//!   "builds": [
//!     {
//!       "php": "8.5.7", "minor": "8.5", "os": "macos", "arch": "aarch64",
//!       "revision": 1,
//!       "cli": { "file": "php-8.5.7-1-cli-macos-aarch64.tar.gz", "sha256": "…", "size": 123 },
//!       "fpm": { "file": "php-8.5.7-1-fpm-macos-aarch64.tar.gz", "sha256": "…", "size": 123 }
//!     }
//!   ]
//! }
//! ```
//!
//! We consume the manifest's `file` field **verbatim** to build the download URL
//! (never reconstruct it), so a future naming tweak can't desync producer and
//! consumer. The `schema` field gates compatibility - an unknown schema is
//! rejected rather than misparsed.

use serde::Deserialize;
use yerd_core::PhpVersion;

use crate::error::PhpError;

/// Lowest PHP minor Yerd supports installing. The bundled `pcov` / `yerd-dump`
/// extensions are only built for 8.2+, so older minors are filtered out of the
/// installable list and rejected at resolve time even when the manifest still
/// publishes them.
pub const MIN_SUPPORTED: PhpVersion = PhpVersion::new(8, 2);

/// The `php.json` schema version this build understands. A producer-side bump
/// signals an incompatible format change (additive changes do not bump it).
pub const PHP_LISTING_SCHEMA: u32 = 1;

/// Base URL of yerd's hosted, signed PHP distribution.
///
/// A single rolling `php` release of the **separate** `forjedio/yerd-php` build
/// repo holds every `php-<full>-<revision>-<cli|fpm>-<os>-<arch>.tar.gz` asset
/// plus the generated `php.json` manifest and its detached `php.json.minisig`
/// signature. Asset URLs 302-redirect to the blob; the daemon's downloader
/// follows redirects. This crate is a pure *consumer* - the producer lives
/// entirely in `forjedio/yerd-php`.
pub const PHP_LISTING_BASE_URL: &str = "https://github.com/forjedio/yerd-php/releases/download/php";

// ── manifest wire shape (private; deserialised from `php.json`) ──────────────

#[derive(Debug, Deserialize)]
struct Listing {
    schema: u32,
    #[serde(default)]
    builds: Vec<BuildEntry>,
}

#[derive(Debug, Deserialize)]
struct BuildEntry {
    php: String,
    minor: String,
    os: String,
    arch: String,
    revision: u32,
    cli: FileEntry,
    fpm: FileEntry,
}

#[derive(Debug, Deserialize)]
struct FileEntry {
    file: String,
    sha256: String,
    #[allow(dead_code)]
    #[serde(default)]
    size: u64,
}

/// Parse + schema-check a `php.json` body.
fn parse_listing(listing: &str) -> Result<Listing, PhpError> {
    let parsed: Listing = serde_json::from_str(listing).map_err(|e| PhpError::ListingParse {
        detail: e.to_string(),
    })?;
    if parsed.schema != PHP_LISTING_SCHEMA {
        return Err(PhpError::UnsupportedListingSchema {
            found: parsed.schema,
            supported: PHP_LISTING_SCHEMA,
        });
    }
    Ok(parsed)
}

/// Target operating system for a prebuilt artifact.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Os {
    /// Linux (glibc build - can load shared extensions; the manifest never
    /// ships a fully-static musl build, which can't `dlopen`).
    Linux,
    /// macOS.
    Macos,
}

impl Os {
    /// The token used in artifact filenames.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Os::Linux => "linux",
            Os::Macos => "macos",
        }
    }
}

/// Target CPU architecture for a prebuilt artifact.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Arch {
    /// 64-bit x86.
    X86_64,
    /// 64-bit ARM.
    Aarch64,
}

impl Arch {
    /// The token used in artifact filenames.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Arch::X86_64 => "x86_64",
            Arch::Aarch64 => "aarch64",
        }
    }
}

/// Which binary within a PHP build.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinaryKind {
    /// The CLI interpreter (`php`).
    Cli,
    /// The `FastCGI` process manager (`php-fpm`).
    Fpm,
}

impl BinaryKind {
    /// The token used in artifact filenames.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            BinaryKind::Cli => "cli",
            BinaryKind::Fpm => "fpm",
        }
    }

    /// Relative path segments where this binary is installed inside a
    /// per-version dir (CLI → `bin/php`, FPM → `sbin/php-fpm`; the FPM path
    /// matches `version::discover_bundled`).
    #[must_use]
    pub const fn install_segments(self) -> &'static [&'static str] {
        match self {
            BinaryKind::Cli => &["bin", "php"],
            BinaryKind::Fpm => &["sbin", "php-fpm"],
        }
    }

    /// The single file name inside the downloaded tarball.
    #[must_use]
    pub const fn archive_member(self) -> &'static str {
        match self {
            BinaryKind::Cli => "php",
            BinaryKind::Fpm => "php-fpm",
        }
    }
}

/// A resolved download plan for one PHP version + platform.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Artifact {
    /// The requested major.minor version.
    pub version: PhpVersion,
    /// The resolved full patch version (e.g. `"8.5.7"`).
    pub full_version: String,
    /// Rebuild counter of the resolved build (the `-N` suffix; `>= 1`). Written
    /// to the install's `.yerd-revision` marker and compared for upgrades.
    pub revision: u32,
    /// Per-version install directory name (e.g. `"php-8.5"`).
    pub install_dir_name: String,
    /// URL of the CLI tarball.
    pub cli_url: String,
    /// Expected SHA-256 (lowercase hex) of the CLI tarball bytes.
    pub cli_sha256: String,
    /// URL of the FPM tarball.
    pub fpm_url: String,
    /// Expected SHA-256 (lowercase hex) of the FPM tarball bytes.
    pub fpm_sha256: String,
}

/// URL of the signed `php.json` manifest (the daemon fetches this, verifies its
/// signature, then hands the body to [`resolve_from_listing`]).
#[must_use]
pub fn listing_url() -> String {
    format!("{PHP_LISTING_BASE_URL}/php.json")
}

/// URL of the detached minisign signature over [`listing_url`]'s `php.json`.
#[must_use]
pub fn listing_sig_url() -> String {
    format!("{PHP_LISTING_BASE_URL}/php.json.minisig")
}

/// Resolve a requested major.minor version + platform to an [`Artifact`] from
/// the `php.json` manifest body.
///
/// Retention guarantees at most one build per `(minor, os, arch)`, so this
/// selects the single matching entry (no patch scanning) and builds both URLs
/// from the manifest's `file` fields **verbatim**. Errors with
/// [`PhpError::VersionUnavailable`] when no matching build is published, and
/// with [`PhpError::ListingParse`] / [`PhpError::UnsupportedListingSchema`] when
/// the manifest is malformed or a newer schema.
pub fn resolve_from_listing(
    listing: &str,
    version: PhpVersion,
    os: Os,
    arch: Arch,
) -> Result<Artifact, PhpError> {
    if version < MIN_SUPPORTED {
        return Err(PhpError::VersionUnavailable { version });
    }
    let parsed = parse_listing(listing)?;
    let want_minor = format!("{}.{}", version.major, version.minor);

    let entry = parsed
        .builds
        .into_iter()
        .find(|b| b.os == os.as_str() && b.arch == arch.as_str() && b.minor == want_minor)
        .ok_or(PhpError::VersionUnavailable { version })?;

    if entry.revision == 0 {
        return Err(PhpError::ListingParse {
            detail: format!(
                "build {} ({}-{}) has revision 0, but published builds must be >= 1",
                entry.php,
                os.as_str(),
                arch.as_str()
            ),
        });
    }

    Ok(Artifact {
        install_dir_name: format!("php-{}.{}", version.major, version.minor),
        revision: entry.revision,
        cli_url: format!("{PHP_LISTING_BASE_URL}/{}", entry.cli.file),
        cli_sha256: entry.cli.sha256,
        fpm_url: format!("{PHP_LISTING_BASE_URL}/{}", entry.fpm.file),
        fpm_sha256: entry.fpm.sha256,
        full_version: entry.php,
        version,
    })
}

/// Every distinct major.minor in the manifest that has a build for `(os, arch)`,
/// ascending. Pure; the daemon fetches + verifies the manifest and hands the
/// body here to populate the "installable versions" list (the GUI dropdown /
/// `yerd list php --available`).
///
/// A malformed or unknown-schema manifest yields an empty list (the caller
/// treats PHP as uninstallable rather than erroring); use
/// [`resolve_from_listing`] when a hard error is wanted.
#[must_use]
pub fn available_minors(listing: &str, os: Os, arch: Arch) -> Vec<PhpVersion> {
    let Ok(parsed) = parse_listing(listing) else {
        return Vec::new();
    };
    let mut out: Vec<PhpVersion> = parsed
        .builds
        .iter()
        .filter(|b| b.os == os.as_str() && b.arch == arch.as_str())
        .filter_map(|b| parse_minor(&b.minor))
        .filter(|v| *v >= MIN_SUPPORTED)
        .collect();
    out.sort_unstable();
    out.dedup();
    out
}

/// Parse a `"<maj>.<min>"` minor string into a [`PhpVersion`]; `None` if either
/// component is missing or overflows `u8`.
fn parse_minor(s: &str) -> Option<PhpVersion> {
    let (major, minor) = s.split_once('.')?;
    Some(PhpVersion::new(major.parse().ok()?, minor.parse().ok()?))
}

/// Detect the running platform, erroring on anything yerd can't install for
/// (e.g. Windows, 32-bit). Call this **before** any download.
pub fn current_os_arch() -> Result<(Os, Arch), PhpError> {
    let os = match std::env::consts::OS {
        "linux" => Os::Linux,
        "macos" => Os::Macos,
        other => {
            return Err(PhpError::UnsupportedPlatform {
                detail: format!("no prebuilt PHP for OS {other:?}"),
            })
        }
    };
    let arch = match std::env::consts::ARCH {
        "x86_64" => Arch::X86_64,
        "aarch64" => Arch::Aarch64,
        other => {
            return Err(PhpError::UnsupportedPlatform {
                detail: format!("no prebuilt PHP for architecture {other:?}"),
            })
        }
    };
    Ok((os, arch))
}

/// Zip-slip guard: a tar member name is safe to trust only if it is relative
/// and contains no `..`, root, or prefix components.
#[must_use]
pub fn is_safe_member(name: &str) -> bool {
    use std::path::Component;
    !name.is_empty()
        && std::path::Path::new(name)
            .components()
            .all(|c| matches!(c, Component::Normal(_) | Component::CurDir))
}

/// The patch component of a `"<maj>.<min>.<patch>"` version string.
#[must_use]
pub fn patch_of(full_version: &str) -> Option<u32> {
    full_version.split('.').nth(2)?.parse().ok()
}

/// Whether the candidate build `(patch, revision)` is newer than the installed
/// one (same major.minor assumed). True when the candidate patch is higher, or
/// the patch is equal and the candidate revision is higher. A malformed patch on
/// either side → `false`.
///
/// The revision dimension is what makes a *rebuild of an unchanged patch* (e.g.
/// the c-ares cutover, `8.5.7-1`) reach an existing `8.5.7` install recorded as
/// revision 0. It never downgrades.
#[must_use]
pub fn is_newer_build(
    installed_patch: &str,
    installed_rev: u32,
    candidate_patch: &str,
    candidate_rev: u32,
) -> bool {
    match (patch_of(installed_patch), patch_of(candidate_patch)) {
        (Some(installed), Some(candidate)) => {
            candidate > installed || (candidate == installed && candidate_rev > installed_rev)
        }
        _ => false,
    }
}

/// The user-visible build identity `"<patch>-<revision>"`, e.g. `"8.5.7-1"`.
/// A revision of 0 (a legacy install predating the `.yerd-revision` marker)
/// renders as the bare patch, so pre-cutover installs keep their old display.
#[must_use]
pub fn display_build(patch: &str, revision: u32) -> String {
    if revision >= 1 {
        format!("{patch}-{revision}")
    } else {
        patch.to_owned()
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::panic, clippy::indexing_slicing)]
mod tests {
    use super::*;

    /// A `php.json` body spanning several minors and all four targets, shaped
    /// like the real manifest (§7). 8.1 is below the floor; 8.5 has a rebuild.
    const LISTING: &str = r#"{
        "schema": 1,
        "generated_at": "2026-07-01T00:00:00Z",
        "builds": [
            { "php": "8.1.31", "minor": "8.1", "os": "linux", "arch": "x86_64", "revision": 1,
              "cli": { "file": "php-8.1.31-1-cli-linux-x86_64.tar.gz", "sha256": "aa", "size": 1 },
              "fpm": { "file": "php-8.1.31-1-fpm-linux-x86_64.tar.gz", "sha256": "bb", "size": 1 } },
            { "php": "8.4.21", "minor": "8.4", "os": "linux", "arch": "x86_64", "revision": 3,
              "cli": { "file": "php-8.4.21-3-cli-linux-x86_64.tar.gz", "sha256": "cc", "size": 1 },
              "fpm": { "file": "php-8.4.21-3-fpm-linux-x86_64.tar.gz", "sha256": "dd", "size": 1 } },
            { "php": "8.5.7", "minor": "8.5", "os": "linux", "arch": "x86_64", "revision": 2,
              "cli": { "file": "php-8.5.7-2-cli-linux-x86_64.tar.gz", "sha256": "ee", "size": 1 },
              "fpm": { "file": "php-8.5.7-2-fpm-linux-x86_64.tar.gz", "sha256": "ff", "size": 1 } },
            { "php": "8.5.7", "minor": "8.5", "os": "linux", "arch": "aarch64", "revision": 2,
              "cli": { "file": "php-8.5.7-2-cli-linux-aarch64.tar.gz", "sha256": "11", "size": 1 },
              "fpm": { "file": "php-8.5.7-2-fpm-linux-aarch64.tar.gz", "sha256": "22", "size": 1 } },
            { "php": "8.5.7", "minor": "8.5", "os": "macos", "arch": "aarch64", "revision": 2,
              "cli": { "file": "php-8.5.7-2-cli-macos-aarch64.tar.gz", "sha256": "33", "size": 1 },
              "fpm": { "file": "php-8.5.7-2-fpm-macos-aarch64.tar.gz", "sha256": "44", "size": 1 } }
        ]
    }"#;

    #[test]
    fn resolve_from_listing_selects_entry_and_builds_urls() {
        let a =
            resolve_from_listing(LISTING, PhpVersion::new(8, 5), Os::Linux, Arch::X86_64).unwrap();
        assert_eq!(a.full_version, "8.5.7");
        assert_eq!(a.revision, 2);
        assert_eq!(a.install_dir_name, "php-8.5");
        assert_eq!(
            a.cli_url,
            "https://github.com/forjedio/yerd-php/releases/download/php/php-8.5.7-2-cli-linux-x86_64.tar.gz"
        );
        assert_eq!(a.cli_sha256, "ee");
        assert_eq!(
            a.fpm_url,
            "https://github.com/forjedio/yerd-php/releases/download/php/php-8.5.7-2-fpm-linux-x86_64.tar.gz"
        );
        assert_eq!(a.fpm_sha256, "ff");
    }

    #[test]
    fn listing_urls_point_at_the_signed_manifest() {
        assert_eq!(
            listing_url(),
            "https://github.com/forjedio/yerd-php/releases/download/php/php.json"
        );
        assert_eq!(
            listing_sig_url(),
            "https://github.com/forjedio/yerd-php/releases/download/php/php.json.minisig"
        );
    }

    #[test]
    fn resolve_from_listing_anchors_arch() {
        let a =
            resolve_from_listing(LISTING, PhpVersion::new(8, 5), Os::Linux, Arch::Aarch64).unwrap();
        assert!(a.cli_url.contains("linux-aarch64"));
        assert_eq!(a.cli_sha256, "11");
    }

    #[test]
    fn resolve_from_listing_unknown_minor_errors() {
        match resolve_from_listing(LISTING, PhpVersion::new(8, 3), Os::Linux, Arch::X86_64) {
            Err(PhpError::VersionUnavailable { version }) => {
                assert_eq!(version, PhpVersion::new(8, 3));
            }
            other => panic!("expected VersionUnavailable, got {other:?}"),
        }
    }

    #[test]
    fn resolve_rejects_unknown_schema() {
        let bad = r#"{ "schema": 99, "builds": [] }"#;
        match resolve_from_listing(bad, PhpVersion::new(8, 5), Os::Linux, Arch::X86_64) {
            Err(PhpError::UnsupportedListingSchema { found, supported }) => {
                assert_eq!(found, 99);
                assert_eq!(supported, PHP_LISTING_SCHEMA);
            }
            other => panic!("expected UnsupportedListingSchema, got {other:?}"),
        }
    }

    #[test]
    fn resolve_reports_parse_error_on_garbage() {
        match resolve_from_listing("not json", PhpVersion::new(8, 5), Os::Linux, Arch::X86_64) {
            Err(PhpError::ListingParse { .. }) => {}
            other => panic!("expected ListingParse, got {other:?}"),
        }
    }

    #[test]
    fn resolve_rejects_revision_zero() {
        let bad = r#"{ "schema": 1, "builds": [
            { "php": "8.5.7", "minor": "8.5", "os": "linux", "arch": "x86_64", "revision": 0,
              "cli": { "file": "c.tar.gz", "sha256": "aa", "size": 1 },
              "fpm": { "file": "f.tar.gz", "sha256": "bb", "size": 1 } }
        ] }"#;
        match resolve_from_listing(bad, PhpVersion::new(8, 5), Os::Linux, Arch::X86_64) {
            Err(PhpError::ListingParse { .. }) => {}
            other => panic!("expected ListingParse for revision 0, got {other:?}"),
        }
    }

    #[test]
    fn min_supported_floor_drops_below_8_2() {
        let got = available_minors(LISTING, Os::Linux, Arch::X86_64);
        assert_eq!(got, vec![PhpVersion::new(8, 4), PhpVersion::new(8, 5)]);
        match resolve_from_listing(LISTING, PhpVersion::new(8, 1), Os::Linux, Arch::X86_64) {
            Err(PhpError::VersionUnavailable { version }) => {
                assert_eq!(version, PhpVersion::new(8, 1));
            }
            other => panic!("expected VersionUnavailable for 8.1, got {other:?}"),
        }
    }

    #[test]
    fn available_minors_anchors_platform() {
        assert_eq!(
            available_minors(LISTING, Os::Macos, Arch::Aarch64),
            vec![PhpVersion::new(8, 5)]
        );
        assert_eq!(
            available_minors(LISTING, Os::Linux, Arch::Aarch64),
            vec![PhpVersion::new(8, 5)]
        );
    }

    #[test]
    fn available_minors_malformed_listing_is_empty() {
        assert!(available_minors("", Os::Linux, Arch::X86_64).is_empty());
        assert!(available_minors("not json", Os::Linux, Arch::X86_64).is_empty());
        let unknown_schema = r#"{ "schema": 2, "builds": [] }"#;
        assert!(available_minors(unknown_schema, Os::Linux, Arch::X86_64).is_empty());
    }

    #[test]
    fn is_newer_build_covers_patch_revision_and_autoheal() {
        assert!(is_newer_build("8.5.6", 1, "8.5.7", 1));
        assert!(is_newer_build("8.5.7", 1, "8.5.7", 2));
        assert!(is_newer_build("8.5.7", 0, "8.5.7", 1));
        assert!(!is_newer_build("8.5.7", 1, "8.5.7", 1));
        assert!(!is_newer_build("8.5.7", 2, "8.5.7", 1));
        assert!(!is_newer_build("8.5.9", 1, "8.5.7", 1));
        assert!(!is_newer_build("8.5", 0, "8.5.7", 1));
        assert_eq!(patch_of("8.5.7"), Some(7));
        assert_eq!(patch_of("8.5"), None);
    }

    #[test]
    fn display_build_omits_zero_revision() {
        assert_eq!(display_build("8.5.7", 1), "8.5.7-1");
        assert_eq!(display_build("8.5.7", 2), "8.5.7-2");
        assert_eq!(display_build("8.5.7", 0), "8.5.7");
    }

    #[test]
    fn install_segments_match_layout() {
        assert_eq!(BinaryKind::Cli.install_segments(), &["bin", "php"]);
        assert_eq!(BinaryKind::Fpm.install_segments(), &["sbin", "php-fpm"]);
        assert_eq!(BinaryKind::Cli.archive_member(), "php");
        assert_eq!(BinaryKind::Fpm.archive_member(), "php-fpm");
    }

    #[test]
    fn is_safe_member_rejects_traversal_and_absolute() {
        assert!(is_safe_member("php"));
        assert!(is_safe_member("./php"));
        assert!(!is_safe_member("../php"));
        assert!(!is_safe_member("/etc/php"));
        assert!(!is_safe_member("a/../../b"));
        assert!(!is_safe_member(""));
    }
}
