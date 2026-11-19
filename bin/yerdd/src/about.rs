//! Release notes and GitHub contributors for the GUI's About data.
//!
//! Release notes come from the CDN: `releases.json` gives the tag list, and
//! each release's categorised notes live at `release-notes/<tag>.json` (written
//! by the `cdn-mirror` CI step). Contributors come from the GitHub API. Both go
//! through `crate::network::downloader`, so they honour the outbound proxy.
//!
//! Each reply is served from an in-memory TTL cache. When a refresh fails, the
//! stale value is served; with nothing cached the reply is `ErrorCode::Internal`.

use std::time::{Duration, Instant};

use serde::Deserialize;
use yerd_ipc::{Contributor, ReleaseNoteItem, ReleaseNotesEntry, Response};
use yerd_php::Downloader;
use yerd_release_manifest::{
    earlier_tags, notes_summary, release_notes_path, NoteItem, ReleaseEntry, ReleaseNotesFile,
    RELEASE_NOTES_SCHEMA,
};

use crate::ipc_server::internal;
use crate::self_update::CDN_BASE;
use crate::state::DaemonState;

/// GitHub's contributor list for the Yerd repository, most contributions first.
pub(crate) const CONTRIBUTORS_URL: &str =
    "https://api.github.com/repos/forjedio/yerd/contributors?per_page=100";
/// How many earlier stable releases the notes reply carries.
pub(crate) const EARLIER_LIMIT: usize = 5;
/// How long a release-notes reply is served without a refresh.
pub(crate) const NOTES_TTL: Duration = Duration::from_secs(6 * 60 * 60);
/// How long a contributors reply is served without a refresh.
pub(crate) const CONTRIBUTORS_TTL: Duration = Duration::from_secs(24 * 60 * 60);

const CURRENT_TAG: &str = concat!("v", env!("CARGO_PKG_VERSION"));

/// The cached release-notes reply: the running release's notes (if published)
/// and the earlier stable releases, newest first.
pub type ReleaseNotesPayload = (Option<ReleaseNotesEntry>, Vec<ReleaseNotesEntry>);

#[derive(Deserialize)]
struct GhContributor {
    #[serde(default)]
    login: String,
    #[serde(default)]
    avatar_url: String,
    #[serde(default)]
    contributions: u64,
    #[serde(default, rename = "type")]
    kind: String,
}

fn is_bot(c: &GhContributor) -> bool {
    c.kind == "Bot" || c.login.ends_with("[bot]")
}

/// Decode GitHub's contributor list, keeping its order and dropping bots and
/// entries without a login.
fn parse_contributors(bytes: &[u8]) -> Result<Vec<Contributor>, String> {
    let raw: Vec<GhContributor> = serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
    Ok(raw
        .into_iter()
        .filter(|c| !c.login.is_empty() && !is_bot(c))
        .map(|c| Contributor {
            commits: u32::try_from(c.contributions).unwrap_or(u32::MAX),
            login: c.login,
            avatar_url: c.avatar_url,
        })
        .collect())
}

fn to_item(item: NoteItem) -> ReleaseNoteItem {
    ReleaseNoteItem {
        title: item.title,
        author: item.author,
        url: item.url,
    }
}

/// Convert a notes file to its wire entry, or `None` for an unknown schema.
fn to_entry(file: ReleaseNotesFile) -> Option<ReleaseNotesEntry> {
    if file.schema != RELEASE_NOTES_SCHEMA {
        return None;
    }
    let summary = notes_summary(&file);
    Some(ReleaseNotesEntry {
        version: file.version,
        tag: file.tag,
        published_at: file.published_at,
        prerelease: file.prerelease,
        summary,
        features: file.features.into_iter().map(to_item).collect(),
        fixes: file.fixes.into_iter().map(to_item).collect(),
        others: file.others.into_iter().map(to_item).collect(),
    })
}

fn is_fresh(at: Instant, now: Instant, ttl: Duration) -> bool {
    now.saturating_duration_since(at) < ttl
}

fn notes_response((current, earlier): ReleaseNotesPayload) -> Response {
    Response::ReleaseNotes {
        current: current.map(Box::new),
        earlier,
    }
}

fn contributors_response(contributors: Vec<Contributor>) -> Response {
    let total = u32::try_from(contributors.len()).unwrap_or(u32::MAX);
    Response::Contributors {
        contributors,
        total,
    }
}

async fn fetch_entry(dl: &dyn Downloader, tag: &str) -> Option<ReleaseNotesEntry> {
    let url = format!("{CDN_BASE}/{}", release_notes_path(tag));
    let bytes = match dl.download(&url).await {
        Ok(bytes) => bytes,
        Err(e) => {
            tracing::debug!(%tag, error = %e, "release notes file unavailable");
            return None;
        }
    };
    let file: ReleaseNotesFile = match serde_json::from_slice(&bytes) {
        Ok(file) => file,
        Err(e) => {
            tracing::debug!(%tag, error = %e, "release notes file is not valid");
            return None;
        }
    };
    to_entry(file)
}

async fn fetch_notes(dl: &dyn Downloader) -> Result<ReleaseNotesPayload, String> {
    let url = format!("{CDN_BASE}/releases.json");
    let bytes = dl.download(&url).await.map_err(|e| e.to_string())?;
    let releases: Vec<ReleaseEntry> =
        serde_json::from_slice(&bytes).map_err(|e| format!("invalid releases.json: {e}"))?;
    let current = fetch_entry(dl, CURRENT_TAG).await;
    let mut earlier = Vec::new();
    for tag in earlier_tags(&releases, CURRENT_TAG, EARLIER_LIMIT) {
        if let Some(entry) = fetch_entry(dl, &tag).await {
            earlier.push(entry);
        }
    }
    Ok((current, earlier))
}

/// Serve the release notes from the cache, refreshing it through `dl` once it
/// is older than [`NOTES_TTL`]. A refresh missing the running release's notes
/// is never cached: the stale payload is served instead, or the partial one
/// when nothing is cached, so the next call retries.
pub async fn release_notes_with(
    state: &DaemonState,
    dl: &dyn Downloader,
    now: Instant,
) -> Response {
    let cached = state.release_notes_cache.read().await.clone();
    if let Some((at, payload)) = &cached {
        if is_fresh(*at, now, NOTES_TTL) {
            return notes_response(payload.clone());
        }
    }
    match fetch_notes(dl).await {
        Ok(payload) if payload.0.is_none() => match cached {
            Some((_, old)) => notes_response(old),
            None => notes_response(payload),
        },
        Ok(payload) => {
            *state.release_notes_cache.write().await = Some((now, payload.clone()));
            notes_response(payload)
        }
        Err(e) => match cached {
            Some((_, payload)) => {
                tracing::debug!(error = %e, "release notes refresh failed, serving stale");
                notes_response(payload)
            }
            None => internal(format!("couldn't load release notes: {e}")),
        },
    }
}

/// Serve the contributors from the cache, refreshing it through `dl` once it is
/// older than [`CONTRIBUTORS_TTL`].
pub async fn contributors_with(state: &DaemonState, dl: &dyn Downloader, now: Instant) -> Response {
    let cached = state.contributors_cache.read().await.clone();
    if let Some((at, list)) = &cached {
        if is_fresh(*at, now, CONTRIBUTORS_TTL) {
            return contributors_response(list.clone());
        }
    }
    let fetched = match dl.download(CONTRIBUTORS_URL).await {
        Ok(bytes) => parse_contributors(&bytes),
        Err(e) => Err(e.to_string()),
    };
    match fetched {
        Ok(list) => {
            *state.contributors_cache.write().await = Some((now, list.clone()));
            contributors_response(list)
        }
        Err(e) => match cached {
            Some((_, list)) => {
                tracing::debug!(error = %e, "contributors refresh failed, serving stale");
                contributors_response(list)
            }
            None => internal(format!("couldn't load contributors: {e}")),
        },
    }
}

/// Handle `Request::ReleaseNotes` through the proxy-aware downloader.
pub async fn release_notes(state: &DaemonState) -> Response {
    let dl = crate::network::downloader(state).await;
    release_notes_with(state, &dl, Instant::now()).await
}

/// Handle `Request::Contributors` through the proxy-aware downloader.
pub async fn contributors(state: &DaemonState) -> Response {
    let dl = crate::network::downloader(state).await;
    contributors_with(state, &dl, Instant::now()).await
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;
    use crate::test_support::{state_in, UrlDl};

    struct Counting {
        inner: UrlDl,
        calls: AtomicUsize,
    }

    impl Counting {
        fn new(inner: UrlDl) -> Self {
            Self {
                inner,
                calls: AtomicUsize::new(0),
            }
        }

        fn calls(&self) -> usize {
            self.calls.load(Ordering::SeqCst)
        }
    }

    #[async_trait::async_trait]
    impl Downloader for Counting {
        async fn download(&self, url: &str) -> Result<Vec<u8>, yerd_php::DownloadError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.inner.download(url).await
        }
    }

    const EARLIER: [&str; 6] = ["v2.0.5", "v2.0.4", "v2.0.3", "v2.0.2", "v2.0.1", "v2.0.0"];

    fn releases_json() -> String {
        let mut tags = vec![CURRENT_TAG.to_owned(), "v2.0.6-rc.1".to_owned()];
        tags.extend(EARLIER.iter().map(|t| (*t).to_owned()));
        let entries: Vec<String> = tags
            .iter()
            .map(|t| format!(r#"{{"tag_name":"{t}","prerelease":false,"draft":false,"body":null,"assets":[]}}"#))
            .collect();
        format!("[{}]", entries.join(","))
    }

    fn notes_file(tag: &str, schema: u32) -> String {
        let version = tag.trim_start_matches('v');
        format!(
            r#"{{"schema":{schema},"tag":"{tag}","version":"{version}","prerelease":false,"published_at":"2026-07-25T15:49:58Z","features":[],"fixes":[{{"title":"Fix {tag}","author":"dev","url":"https://github.com/forjedio/yerd/pull/1"}}],"others":[]}}"#
        )
    }

    fn notes_url(tag: &str) -> String {
        format!("{CDN_BASE}/{}", release_notes_path(tag))
    }

    fn cdn(skip: &[&str]) -> UrlDl {
        let mut dl = UrlDl::default().with(&format!("{CDN_BASE}/releases.json"), &releases_json());
        for tag in std::iter::once(CURRENT_TAG).chain(EARLIER) {
            if !skip.contains(&tag) {
                dl = dl.with(&notes_url(tag), &notes_file(tag, 1));
            }
        }
        dl
    }

    fn notes(resp: Response) -> (Option<Box<ReleaseNotesEntry>>, Vec<ReleaseNotesEntry>) {
        match resp {
            Response::ReleaseNotes { current, earlier } => (current, earlier),
            other => panic!("expected ReleaseNotes, got {other:?}"),
        }
    }

    fn tags(entries: &[ReleaseNotesEntry]) -> Vec<&str> {
        entries.iter().map(|e| e.tag.as_str()).collect()
    }

    #[tokio::test]
    async fn serves_current_and_five_earlier_stable_releases() {
        let tmp = tempfile::tempdir().unwrap();
        let state = state_in(tmp.path());
        let (current, earlier) = notes(release_notes_with(&state, &cdn(&[]), Instant::now()).await);
        let current = current.unwrap();
        assert_eq!(current.tag, CURRENT_TAG);
        assert_eq!(current.summary, format!("Fix {CURRENT_TAG}"));
        assert_eq!(
            tags(&earlier),
            vec!["v2.0.5", "v2.0.4", "v2.0.3", "v2.0.2", "v2.0.1"]
        );
        assert!(earlier
            .iter()
            .all(|e| e.summary == format!("Fix {}", e.tag)));
        let first_fix = earlier.first().unwrap().fixes.first().unwrap();
        assert_eq!(first_fix.author.as_deref(), Some("dev"));
    }

    #[tokio::test]
    async fn missing_or_unknown_schema_files_are_skipped() {
        let tmp = tempfile::tempdir().unwrap();
        let state = state_in(tmp.path());
        let dl = cdn(&[CURRENT_TAG, "v2.0.4"]).with(&notes_url("v2.0.3"), &notes_file("v2.0.3", 2));
        let (current, earlier) = notes(release_notes_with(&state, &dl, Instant::now()).await);
        assert!(current.is_none());
        assert_eq!(tags(&earlier), vec!["v2.0.5", "v2.0.2", "v2.0.1"]);
    }

    #[tokio::test]
    async fn fresh_cache_skips_download_and_stale_cache_survives_failure() {
        let tmp = tempfile::tempdir().unwrap();
        let state = state_in(tmp.path());
        let start = Instant::now();
        let online = Counting::new(cdn(&[]));
        let first = release_notes_with(&state, &online, start).await;
        let calls = online.calls();
        assert!(calls > 0);

        let again = release_notes_with(&state, &online, start + NOTES_TTL / 2).await;
        assert_eq!(online.calls(), calls);
        assert_eq!(again, first);

        let offline = Counting::new(UrlDl::default());
        let served =
            release_notes_with(&state, &offline, start + NOTES_TTL + Duration::from_secs(1)).await;
        assert_eq!(offline.calls(), 1);
        assert_eq!(served, first);
    }

    #[tokio::test]
    async fn expired_cache_is_refreshed() {
        let tmp = tempfile::tempdir().unwrap();
        let state = state_in(tmp.path());
        let start = Instant::now();
        release_notes_with(&state, &cdn(&["v2.0.5"]), start).await;
        let later = start + NOTES_TTL + Duration::from_secs(1);
        let (_, earlier) = notes(release_notes_with(&state, &cdn(&[]), later).await);
        assert_eq!(earlier.first().unwrap().tag, "v2.0.5");
        assert_eq!(
            state.release_notes_cache.read().await.as_ref().unwrap().0,
            later
        );
    }

    #[tokio::test]
    async fn partial_refresh_keeps_stale_payload() {
        let tmp = tempfile::tempdir().unwrap();
        let state = state_in(tmp.path());
        let start = Instant::now();
        let first = release_notes_with(&state, &cdn(&[]), start).await;
        let later = start + NOTES_TTL + Duration::from_secs(1);
        let served = release_notes_with(&state, &cdn(&[CURRENT_TAG]), later).await;
        assert_eq!(served, first);
        assert_eq!(
            state.release_notes_cache.read().await.as_ref().unwrap().0,
            start
        );
    }

    #[tokio::test]
    async fn partial_refresh_without_cache_is_served_but_not_cached() {
        let tmp = tempfile::tempdir().unwrap();
        let state = state_in(tmp.path());
        let start = Instant::now();
        let (current, earlier) =
            notes(release_notes_with(&state, &cdn(&[CURRENT_TAG]), start).await);
        assert!(current.is_none());
        assert_eq!(earlier.len(), EARLIER_LIMIT);
        assert!(state.release_notes_cache.read().await.is_none());

        let online = Counting::new(cdn(&[]));
        let (current, _) = notes(release_notes_with(&state, &online, start).await);
        assert!(online.calls() > 0);
        assert_eq!(current.unwrap().tag, CURRENT_TAG);
    }

    #[tokio::test]
    async fn no_cache_and_failure_is_internal() {
        let tmp = tempfile::tempdir().unwrap();
        let state = state_in(tmp.path());
        let invalid = UrlDl::default().with(&format!("{CDN_BASE}/releases.json"), "not json");
        for dl in [UrlDl::default(), invalid] {
            let resp = release_notes_with(&state, &dl, Instant::now()).await;
            assert!(
                matches!(
                    resp,
                    Response::Error {
                        code: yerd_ipc::ErrorCode::Internal,
                        ..
                    }
                ),
                "{resp:?}"
            );
        }
        let resp = contributors_with(&state, &UrlDl::default(), Instant::now()).await;
        assert!(matches!(
            resp,
            Response::Error {
                code: yerd_ipc::ErrorCode::Internal,
                ..
            }
        ));
    }

    const CONTRIBUTORS: &str = r#"[
        {"login":"RichardAnderson","avatar_url":"https://a.test/1","contributions":240,"type":"User"},
        {"login":"dependabot[bot]","avatar_url":"https://a.test/2","contributions":50,"type":"Bot"},
        {"login":"totov","avatar_url":"https://a.test/3","contributions":12,"type":"User"},
        {"login":"renovate","avatar_url":"https://a.test/4","contributions":9,"type":"Bot"},
        {"login":"helper[bot]","avatar_url":"https://a.test/5","contributions":3,"type":"User"},
        {"login":"ralphschindler","avatar_url":"https://a.test/6","contributions":99999999999,"type":"User"}
    ]"#;

    #[test]
    fn parse_contributors_drops_bots_and_keeps_order() {
        let list = parse_contributors(CONTRIBUTORS.as_bytes()).unwrap();
        let logins: Vec<&str> = list.iter().map(|c| c.login.as_str()).collect();
        assert_eq!(logins, vec!["RichardAnderson", "totov", "ralphschindler"]);
        assert_eq!(list.first().unwrap().commits, 240);
        assert_eq!(list.first().unwrap().avatar_url, "https://a.test/1");
        assert_eq!(list.last().unwrap().commits, u32::MAX);
        assert!(parse_contributors(b"{}").is_err());
        assert!(parse_contributors(b"[]").unwrap().is_empty());
    }

    #[tokio::test]
    async fn contributors_total_counts_humans_and_caches() {
        let tmp = tempfile::tempdir().unwrap();
        let state = state_in(tmp.path());
        let start = Instant::now();
        let online = Counting::new(UrlDl::default().with(CONTRIBUTORS_URL, CONTRIBUTORS));
        let first = contributors_with(&state, &online, start).await;
        let Response::Contributors {
            contributors,
            total,
        } = &first
        else {
            panic!("expected Contributors, got {first:?}");
        };
        assert_eq!(*total, 3);
        assert_eq!(contributors.len(), 3);

        contributors_with(&state, &online, start + CONTRIBUTORS_TTL / 2).await;
        assert_eq!(online.calls(), 1);

        let offline = Counting::new(UrlDl::default());
        let later = start + CONTRIBUTORS_TTL + Duration::from_secs(1);
        assert_eq!(contributors_with(&state, &offline, later).await, first);
        assert_eq!(offline.calls(), 1);
    }
}
