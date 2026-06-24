//! Yerd self-update version checking (notify-only, Phase A).
//!
//! The daemon polls the GitHub Releases API for `forjedio/yerd`, caches the
//! parsed releases (`DaemonState::yerd_update`), and answers `CheckUpdate` by
//! running the pure [`yerd_update::select_target`] decision over them. Like the
//! PHP checker this is **notify-only**: it never installs anything (apply is a
//! CLI/GUI-initiated, interactively-elevated path — see the feature plan).
//!
//! Network failure is tolerated: the periodic poll leaves the cache untouched,
//! and `CheckUpdate` falls back to the cache with [`UpdateSource::Cached`].

use serde::Deserialize;

use yerd_ipc::{Response, StagedArtifact, UpdateSource};
use yerd_php::Downloader;
use yerd_update::{
    select_asset, select_target, verify_minisign, verify_sha256, ArtifactKind, Asset, Channel,
    Platform, ReleaseMeta,
};

use crate::ipc_server::internal;
use crate::state::DaemonState;

/// GitHub repository the release artifacts are published under.
const GITHUB_OWNER: &str = "forjedio";
const GITHUB_REPO: &str = "yerd";
/// Releases per page (GitHub max is 100). One page covers stable + edge for any
/// realistic release history; [`MAX_PAGES`] bounds the walk defensively.
const PER_PAGE: usize = 100;
/// Hard cap on pages walked, so a misbehaving API can never loop unbounded.
const MAX_PAGES: u32 = 3;

/// The running daemon version (compile-time). Falls back to `0.0.0` if the
/// crate version is ever not valid semver (it always is — the workspace pins it).
fn current_version() -> semver::Version {
    semver::Version::parse(env!("CARGO_PKG_VERSION"))
        .unwrap_or_else(|_| semver::Version::new(0, 0, 0))
}

/// One GitHub release, as far as we parse it.
#[derive(Deserialize)]
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

/// One asset attached to a GitHub release.
#[derive(Deserialize)]
struct GhAsset {
    name: String,
    browser_download_url: String,
    #[serde(default)]
    size: u64,
}

/// Fetch and parse the release list from GitHub. Returns `None` on any network
/// or decode failure (caller falls back to the cache). Drafts and releases whose
/// tag is not valid semver are skipped.
async fn fetch_releases(dl: &dyn Downloader) -> Option<Vec<ReleaseMeta>> {
    let mut out: Vec<ReleaseMeta> = Vec::new();
    for page in 1..=MAX_PAGES {
        let url = format!(
            "https://api.github.com/repos/{GITHUB_OWNER}/{GITHUB_REPO}/releases?per_page={PER_PAGE}&page={page}"
        );
        let bytes = match dl.download(&url).await {
            Ok(b) => b,
            Err(e) => {
                // A failure on the first page means we have nothing; later pages
                // failing just truncate the (already useful) accumulated list.
                if page == 1 {
                    tracing::debug!(error = %e, "yerd self-update: releases fetch failed");
                    return None;
                }
                tracing::debug!(error = %e, page, "yerd self-update: stopping pagination early");
                break;
            }
        };
        let releases: Vec<GhRelease> = match serde_json::from_slice(&bytes) {
            Ok(r) => r,
            Err(e) => {
                if page == 1 {
                    tracing::debug!(error = %e, "yerd self-update: releases decode failed");
                    return None;
                }
                break;
            }
        };
        let count = releases.len();
        for r in releases {
            if r.draft {
                continue;
            }
            let Some(version) = yerd_update::parse_tag(&r.tag_name) else {
                continue;
            };
            out.push(ReleaseMeta {
                version,
                tag: r.tag_name,
                prerelease: r.prerelease,
                assets: r
                    .assets
                    .into_iter()
                    .map(|a| Asset {
                        name: a.name,
                        url: a.browser_download_url,
                        size: a.size,
                    })
                    .collect(),
                notes: r.body,
            });
        }
        // A short page is the last page.
        if count < PER_PAGE {
            break;
        }
    }
    Some(out)
}

/// The effective channel from persisted config (defaulting to stable).
async fn configured_channel(state: &DaemonState) -> Channel {
    let s = state.config.lock().await.update_channel.clone();
    Channel::parse(&s).unwrap_or_default()
}

/// Map the wire channel to the decision-logic channel. The wire enum is
/// `#[non_exhaustive]`; an unknown future value is treated as stable.
fn from_ipc(c: yerd_ipc::Channel) -> Channel {
    match c {
        yerd_ipc::Channel::Edge => Channel::Edge,
        _ => Channel::Stable,
    }
}

/// Map the decision-logic channel back to the wire channel.
fn to_ipc(c: Channel) -> yerd_ipc::Channel {
    match c {
        Channel::Stable => yerd_ipc::Channel::Stable,
        Channel::Edge => yerd_ipc::Channel::Edge,
    }
}

/// Build the `UpdateStatus` reply from a decision + freshness.
fn status_response(decision: &yerd_update::UpdateDecision, source: UpdateSource) -> Response {
    Response::UpdateStatus {
        current: decision.current.to_string(),
        latest_stable: decision.latest_stable.as_ref().map(ToString::to_string),
        latest_edge: decision.latest_edge.as_ref().map(ToString::to_string),
        channel: to_ipc(decision.channel),
        available: decision.available,
        target: decision.target.as_ref().map(ToString::to_string),
        ahead_of_stable: decision.ahead_of_stable,
        source,
    }
}

/// Poll GitHub once and refresh `state.yerd_update`. **Failure-tolerant**: a
/// fetch error logs at `debug` and leaves the cache untouched. Notify-only: logs
/// (does not install) when a newer version is available on the configured
/// channel. Run at startup and every 12h alongside the PHP checker.
pub async fn poll_and_refresh(state: &DaemonState, dl: &dyn Downloader) {
    let Some(releases) = fetch_releases(dl).await else {
        return;
    };
    let channel = configured_channel(state).await;
    let decision = select_target(&releases, channel, &current_version());
    if let Some(target) = &decision.target {
        tracing::info!(
            current = %decision.current,
            latest = %target,
            channel = %channel,
            "a newer Yerd version is available (run `yerd update`)"
        );
    }
    *state.yerd_update.write().await = releases;
}

/// `CheckUpdate` handler: do a live fetch (refreshing the cache) and report; on
/// fetch failure, serve the cache marked [`UpdateSource::Cached`]. `channel`
/// overrides the configured preference for this check only.
pub async fn check_update(
    channel_override: Option<yerd_ipc::Channel>,
    state: &DaemonState,
    dl: &dyn Downloader,
) -> Response {
    let current = current_version();
    let channel = match channel_override {
        Some(c) => from_ipc(c),
        None => configured_channel(state).await,
    };
    if let Some(releases) = fetch_releases(dl).await {
        let decision = select_target(&releases, channel, &current);
        *state.yerd_update.write().await = releases;
        status_response(&decision, UpdateSource::Live)
    } else {
        let cache = state.yerd_update.read().await;
        let decision = select_target(&cache, channel, &current);
        status_response(&decision, UpdateSource::Cached)
    }
}

/// `StageUpdate` handler: resolve the target on `channel`, download its
/// artifact + signature + checksums, verify (SHA-256 against `SHA256SUMS` and a
/// minisign signature against `public_key`), and write the verified artifact
/// into the cache dir. Returns [`Response::Staged`] with the on-disk path.
///
/// `public_key` is [`yerd_update::UPDATE_PUBLIC_KEY`] in production and a test
/// key in unit tests. The privileged install/swap is the applier's job, not the
/// daemon's — this only produces a verified local file.
pub async fn stage_update(
    channel_override: Option<yerd_ipc::Channel>,
    state: &DaemonState,
    dl: &dyn Downloader,
    public_key: &str,
) -> Response {
    let current = current_version();
    let channel = match channel_override {
        Some(c) => from_ipc(c),
        None => configured_channel(state).await,
    };

    let Some(releases) = fetch_releases(dl).await else {
        return internal("could not fetch releases (offline or rate-limited)".to_owned());
    };
    let decision = select_target(&releases, channel, &current);
    let Some(target_ver) = decision.target.clone() else {
        return internal("already up to date — nothing to stage".to_owned());
    };
    let Some(target_rel) = releases.iter().find(|r| r.version == target_ver) else {
        return internal("internal: resolved target release vanished".to_owned());
    };
    let sel = match select_asset(target_rel, Platform::current()) {
        Ok(s) => s,
        Err(e) => return internal(format!("no installable artifact: {e}")),
    };

    // Download artifact + detached signature + checksum manifest.
    let artifact = match dl.download(&sel.artifact.url).await {
        Ok(b) => b,
        Err(e) => return internal(format!("artifact download failed: {e}")),
    };
    let sig = match dl.download(&sel.signature.url).await {
        Ok(b) => String::from_utf8_lossy(&b).into_owned(),
        Err(e) => return internal(format!("signature download failed: {e}")),
    };
    let sums = match dl.download(&sel.checksums.url).await {
        Ok(b) => String::from_utf8_lossy(&b).into_owned(),
        Err(e) => return internal(format!("checksums download failed: {e}")),
    };

    // Verify before writing anything to disk.
    if let Err(e) = verify_sha256(&artifact, &sums, &sel.artifact.name) {
        return internal(format!("checksum verification failed: {e}"));
    }
    if let Err(e) = verify_minisign(public_key, &sig, &artifact) {
        return internal(format!("signature verification failed: {e}"));
    }

    // Write the verified artifact into the cache dir.
    let dir = state.dirs.cache.join("update");
    if let Err(e) = tokio::fs::create_dir_all(&dir).await {
        return internal(format!("could not create staging dir: {e}"));
    }
    let path = dir.join(&sel.artifact.name);
    if let Err(e) = tokio::fs::write(&path, &artifact).await {
        return internal(format!("could not write staged artifact: {e}"));
    }
    // Persist the detached signature beside it so the applier can re-verify
    // (closes the daemon-verify → applier-swap TOCTOU window).
    let sig_path = dir.join(format!("{}.sig", sel.artifact.name));
    if let Err(e) = tokio::fs::write(&sig_path, sig.as_bytes()).await {
        return internal(format!("could not write staged signature: {e}"));
    }

    let kind = match sel.kind {
        ArtifactKind::AppTarGz => StagedArtifact::AppTarGz,
        ArtifactKind::Deb => StagedArtifact::Deb,
    };
    tracing::info!(version = %target_ver, path = %path.display(), "staged verified update artifact");
    Response::Staged {
        path: path.to_string_lossy().into_owned(),
        version: target_ver.to_string(),
        kind,
    }
}

/// `SetUpdateChannel` handler: persist the channel preference. Mirrors the
/// established build → validate → save → commit set-pattern.
pub async fn set_update_channel(channel: yerd_ipc::Channel, state: &DaemonState) -> Response {
    let value = from_ipc(channel).as_str().to_owned();
    let mut cfg_guard = state.config.lock().await;
    let mut new = cfg_guard.clone();
    new.update_channel.clone_from(&value);
    if let Err(e) = new.validate() {
        return internal(format!("config validation failed: {e}"));
    }
    if let Err(e) = new.save(&state.config_path) {
        return internal(format!("config save failed: {e}"));
    }
    *cfg_guard = new;
    tracing::info!(channel = %value, "set update channel");
    Response::Ok
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn channel_wire_mapping_round_trips() {
        assert_eq!(from_ipc(yerd_ipc::Channel::Stable), Channel::Stable);
        assert_eq!(from_ipc(yerd_ipc::Channel::Edge), Channel::Edge);
        assert_eq!(to_ipc(Channel::Stable), yerd_ipc::Channel::Stable);
        assert_eq!(to_ipc(Channel::Edge), yerd_ipc::Channel::Edge);
    }

    #[test]
    fn current_version_parses() {
        // The crate version is always valid semver, so this is never the fallback.
        assert_ne!(current_version(), semver::Version::new(0, 0, 0));
    }

    #[test]
    fn status_response_maps_decision_fields() {
        let decision = yerd_update::UpdateDecision {
            current: semver::Version::parse("2.0.0").unwrap(),
            latest_stable: Some(semver::Version::parse("2.0.5").unwrap()),
            latest_edge: Some(semver::Version::parse("2.1.0-rc.1").unwrap()),
            channel: Channel::Stable,
            target: Some(semver::Version::parse("2.0.5").unwrap()),
            available: true,
            ahead_of_stable: false,
        };
        match status_response(&decision, UpdateSource::Live) {
            Response::UpdateStatus {
                current,
                latest_stable,
                latest_edge,
                channel,
                available,
                target,
                ahead_of_stable,
                source,
            } => {
                assert_eq!(current, "2.0.0");
                assert_eq!(latest_stable.as_deref(), Some("2.0.5"));
                assert_eq!(latest_edge.as_deref(), Some("2.1.0-rc.1"));
                assert_eq!(channel, yerd_ipc::Channel::Stable);
                assert!(available);
                assert_eq!(target.as_deref(), Some("2.0.5"));
                assert!(!ahead_of_stable);
                assert_eq!(source, UpdateSource::Live);
            }
            other => panic!("expected UpdateStatus, got {other:?}"),
        }
    }
}
