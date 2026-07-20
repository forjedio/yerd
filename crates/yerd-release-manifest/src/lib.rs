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
