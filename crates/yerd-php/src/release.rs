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
//!
//! ## Windows lives in separate manifests
//!
//! There is no `php-fpm` SAPI on Windows, so the producer publishes Windows
//! builds in dedicated per-channel manifests `php-windows.json` /
//! `php-windows-legacy.json` (each with its own `.minisig`, same signing key).
//! Their build rows carry a single `bundle` object instead of `cli`/`fpm`:
//!
//! ```json
//! { "php": "8.5.9", "minor": "8.5", "os": "windows", "arch": "x86_64",
//!   "revision": 1,
//!   "bundle": { "file": "php-8.5.9-1-bundle-windows-x86_64.tar.gz", "sha256": "…", "size": 123 } }
//! ```
//!
//! The Unix `php.json` / `php-legacy.json` stay pure `cli`/`fpm`. [`Os`] selects
//! which manifest name is fetched ([`listing_url`]); the shapes never mix in one
//! file, so [`resolve_from_listing`] serves the Unix rows and
//! [`resolve_bundle_from_listing`] the Windows rows.

use serde::Deserialize;
use yerd_core::target::UnsupportedTarget;
use yerd_core::PhpVersion;

/// Host-target vocabulary, re-exported so `yerd_php::{Os, Arch}` keeps working
/// for the consumers that already import it (including the daemon's non-PHP
/// downloads). The definitions live in [`yerd_core::target`].
pub use yerd_core::target::{Arch, Os};

/// Zip-slip guard for archive member names, re-exported from
/// [`yerd_core::path_norm`] where it now lives.
pub use yerd_core::path_norm::is_safe_member;

use crate::error::PhpError;

/// Lowest PHP minor on the **stable** channel. The bundled `pcov` / `yerd-dump`
/// extensions are only built for 8.2+, so older minors are served from the
/// separate [`Channel::Legacy`] manifest and never resolve off the stable one.
/// Tied to the single core cutoff [`yerd_core::FIRST_SUPPORTED_MINOR`] so there
/// is exactly one boundary in the codebase.
pub const MIN_SUPPORTED: PhpVersion = yerd_core::FIRST_SUPPORTED_MINOR;

/// Which signed PHP distribution manifest a version is sourced from. Stable is
/// the supported channel (8.2+, `php.json`); Legacy carries out-of-support
/// minors (< 8.2) from a separately-signed `php-legacy.json` with the SAME
/// embedded minisign key and the SAME per-tarball SHA-256 verification.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Channel {
    /// Supported minors (>= [`MIN_SUPPORTED`]) from `php.json`.
    Stable,
    /// Out-of-support legacy minors (< [`MIN_SUPPORTED`]) from `php-legacy.json`.
    Legacy,
}

impl Channel {
    /// The channel a version is sourced from, via the pure core cutoff
    /// [`PhpVersion::is_legacy`].
    #[must_use]
    pub fn of(version: PhpVersion) -> Self {
        if version.is_legacy() {
            Channel::Legacy
        } else {
            Channel::Stable
        }
    }

    /// Manifest basename for this channel on `os`. Unix consumes the pure
    /// cli/fpm manifests `php` / `php-legacy`; Windows consumes the dedicated
    /// bundle-shaped manifests `php-windows` / `php-windows-legacy`. All four
    /// are published to the same release and signed with the same key.
    const fn manifest_stem(self, os: Os) -> &'static str {
        match (self, os) {
            (Channel::Stable, Os::Windows) => "php-windows",
            (Channel::Legacy, Os::Windows) => "php-windows-legacy",
            (Channel::Stable, _) => "php",
            (Channel::Legacy, _) => "php-legacy",
        }
    }
}

/// The `php.json` schema version this build understands. A producer-side bump
/// signals an incompatible format change (additive changes do not bump it).
pub const PHP_LISTING_SCHEMA: u32 = 1;

/// Base URL of yerd's hosted, signed PHP distribution.
///
/// A single rolling `php` release of the **separate** `forjedio/yerd-php` build
/// repo holds every `php-<full>-<revision>-<cli|fpm>-<os>-<arch>.tar.gz` (Unix)
/// and `php-<full>-<revision>-bundle-windows-<arch>.tar.gz` (Windows) asset plus
/// the four generated manifests (`php.json`, `php-legacy.json`,
/// `php-windows.json`, `php-windows-legacy.json`) and their detached
/// `.minisig` signatures. Asset URLs 302-redirect to the blob; the daemon's
/// downloader follows redirects. This crate is a pure *consumer* - the producer
/// lives entirely in `forjedio/yerd-php`.
pub const PHP_LISTING_BASE_URL: &str = "https://github.com/forjedio/yerd-php/releases/download/php";

// ── manifest wire shape (private; deserialised from `php.json`) ──────────────

#[derive(Debug, Deserialize)]
struct Listing {
    schema: u32,
    #[serde(default)]
    builds: Vec<BuildEntry>,
}

/// One manifest build row. `cli`/`fpm` (Unix cli/fpm manifests) and `bundle`
/// (the Windows bundle manifests) are all optional on the wire so a single
/// parser serves both manifest families; the resolvers enforce that the
/// os-appropriate payload is present (see [`resolve_from_listing`] /
/// [`resolve_bundle_from_listing`]). The shapes never mix within one manifest.
#[derive(Debug, Deserialize)]
struct BuildEntry {
    php: String,
    minor: String,
    os: String,
    arch: String,
    revision: u32,
    #[serde(default)]
    cli: Option<FileEntry>,
    #[serde(default)]
    fpm: Option<FileEntry>,
    #[serde(default)]
    bundle: Option<FileEntry>,
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

/// Which binary within a PHP build. Models the Unix cli/fpm tarball layout;
/// the Windows install flow ships a single flat bundle and never uses
/// [`BinaryKind::archive_member`] / [`BinaryKind::install_segments`].
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

/// URL of the signed manifest for `channel` on `os` (the daemon fetches this,
/// verifies its signature, then hands the body to [`resolve_from_listing`] or
/// [`resolve_bundle_from_listing`]). Unix: `php.json` / `php-legacy.json`;
/// Windows: `php-windows.json` / `php-windows-legacy.json`.
#[must_use]
pub fn listing_url(channel: Channel, os: Os) -> String {
    format!("{PHP_LISTING_BASE_URL}/{}.json", channel.manifest_stem(os))
}

/// URL of the detached minisign signature over [`listing_url`]'s manifest for
/// `channel` on `os`.
#[must_use]
pub fn listing_sig_url(channel: Channel, os: Os) -> String {
    format!(
        "{PHP_LISTING_BASE_URL}/{}.json.minisig",
        channel.manifest_stem(os)
    )
}

/// A resolved download plan for one Windows PHP bundle: a single `.tar.gz` that
/// unpacks the whole runtime tree (`php.exe`, `php-cgi.exe`, `php.ini`,
/// `cacert.pem`, `ext/*.dll`, support DLLs). The Windows analogue of
/// [`Artifact`]; produced only by [`resolve_bundle_from_listing`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BundleArtifact {
    /// The requested major.minor version.
    pub version: PhpVersion,
    /// The resolved full patch version (e.g. `"8.5.9"`).
    pub full_version: String,
    /// Rebuild counter of the resolved build (the `-N` suffix; `>= 1`).
    pub revision: u32,
    /// Per-version install directory name (e.g. `"php-8.5"`).
    pub install_dir_name: String,
    /// URL of the bundle tarball.
    pub bundle_url: String,
    /// Expected SHA-256 (lowercase hex) of the bundle tarball bytes.
    pub bundle_sha256: String,
}

/// Select the single build for `(version, os, arch)` on `channel` from a
/// manifest body, rejecting a cross-channel version, an unknown schema, and a
/// published revision of 0. Shared by every resolver; the payload-shape check
/// (cli+fpm vs bundle) is the caller's, so one parse+select path serves both
/// manifest families. Preserves the original order (channel gate, then parse,
/// then find) so a cross-channel request on a garbage body still reports
/// [`PhpError::VersionUnavailable`].
fn select_entry(
    listing: &str,
    version: PhpVersion,
    os: Os,
    arch: Arch,
    channel: Channel,
) -> Result<BuildEntry, PhpError> {
    if Channel::of(version) != channel {
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
    Ok(entry)
}

/// A required manifest payload field (`cli`/`fpm`/`bundle`) that must be present
/// for the target's manifest family. A missing field is a producer/consumer
/// desync, surfaced as [`PhpError::ListingParse`] naming the field, i.e. the
/// same error variant a serde failure produced before the fields were optional.
fn require_field(
    field: Option<FileEntry>,
    name: &str,
    php: &str,
    os: Os,
    arch: Arch,
) -> Result<FileEntry, PhpError> {
    field.ok_or_else(|| PhpError::ListingParse {
        detail: format!(
            "build {php} ({}-{}) is missing required field {name:?}",
            os.as_str(),
            arch.as_str()
        ),
    })
}

/// Resolve a requested major.minor version + platform to a cli/fpm [`Artifact`]
/// from a Unix manifest body (`php.json` / `php-legacy.json`).
///
/// Retention guarantees at most one build per `(minor, os, arch)`, so this
/// selects the single matching entry (no patch scanning) and builds both URLs
/// from the manifest's `file` fields **verbatim**. Errors with
/// [`PhpError::VersionUnavailable`] when no matching build is published, and
/// with [`PhpError::ListingParse`] / [`PhpError::UnsupportedListingSchema`] when
/// the manifest is malformed, a newer schema, or a selected row is missing its
/// `cli`/`fpm` payload (e.g. a Windows bundle manifest fed here by mistake).
pub fn resolve_from_listing(
    listing: &str,
    version: PhpVersion,
    os: Os,
    arch: Arch,
    channel: Channel,
) -> Result<Artifact, PhpError> {
    let entry = select_entry(listing, version, os, arch, channel)?;
    let cli = require_field(entry.cli, "cli", &entry.php, os, arch)?;
    let fpm = require_field(entry.fpm, "fpm", &entry.php, os, arch)?;

    Ok(Artifact {
        install_dir_name: format!("php-{}.{}", version.major, version.minor),
        revision: entry.revision,
        cli_url: format!("{PHP_LISTING_BASE_URL}/{}", cli.file),
        cli_sha256: cli.sha256,
        fpm_url: format!("{PHP_LISTING_BASE_URL}/{}", fpm.file),
        fpm_sha256: fpm.sha256,
        full_version: entry.php,
        version,
    })
}

/// Resolve a requested major.minor version + platform to a [`BundleArtifact`]
/// from a Windows bundle manifest body (`php-windows.json` /
/// `php-windows-legacy.json`).
///
/// Same selection rules as [`resolve_from_listing`], but requires the row's
/// `bundle` payload and builds a single download URL from its `file` field
/// **verbatim**. Called only by the Windows install path.
pub fn resolve_bundle_from_listing(
    listing: &str,
    version: PhpVersion,
    os: Os,
    arch: Arch,
    channel: Channel,
) -> Result<BundleArtifact, PhpError> {
    let entry = select_entry(listing, version, os, arch, channel)?;
    let bundle = require_field(entry.bundle, "bundle", &entry.php, os, arch)?;

    Ok(BundleArtifact {
        install_dir_name: format!("php-{}.{}", version.major, version.minor),
        revision: entry.revision,
        bundle_url: format!("{PHP_LISTING_BASE_URL}/{}", bundle.file),
        bundle_sha256: bundle.sha256,
        full_version: entry.php,
        version,
    })
}

/// Resolve just the build identity `(full_version, revision)` for
/// `(version, os, arch)`, checking the os-appropriate payload is present
/// (Windows → `bundle`; otherwise `cli` + `fpm`) but building no URLs. The
/// update-poll and update-apply paths use this so they stay OS-agnostic - the
/// per-OS manifest shape never leaks into their logic.
pub fn resolve_build(
    listing: &str,
    version: PhpVersion,
    os: Os,
    arch: Arch,
    channel: Channel,
) -> Result<(String, u32), PhpError> {
    let entry = select_entry(listing, version, os, arch, channel)?;
    match os {
        Os::Windows => {
            require_field(entry.bundle, "bundle", &entry.php, os, arch)?;
        }
        Os::Linux | Os::Macos => {
            require_field(entry.cli, "cli", &entry.php, os, arch)?;
            require_field(entry.fpm, "fpm", &entry.php, os, arch)?;
        }
    }
    Ok((entry.php, entry.revision))
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
pub fn available_minors(listing: &str, os: Os, arch: Arch, channel: Channel) -> Vec<PhpVersion> {
    let Ok(parsed) = parse_listing(listing) else {
        return Vec::new();
    };
    let mut out: Vec<PhpVersion> = parsed
        .builds
        .iter()
        .filter(|b| b.os == os.as_str() && b.arch == arch.as_str())
        .filter_map(|b| parse_minor(&b.minor))
        .filter(|v| Channel::of(*v) == channel)
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

/// Detect the running platform, erroring on anything yerd has no prebuilt PHP
/// for (e.g. a 32-bit or unknown OS). Thin wrapper over
/// [`yerd_core::target::current_os_arch`] that renders the failure in this
/// crate's error vocabulary. Call this **before** any download.
pub fn current_os_arch() -> Result<(Os, Arch), PhpError> {
    yerd_core::target::current_os_arch().map_err(|e| PhpError::UnsupportedPlatform {
        detail: match e {
            UnsupportedTarget::Os(os) => format!("no prebuilt PHP for OS {os:?}"),
            UnsupportedTarget::Arch(arch) => format!("no prebuilt PHP for architecture {arch:?}"),
        },
    })
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
    /// like the real manifest. 8.1 is below the floor; 8.5 has a rebuild.
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

    /// A `php-windows.json` body, bundle-shaped, copied from the live manifest
    /// (`generated_at 2026-08-03T00:57:24Z`). No `cli`/`fpm` keys.
    const WINDOWS_LISTING: &str = r#"{
        "schema": 1,
        "generated_at": "2026-08-03T00:57:24Z",
        "builds": [
            { "php": "8.2.33", "minor": "8.2", "os": "windows", "arch": "x86_64", "revision": 1,
              "bundle": { "file": "php-8.2.33-1-bundle-windows-x86_64.tar.gz", "sha256": "6a83aa00a260c26be0a2f3edd6b5e64df05e4f79c63cac811a624c7132b12b74", "size": 47207827 } },
            { "php": "8.5.9", "minor": "8.5", "os": "windows", "arch": "x86_64", "revision": 1,
              "bundle": { "file": "php-8.5.9-1-bundle-windows-x86_64.tar.gz", "sha256": "01e220add4a6f856b5e4846859d7ee675b31bbe6bd26034ca189fa675f652474", "size": 49997357 } }
        ]
    }"#;

    /// A one-row `php-windows-legacy.json` body, bundle-shaped, copied from the
    /// live legacy manifest.
    const WINDOWS_LEGACY_LISTING: &str = r#"{
        "schema": 1,
        "generated_at": "2026-08-03T00:57:56Z",
        "builds": [
            { "php": "8.1.34", "minor": "8.1", "os": "windows", "arch": "x86_64", "revision": 2,
              "bundle": { "file": "php-8.1.34-2-bundle-windows-x86_64.tar.gz", "sha256": "7144602e4dfe720b1b82b49887d1e60e51fa0e412f63f663558cd99fad977cda", "size": 44492125 } }
        ]
    }"#;

    /// A `php-legacy.json` body spanning the three legacy minors across the four
    /// targets, shaped like the real manifest.
    const LEGACY_LISTING: &str = r#"{
        "schema": 1,
        "generated_at": "2026-07-01T00:00:00Z",
        "builds": [
            { "php": "7.4.33", "minor": "7.4", "os": "linux", "arch": "x86_64", "revision": 1,
              "cli": { "file": "php-7.4.33-1-cli-linux-x86_64.tar.gz", "sha256": "aa", "size": 1 },
              "fpm": { "file": "php-7.4.33-1-fpm-linux-x86_64.tar.gz", "sha256": "bb", "size": 1 } },
            { "php": "8.0.30", "minor": "8.0", "os": "linux", "arch": "x86_64", "revision": 1,
              "cli": { "file": "php-8.0.30-1-cli-linux-x86_64.tar.gz", "sha256": "cc", "size": 1 },
              "fpm": { "file": "php-8.0.30-1-fpm-linux-x86_64.tar.gz", "sha256": "dd", "size": 1 } },
            { "php": "8.1.33", "minor": "8.1", "os": "linux", "arch": "x86_64", "revision": 1,
              "cli": { "file": "php-8.1.33-1-cli-linux-x86_64.tar.gz", "sha256": "ee", "size": 1 },
              "fpm": { "file": "php-8.1.33-1-fpm-linux-x86_64.tar.gz", "sha256": "ff", "size": 1 } },
            { "php": "8.1.33", "minor": "8.1", "os": "macos", "arch": "aarch64", "revision": 1,
              "cli": { "file": "php-8.1.33-1-cli-macos-aarch64.tar.gz", "sha256": "11", "size": 1 },
              "fpm": { "file": "php-8.1.33-1-fpm-macos-aarch64.tar.gz", "sha256": "22", "size": 1 } }
        ]
    }"#;

    #[test]
    fn resolve_from_listing_selects_entry_and_builds_urls() {
        let a = resolve_from_listing(
            LISTING,
            PhpVersion::new(8, 5),
            Os::Linux,
            Arch::X86_64,
            Channel::Stable,
        )
        .unwrap();
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
    fn resolve_bundle_from_listing_selects_windows_entry_and_builds_url() {
        let a = resolve_bundle_from_listing(
            WINDOWS_LISTING,
            PhpVersion::new(8, 5),
            Os::Windows,
            Arch::X86_64,
            Channel::Stable,
        )
        .unwrap();
        assert_eq!(a.full_version, "8.5.9");
        assert_eq!(a.revision, 1);
        assert_eq!(a.install_dir_name, "php-8.5");
        assert_eq!(
            a.bundle_url,
            "https://github.com/forjedio/yerd-php/releases/download/php/php-8.5.9-1-bundle-windows-x86_64.tar.gz"
        );
        assert_eq!(
            a.bundle_sha256,
            "01e220add4a6f856b5e4846859d7ee675b31bbe6bd26034ca189fa675f652474"
        );
    }

    /// A Windows bundle manifest fed to the cli/fpm resolver must fail loudly
    /// rather than silently, guarding against ever cross-wiring the Unix resolve
    /// to a Windows manifest.
    #[test]
    fn resolve_from_listing_rejects_bundle_manifest() {
        match resolve_from_listing(
            WINDOWS_LISTING,
            PhpVersion::new(8, 5),
            Os::Windows,
            Arch::X86_64,
            Channel::Stable,
        ) {
            Err(PhpError::ListingParse { detail }) => assert!(detail.contains("cli"), "{detail}"),
            other => panic!("expected ListingParse naming cli, got {other:?}"),
        }
    }

    /// A Unix row missing `fpm` is a producer/consumer desync, surfaced as
    /// `ListingParse` naming the field.
    #[test]
    fn resolve_from_listing_rejects_unix_row_missing_fpm() {
        let bad = r#"{ "schema": 1, "builds": [
            { "php": "8.5.7", "minor": "8.5", "os": "linux", "arch": "x86_64", "revision": 1,
              "cli": { "file": "c.tar.gz", "sha256": "aa", "size": 1 } }
        ] }"#;
        match resolve_from_listing(
            bad,
            PhpVersion::new(8, 5),
            Os::Linux,
            Arch::X86_64,
            Channel::Stable,
        ) {
            Err(PhpError::ListingParse { detail }) => assert!(detail.contains("fpm"), "{detail}"),
            other => panic!("expected ListingParse naming fpm, got {other:?}"),
        }
    }

    /// A Windows row without a `bundle` payload → `ListingParse` naming the field.
    #[test]
    fn resolve_bundle_from_listing_rejects_row_missing_bundle() {
        let bad = r#"{ "schema": 1, "builds": [
            { "php": "8.5.9", "minor": "8.5", "os": "windows", "arch": "x86_64", "revision": 1 }
        ] }"#;
        match resolve_bundle_from_listing(
            bad,
            PhpVersion::new(8, 5),
            Os::Windows,
            Arch::X86_64,
            Channel::Stable,
        ) {
            Err(PhpError::ListingParse { detail }) => {
                assert!(detail.contains("bundle"), "{detail}");
            }
            other => panic!("expected ListingParse naming bundle, got {other:?}"),
        }
    }

    #[test]
    fn resolve_build_checks_os_appropriate_payload() {
        assert_eq!(
            resolve_build(
                WINDOWS_LISTING,
                PhpVersion::new(8, 5),
                Os::Windows,
                Arch::X86_64,
                Channel::Stable,
            )
            .unwrap(),
            ("8.5.9".to_owned(), 1)
        );
        assert_eq!(
            resolve_build(
                LISTING,
                PhpVersion::new(8, 5),
                Os::Linux,
                Arch::X86_64,
                Channel::Stable,
            )
            .unwrap(),
            ("8.5.7".to_owned(), 2)
        );
        assert!(matches!(
            resolve_build(
                WINDOWS_LISTING,
                PhpVersion::new(8, 5),
                Os::Linux,
                Arch::X86_64,
                Channel::Stable,
            ),
            Err(PhpError::VersionUnavailable { .. })
        ));
    }

    #[test]
    fn resolve_bundle_legacy_channel() {
        let a = resolve_bundle_from_listing(
            WINDOWS_LEGACY_LISTING,
            PhpVersion::new(8, 1),
            Os::Windows,
            Arch::X86_64,
            Channel::Legacy,
        )
        .unwrap();
        assert_eq!(a.full_version, "8.1.34");
        assert_eq!(a.revision, 2);
        assert_eq!(a.install_dir_name, "php-8.1");
    }

    #[test]
    fn available_minors_anchors_windows() {
        assert_eq!(
            available_minors(WINDOWS_LISTING, Os::Windows, Arch::X86_64, Channel::Stable),
            vec![PhpVersion::new(8, 2), PhpVersion::new(8, 5)]
        );
        assert!(
            available_minors(WINDOWS_LISTING, Os::Windows, Arch::Aarch64, Channel::Stable)
                .is_empty()
        );
    }

    /// Only meaningful on the Windows CI leg: asserts the host resolves to the
    /// `Windows`/`x86_64` token pair the manifest keys off.
    #[cfg(all(target_os = "windows", target_arch = "x86_64"))]
    #[test]
    fn current_os_arch_is_windows_x86_64() {
        assert_eq!(current_os_arch().unwrap(), (Os::Windows, Arch::X86_64));
    }

    #[test]
    fn listing_urls_point_at_the_signed_manifest() {
        let base = "https://github.com/forjedio/yerd-php/releases/download/php";
        assert_eq!(
            listing_url(Channel::Stable, Os::Linux),
            format!("{base}/php.json")
        );
        assert_eq!(
            listing_sig_url(Channel::Stable, Os::Macos),
            format!("{base}/php.json.minisig")
        );
        assert_eq!(
            listing_url(Channel::Legacy, Os::Linux),
            format!("{base}/php-legacy.json")
        );
        assert_eq!(
            listing_sig_url(Channel::Legacy, Os::Linux),
            format!("{base}/php-legacy.json.minisig")
        );
        assert_eq!(
            listing_url(Channel::Stable, Os::Windows),
            format!("{base}/php-windows.json")
        );
        assert_eq!(
            listing_sig_url(Channel::Stable, Os::Windows),
            format!("{base}/php-windows.json.minisig")
        );
        assert_eq!(
            listing_url(Channel::Legacy, Os::Windows),
            format!("{base}/php-windows-legacy.json")
        );
        assert_eq!(
            listing_sig_url(Channel::Legacy, Os::Windows),
            format!("{base}/php-windows-legacy.json.minisig")
        );
    }

    #[test]
    fn channel_of_splits_at_the_floor() {
        for (m, n) in [(7, 4), (8, 0), (8, 1)] {
            assert_eq!(Channel::of(PhpVersion::new(m, n)), Channel::Legacy);
        }
        for (m, n) in [(8, 2), (8, 3), (8, 4), (8, 5)] {
            assert_eq!(Channel::of(PhpVersion::new(m, n)), Channel::Stable);
        }
    }

    #[test]
    fn resolve_from_listing_anchors_arch() {
        let a = resolve_from_listing(
            LISTING,
            PhpVersion::new(8, 5),
            Os::Linux,
            Arch::Aarch64,
            Channel::Stable,
        )
        .unwrap();
        assert!(a.cli_url.contains("linux-aarch64"));
        assert_eq!(a.cli_sha256, "11");
    }

    #[test]
    fn resolve_from_listing_unknown_minor_errors() {
        match resolve_from_listing(
            LISTING,
            PhpVersion::new(8, 3),
            Os::Linux,
            Arch::X86_64,
            Channel::Stable,
        ) {
            Err(PhpError::VersionUnavailable { version }) => {
                assert_eq!(version, PhpVersion::new(8, 3));
            }
            other => panic!("expected VersionUnavailable, got {other:?}"),
        }
    }

    #[test]
    fn resolve_rejects_unknown_schema() {
        let bad = r#"{ "schema": 99, "builds": [] }"#;
        match resolve_from_listing(
            bad,
            PhpVersion::new(8, 5),
            Os::Linux,
            Arch::X86_64,
            Channel::Stable,
        ) {
            Err(PhpError::UnsupportedListingSchema { found, supported }) => {
                assert_eq!(found, 99);
                assert_eq!(supported, PHP_LISTING_SCHEMA);
            }
            other => panic!("expected UnsupportedListingSchema, got {other:?}"),
        }
    }

    #[test]
    fn resolve_reports_parse_error_on_garbage() {
        match resolve_from_listing(
            "not json",
            PhpVersion::new(8, 5),
            Os::Linux,
            Arch::X86_64,
            Channel::Stable,
        ) {
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
        match resolve_from_listing(
            bad,
            PhpVersion::new(8, 5),
            Os::Linux,
            Arch::X86_64,
            Channel::Stable,
        ) {
            Err(PhpError::ListingParse { .. }) => {}
            other => panic!("expected ListingParse for revision 0, got {other:?}"),
        }
    }

    #[test]
    fn min_supported_floor_drops_below_8_2() {
        let got = available_minors(LISTING, Os::Linux, Arch::X86_64, Channel::Stable);
        assert_eq!(got, vec![PhpVersion::new(8, 4), PhpVersion::new(8, 5)]);
        match resolve_from_listing(
            LISTING,
            PhpVersion::new(8, 1),
            Os::Linux,
            Arch::X86_64,
            Channel::Stable,
        ) {
            Err(PhpError::VersionUnavailable { version }) => {
                assert_eq!(version, PhpVersion::new(8, 1));
            }
            other => panic!("expected VersionUnavailable for 8.1, got {other:?}"),
        }
    }

    #[test]
    fn legacy_channel_resolves_legacy_minors_and_rejects_cross_channel() {
        let a = resolve_from_listing(
            LEGACY_LISTING,
            PhpVersion::new(7, 4),
            Os::Linux,
            Arch::X86_64,
            Channel::Legacy,
        )
        .unwrap();
        assert_eq!(a.full_version, "7.4.33");
        assert_eq!(a.install_dir_name, "php-7.4");
        assert_eq!(
            a.cli_url,
            "https://github.com/forjedio/yerd-php/releases/download/php/php-7.4.33-1-cli-linux-x86_64.tar.gz"
        );

        assert!(matches!(
            resolve_from_listing(
                LEGACY_LISTING,
                PhpVersion::new(8, 5),
                Os::Linux,
                Arch::X86_64,
                Channel::Legacy,
            ),
            Err(PhpError::VersionUnavailable { .. })
        ));
        assert!(matches!(
            resolve_from_listing(
                LISTING,
                PhpVersion::new(8, 5),
                Os::Linux,
                Arch::X86_64,
                Channel::Legacy,
            ),
            Err(PhpError::VersionUnavailable { .. })
        ));
    }

    #[test]
    fn available_minors_partitions_by_channel() {
        assert_eq!(
            available_minors(LEGACY_LISTING, Os::Linux, Arch::X86_64, Channel::Legacy),
            vec![
                PhpVersion::new(7, 4),
                PhpVersion::new(8, 0),
                PhpVersion::new(8, 1)
            ]
        );
        assert!(
            available_minors(LEGACY_LISTING, Os::Linux, Arch::X86_64, Channel::Stable).is_empty()
        );
    }

    #[test]
    fn available_minors_anchors_platform() {
        assert_eq!(
            available_minors(LISTING, Os::Macos, Arch::Aarch64, Channel::Stable),
            vec![PhpVersion::new(8, 5)]
        );
        assert_eq!(
            available_minors(LISTING, Os::Linux, Arch::Aarch64, Channel::Stable),
            vec![PhpVersion::new(8, 5)]
        );
    }

    #[test]
    fn available_minors_malformed_listing_is_empty() {
        assert!(available_minors("", Os::Linux, Arch::X86_64, Channel::Stable).is_empty());
        assert!(available_minors("not json", Os::Linux, Arch::X86_64, Channel::Stable).is_empty());
        let unknown_schema = r#"{ "schema": 2, "builds": [] }"#;
        assert!(
            available_minors(unknown_schema, Os::Linux, Arch::X86_64, Channel::Stable).is_empty()
        );
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
}
