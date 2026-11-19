//! CDN release-manifest generation and reconcile-plan computation.
//!
//! Thin I/O glue over the pure [`yerd_release_manifest`] crate: read the GitHub
//! Releases API response and a CDN listing off disk, call the pure transforms,
//! and write `latest.json` / `releases.json` / `plan.json` / the per-release
//! `<tag>.json` notes files. All the decision logic (URL rewrite gating,
//! stable/rc selection, the CDN<->GitHub diff, notes parsing) lives in the
//! crate; this module never decides anything.

use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use anyhow::{bail, Context, Result};
use serde::Deserialize;
use yerd_release_manifest::{
    build_manifests, reconcile, release_notes, CdnObject, ExpectedAsset, GithubRelease,
    ReleaseEntry,
};

/// One object from a CDN listing (`bunny-list.sh` output). Files only -
/// directories are filtered out by the lister.
#[derive(Debug, Deserialize)]
struct ListingObject {
    /// Object path, `releases/<tag>/<name>`.
    path: String,
    #[serde(default)]
    size: u64,
    #[serde(default)]
    checksum: Option<String>,
}

/// Split a `releases/<tag>/<name>` path into its `(tag, name)`, or `None` for
/// anything not exactly those three components.
fn split_release_path(path: &str) -> Option<(String, String)> {
    let mut parts = path.split('/');
    match (parts.next(), parts.next(), parts.next(), parts.next()) {
        (Some("releases"), Some(tag), Some(name), None) if !tag.is_empty() && !name.is_empty() => {
            Some((tag.to_string(), name.to_string()))
        }
        _ => None,
    }
}

fn read_releases(path: &Path) -> Result<Vec<ReleaseEntry>> {
    let bytes = fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    serde_json::from_slice(&bytes)
        .with_context(|| format!("parsing GitHub releases JSON at {}", path.display()))
}

fn read_listing(path: &Path) -> Result<Vec<ListingObject>> {
    let bytes = fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    serde_json::from_slice(&bytes)
        .with_context(|| format!("parsing CDN listing JSON at {}", path.display()))
}

fn write_pretty<T: serde::Serialize>(dir: &Path, name: &str, value: &T) -> Result<()> {
    fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    let path = dir.join(name);
    let json = serde_json::to_vec_pretty(value).with_context(|| format!("serializing {name}"))?;
    fs::write(&path, json).with_context(|| format!("writing {}", path.display()))?;
    println!("wrote {}", path.display());
    Ok(())
}

/// `cdn-manifests`: build `latest.json` + `releases.json`.
pub fn run_manifests(
    releases_json: &Path,
    cdn_listing: &Path,
    cdn_base: &str,
    out_dir: &Path,
) -> Result<()> {
    let releases = read_releases(releases_json)?;
    let listing = read_listing(cdn_listing)?;

    let mirrored: BTreeSet<(String, String)> = listing
        .iter()
        .filter_map(|o| split_release_path(&o.path))
        .collect();

    let (list, latest) = build_manifests(releases, cdn_base, &mirrored);
    write_pretty(out_dir, "releases.json", &list)?;
    write_pretty(out_dir, "latest.json", &latest)?;
    Ok(())
}

/// `cdn-release-notes`: write one `<tag>.json` notes file per published release
/// into `out_dir`.
///
/// With `tag`, only that release is written, and the run fails if it is missing,
/// a draft, or otherwise unpublishable. Without it, every published release is
/// written (the backfill).
pub fn run_release_notes(releases_json: &Path, out_dir: &Path, tag: Option<&str>) -> Result<()> {
    let bytes =
        fs::read(releases_json).with_context(|| format!("reading {}", releases_json.display()))?;
    let releases: Vec<GithubRelease> = serde_json::from_slice(&bytes).with_context(|| {
        format!(
            "parsing GitHub releases JSON at {}",
            releases_json.display()
        )
    })?;

    let files: Vec<_> = match tag {
        Some(tag) => {
            let Some(release) = releases.iter().find(|r| r.tag_name == tag) else {
                bail!("release {tag} not found in {}", releases_json.display());
            };
            let Some(file) = release_notes(release) else {
                bail!("release {tag} is a draft, unpublished, or not a semver tag");
            };
            vec![file]
        }
        None => releases.iter().filter_map(release_notes).collect(),
    };

    for file in &files {
        write_pretty(out_dir, &format!("{}.json", file.tag), file)?;
    }
    println!("release notes: {} file(s) written", files.len());
    Ok(())
}

/// `cdn-reconcile-plan`: compute the CDN<->GitHub reconcile plan.
///
/// `sha256sums_dir` holds one `<tag>/SHA256SUMS` per downloaded release; each
/// expected artifact's hash is looked up there (absent for `.minisig`/`.sig`/
/// `SHA256SUMS`, which fall back to a size comparison).
pub fn run_reconcile_plan(
    releases_json: &Path,
    cdn_listing: &Path,
    sha256sums_dir: &Path,
    out_dir: &Path,
) -> Result<()> {
    let releases = read_releases(releases_json)?;
    let listing = read_listing(cdn_listing)?;

    let known_tags: BTreeSet<String> = releases.iter().map(|r| r.tag_name.clone()).collect();
    let public_tags: BTreeSet<String> = releases
        .iter()
        .filter(|r| !r.draft)
        .map(|r| r.tag_name.clone())
        .collect();

    let mut sums_cache: std::collections::BTreeMap<String, Option<String>> =
        std::collections::BTreeMap::new();

    let mut expected: Vec<ExpectedAsset> = Vec::new();
    for r in &releases {
        if r.draft {
            continue;
        }
        let sums = sums_cache.entry(r.tag_name.clone()).or_insert_with(|| {
            let p = sha256sums_dir.join(&r.tag_name).join("SHA256SUMS");
            fs::read_to_string(&p).ok()
        });
        for a in &r.assets {
            let sha256 = sums
                .as_deref()
                .and_then(|s| yerd_update::sha256_for(s, &a.name))
                .map(str::to_string);
            expected.push(ExpectedAsset {
                tag: r.tag_name.clone(),
                name: a.name.clone(),
                size: a.size,
                sha256,
            });
        }
    }

    let cdn: Vec<CdnObject> = listing
        .into_iter()
        .filter(|o| split_release_path(&o.path).is_some())
        .map(|o| CdnObject {
            path: o.path,
            size: o.size,
            checksum: o.checksum,
        })
        .collect();

    let plan = reconcile(&expected, &cdn, &known_tags, &public_tags);

    let out = PlanJson {
        to_upload: plan.to_upload.iter().map(action_json).collect(),
        to_update: plan.to_update.iter().map(action_json).collect(),
        to_delete: plan.to_delete,
    };
    write_pretty(out_dir, "plan.json", &out)?;

    println!(
        "plan: {} upload, {} update, {} delete",
        out.to_upload.len(),
        out.to_update.len(),
        out.to_delete.len()
    );
    Ok(())
}

#[derive(serde::Serialize)]
struct ActionJson {
    tag: String,
    name: String,
    path: String,
}

// The `to_*` field names are the JSON contract the workflow drives with jq.
#[derive(serde::Serialize)]
#[allow(clippy::struct_field_names)]
struct PlanJson {
    to_upload: Vec<ActionJson>,
    to_update: Vec<ActionJson>,
    to_delete: Vec<String>,
}

fn action_json(a: &ExpectedAsset) -> ActionJson {
    ActionJson {
        tag: a.tag.clone(),
        name: a.name.clone(),
        path: a.cdn_path(),
    }
}
