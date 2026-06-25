//! Pure resolution of prebuilt static-PHP download artifacts.
//!
//! Versions are discovered **dynamically** from the static-php-cli distribution
//! (`dl.static-php.dev`) so yerd needs no release to support a new PHP patch:
//! the daemon fetches the directory listing and `resolve_from_listing` (pure)
//! picks the latest patch of the requested minor and builds the download URLs.
//! Integrity rests on HTTPS to the distribution host (it publishes no checksum
//! sidecars); there is no sha256 pinning — a deliberate trade-off so the
//! supported set isn't frozen into the binary.

use yerd_core::PhpVersion;

use crate::error::PhpError;

/// Lowest PHP minor Yerd supports installing. The bundled `pcov` / `yerd-dump`
/// extensions are only built for 8.2+, so older minors are filtered out of the
/// installable list and rejected at resolve time even when the distribution
/// still publishes them.
pub const MIN_SUPPORTED: PhpVersion = PhpVersion::new(8, 2);

/// Base URL of the static-php-cli prebuilt distribution for `os`.
///
/// Both platforms use the **bulk** extension set so a real-world Laravel app
/// has what it needs out of the box — notably `intl` (ICU), plus `sodium`,
/// `mysqli`, `xsl`, `readline`, `apcu`, … which the leaner `common` channel
/// omits. The cost is a larger binary (~38 MB compressed); acceptable for a dev
/// tool, and it keeps macOS and Linux on the *same* extension set.
///
/// Linux specifically needs the **`gnu-bulk`** (glibc) variant rather than the
/// musl `bulk`: a fully-static musl PHP **cannot `dlopen` a shared extension**,
/// which yerd needs for the `yerd-dump` / `pcov` extensions. The trade-off is a
/// glibc-linked binary that no longer "runs on any libc" (Alpine/musl-only hosts
/// are unsupported). macOS uses plain **`bulk`** (macOS permits `dlopen`
/// regardless, and there is no separate glibc channel for it).
const fn channel_base(os: Os) -> &'static str {
    match os {
        Os::Linux => "https://dl.static-php.dev/static-php-cli/gnu-bulk",
        Os::Macos => "https://dl.static-php.dev/static-php-cli/bulk",
    }
}

/// Target operating system for a prebuilt artifact.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Os {
    /// Linux (glibc `gnu-bulk` build — can load shared extensions; **not** the
    /// musl `bulk` build, which can't `dlopen`).
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
    /// The resolved full patch version (e.g. `"8.5.6"`).
    pub full_version: String,
    /// Per-version install directory name (e.g. `"php-8.5"`).
    pub install_dir_name: String,
    /// URL of the CLI tarball.
    pub cli_url: String,
    /// URL of the FPM tarball.
    pub fpm_url: String,
}

/// Build the canonical artifact URL for a `(full_version, kind, os, arch)`.
#[must_use]
pub fn artifact_url(full_version: &str, kind: BinaryKind, os: Os, arch: Arch) -> String {
    format!(
        "{}/php-{full_version}-{}-{}-{}.tar.gz",
        channel_base(os),
        kind.as_str(),
        os.as_str(),
        arch.as_str()
    )
}

/// URL of the distribution's directory listing for `os` (the daemon fetches
/// this, then hands the body to [`resolve_from_listing`]). The channel differs
/// per OS (see [`channel_base`]), so the listing URL does too.
#[must_use]
pub fn listing_url(os: Os) -> String {
    format!("{}/", channel_base(os))
}

/// Resolve a requested major.minor version + platform to an [`Artifact`] by
/// scanning the distribution's directory `listing` for the **latest patch**.
///
/// Looks for filenames `php-<maj>.<min>.<patch>-cli-<os>-<arch>.tar.gz` (the
/// `php-<maj>.<min>.` prefix carries a trailing dot, so `8.5` never matches
/// `8.50`; the `-cli-<os>-<arch>` suffix anchors the arch), takes the highest
/// patch, and builds both the CLI and FPM URLs. Errors with
/// [`PhpError::VersionUnavailable`] when no matching build is published.
pub fn resolve_from_listing(
    listing: &str,
    version: PhpVersion,
    os: Os,
    arch: Arch,
) -> Result<Artifact, PhpError> {
    // Reject unsupported minors (8.0/8.1) up front, reusing `VersionUnavailable`
    // so callers get one consistent "can't install that" error regardless of
    // whether the distribution still lists it.
    if version < MIN_SUPPORTED {
        return Err(PhpError::VersionUnavailable { version });
    }
    let prefix = format!("php-{}.{}.", version.major, version.minor);
    let suffix = format!(
        "-{}-{}-{}.tar.gz",
        BinaryKind::Cli.as_str(),
        os.as_str(),
        arch.as_str()
    );

    // Split on the prefix; each chunk after the first begins right after
    // `php-<maj>.<min>.`. Take its leading digits as the patch, then require the
    // exact CLI suffix. Uses `split`/`strip_prefix`/`starts_with` only — no
    // indexing (the prefix's trailing dot already rules out `8.5` vs `8.50`).
    let mut best: Option<u32> = None;
    let mut chunks = listing.split(prefix.as_str());
    let _ = chunks.next(); // text before the first occurrence
    for chunk in chunks {
        let digits: String = chunk.chars().take_while(char::is_ascii_digit).collect();
        if digits.is_empty() {
            continue;
        }
        let Some(remainder) = chunk.strip_prefix(&digits) else {
            continue;
        };
        if remainder.starts_with(&suffix) {
            if let Ok(patch) = digits.parse::<u32>() {
                best = Some(best.map_or(patch, |b| b.max(patch)));
            }
        }
    }

    let patch = best.ok_or(PhpError::VersionUnavailable { version })?;
    let full_version = format!("{}.{}.{}", version.major, version.minor, patch);
    Ok(Artifact {
        install_dir_name: format!("php-{}.{}", version.major, version.minor),
        cli_url: artifact_url(&full_version, BinaryKind::Cli, os, arch),
        fpm_url: artifact_url(&full_version, BinaryKind::Fpm, os, arch),
        full_version,
        version,
    })
}

/// Every distinct major.minor in `listing` that has a CLI build for
/// `(os, arch)`, ascending. Pure; the daemon fetches the listing and hands the
/// body here to populate the "installable versions" list (the GUI dropdown /
/// `yerd list php --available`).
///
/// Scans for filenames `php-<maj>.<min>.<patch>-cli-<os>-<arch>.tar.gz`,
/// parsing `<maj>` and `<min>` as **integers** (not substrings) so `8.5` and
/// `8.50` stay distinct. Entries whose major/minor overflow `u8`, or that lack
/// the exact CLI/arch suffix, are skipped. The result is sorted and deduped.
#[must_use]
pub fn available_minors(listing: &str, os: Os, arch: Arch) -> Vec<PhpVersion> {
    let suffix = format!(
        "-{}-{}-{}.tar.gz",
        BinaryKind::Cli.as_str(),
        os.as_str(),
        arch.as_str()
    );

    // Each chunk after the first begins right after a literal `php-`. Parse
    // `<digits>.<digits>.<digits>` then require the exact CLI suffix; only the
    // major.minor is kept. Uses `split`/`strip_prefix`/`split_once` — no
    // indexing.
    let mut out: Vec<PhpVersion> = Vec::new();
    let mut chunks = listing.split("php-");
    let _ = chunks.next(); // text before the first occurrence
    for chunk in chunks {
        let Some((major, rest)) = take_u8(chunk) else {
            continue;
        };
        let Some(rest) = rest.strip_prefix('.') else {
            continue;
        };
        let Some((minor, rest)) = take_u8(rest) else {
            continue;
        };
        let Some(rest) = rest.strip_prefix('.') else {
            continue;
        };
        // Patch digits then the exact `-cli-<os>-<arch>.tar.gz` suffix.
        let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
        if digits.is_empty() {
            continue;
        }
        let Some(after_patch) = rest.strip_prefix(&digits) else {
            continue;
        };
        if after_patch.starts_with(&suffix) {
            let v = PhpVersion::new(major, minor);
            // Hide unsupported minors (8.0/8.1) from the installable list.
            if v >= MIN_SUPPORTED {
                out.push(v);
            }
        }
    }

    out.sort_unstable();
    out.dedup();
    out
}

/// Take a leading run of ASCII digits as a `u8`, returning it with the rest of
/// the string. `None` if there are no leading digits or they overflow `u8`.
fn take_u8(s: &str) -> Option<(u8, &str)> {
    let digits: String = s.chars().take_while(char::is_ascii_digit).collect();
    let value: u8 = digits.parse().ok()?;
    let rest = s.strip_prefix(&digits)?;
    Some((value, rest))
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

/// Whether `candidate` is a newer patch than `installed` (same major.minor
/// assumed; malformed inputs → `false`).
#[must_use]
pub fn is_newer(installed_full: &str, candidate_full: &str) -> bool {
    match (patch_of(installed_full), patch_of(candidate_full)) {
        (Some(installed), Some(candidate)) => candidate > installed,
        _ => false,
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::panic, clippy::indexing_slicing)]
mod tests {
    use super::*;

    /// A listing snippet shaped like the real autoindex (href + duplicate text
    /// node), spanning several patches, minors, kinds, and arches.
    const LISTING: &str = r#"
        <a href="/static-php-cli/gnu-bulk/php-8.5.2-cli-linux-x86_64.tar.gz">php-8.5.2-cli-linux-x86_64.tar.gz</a>
        <a href="/static-php-cli/gnu-bulk/php-8.5.6-cli-linux-x86_64.tar.gz">php-8.5.6-cli-linux-x86_64.tar.gz</a>
        <a href="/static-php-cli/gnu-bulk/php-8.5.6-fpm-linux-x86_64.tar.gz">php-8.5.6-fpm-linux-x86_64.tar.gz</a>
        <a href="/static-php-cli/gnu-bulk/php-8.5.4-cli-linux-aarch64.tar.gz">php-8.5.4-cli-linux-aarch64.tar.gz</a>
        <a href="/static-php-cli/gnu-bulk/php-8.50.1-cli-linux-x86_64.tar.gz">php-8.50.1-cli-linux-x86_64.tar.gz</a>
        <a href="/static-php-cli/gnu-bulk/php-8.4.21-cli-linux-x86_64.tar.gz">php-8.4.21-cli-linux-x86_64.tar.gz</a>
    "#;

    #[test]
    fn resolve_from_listing_picks_max_patch_and_builds_urls() {
        let a =
            resolve_from_listing(LISTING, PhpVersion::new(8, 5), Os::Linux, Arch::X86_64).unwrap();
        assert_eq!(a.full_version, "8.5.6"); // 8.5.6 > 8.5.2, ignores 8.50.1
        assert_eq!(a.install_dir_name, "php-8.5");
        assert_eq!(
            a.cli_url,
            "https://dl.static-php.dev/static-php-cli/gnu-bulk/php-8.5.6-cli-linux-x86_64.tar.gz"
        );
        assert_eq!(
            a.fpm_url,
            "https://dl.static-php.dev/static-php-cli/gnu-bulk/php-8.5.6-fpm-linux-x86_64.tar.gz"
        );
    }

    #[test]
    fn channel_differs_by_os() {
        // Linux → glibc `gnu-bulk` (its PHP can dlopen the dump extension).
        assert_eq!(
            artifact_url("8.5.6", BinaryKind::Fpm, Os::Linux, Arch::X86_64),
            "https://dl.static-php.dev/static-php-cli/gnu-bulk/php-8.5.6-fpm-linux-x86_64.tar.gz"
        );
        assert_eq!(
            listing_url(Os::Linux),
            "https://dl.static-php.dev/static-php-cli/gnu-bulk/"
        );
        // macOS → `bulk` channel (same extension set as Linux's gnu-bulk).
        assert_eq!(
            artifact_url("8.5.6", BinaryKind::Cli, Os::Macos, Arch::Aarch64),
            "https://dl.static-php.dev/static-php-cli/bulk/php-8.5.6-cli-macos-aarch64.tar.gz"
        );
        assert_eq!(
            listing_url(Os::Macos),
            "https://dl.static-php.dev/static-php-cli/bulk/"
        );
    }

    #[test]
    fn resolve_from_listing_does_not_confuse_8_5_with_8_50() {
        // Only 8.50.1 is present for x86_64; asking for 8.5 must NOT match it.
        let only_850 = "php-8.50.1-cli-linux-x86_64.tar.gz";
        assert!(matches!(
            resolve_from_listing(only_850, PhpVersion::new(8, 5), Os::Linux, Arch::X86_64),
            Err(PhpError::VersionUnavailable { .. })
        ));
    }

    #[test]
    fn resolve_from_listing_anchors_arch() {
        // 8.5 only has an aarch64 build in LISTING beyond x86_64; asking x86_64
        // must not pick the aarch64 patch (8.5.4).
        let a =
            resolve_from_listing(LISTING, PhpVersion::new(8, 5), Os::Linux, Arch::Aarch64).unwrap();
        assert_eq!(a.full_version, "8.5.4");
        assert!(a.cli_url.contains("linux-aarch64"));
    }

    #[test]
    fn resolve_from_listing_unknown_minor_errors() {
        match resolve_from_listing(LISTING, PhpVersion::new(7, 4), Os::Linux, Arch::X86_64) {
            Err(PhpError::VersionUnavailable { version }) => {
                assert_eq!(version, PhpVersion::new(7, 4));
            }
            other => panic!("expected VersionUnavailable, got {other:?}"),
        }
    }

    #[test]
    fn min_supported_floor_drops_8_0_and_8_1() {
        // A listing that includes 8.0/8.1 alongside supported minors.
        let listing = "\
            php-8.0.30-cli-linux-x86_64.tar.gz \
            php-8.1.31-cli-linux-x86_64.tar.gz \
            php-8.2.27-cli-linux-x86_64.tar.gz \
            php-8.5.6-cli-linux-x86_64.tar.gz";
        // available_minors hides 8.0/8.1.
        let got = available_minors(listing, Os::Linux, Arch::X86_64);
        assert_eq!(got, vec![PhpVersion::new(8, 2), PhpVersion::new(8, 5)]);
        // resolve_from_listing rejects them even though the build is published.
        for minor in [PhpVersion::new(8, 0), PhpVersion::new(8, 1)] {
            match resolve_from_listing(listing, minor, Os::Linux, Arch::X86_64) {
                Err(PhpError::VersionUnavailable { version }) => assert_eq!(version, minor),
                other => panic!("expected VersionUnavailable for {minor}, got {other:?}"),
            }
        }
        // 8.2 still resolves.
        assert_eq!(
            resolve_from_listing(listing, PhpVersion::new(8, 2), Os::Linux, Arch::X86_64)
                .unwrap()
                .full_version,
            "8.2.27"
        );
    }

    #[test]
    fn available_minors_lists_distinct_cli_builds_for_platform() {
        // x86_64 linux: 8.4.21, 8.5.2, 8.5.6, 8.50.1 have CLI builds; the
        // aarch64-only 8.5.4 must not leak in.
        let got = available_minors(LISTING, Os::Linux, Arch::X86_64);
        assert_eq!(
            got,
            vec![
                PhpVersion::new(8, 4),
                PhpVersion::new(8, 5),
                PhpVersion::new(8, 50),
            ]
        );
    }

    #[test]
    fn available_minors_anchors_arch() {
        // aarch64 linux only has the 8.5.4 CLI build in LISTING.
        let got = available_minors(LISTING, Os::Linux, Arch::Aarch64);
        assert_eq!(got, vec![PhpVersion::new(8, 5)]);
    }

    #[test]
    fn available_minors_keeps_8_5_and_8_50_distinct() {
        let got = available_minors(LISTING, Os::Linux, Arch::X86_64);
        assert!(got.contains(&PhpVersion::new(8, 5)));
        assert!(got.contains(&PhpVersion::new(8, 50)));
        assert_ne!(PhpVersion::new(8, 5), PhpVersion::new(8, 50));
    }

    #[test]
    fn available_minors_empty_listing_is_empty() {
        assert!(available_minors("", Os::Linux, Arch::X86_64).is_empty());
        // fpm-only build (no CLI) contributes no minor.
        let fpm_only = "php-8.5.6-fpm-linux-x86_64.tar.gz";
        assert!(available_minors(fpm_only, Os::Linux, Arch::X86_64).is_empty());
    }

    #[test]
    fn is_newer_compares_patch() {
        assert!(is_newer("8.5.6", "8.5.7"));
        assert!(!is_newer("8.5.6", "8.5.6"));
        assert!(!is_newer("8.5.7", "8.5.6"));
        assert!(!is_newer("8.5", "8.5.7")); // malformed installed
        assert!(!is_newer("8.5.6", "nope"));
        assert_eq!(patch_of("8.5.6"), Some(6));
        assert_eq!(patch_of("8.5"), None);
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
