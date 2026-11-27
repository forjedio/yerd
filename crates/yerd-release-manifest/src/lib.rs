//! Pure schema and transforms for the Yerd CDN release manifests.
//!
//! This crate does **no I/O**. It defines the JSON shapes published to the CDN
//! and the pure functions that build them from the GitHub Releases API and
//! reconcile the CDN against GitHub. The workflow layer (xtask + CI scripts)
//! does the fetching, uploading, listing, and deleting; this crate is the
//! testable core it calls.
//!
//! Two files are published at the CDN root:
//! - `releases.json` - a **bare array** of [`ReleaseEntry`], field-compatible
//!   with the daemon's GitHub-release wire struct so a future self-update
//!   migration is a one-URL change rather than a new parser.
//! - `latest.json` - a [`LatestManifest`] envelope carrying the latest stable
//!   and RC releases (GitHub has no two-channel "latest" equivalent).
//!
//! Each release also gets a categorised notes file at
//! `release-notes/<tag>.json` ([`ReleaseNotesFile`], built by
//! [`release_notes`]), outside the reconciled `releases/` folder.
//!
//! Version precedence and pre-release classification are borrowed from
//! [`yerd_update`] (`parse_tag` + semver ordering + the flag-or-semver
//! pre-release rule) so this crate cannot drift from the self-update decision
//! logic. `latest.stable` corresponds exactly to `select_target`'s
//! `latest_stable`; a test asserts that. `latest.rc` is a **pre-release-only**
//! channel (the highest pre-release newer than stable), which is deliberately
//! *not* `select_target`'s pre-release-inclusive `latest_edge` - a future edge
//! consumer reconstructs edge as `rc ?? stable`.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

/// The `latest.json` schema version. Bumped only on a breaking envelope change.
pub const LATEST_SCHEMA: u32 = 1;

/// One release, shaped exactly like the subset of the GitHub Releases API that
/// the daemon's self-update path consumes. Serializing this is what makes
/// `releases.json` byte-compatible with that parser; deserializing it is also
/// how the CI layer reads the `gh api .../releases` response (unknown fields are
/// ignored). All fields default so a sparse source still decodes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReleaseEntry {
    /// The release tag, verbatim (e.g. `v2.0.4-rc.1`).
    pub tag_name: String,
    /// Whether the source flagged this as a pre-release.
    #[serde(default)]
    pub prerelease: bool,
    /// Whether this is an unpublished draft. Never true on the CDN.
    #[serde(default)]
    pub draft: bool,
    /// Release notes / body, if any.
    #[serde(default)]
    pub body: Option<String>,
    /// Attached downloadable assets.
    #[serde(default)]
    pub assets: Vec<AssetEntry>,
}

/// One downloadable asset attached to a release.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AssetEntry {
    /// The asset filename (e.g. `Yerd_Linux_x86_64_v2-0-4.deb`).
    pub name: String,
    /// The download URL - pointed at the CDN once the asset is mirrored, else
    /// left on GitHub.
    pub browser_download_url: String,
    /// Size in bytes (0 if unknown).
    #[serde(default)]
    pub size: u64,
}

/// The `latest.json` envelope: the latest stable and RC releases.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LatestManifest {
    /// Envelope schema version ([`LATEST_SCHEMA`]).
    pub schema: u32,
    /// The highest non-pre-release, if any.
    pub stable: Option<ReleaseEntry>,
    /// The highest pre-release, but only when it is strictly newer than
    /// `stable` (else `None` - an old RC is not worth surfacing). `None` when
    /// there are no pre-releases newer than stable.
    pub rc: Option<ReleaseEntry>,
}

impl ReleaseEntry {
    /// The parsed semver version, or `None` if the tag is not valid semver
    /// (such a release is dropped, mirroring the daemon's `fetch_releases`).
    fn version(&self) -> Option<semver::Version> {
        yerd_update::parse_tag(&self.tag_name)
    }

    /// True if this should be treated as a pre-release: the source flag is set
    /// **or** the semver carries a pre-release component. Matches
    /// `yerd_update::ReleaseMeta::is_prerelease`.
    fn is_prerelease(&self, version: &semver::Version) -> bool {
        self.prerelease || !version.pre.is_empty()
    }
}

/// Rewrite `browser_download_url` to the CDN for every asset whose exact
/// `(tag, name)` is present in `mirrored_assets`; leave the rest on GitHub.
///
/// Gating is **per-asset, not per-folder**: a partially-uploaded release folder
/// (folder present, one file missing - reachable because the mirror job is
/// non-fatal) must not get the missing file's URL rewritten to a CDN path that
/// 404s. `mirrored_assets` comes from a recursive CDN listing, which yields
/// per-object presence.
fn rewrite_urls(
    entry: &ReleaseEntry,
    cdn_base: &str,
    mirrored_assets: &BTreeSet<(String, String)>,
) -> ReleaseEntry {
    let base = cdn_base.trim_end_matches('/');
    let assets = entry
        .assets
        .iter()
        .map(|a| {
            let key = (entry.tag_name.clone(), a.name.clone());
            let url = if mirrored_assets.contains(&key) {
                format!("{base}/releases/{}/{}", entry.tag_name, a.name)
            } else {
                a.browser_download_url.clone()
            };
            AssetEntry {
                name: a.name.clone(),
                browser_download_url: url,
                size: a.size,
            }
        })
        .collect();
    ReleaseEntry {
        tag_name: entry.tag_name.clone(),
        prerelease: entry.prerelease,
        draft: entry.draft,
        body: entry.body.clone(),
        assets,
    }
}

/// Build `releases.json` (the returned vec) and `latest.json` from the GitHub
/// release list.
///
/// Drafts and releases with an unparseable tag are dropped (mirroring the
/// daemon). Every asset URL is rewritten to the CDN only when that exact
/// `(tag, name)` is in `mirrored_assets` (see [`rewrite_urls`]). The vec is
/// sorted newest-first by semver. `latest.stable` is the highest non-pre-release;
/// `latest.rc` is the highest pre-release, exposed only when strictly newer than
/// stable.
#[must_use]
pub fn build_manifests(
    releases: Vec<ReleaseEntry>,
    cdn_base: &str,
    mirrored_assets: &BTreeSet<(String, String)>,
) -> (Vec<ReleaseEntry>, LatestManifest) {
    let mut kept: Vec<(semver::Version, ReleaseEntry)> = releases
        .into_iter()
        .filter(|r| !r.draft)
        .filter_map(|r| {
            let v = r.version()?;
            let rewritten = rewrite_urls(&r, cdn_base, mirrored_assets);
            Some((v, rewritten))
        })
        .collect();

    kept.sort_by(|a, b| b.0.cmp(&a.0));

    let mut stable: Option<&(semver::Version, ReleaseEntry)> = None;
    let mut highest_pre: Option<&(semver::Version, ReleaseEntry)> = None;
    for item in &kept {
        let (v, entry) = item;
        if entry.is_prerelease(v) {
            if highest_pre.map_or(true, |h| v > &h.0) {
                highest_pre = Some(item);
            }
        } else if stable.map_or(true, |s| v > &s.0) {
            stable = Some(item);
        }
    }

    let rc = highest_pre.filter(|pre| stable.map_or(true, |s| pre.0 > s.0));

    let latest = LatestManifest {
        schema: LATEST_SCHEMA,
        stable: stable.map(|s| s.1.clone()),
        rc: rc.map(|r| r.1.clone()),
    };
    (kept.into_iter().map(|(_, e)| e).collect(), latest)
}

/// One expected asset: a file that a non-draft GitHub release attaches, plus its
/// expected SHA-256 when that file is listed in the release's `SHA256SUMS`
/// (`None` for `.minisig` / `.sig` / `SHA256SUMS` itself, which aren't hashed
/// there).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExpectedAsset {
    /// The release tag the asset belongs to.
    pub tag: String,
    /// The asset filename.
    pub name: String,
    /// Size in bytes.
    pub size: u64,
    /// Lowercase-hex SHA-256, when known.
    pub sha256: Option<String>,
}

impl ExpectedAsset {
    /// The CDN object path this asset maps to (`releases/<tag>/<name>`).
    #[must_use]
    pub fn cdn_path(&self) -> String {
        format!("releases/{}/{}", self.tag, self.name)
    }
}

/// One object already present on the CDN under `releases/`, from a recursive
/// listing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CdnObject {
    /// The object path (`releases/<tag>/<name>`).
    pub path: String,
    /// Size in bytes.
    pub size: u64,
    /// The storage checksum (uppercase hex) when the CDN populated it, else
    /// `None`. Compared case-insensitively; `None` falls back to a size check.
    pub checksum: Option<String>,
}

impl CdnObject {
    /// The tag component of `releases/<tag>/<name>`, or `None` for any path that
    /// is not exactly three `releases/<tag>/<name>` components (a malformed or
    /// stray object we never keep-list for deletion).
    fn tag(&self) -> Option<&str> {
        let mut parts = self.path.split('/');
        match (parts.next(), parts.next(), parts.next(), parts.next()) {
            (Some("releases"), Some(tag), Some(name), None)
                if !tag.is_empty() && !name.is_empty() =>
            {
                Some(tag)
            }
            _ => None,
        }
    }
}

/// The reconcile decision: what to upload, re-upload, and delete to make the CDN
/// match GitHub.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ReconcilePlan {
    /// Expected assets absent from the CDN.
    pub to_upload: Vec<ExpectedAsset>,
    /// Expected assets present but whose bytes differ (size, or a populated
    /// checksum mismatch).
    pub to_update: Vec<ExpectedAsset>,
    /// CDN object paths under `releases/` to delete.
    pub to_delete: Vec<String>,
}

/// True if `expected`'s checksum is known, the CDN checksum is known, and they
/// disagree (case-insensitively). An unknown value on either side is not a
/// mismatch - the caller falls back to the size comparison.
fn checksum_mismatch(expected: Option<&str>, cdn: Option<&str>) -> bool {
    match (expected, cdn) {
        (Some(e), Some(c)) => !e.eq_ignore_ascii_case(c),
        _ => false,
    }
}

/// Diff the CDN against GitHub and produce the actions needed to sync.
///
/// `expected` is every non-draft release's assets; `cdn` is the recursive
/// listing under `releases/`; `known_tags` is **every** GitHub tag including
/// drafts; `public_tags` is the set of **non-draft** (published) tags. Rules:
/// - expected-but-absent -> `to_upload`.
/// - present with a size mismatch, or a populated checksum mismatch ->
///   `to_update` (an unknown checksum on either side falls back to size).
/// - a CDN object whose `(tag, name)` is not expected is deleted only when its
///   tag is an orphan (`tag not in known_tags`) or published (`tag in
///   public_tags`). A tag that is known but not published is a draft, and its
///   objects are left alone - a public-then-reverted-to-draft folder is
///   protected from surprise pruning. Publication is taken from `public_tags`
///   (not from asset presence), so a published release that happens to attach no
///   assets still has its stale objects pruned.
///
/// Safety: if `expected` is empty the delete set is forced empty, so a failed or
/// empty GitHub read can never orphan everything.
#[must_use]
pub fn reconcile(
    expected: &[ExpectedAsset],
    cdn: &[CdnObject],
    known_tags: &BTreeSet<String>,
    public_tags: &BTreeSet<String>,
) -> ReconcilePlan {
    let mut plan = ReconcilePlan::default();

    let cdn_by_path: std::collections::BTreeMap<&str, &CdnObject> =
        cdn.iter().map(|o| (o.path.as_str(), o)).collect();

    for exp in expected {
        let path = exp.cdn_path();
        match cdn_by_path.get(path.as_str()) {
            None => plan.to_upload.push(exp.clone()),
            Some(obj) => {
                let differs = obj.size != exp.size
                    || checksum_mismatch(exp.sha256.as_deref(), obj.checksum.as_deref());
                if differs {
                    plan.to_update.push(exp.clone());
                }
            }
        }
    }

    if expected.is_empty() {
        return plan;
    }

    let expected_paths: BTreeSet<String> = expected.iter().map(ExpectedAsset::cdn_path).collect();

    for obj in cdn {
        if expected_paths.contains(&obj.path) {
            continue;
        }
        let Some(tag) = obj.tag() else {
            continue;
        };
        let orphan = !known_tags.contains(tag);
        let published = public_tags.contains(tag);
        if orphan || published {
            plan.to_delete.push(obj.path.clone());
        }
    }

    plan
}

/// The release-notes file schema version. Bumped only on a breaking change.
pub const RELEASE_NOTES_SCHEMA: u32 = 1;

/// The CDN object path of one release's notes file: `release-notes/<tag>.json`.
///
/// The folder sits outside `releases/`, so the CDN reconcile never lists or
/// prunes it.
#[must_use]
pub fn release_notes_path(tag: &str) -> String {
    format!("release-notes/{tag}.json")
}

/// The subset of the GitHub Releases API that release notes are built from.
/// Unknown fields are ignored.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct GithubRelease {
    /// The release tag, verbatim.
    pub tag_name: String,
    /// Whether this is an unpublished draft.
    #[serde(default)]
    pub draft: bool,
    /// Whether the source flagged this as a pre-release.
    #[serde(default)]
    pub prerelease: bool,
    /// The release body (markdown), if any.
    #[serde(default)]
    pub body: Option<String>,
    /// The RFC 3339 publish time; absent on drafts.
    #[serde(default)]
    pub published_at: Option<String>,
}

/// One change listed in a release's notes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NoteItem {
    /// The change title, with any `[Feat]`/`[Fix]` prefix removed.
    pub title: String,
    /// The GitHub login of the author, without the `@`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
    /// The pull request URL.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
}

/// One release's categorised notes, published at [`release_notes_path`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReleaseNotesFile {
    /// File schema version ([`RELEASE_NOTES_SCHEMA`]).
    pub schema: u32,
    /// The release tag, verbatim (e.g. `v2.1.0-rc.3`).
    pub tag: String,
    /// The bare semver version (e.g. `2.1.0-rc.3`).
    pub version: String,
    /// True when the release is flagged as, or versioned as, a pre-release.
    pub prerelease: bool,
    /// The RFC 3339 publish time.
    pub published_at: String,
    /// New features.
    pub features: Vec<NoteItem>,
    /// Bug fixes.
    pub fixes: Vec<NoteItem>,
    /// Everything else that is user-facing.
    pub others: Vec<NoteItem>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Section {
    Features,
    Fixes,
    Others,
    Changelog,
    Skip,
}

fn heading_section(line: &str) -> Option<Section> {
    if let Some(rest) = line.strip_prefix("###") {
        let name = rest.trim().to_ascii_lowercase();
        return Some(match name.as_str() {
            "features" => Section::Features,
            "fixes" => Section::Fixes,
            "others" => Section::Others,
            _ => Section::Skip,
        });
    }
    let rest = line.strip_prefix("##")?;
    let normalised: String = rest
        .to_ascii_lowercase()
        .replace('\'', "")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    Some(if normalised == "whats changed" {
        Section::Changelog
    } else {
        Section::Skip
    })
}

fn is_housekeeping(entry: &str) -> bool {
    entry.starts_with("[Docs] ") || entry.starts_with("[Meta] ")
}

fn single_token(s: &str) -> bool {
    !s.is_empty() && !s.chars().any(char::is_whitespace)
}

/// Split `title by @login in https://...` into its parts. A line that does not
/// have that shape becomes a title-only item.
fn note_item(entry: &str) -> NoteItem {
    let whole = || NoteItem {
        title: entry.to_string(),
        author: None,
        url: None,
    };
    let Some((left, url_rest)) = entry.rsplit_once(" in https://") else {
        return whole();
    };
    let Some((title, author)) = left.rsplit_once(" by @") else {
        return whole();
    };
    if title.trim().is_empty() || !single_token(author) || !single_token(url_rest) {
        return whole();
    }
    NoteItem {
        title: title.trim().to_string(),
        author: Some(author.to_string()),
        url: Some(format!("https://{url_rest}")),
    }
}

/// Split a GitHub release body into `(features, fixes, others)`.
///
/// Handles both shapes the releases have used: the grouped body written by
/// `group-release-notes.sh` (`### Features` / `### Fixes` / `### Others`) and
/// GitHub's raw `## What's Changed` list, where `[Feat]` and `[Fix]` items are
/// routed and stripped and everything else lands in others. `[Docs]` and
/// `[Meta]` items are dropped in every section. Indented lines (sub-bullets,
/// wrapped continuations) are ignored. Any other heading, or a plain paragraph
/// line, ends collection until the next recognised heading.
#[must_use]
pub fn parse_notes_body(body: &str) -> (Vec<NoteItem>, Vec<NoteItem>, Vec<NoteItem>) {
    let mut features = Vec::new();
    let mut fixes = Vec::new();
    let mut others = Vec::new();
    let mut section = Section::Skip;

    for raw in body.lines() {
        let line = raw.trim_end();
        if let Some(next) = heading_section(line) {
            section = next;
            continue;
        }
        if section == Section::Skip || line.starts_with(char::is_whitespace) {
            continue;
        }
        let entry = line
            .strip_prefix("* ")
            .or_else(|| line.strip_prefix("- "))
            .map(str::trim);
        let Some(entry) = entry else {
            if !line.trim().is_empty() {
                section = Section::Skip;
            }
            continue;
        };
        if entry.is_empty() || is_housekeeping(entry) {
            continue;
        }
        match section {
            Section::Features => features.push(note_item(entry)),
            Section::Fixes => fixes.push(note_item(entry)),
            Section::Others => others.push(note_item(entry)),
            Section::Changelog => {
                if let Some(rest) = entry.strip_prefix("[Feat] ") {
                    features.push(note_item(rest.trim_start()));
                } else if let Some(rest) = entry.strip_prefix("[Fix] ") {
                    fixes.push(note_item(rest.trim_start()));
                } else {
                    others.push(note_item(entry));
                }
            }
            Section::Skip => {}
        }
    }

    (features, fixes, others)
}

/// Build the notes file for one GitHub release.
///
/// Returns `None` for a draft, a tag that is not valid semver (which also keeps
/// the CDN path safe), or a release with no publish time.
#[must_use]
pub fn release_notes(r: &GithubRelease) -> Option<ReleaseNotesFile> {
    if r.draft {
        return None;
    }
    let version = yerd_update::parse_tag(&r.tag_name)?;
    let published_at = r.published_at.clone()?;
    let (features, fixes, others) = parse_notes_body(r.body.as_deref().unwrap_or_default());
    Some(ReleaseNotesFile {
        schema: RELEASE_NOTES_SCHEMA,
        tag: r.tag_name.clone(),
        prerelease: r.prerelease || !version.pre.is_empty(),
        version: version.to_string(),
        published_at,
        features,
        fixes,
        others,
    })
}

/// A one-line summary of a release: the first feature title, else the first
/// fix, else the first other item, else empty.
#[must_use]
pub fn notes_summary(f: &ReleaseNotesFile) -> String {
    f.features
        .first()
        .or_else(|| f.fixes.first())
        .or_else(|| f.others.first())
        .map(|i| i.title.clone())
        .unwrap_or_default()
}

/// The stable tags strictly older than `current_tag`, newest first, capped at
/// `limit`. Drafts, pre-releases and non-semver tags are skipped; a current tag
/// that is not semver gives an empty list.
#[must_use]
pub fn earlier_tags(releases: &[ReleaseEntry], current_tag: &str, limit: usize) -> Vec<String> {
    let Some(current) = yerd_update::parse_tag(current_tag) else {
        return Vec::new();
    };
    let mut older: Vec<(semver::Version, &str)> = releases
        .iter()
        .filter(|r| !r.draft)
        .filter_map(|r| {
            let v = r.version()?;
            (!r.is_prerelease(&v) && v < current).then_some((v, r.tag_name.as_str()))
        })
        .collect();
    older.sort_by(|a, b| b.0.cmp(&a.0));
    older.dedup_by(|a, b| a.0 == b.0);
    older
        .into_iter()
        .take(limit)
        .map(|(_, t)| t.to_string())
        .collect()
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    fn entry(tag: &str, prerelease: bool, draft: bool, assets: &[&str]) -> ReleaseEntry {
        ReleaseEntry {
            tag_name: tag.to_string(),
            prerelease,
            draft,
            body: Some(format!("notes for {tag}")),
            assets: assets
                .iter()
                .map(|n| AssetEntry {
                    name: (*n).to_string(),
                    browser_download_url: format!("https://github.test/{tag}/{n}"),
                    size: 4,
                })
                .collect(),
        }
    }

    fn mirrored(pairs: &[(&str, &str)]) -> BTreeSet<(String, String)> {
        pairs
            .iter()
            .map(|(t, n)| ((*t).to_string(), (*n).to_string()))
            .collect()
    }

    #[test]
    fn drops_drafts_and_unparseable_and_sorts_newest_first() {
        let releases = vec![
            entry("v2.0.0", false, false, &["a.deb"]),
            entry("v2.1.0", false, false, &["a.deb"]),
            entry("draft", false, true, &["a.deb"]),
            entry("not-semver", false, false, &["a.deb"]),
        ];
        let (list, _) = build_manifests(releases, "https://cdn.test", &BTreeSet::new());
        let tags: Vec<_> = list.iter().map(|r| r.tag_name.as_str()).collect();
        assert_eq!(tags, vec!["v2.1.0", "v2.0.0"]);
    }

    #[test]
    fn per_asset_url_rewrite_gating() {
        let releases = vec![entry("v2.0.4", false, false, &["a.deb", "b.dmg"])];
        // Only a.deb is mirrored; b.dmg is missing from the folder.
        let (list, _) = build_manifests(
            releases,
            "https://cdn.test/",
            &mirrored(&[("v2.0.4", "a.deb")]),
        );
        let assets = &list.first().unwrap().assets;
        let a = assets.iter().find(|x| x.name == "a.deb").unwrap();
        let b = assets.iter().find(|x| x.name == "b.dmg").unwrap();
        assert_eq!(
            a.browser_download_url,
            "https://cdn.test/releases/v2.0.4/a.deb"
        );
        assert_eq!(b.browser_download_url, "https://github.test/v2.0.4/b.dmg");
    }

    #[test]
    fn latest_stable_and_rc_selection() {
        let releases = vec![
            entry("v2.0.5", false, false, &["a.deb"]),
            entry("v2.1.0-rc.1", true, false, &["a.deb"]),
            entry("v2.1.0-rc.2", false, false, &["a.deb"]), // pre via semver, flag off
        ];
        let (_, latest) = build_manifests(releases, "https://cdn.test", &BTreeSet::new());
        assert_eq!(latest.schema, LATEST_SCHEMA);
        assert_eq!(latest.stable.as_ref().unwrap().tag_name, "v2.0.5");
        assert_eq!(latest.rc.as_ref().unwrap().tag_name, "v2.1.0-rc.2");
    }

    #[test]
    fn rc_hidden_when_not_newer_than_stable() {
        let releases = vec![
            entry("v2.1.0", false, false, &["a.deb"]),
            entry("v2.1.0-rc.1", true, false, &["a.deb"]),
        ];
        let (_, latest) = build_manifests(releases, "https://cdn.test", &BTreeSet::new());
        assert_eq!(latest.stable.as_ref().unwrap().tag_name, "v2.1.0");
        assert!(latest.rc.is_none(), "old rc must not surface");
    }

    /// The stable/rc versions must agree with `yerd_update::select_target`'s
    /// `latest_stable` / `latest_edge` so this crate cannot drift from the
    /// self-update decision logic.
    #[test]
    fn selection_agrees_with_select_target() {
        // Helper: `latest.stable` must ALWAYS equal `select_target`'s
        // `latest_stable`. `latest.rc` is pre-release-ONLY and does NOT track the
        // pre-release-INCLUSIVE `latest_edge`; the two only coincide when the
        // newest release is itself a pre-release, so we don't assert rc==edge.
        fn stable_agrees(releases: Vec<ReleaseEntry>) {
            let metas: Vec<yerd_update::ReleaseMeta> = releases
                .iter()
                .map(|r| yerd_update::ReleaseMeta {
                    version: yerd_update::parse_tag(&r.tag_name).unwrap(),
                    tag: r.tag_name.clone(),
                    prerelease: r.prerelease,
                    assets: Vec::new(),
                    notes: None,
                })
                .collect();
            let decision = yerd_update::select_target(
                &metas,
                yerd_update::Channel::Stable,
                &semver::Version::new(0, 0, 1),
            );
            let (_, latest) = build_manifests(releases, "https://cdn.test", &BTreeSet::new());
            assert_eq!(
                latest
                    .stable
                    .as_ref()
                    .and_then(|s| yerd_update::parse_tag(&s.tag_name)),
                decision.latest_stable
            );
        }

        // Newest is a pre-release.
        stable_agrees(vec![
            entry("v2.0.5", false, false, &["a.deb"]),
            entry("v2.1.0-rc.1", true, false, &["a.deb"]),
            entry("v2.1.0-rc.3", true, false, &["a.deb"]),
        ]);
        // Newest is stable: here select_target's `latest_edge` is the stable
        // v2.1.0, but our `rc` is None (no pre-release newer than stable) - the
        // exact case where rc and latest_edge diverge.
        let releases = vec![
            entry("v2.1.0", false, false, &["a.deb"]),
            entry("v2.0.0-rc.1", true, false, &["a.deb"]),
        ];
        stable_agrees(releases.clone());
        let (_, latest) = build_manifests(releases, "https://cdn.test", &BTreeSet::new());
        assert!(
            latest.rc.is_none(),
            "rc is pre-release-only, not latest_edge (which would be the stable v2.1.0 here)"
        );
    }

    /// A serialized `ReleaseEntry` must deserialize into a struct byte-identical
    /// to the daemon's `GhRelease`/`GhAsset` (the whole point: `releases.json`
    /// migration is a one-URL change, not a new parser).
    #[test]
    fn wire_compatible_with_daemon_ghrelease() {
        #[derive(Deserialize, PartialEq, Debug)]
        struct GhRelease {
            tag_name: String,
            #[serde(default)]
            prerelease: bool,
            #[serde(default)]
            draft: bool,
            #[serde(default)]
            body: Option<String>,
            #[serde(default)]
            assets: Vec<GhAsset>,
        }
        #[derive(Deserialize, PartialEq, Debug)]
        struct GhAsset {
            name: String,
            browser_download_url: String,
            #[serde(default)]
            size: u64,
        }

        let list = vec![entry(
            "v2.0.4",
            false,
            false,
            &["Yerd_Linux_x86_64_v2-0-4.deb"],
        )];
        let json = serde_json::to_string(&list).unwrap();
        let parsed: Vec<GhRelease> = serde_json::from_str(&json).unwrap();
        let r = parsed.first().unwrap();
        assert_eq!(r.tag_name, "v2.0.4");
        assert_eq!(
            r.assets.first().unwrap().name,
            "Yerd_Linux_x86_64_v2-0-4.deb"
        );
        assert_eq!(r.assets.first().unwrap().size, 4);
    }

    fn exp(tag: &str, name: &str, size: u64, sha: Option<&str>) -> ExpectedAsset {
        ExpectedAsset {
            tag: tag.to_string(),
            name: name.to_string(),
            size,
            sha256: sha.map(str::to_string),
        }
    }

    fn cdn(path: &str, size: u64, checksum: Option<&str>) -> CdnObject {
        CdnObject {
            path: path.to_string(),
            size,
            checksum: checksum.map(str::to_string),
        }
    }

    fn tags(list: &[&str]) -> BTreeSet<String> {
        list.iter().map(|t| (*t).to_string()).collect()
    }

    #[test]
    fn reconcile_uploads_missing() {
        let expected = vec![exp("v2.0.4", "a.deb", 4, None)];
        let plan = reconcile(&expected, &[], &tags(&["v2.0.4"]), &tags(&["v2.0.4"]));
        assert_eq!(plan.to_upload, expected);
        assert!(plan.to_update.is_empty());
        assert!(plan.to_delete.is_empty());
    }

    #[test]
    fn reconcile_updates_on_size_diff() {
        let expected = vec![exp("v2.0.4", "a.deb", 8, None)];
        let cdn_objs = vec![cdn("releases/v2.0.4/a.deb", 4, None)];
        let plan = reconcile(&expected, &cdn_objs, &tags(&["v2.0.4"]), &tags(&["v2.0.4"]));
        assert_eq!(plan.to_update, expected);
        assert!(plan.to_upload.is_empty());
    }

    #[test]
    fn reconcile_updates_on_checksum_diff_but_not_when_unknown() {
        let cdn_objs = vec![cdn("releases/v2.0.4/a.deb", 4, Some("AABB"))];
        // Same size, mismatching known checksum -> update.
        let mismatch = vec![exp("v2.0.4", "a.deb", 4, Some("ccdd"))];
        assert_eq!(
            reconcile(&mismatch, &cdn_objs, &tags(&["v2.0.4"]), &tags(&["v2.0.4"])).to_update,
            mismatch
        );
        // Same size, matching checksum (case-insensitive) -> no action.
        let matching = vec![exp("v2.0.4", "a.deb", 4, Some("aabb"))];
        assert!(
            reconcile(&matching, &cdn_objs, &tags(&["v2.0.4"]), &tags(&["v2.0.4"]))
                .to_update
                .is_empty()
        );
        // Unknown expected checksum, same size -> size-only, no action.
        let unknown = vec![exp("v2.0.4", "a.deb", 4, None)];
        assert!(
            reconcile(&unknown, &cdn_objs, &tags(&["v2.0.4"]), &tags(&["v2.0.4"]))
                .to_update
                .is_empty()
        );
    }

    #[test]
    fn reconcile_deletes_orphan_tag() {
        let expected = vec![exp("v2.0.4", "a.deb", 4, None)];
        let cdn_objs = vec![
            cdn("releases/v2.0.4/a.deb", 4, None),
            cdn("releases/v1.9.9/old.deb", 4, None),
        ];
        let plan = reconcile(&expected, &cdn_objs, &tags(&["v2.0.4"]), &tags(&["v2.0.4"]));
        assert_eq!(plan.to_delete, vec!["releases/v1.9.9/old.deb".to_string()]);
    }

    #[test]
    fn reconcile_deletes_stale_asset_in_public_tag() {
        let expected = vec![exp("v2.0.4", "a.deb", 4, None)];
        let cdn_objs = vec![
            cdn("releases/v2.0.4/a.deb", 4, None),
            cdn("releases/v2.0.4/removed.dmg", 4, None),
        ];
        let plan = reconcile(&expected, &cdn_objs, &tags(&["v2.0.4"]), &tags(&["v2.0.4"]));
        assert_eq!(
            plan.to_delete,
            vec!["releases/v2.0.4/removed.dmg".to_string()]
        );
    }

    #[test]
    fn reconcile_protects_draft_reverted_folder() {
        // v2.0.4 is public; v2.1.0-rc.1 is a KNOWN draft (in known_tags) but
        // contributes no expected asset. Its folder must be left alone.
        let expected = vec![exp("v2.0.4", "a.deb", 4, None)];
        let cdn_objs = vec![
            cdn("releases/v2.0.4/a.deb", 4, None),
            cdn("releases/v2.1.0-rc.1/a.deb", 4, None),
        ];
        // known_tags has both; public_tags (non-draft) has only v2.0.4.
        let plan = reconcile(
            &expected,
            &cdn_objs,
            &tags(&["v2.0.4", "v2.1.0-rc.1"]),
            &tags(&["v2.0.4"]),
        );
        assert!(
            plan.to_delete.is_empty(),
            "known draft folder must not be pruned, got {:?}",
            plan.to_delete
        );
    }

    #[test]
    fn reconcile_empty_expected_deletes_nothing() {
        let cdn_objs = vec![cdn("releases/v2.0.4/a.deb", 4, None)];
        let plan = reconcile(&[], &cdn_objs, &BTreeSet::new(), &BTreeSet::new());
        assert!(plan.to_delete.is_empty());
        assert!(plan.to_upload.is_empty());
    }

    #[test]
    fn reconcile_prunes_assetless_published_release() {
        // A published (non-draft) tag that attaches no assets still has its
        // stale CDN objects pruned - publication is taken from public_tags, not
        // from asset presence.
        let cdn_objs = vec![cdn("releases/v2.0.4/leftover.deb", 4, None)];
        let plan = reconcile(&[], &cdn_objs, &tags(&["v2.0.4"]), &tags(&["v2.0.4"]));
        // expected is empty here, so the empty-expected guard forces no deletes.
        assert!(plan.to_delete.is_empty(), "empty-expected guard holds");

        // With at least one other published release contributing an expected
        // asset, the assetless published tag's leftover is pruned.
        let expected = vec![exp("v2.0.5", "a.deb", 4, None)];
        let cdn_objs = vec![
            cdn("releases/v2.0.5/a.deb", 4, None),
            cdn("releases/v2.0.4/leftover.deb", 4, None),
        ];
        let plan = reconcile(
            &expected,
            &cdn_objs,
            &tags(&["v2.0.4", "v2.0.5"]),
            &tags(&["v2.0.4", "v2.0.5"]),
        );
        assert_eq!(
            plan.to_delete,
            vec!["releases/v2.0.4/leftover.deb".to_string()]
        );
    }

    #[test]
    fn reconcile_never_touches_release_notes_folder() {
        let expected = vec![exp("v2.0.4", "a.deb", 4, None)];
        let cdn_objs = vec![
            cdn("releases/v2.0.4/a.deb", 4, None),
            cdn("release-notes/v2.0.4.json", 4, None),
        ];
        let plan = reconcile(&expected, &cdn_objs, &tags(&["v2.0.4"]), &tags(&["v2.0.4"]));
        assert!(plan.to_delete.is_empty(), "got {:?}", plan.to_delete);
        assert!(plan.to_upload.is_empty());
        assert!(plan.to_update.is_empty());
    }

    #[test]
    fn reconcile_ignores_malformed_paths() {
        let expected = vec![exp("v2.0.4", "a.deb", 4, None)];
        let cdn_objs = vec![
            cdn("releases/v2.0.4/a.deb", 4, None),
            cdn("releases/stray.txt", 4, None),
            cdn("builds/2024/x.deb", 4, None),
        ];
        let plan = reconcile(&expected, &cdn_objs, &tags(&["v2.0.4"]), &tags(&["v2.0.4"]));
        assert!(
            plan.to_delete.is_empty(),
            "malformed/other-prefix paths kept"
        );
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod notes_tests {
    use super::*;

    const GROUPED_RC3: &str = "### Features\n\n* Project-aware Node version management by @totov in https://github.com/forjedio/yerd/pull/244\n* Outbound proxy support by @RichardAnderson in https://github.com/forjedio/yerd/pull/267\n* Support xdebug by @RichardAnderson in https://github.com/forjedio/yerd/pull/268\n\n### Fixes\n\n* Check the login shell's real PATH for the Doctor bin-dir warning by @ralphschindler in https://github.com/forjedio/yerd/pull/251\n\n## New Contributors\n* @ralphschindler made their first contribution in https://github.com/forjedio/yerd/pull/251\n\n**Full Changelog**: https://github.com/forjedio/yerd/compare/v2.1.0-rc.2...v2.1.0-rc.3";

    const RAW_RC10: &str = "## What's Changed\n* [Fix] Open menubar window on the active Space (macOS) by @saeedvaziry in https://github.com/forjedio/yerd/pull/43\n* [Fix] Cmd+W not closing borderless windows on macOS by @saeedvaziry in https://github.com/forjedio/yerd/pull/45\n* [Feat] GitHub templates & documentation updates by @RichardAnderson in https://github.com/forjedio/yerd/pull/46\n* [Fix] Simplify comments by @RichardAnderson in https://github.com/forjedio/yerd/pull/47\n* [Fix] Resolves timeout warning for daemon upgrades/installs by @RichardAnderson in https://github.com/forjedio/yerd/pull/49\n* [Task] Unit test coverage by @RichardAnderson in https://github.com/forjedio/yerd/pull/50\n* [Fix] Linux .deb install, PHP install hang/progress, and Linux-native window chrome by @RichardAnderson in https://github.com/forjedio/yerd/pull/51\n* [Feat] Arch linux support by @RichardAnderson in https://github.com/forjedio/yerd/pull/54\n* [Feat] In-app keyboard shortcuts: command palette, view navigation, and contextual actions by @RichardAnderson in https://github.com/forjedio/yerd/pull/55\n* [Docs] Social card & doc updates by @RichardAnderson in https://github.com/forjedio/yerd/pull/56\n* [Feat] Dynamic system-tray dropdown, unread-mail indicators, and frontend state layer by @RichardAnderson in https://github.com/forjedio/yerd/pull/57\n* [Feat] Share sites publicly via Cloudflare Tunnel by @RichardAnderson in https://github.com/forjedio/yerd/pull/58\n* [Feat] Various UI updates by @RichardAnderson in https://github.com/forjedio/yerd/pull/61\n* [Fix] Resolve libcurl issue with resolving .test locally by @RichardAnderson in https://github.com/forjedio/yerd/pull/62\n* [Fix] PHP trusts Yerd's CA by @RichardAnderson in https://github.com/forjedio/yerd/pull/63\n\n## New Contributors\n* @saeedvaziry made their first contribution in https://github.com/forjedio/yerd/pull/43\n\n**Full Changelog**: https://github.com/forjedio/yerd/compare/v2.0.2-rc.9...v2.0.2-rc.10";

    const GROUPED_V202: &str = "### Fixes\n\n* Onboarding Auto Install by @RichardAnderson in https://github.com/forjedio/yerd/pull/106\n* Linux titlebar not triggering actions by @RichardAnderson in https://github.com/forjedio/yerd/pull/105\n\n### Others\n\n* [Docs] LLM support for the docs by @RichardAnderson in https://github.com/forjedio/yerd/pull/103\n* [Docs] Updates for the v2 release by @RichardAnderson in https://github.com/forjedio/yerd/pull/104\n\n**Full Changelog**: https://github.com/forjedio/yerd/compare/v2.0.2-rc.13...v2.0.2";

    fn titles(items: &[NoteItem]) -> Vec<&str> {
        items.iter().map(|i| i.title.as_str()).collect()
    }

    fn gh(tag: &str, draft: bool, prerelease: bool, body: Option<&str>) -> GithubRelease {
        GithubRelease {
            tag_name: tag.to_string(),
            draft,
            prerelease,
            body: body.map(str::to_string),
            published_at: Some("2026-11-18T16:34:09Z".to_string()),
        }
    }

    fn rel(tag: &str, prerelease: bool, draft: bool) -> ReleaseEntry {
        ReleaseEntry {
            tag_name: tag.to_string(),
            prerelease,
            draft,
            body: None,
            assets: Vec::new(),
        }
    }

    #[test]
    fn release_notes_path_is_outside_releases_folder() {
        assert_eq!(release_notes_path("v2.0.4"), "release-notes/v2.0.4.json");
    }

    #[test]
    fn parses_grouped_body() {
        let (features, fixes, others) = parse_notes_body(GROUPED_RC3);
        assert_eq!(
            titles(&features),
            vec![
                "Project-aware Node version management",
                "Outbound proxy support",
                "Support xdebug"
            ]
        );
        assert_eq!(
            titles(&fixes),
            vec!["Check the login shell's real PATH for the Doctor bin-dir warning"]
        );
        assert!(others.is_empty());
        let first = features.first().unwrap();
        assert_eq!(first.author.as_deref(), Some("totov"));
        assert_eq!(
            first.url.as_deref(),
            Some("https://github.com/forjedio/yerd/pull/244")
        );
    }

    #[test]
    fn parses_raw_whats_changed_body() {
        let (features, fixes, others) = parse_notes_body(RAW_RC10);
        assert_eq!(features.len(), 6);
        assert_eq!(fixes.len(), 7);
        assert_eq!(titles(&others), vec!["[Task] Unit test coverage"]);
        assert_eq!(
            features.first().unwrap().title,
            "GitHub templates & documentation updates"
        );
        assert_eq!(
            fixes.first().unwrap().title,
            "Open menubar window on the active Space (macOS)"
        );
        assert!(features
            .iter()
            .chain(&fixes)
            .chain(&others)
            .all(|i| !i.title.contains("Social card")));
    }

    #[test]
    fn drops_docs_items_under_grouped_others() {
        let (features, fixes, others) = parse_notes_body(GROUPED_V202);
        assert!(features.is_empty());
        assert_eq!(
            titles(&fixes),
            vec![
                "Onboarding Auto Install",
                "Linux titlebar not triggering actions"
            ]
        );
        assert!(others.is_empty());
    }

    #[test]
    fn note_item_shapes() {
        let cases: &[(&str, &str, Option<&str>, Option<&str>)] = &[
            (
                "Fix by-pass by @a in https://x.test/pull/1",
                "Fix by-pass",
                Some("a"),
                Some("https://x.test/pull/1"),
            ),
            (
                "Use @mentions by @me by @bob in https://x.test/2",
                "Use @mentions by @me",
                Some("bob"),
                Some("https://x.test/2"),
            ),
            ("Just a title", "Just a title", None, None),
            (
                "No author in https://x.test/3",
                "No author in https://x.test/3",
                None,
                None,
            ),
            (
                "Spaced by @a b in https://x.test/4",
                "Spaced by @a b in https://x.test/4",
                None,
                None,
            ),
        ];
        for (line, title, author, url) in cases {
            let item = note_item(line);
            assert_eq!(item.title, *title, "{line}");
            assert_eq!(item.author.as_deref(), *author, "{line}");
            assert_eq!(item.url.as_deref(), *url, "{line}");
        }
    }

    #[test]
    fn parse_body_edge_cases() {
        let cases: &[(&str, usize, usize, usize)] = &[
            ("", 0, 0, 0),
            ("* stray item before any heading", 0, 0, 0),
            (
                "## What's Changed\r\n* [Feat] A by @a in https://x/1\r\n",
                1,
                0,
                0,
            ),
            ("## whats   changed\n- [Fix] B\n", 0, 1, 0),
            ("## What's Changed\n* [Meta] C\n* D\n", 0, 0, 1),
            ("### Features\n* A\nSome paragraph\n* B\n", 1, 0, 0),
            ("### Features\n* A\n### Highlights\n* B\n", 1, 0, 0),
            ("### Features\n* A\n  * detail\n* B\n", 2, 0, 0),
            ("### Fixes\n* A wraps\n  onto a second line\n* B\n", 0, 2, 0),
            (
                "### Fixes\n* A\n## New Contributors\n* @x made it\n",
                0,
                1,
                0,
            ),
        ];
        for (body, f, x, o) in cases {
            let (features, fixes, others) = parse_notes_body(body);
            assert_eq!(
                (features.len(), fixes.len(), others.len()),
                (*f, *x, *o),
                "{body:?}"
            );
        }
    }

    #[test]
    fn release_notes_builds_file() {
        let file = release_notes(&gh("v2.1.0-rc.3", false, false, Some(GROUPED_RC3))).unwrap();
        assert_eq!(file.schema, RELEASE_NOTES_SCHEMA);
        assert_eq!(file.tag, "v2.1.0-rc.3");
        assert_eq!(file.version, "2.1.0-rc.3");
        assert!(file.prerelease);
        assert_eq!(file.published_at, "2026-11-18T16:34:09Z");
        assert_eq!(file.features.len(), 3);
        assert_eq!(file.fixes.len(), 1);
        assert!(file.others.is_empty());
    }

    #[test]
    fn release_notes_rejections() {
        let mut unpublished = gh("v2.0.4", false, false, None);
        unpublished.published_at = None;
        let cases = [
            (gh("v2.0.4", true, false, Some(GROUPED_V202)), false),
            (gh("nightly", false, false, Some(GROUPED_V202)), false),
            (gh("../evil", false, false, None), false),
            (unpublished, false),
            (gh("v2.0.4", false, false, None), true),
        ];
        for (release, kept) in cases {
            assert_eq!(
                release_notes(&release).is_some(),
                kept,
                "{}",
                release.tag_name
            );
        }
    }

    #[test]
    fn release_notes_prerelease_flag_or_semver() {
        let flagged = release_notes(&gh("v2.0.4", false, true, None)).unwrap();
        let stable = release_notes(&gh("v2.0.4", false, false, None)).unwrap();
        assert!(flagged.prerelease);
        assert!(!stable.prerelease);
    }

    #[test]
    fn file_json_shape_and_roundtrip() {
        let file = release_notes(&gh("v2.0.2", false, false, Some(GROUPED_V202))).unwrap();
        let json = serde_json::to_string(&file).unwrap();
        assert!(json.starts_with(
            r#"{"schema":1,"tag":"v2.0.2","version":"2.0.2","prerelease":false,"published_at":"2026-11-18T16:34:09Z","features":[],"fixes":[{"title":"Onboarding Auto Install","author":"RichardAnderson","url":"https://github.com/forjedio/yerd/pull/106"}"#
        ));
        assert!(json.ends_with(r#"],"others":[]}"#));
        let back: ReleaseNotesFile = serde_json::from_str(&json).unwrap();
        assert_eq!(back, file);
        let bare = serde_json::to_string(&NoteItem {
            title: "t".into(),
            author: None,
            url: None,
        })
        .unwrap();
        assert_eq!(bare, r#"{"title":"t"}"#);
    }

    #[test]
    fn github_release_ignores_unknown_fields() {
        let json = r#"[{"tag_name":"v2.0.4","id":1,"published_at":"2026-07-25T15:49:58Z","assets":[]},{"tag_name":"v2.0.5","draft":true}]"#;
        let list: Vec<GithubRelease> = serde_json::from_str(json).unwrap();
        assert_eq!(list.len(), 2);
        assert!(release_notes(list.first().unwrap()).is_some());
        assert!(release_notes(list.get(1).unwrap()).is_none());
    }

    #[test]
    fn notes_summary_picks_first_available() {
        let item = |t: &str| NoteItem {
            title: t.to_string(),
            author: None,
            url: None,
        };
        let base = release_notes(&gh("v2.0.4", false, false, None)).unwrap();
        let mut all = base.clone();
        all.features = vec![item("feat")];
        all.fixes = vec![item("fix")];
        all.others = vec![item("other")];
        let mut fix_only = base.clone();
        fix_only.fixes = vec![item("fix")];
        fix_only.others = vec![item("other")];
        let mut other_only = base.clone();
        other_only.others = vec![item("other")];
        let cases = [
            (all, "feat"),
            (fix_only, "fix"),
            (other_only, "other"),
            (base, ""),
        ];
        for (file, want) in cases {
            assert_eq!(notes_summary(&file), want);
        }
    }

    #[test]
    fn earlier_tags_table() {
        let releases = vec![
            rel("v2.1.0-rc.3", true, false),
            rel("v2.1.0-rc.2", true, false),
            rel("v2.0.4", false, false),
            rel("v2.0.4-rc.1", true, false),
            rel("v2.0.3", false, false),
            rel("v2.0.2", false, false),
            rel("v2.0.1", false, true),
            rel("v2.0.0-rc.1", false, false),
            rel("v1.9.0", false, false),
            rel("junk", false, false),
        ];
        let cases: &[(&str, usize, &[&str])] = &[
            ("v2.1.0-rc.3", 5, &["v2.0.4", "v2.0.3", "v2.0.2", "v1.9.0"]),
            ("v2.0.4", 5, &["v2.0.3", "v2.0.2", "v1.9.0"]),
            ("v2.1.0-rc.3", 2, &["v2.0.4", "v2.0.3"]),
            ("v1.9.0", 5, &[]),
            ("unknown", 5, &[]),
            ("v9.0.0", 0, &[]),
        ];
        for (current, limit, want) in cases {
            assert_eq!(
                earlier_tags(&releases, current, *limit),
                want.iter().map(|t| (*t).to_string()).collect::<Vec<_>>(),
                "{current} limit {limit}"
            );
        }
    }
}
