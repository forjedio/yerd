//! Pure version/channel decision logic for Yerd self-update.
//!
//! This crate does **no I/O**: it operates on already-fetched release metadata
//! ([`ReleaseMeta`]) and the running version, and decides what (if anything) the
//! configured [`Channel`] should update to. Fetching releases, downloading
//! artifacts, verifying signatures, and performing the swap all live in the I/O
//! layers (`yerdd` and the applier); this crate is the testable brain they call.
//!
//! The two release channels are:
//! - [`Channel::Stable`] - the highest non-pre-release version.
//! - [`Channel::Edge`] - the highest version across *all* releases, including
//!   pre-releases / release candidates.
//!
//! Version precedence follows semver, so `2.1.0-rc.1 < 2.1.0` and build metadata
//! is ignored in comparisons.

use semver::Version;

mod artifact;
pub use artifact::{
    select_asset, sha256_for, sha256_hex, verify_minisign, verify_sha256, ArtifactKind,
    ArtifactSelection, AssetError, Platform, VerifyError, UPDATE_PUBLIC_KEY,
};

/// Self-update release channel.
///
/// Mirrors the persisted `update_channel` config string and the `yerd-ipc` wire
/// enum; conversion happens at the I/O boundary so this crate stays free of a
/// serde / protocol dependency.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Channel {
    /// Track the latest non-pre-release. The default.
    #[default]
    Stable,
    /// Track the latest version including pre-releases / release candidates.
    Edge,
}

impl Channel {
    /// The lowercase wire/string form (`"stable"` / `"edge"`).
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Stable => "stable",
            Self::Edge => "edge",
        }
    }

    /// Parse the lowercase string form. Returns `None` for any other value.
    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "stable" => Some(Self::Stable),
            "edge" => Some(Self::Edge),
            _ => None,
        }
    }
}

impl std::fmt::Display for Channel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A downloadable release asset (one file attached to a GitHub release).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Asset {
    /// The asset filename (e.g. `Yerd_MacOS_AppleSilicon_v2-0-2.app.tar.gz`).
    pub name: String,
    /// The download URL.
    pub url: String,
    /// Size in bytes (0 if unknown).
    pub size: u64,
}

/// Metadata for a single release, already parsed from whatever source produced
/// it (the GitHub Releases API in production). Plain data - no serde derive, so
/// the I/O layer maps its own wire structs into this.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReleaseMeta {
    /// The release version, parsed from the tag (leading `v` stripped).
    pub version: Version,
    /// The release's tag (verbatim, e.g. `v2.0.2-rc.3`).
    pub tag: String,
    /// Whether the *source* flagged this as a pre-release. Treated as
    /// pre-release if this is `true` **or** the semver carries a pre-release
    /// component (`-rc.N`), so a mis-flagged release is still classified safely.
    pub prerelease: bool,
    /// Attached downloadable assets.
    pub assets: Vec<Asset>,
    /// Release notes / body, if any.
    pub notes: Option<String>,
}

impl ReleaseMeta {
    /// True if this release should be treated as a pre-release for channel
    /// partitioning: either the source flag is set or the semver has a
    /// pre-release component.
    #[must_use]
    pub fn is_prerelease(&self) -> bool {
        self.prerelease || !self.version.pre.is_empty()
    }
}

/// The outcome of [`select_target`]: both channel latests, the running version,
/// and what the requested channel resolves to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdateDecision {
    /// The currently running version.
    pub current: Version,
    /// Highest non-pre-release version available (`None` if there are none).
    pub latest_stable: Option<Version>,
    /// Highest version available including pre-releases (`None` if none).
    pub latest_edge: Option<Version>,
    /// The channel this decision was made for.
    pub channel: Channel,
    /// The version to update to on `channel`: the channel's latest, but only
    /// when it is strictly newer than `current`. `None` means "nothing to do".
    pub target: Option<Version>,
    /// Convenience: `target.is_some()`.
    pub available: bool,
    /// True when `current` is a pre-release that is at least as new as the
    /// latest stable - i.e. switching to the stable channel would be a
    /// *downgrade*. Drives the `--stable` downgrade guard.
    pub ahead_of_stable: bool,
}

/// Strip an optional leading `v`/`V` from a release tag and parse it as semver.
/// Returns `None` for tags that are not valid semver (the caller skips them).
#[must_use]
pub fn parse_tag(tag: &str) -> Option<Version> {
    let trimmed = tag
        .strip_prefix('v')
        .or_else(|| tag.strip_prefix('V'))
        .unwrap_or(tag);
    Version::parse(trimmed).ok()
}

/// Decide what `channel` should update to, given the full set of `releases` and
/// the running `current` version.
///
/// `latest_stable` is the max over non-pre-releases; `latest_edge` is the max
/// over everything. The chosen channel's latest becomes `target` only if it is
/// strictly newer than `current` (so the stable channel never *downgrades* a
/// user who is running a newer pre-release - see [`UpdateDecision::ahead_of_stable`]).
#[must_use]
pub fn select_target(
    releases: &[ReleaseMeta],
    channel: Channel,
    current: &Version,
) -> UpdateDecision {
    let mut latest_stable: Option<&Version> = None;
    let mut latest_edge: Option<&Version> = None;
    for r in releases {
        let v = &r.version;
        if latest_edge.map_or(true, |e| v > e) {
            latest_edge = Some(v);
        }
        if !r.is_prerelease() && latest_stable.map_or(true, |s| v > s) {
            latest_stable = Some(v);
        }
    }

    let candidate = match channel {
        Channel::Stable => latest_stable,
        Channel::Edge => latest_edge,
    };
    let target = candidate.filter(|c| *c > current).cloned();
    let available = target.is_some();
    let ahead_of_stable = !current.pre.is_empty() && latest_stable.map_or(true, |s| current > s);

    UpdateDecision {
        current: current.clone(),
        latest_stable: latest_stable.cloned(),
        latest_edge: latest_edge.cloned(),
        channel,
        target,
        available,
        ahead_of_stable,
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    fn ver(s: &str) -> Version {
        Version::parse(s).unwrap()
    }

    fn rel(tag: &str, prerelease: bool) -> ReleaseMeta {
        ReleaseMeta {
            version: parse_tag(tag).unwrap_or_else(|| panic!("bad tag {tag}")),
            tag: tag.to_string(),
            prerelease,
            assets: Vec::new(),
            notes: None,
        }
    }

    #[test]
    fn channel_string_round_trips() {
        for ch in [Channel::Stable, Channel::Edge] {
            assert_eq!(Channel::parse(ch.as_str()), Some(ch));
            assert_eq!(ch.to_string(), ch.as_str());
        }
        assert_eq!(Channel::parse("nightly"), None);
        assert_eq!(Channel::default(), Channel::Stable);
    }

    #[test]
    fn parse_tag_strips_v_prefix_and_rejects_junk() {
        assert_eq!(parse_tag("v2.0.2"), Some(ver("2.0.2")));
        assert_eq!(parse_tag("V2.0.2-rc.3"), Some(ver("2.0.2-rc.3")));
        assert_eq!(parse_tag("2.0.2"), Some(ver("2.0.2")));
        assert_eq!(parse_tag("not-a-version"), None);
        assert_eq!(parse_tag("v2.0"), None);
    }

    #[test]
    fn prerelease_classification_uses_flag_or_semver() {
        assert!(rel("v2.0.0-rc.1", false).is_prerelease(), "semver pre");
        assert!(rel("v2.0.0", true).is_prerelease(), "source flag");
        assert!(!rel("v2.0.0", false).is_prerelease());
    }

    #[test]
    fn stable_channel_picks_highest_non_prerelease() {
        let releases = [
            rel("v2.0.0", false),
            rel("v2.1.0-rc.1", false),
            rel("v2.0.5", false),
        ];
        let d = select_target(&releases, Channel::Stable, &ver("2.0.0"));
        assert_eq!(d.latest_stable, Some(ver("2.0.5")));
        assert_eq!(d.latest_edge, Some(ver("2.1.0-rc.1")));
        assert_eq!(d.target, Some(ver("2.0.5")));
        assert!(d.available);
        assert!(!d.ahead_of_stable);
    }

    #[test]
    fn edge_channel_picks_highest_including_prerelease() {
        let releases = [rel("v2.0.5", false), rel("v2.1.0-rc.1", true)];
        let d = select_target(&releases, Channel::Edge, &ver("2.0.5"));
        assert_eq!(d.target, Some(ver("2.1.0-rc.1")));
        assert!(d.available);
    }

    #[test]
    fn prerelease_ordering_respects_semver() {
        let releases = [rel("v2.1.0-rc.1", true), rel("v2.1.0-rc.2", true)];
        let d = select_target(&releases, Channel::Edge, &ver("2.1.0-rc.1"));
        assert_eq!(d.latest_edge, Some(ver("2.1.0-rc.2")));
        assert_eq!(d.target, Some(ver("2.1.0-rc.2")));
    }

    #[test]
    fn up_to_date_when_current_equals_latest() {
        let releases = [rel("v2.0.5", false)];
        let d = select_target(&releases, Channel::Stable, &ver("2.0.5"));
        assert_eq!(d.target, None);
        assert!(!d.available);
    }

    #[test]
    fn ahead_of_stable_when_on_newer_prerelease() {
        let releases = [rel("v2.0.5", false), rel("v2.1.0-rc.3", true)];
        let d = select_target(&releases, Channel::Stable, &ver("2.1.0-rc.3"));
        assert_eq!(d.latest_stable, Some(ver("2.0.5")));
        assert_eq!(d.target, None, "no downgrade on stable");
        assert!(!d.available);
        assert!(d.ahead_of_stable);
    }

    #[test]
    fn not_ahead_of_stable_when_on_old_stable() {
        let releases = [rel("v2.0.5", false)];
        let d = select_target(&releases, Channel::Stable, &ver("2.0.0"));
        assert!(!d.ahead_of_stable);
    }

    #[test]
    fn empty_releases_yields_nothing() {
        let d = select_target(&[], Channel::Edge, &ver("2.0.0"));
        assert_eq!(d.latest_stable, None);
        assert_eq!(d.latest_edge, None);
        assert_eq!(d.target, None);
        assert!(!d.available);
        assert!(!d.ahead_of_stable);
    }

    #[test]
    fn no_stable_releases_at_all() {
        let releases = [rel("v2.1.0-rc.1", true)];
        let d = select_target(&releases, Channel::Stable, &ver("2.1.0-rc.1"));
        assert_eq!(d.latest_stable, None);
        assert_eq!(d.latest_edge, Some(ver("2.1.0-rc.1")));
        assert_eq!(d.target, None);
        assert!(d.ahead_of_stable);
    }

    #[test]
    fn build_metadata_is_ignored_in_precedence() {
        let releases = [rel("v2.0.5", false)];
        let d = select_target(&releases, Channel::Stable, &ver("2.0.5+build.7"));
        assert_eq!(d.target, None);
    }
}
