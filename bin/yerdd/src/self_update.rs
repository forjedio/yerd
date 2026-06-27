//! Yerd self-update version checking (notify-only, Phase A).
//!
//! The daemon polls the GitHub Releases API for `forjedio/yerd`, caches the
//! parsed releases (`DaemonState::yerd_update`), and answers `CheckUpdate` by
//! running the pure [`yerd_update::select_target`] decision over them. Like the
//! PHP checker this is **notify-only**: it never installs anything (apply is a
//! CLI/GUI-initiated, interactively-elevated path - see the feature plan).
//!
//! Network failure is tolerated: the periodic poll leaves the cache untouched,
//! and `CheckUpdate` falls back to the cache with [`UpdateSource::Cached`].

use serde::Deserialize;

use yerd_ipc::{Response, StagedArtifact, UpdateSource};
use yerd_php::Downloader;
use yerd_platform::PlatformDirs;
use yerd_update::{
    select_asset, select_target, verify_minisign, verify_sha256, ArtifactKind, Asset, Channel,
    PkgFormat, Platform, ReleaseMeta,
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
/// crate version is ever not valid semver (it always is - the workspace pins it).
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

/// Build the `UpdateStatus` reply from a decision + freshness + timestamp.
fn status_response(
    decision: &yerd_update::UpdateDecision,
    source: UpdateSource,
    checked_at_epoch: Option<u64>,
) -> Response {
    Response::UpdateStatus {
        current: decision.current.to_string(),
        latest_stable: decision.latest_stable.as_ref().map(ToString::to_string),
        latest_edge: decision.latest_edge.as_ref().map(ToString::to_string),
        channel: to_ipc(decision.channel),
        available: decision.available,
        target: decision.target.as_ref().map(ToString::to_string),
        ahead_of_stable: decision.ahead_of_stable,
        source,
        checked_at_epoch,
    }
}

/// A durable snapshot of the last successful update check, persisted to
/// `{state}/update-check.json` so the UI can pre-fill the Updates section on load
/// (and across daemon restarts / while offline) and show a "last checked …"
/// time. Mirrors the [`Response::UpdateStatus`] display fields plus the
/// timestamp. Lives in the daemon's *cache*, not `yerd.toml` - it is regenerable.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct UpdateSnapshot {
    /// Unix epoch (seconds) when the check completed.
    pub checked_at: u64,
    /// The running Yerd version at check time.
    pub current: String,
    /// Highest stable version seen, if any.
    pub latest_stable: Option<String>,
    /// Highest edge (pre-release-inclusive) version seen, if any.
    pub latest_edge: Option<String>,
    /// Channel the decision resolved against.
    pub channel: yerd_ipc::Channel,
    /// Whether a newer version was available on `channel`.
    pub available: bool,
    /// The version `channel` would update to, if newer.
    pub target: Option<String>,
    /// True when the running version was a pre-release ahead of latest stable.
    pub ahead_of_stable: bool,
}

/// Current wall-clock as Unix epoch seconds (`0` if the clock is before the
/// epoch, which never happens in practice).
fn now_epoch() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

fn snapshot_path(dirs: &PlatformDirs) -> std::path::PathBuf {
    dirs.state.join("update-check.json")
}

/// Read the persisted snapshot, if present and parseable. Best-effort: any I/O
/// or decode error yields `None` (treated as "never checked").
pub fn load_snapshot(dirs: &PlatformDirs) -> Option<UpdateSnapshot> {
    let bytes = std::fs::read(snapshot_path(dirs)).ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// Write the snapshot to disk. Best-effort: a failure is logged at `debug` and
/// otherwise ignored (the in-memory copy still serves this session).
fn persist_snapshot(dirs: &PlatformDirs, snap: &UpdateSnapshot) {
    let path = snapshot_path(dirs);
    let write = || -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let json = serde_json::to_vec_pretty(snap).map_err(std::io::Error::other)?;
        std::fs::write(&path, json)
    };
    if let Err(e) = write() {
        tracing::debug!(error = %e, path = %path.display(), "could not persist update-check cache");
    }
}

/// Build a snapshot from a fresh decision, stamped at `checked_at`.
fn snapshot_from(decision: &yerd_update::UpdateDecision, checked_at: u64) -> UpdateSnapshot {
    UpdateSnapshot {
        checked_at,
        current: decision.current.to_string(),
        latest_stable: decision.latest_stable.as_ref().map(ToString::to_string),
        latest_edge: decision.latest_edge.as_ref().map(ToString::to_string),
        channel: to_ipc(decision.channel),
        available: decision.available,
        target: decision.target.as_ref().map(ToString::to_string),
        ahead_of_stable: decision.ahead_of_stable,
    }
}

/// Build an `UpdateStatus` reply from a persisted snapshot, reconciled against
/// the `effective` channel the caller is answering for.
fn response_from_snapshot(
    snap: &UpdateSnapshot,
    effective: yerd_ipc::Channel,
    source: UpdateSource,
) -> Response {
    let running = current_version().to_string();
    let drifted = snap.current != running || snap.channel != effective;
    Response::UpdateStatus {
        current: running,
        latest_stable: snap.latest_stable.clone(),
        latest_edge: snap.latest_edge.clone(),
        channel: effective,
        available: !drifted && snap.available,
        target: if drifted { None } else { snap.target.clone() },
        ahead_of_stable: !drifted && snap.ahead_of_stable,
        source,
        checked_at_epoch: Some(snap.checked_at),
    }
}

/// Persist a fresh snapshot to disk and store it in `state` for this session.
async fn store_snapshot(state: &DaemonState, snap: UpdateSnapshot) {
    persist_snapshot(&state.dirs, &snap);
    *state.update_snapshot.write().await = Some(snap);
}

/// `CachedUpdateStatus` handler: return the last persisted result without any
/// network access (for pre-filling the UI on load). When nothing was ever
/// checked, report the running version with no remote figures.
pub async fn cached_update_status(state: &DaemonState) -> Response {
    if let Some(snap) = state.update_snapshot.read().await.clone() {
        let effective = to_ipc(configured_channel(state).await);
        return response_from_snapshot(&snap, effective, UpdateSource::Cached);
    }
    Response::UpdateStatus {
        current: current_version().to_string(),
        latest_stable: None,
        latest_edge: None,
        channel: to_ipc(configured_channel(state).await),
        available: false,
        target: None,
        ahead_of_stable: false,
        source: UpdateSource::Cached,
        checked_at_epoch: None,
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
    store_snapshot(state, snapshot_from(&decision, now_epoch())).await;
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
        let snap = snapshot_from(&decision, now_epoch());
        store_snapshot(state, snap.clone()).await;
        response_from_snapshot(&snap, to_ipc(channel), UpdateSource::Live)
    } else if let Some(snap) = state.update_snapshot.read().await.clone() {
        response_from_snapshot(&snap, to_ipc(channel), UpdateSource::Cached)
    } else {
        let cache = state.yerd_update.read().await;
        let decision = select_target(&cache, channel, &current);
        status_response(&decision, UpdateSource::Cached, None)
    }
}

/// `StageUpdate` handler: resolve the target on `channel`, download its
/// artifact + signature + checksums, verify (SHA-256 against `SHA256SUMS` and a
/// minisign signature against `public_key`), and write the verified artifact
/// into the cache dir. Returns [`Response::Staged`] with the on-disk path.
///
/// `public_key` is [`yerd_update::UPDATE_PUBLIC_KEY`] in production and a test
/// key in unit tests. The privileged install/swap is the applier's job, not the
/// daemon's - this only produces a verified local file.
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
    // `PkgFormat::current()` is the build-time deb-vs-pacman tiebreak: a release
    // carries both Linux artifacts and only the format this binary was packaged
    // for is installable here (see `yerd_update::PkgFormat`).
    let sel = match select_asset(target_rel, Platform::current(), PkgFormat::current()) {
        Ok(s) => s,
        Err(e) => return internal(format!("no installable artifact: {e}")),
    };

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

    if let Err(e) = verify_sha256(&artifact, &sums, &sel.artifact.name) {
        return internal(format!("checksum verification failed: {e}"));
    }
    if let Err(e) = verify_minisign(public_key, &sig, &artifact) {
        return internal(format!("signature verification failed: {e}"));
    }

    if !is_safe_filename(&sel.artifact.name) {
        return internal(format!("unsafe asset filename: {:?}", sel.artifact.name));
    }

    let dir = state.dirs.cache.join("update");
    if let Err(e) = tokio::fs::create_dir_all(&dir).await {
        return internal(format!("could not create staging dir: {e}"));
    }
    let path = dir.join(&sel.artifact.name);
    if let Err(e) = tokio::fs::write(&path, &artifact).await {
        return internal(format!("could not write staged artifact: {e}"));
    }
    let sig_path = dir.join(format!("{}.sig", sel.artifact.name));
    if let Err(e) = tokio::fs::write(&sig_path, sig.as_bytes()).await {
        return internal(format!("could not write staged signature: {e}"));
    }

    let kind = match sel.kind {
        ArtifactKind::AppTarGz => StagedArtifact::AppTarGz,
        ArtifactKind::Deb => StagedArtifact::Deb,
        ArtifactKind::Pacman => StagedArtifact::Pacman,
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

/// True if `name` is a single normal path component - no separators, `..`, root,
/// or drive prefix - so joining it onto a directory can't escape that directory.
fn is_safe_filename(name: &str) -> bool {
    use std::path::Component;
    let mut comps = std::path::Path::new(name).components();
    matches!(
        (comps.next(), comps.next()),
        (Some(Component::Normal(_)), None)
    )
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
        assert_ne!(current_version(), semver::Version::new(0, 0, 0));
    }

    #[test]
    fn safe_filename_accepts_plain_names_rejects_traversal() {
        assert!(is_safe_filename("Yerd_Linux_x86_64_v2-0-2.deb"));
        assert!(is_safe_filename("SHA256SUMS"));
        assert!(!is_safe_filename(""));
        assert!(!is_safe_filename("../evil"));
        assert!(!is_safe_filename("a/b"));
        assert!(!is_safe_filename("/etc/passwd"));
        assert!(!is_safe_filename(".."));
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
        match status_response(&decision, UpdateSource::Live, Some(1_719_445_200)) {
            Response::UpdateStatus {
                current,
                latest_stable,
                latest_edge,
                channel,
                available,
                target,
                ahead_of_stable,
                source,
                checked_at_epoch,
            } => {
                assert_eq!(current, "2.0.0");
                assert_eq!(latest_stable.as_deref(), Some("2.0.5"));
                assert_eq!(latest_edge.as_deref(), Some("2.1.0-rc.1"));
                assert_eq!(channel, yerd_ipc::Channel::Stable);
                assert!(available);
                assert_eq!(target.as_deref(), Some("2.0.5"));
                assert!(!ahead_of_stable);
                assert_eq!(source, UpdateSource::Live);
                assert_eq!(checked_at_epoch, Some(1_719_445_200));
            }
            other => panic!("expected UpdateStatus, got {other:?}"),
        }
    }

    #[test]
    fn snapshot_response_suppresses_stale_decision_after_version_drift() {
        let snap = UpdateSnapshot {
            checked_at: 1_719_445_200,
            current: "0.0.1".into(),
            latest_stable: Some("2.0.5".into()),
            latest_edge: Some("2.1.0-rc.1".into()),
            channel: yerd_ipc::Channel::Stable,
            available: true,
            target: Some("9.9.9".into()),
            ahead_of_stable: true,
        };
        match response_from_snapshot(&snap, yerd_ipc::Channel::Stable, UpdateSource::Cached) {
            Response::UpdateStatus {
                current,
                available,
                target,
                ahead_of_stable,
                latest_stable,
                checked_at_epoch,
                ..
            } => {
                assert_eq!(current, current_version().to_string());
                assert!(!available);
                assert_eq!(target, None);
                assert!(!ahead_of_stable);
                assert_eq!(latest_stable.as_deref(), Some("2.0.5"));
                assert_eq!(checked_at_epoch, Some(1_719_445_200));
            }
            other => panic!("expected UpdateStatus, got {other:?}"),
        }
    }

    #[test]
    fn snapshot_response_preserves_decision_when_version_matches() {
        let snap = UpdateSnapshot {
            checked_at: 1_719_445_200,
            current: current_version().to_string(),
            latest_stable: Some("99.0.0".into()),
            latest_edge: Some("99.0.0".into()),
            channel: yerd_ipc::Channel::Stable,
            available: true,
            target: Some("99.0.0".into()),
            ahead_of_stable: false,
        };
        match response_from_snapshot(&snap, yerd_ipc::Channel::Stable, UpdateSource::Cached) {
            Response::UpdateStatus {
                available, target, ..
            } => {
                assert!(available);
                assert_eq!(target.as_deref(), Some("99.0.0"));
            }
            other => panic!("expected UpdateStatus, got {other:?}"),
        }
    }

    #[test]
    fn snapshot_response_suppresses_stale_decision_after_channel_switch() {
        let snap = UpdateSnapshot {
            checked_at: 1_719_445_200,
            current: current_version().to_string(),
            latest_stable: Some("2.0.5".into()),
            latest_edge: Some("2.1.0-rc.1".into()),
            channel: yerd_ipc::Channel::Stable,
            available: true,
            target: Some("2.1.0-rc.1".into()),
            ahead_of_stable: true,
        };
        match response_from_snapshot(&snap, yerd_ipc::Channel::Edge, UpdateSource::Cached) {
            Response::UpdateStatus {
                channel,
                available,
                target,
                ahead_of_stable,
                latest_edge,
                checked_at_epoch,
                ..
            } => {
                assert_eq!(channel, yerd_ipc::Channel::Edge);
                assert!(!available);
                assert_eq!(target, None);
                assert!(!ahead_of_stable);
                assert_eq!(latest_edge.as_deref(), Some("2.1.0-rc.1"));
                assert_eq!(checked_at_epoch, Some(1_719_445_200));
            }
            other => panic!("expected UpdateStatus, got {other:?}"),
        }
    }
}
