//! `WordPress` core version availability, sourced from the hand-maintained
//! `meta/wordpress-versions.json` in the yerd repo and cached for
//! [`CACHE_TTL`] - the file changes only when someone edits it, so there is
//! no reason to hit the network on every wizard open.
//!
//! This deliberately does not call wordpress.org: its `version-check` API
//! only exposes a minimum PHP floor with no upper bound, which makes very old
//! `WordPress` branches look compatible with brand-new PHP releases they were
//! never tested against. The meta file carries a real `min_php`/`max_php`
//! range per branch instead.

use std::time::{Duration, Instant};

use yerd_core::PhpVersion;
use yerd_ipc::{Response, WordPressVersionInfo};
use yerd_php::Downloader;

use crate::state::DaemonState;

/// How long a fetched version list stays fresh before the next request
/// triggers a re-fetch.
const CACHE_TTL: Duration = Duration::from_secs(12 * 60 * 60);

/// GitHub repository the meta file is published under.
const GITHUB_OWNER: &str = "forjedio";
const GITHUB_REPO: &str = "yerd";
/// Branch the meta file is read from, independent of whatever branch this
/// daemon binary was itself built from - it's the published, canonical copy.
const GITHUB_BRANCH: &str = "main";

fn meta_url() -> String {
    format!(
        "https://raw.githubusercontent.com/{GITHUB_OWNER}/{GITHUB_REPO}/{GITHUB_BRANCH}/meta/wordpress-versions.json"
    )
}

/// `Request::AvailableWordpressVersions` handler: serve the cache if it's
/// still fresh, else fetch, parse, and repopulate it. Failure-tolerant: a
/// fetch error falls back to the last-known-good cache (however old), and
/// only produces an empty response if nothing has ever been fetched.
pub async fn available_versions(state: &DaemonState, dl: &dyn Downloader) -> Response {
    {
        let cache = state.wordpress_versions.read().await;
        if let Some((fetched_at, versions)) = cache.as_ref() {
            if fetched_at.elapsed() < CACHE_TTL {
                return Response::WordpressVersions {
                    versions: versions.clone(),
                };
            }
        }
    }

    match fetch_and_parse(dl).await {
        Ok(versions) => {
            *state.wordpress_versions.write().await = Some((Instant::now(), versions.clone()));
            Response::WordpressVersions { versions }
        }
        Err(e) => {
            let cache = state.wordpress_versions.read().await;
            if let Some((_, versions)) = cache.as_ref() {
                tracing::debug!(error = %e, "wordpress meta fetch failed, serving stale cache");
                Response::WordpressVersions {
                    versions: versions.clone(),
                }
            } else {
                tracing::debug!(error = %e, "wordpress meta fetch failed, no cache to fall back to");
                Response::WordpressVersions { versions: vec![] }
            }
        }
    }
}

async fn fetch_and_parse(dl: &dyn Downloader) -> Result<Vec<WordPressVersionInfo>, String> {
    let bytes = dl.download(&meta_url()).await.map_err(|e| e.to_string())?;
    let body = String::from_utf8_lossy(&bytes);
    parse_meta(&body)
}

/// Raw shape of `meta/wordpress-versions.json`.
#[derive(serde::Deserialize)]
struct MetaFile {
    versions: Vec<MetaEntry>,
}

#[derive(serde::Deserialize)]
struct MetaEntry {
    branch: String,
    latest: String,
    min_php: String,
    max_php: String,
}

/// Pure - parses the meta file's JSON body into wire entries, in the file's
/// own order (already curated newest-first). An entry whose `min_php`/
/// `max_php` doesn't parse as a `PhpVersion` is skipped rather than failing
/// the whole fetch, so one bad hand-edited line doesn't blank the dropdown.
fn parse_meta(body: &str) -> Result<Vec<WordPressVersionInfo>, String> {
    let meta: MetaFile = serde_json::from_str(body).map_err(|e| e.to_string())?;
    Ok(meta
        .versions
        .into_iter()
        .filter_map(|e| {
            Some(WordPressVersionInfo {
                branch: e.branch,
                latest: e.latest,
                min_php: e.min_php.parse::<PhpVersion>().ok()?,
                max_php: e.max_php.parse::<PhpVersion>().ok()?,
            })
        })
        .collect())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing, clippy::panic)]
mod tests {
    use super::*;

    const SAMPLE_BODY: &str = r#"{
        "source": "https://make.wordpress.org/core/handbook/references/php-compatibility-and-wordpress-versions/",
        "updated": "2026-07-07",
        "versions": [
            { "branch": "7.0", "latest": "7.0", "min_php": "7.4", "max_php": "8.5" },
            { "branch": "6.9", "latest": "6.9.4", "min_php": "7.3", "max_php": "8.5" },
            { "branch": "6.2", "latest": "6.2.9", "min_php": "5.6", "max_php": "8.2" }
        ]
    }"#;

    #[test]
    fn parse_meta_extracts_in_file_order() {
        let versions = parse_meta(SAMPLE_BODY).unwrap();
        assert_eq!(versions.len(), 3);
        assert_eq!(versions[0].branch, "7.0");
        assert_eq!(versions[0].latest, "7.0");
        assert_eq!(versions[0].min_php, PhpVersion::new(7, 4));
        assert_eq!(versions[0].max_php, PhpVersion::new(8, 5));
        assert_eq!(versions[2].branch, "6.2");
        assert_eq!(versions[2].min_php, PhpVersion::new(5, 6));
        assert_eq!(versions[2].max_php, PhpVersion::new(8, 2));
    }

    #[test]
    fn parse_meta_skips_entries_with_unparseable_php_versions() {
        let body = r#"{"versions": [
            { "branch": "6.7", "latest": "6.7.5", "min_php": "not-a-version", "max_php": "8.4" },
            { "branch": "6.2", "latest": "6.2.9", "min_php": "5.6", "max_php": "8.2" }
        ]}"#;
        let versions = parse_meta(body).unwrap();
        assert_eq!(versions.len(), 1);
        assert_eq!(versions[0].branch, "6.2");
    }

    #[test]
    fn parse_meta_rejects_malformed_json() {
        assert!(parse_meta("not json").is_err());
    }

    #[test]
    fn parse_meta_rejects_missing_versions_array() {
        assert!(parse_meta(r#"{"source": "x"}"#).is_err());
    }

    #[test]
    fn parse_meta_empty_versions_is_ok() {
        let versions = parse_meta(r#"{"versions": []}"#).unwrap();
        assert!(versions.is_empty());
    }

    // ── available_versions: cache / stale-fallback behaviour ────────────────

    use std::sync::Arc;

    use tokio::sync::{Mutex, RwLock as TokioRwLock};
    use yerd_core::{RouterConfig, SiteRouter, Tld};
    use yerd_platform::PlatformDirs;

    fn dirs_in(tmp: &std::path::Path) -> PlatformDirs {
        PlatformDirs {
            config: tmp.join("c"),
            data: tmp.join("d"),
            state: tmp.join("s"),
            cache: tmp.join("ca"),
            runtime: tmp.join("r"),
        }
    }

    /// Copied verbatim from other modules' test suites (`DaemonState` has no
    /// `Default`, so every module needing a full instance keeps its own copy).
    fn state_in(tmp: &std::path::Path) -> DaemonState {
        let dirs = dirs_in(tmp);
        let router = SiteRouter::new(RouterConfig::with_tld(Tld::new("test").unwrap()));
        let ca_path = dirs.data.join("ca.cert.pem");
        let php_manager = Arc::new(Mutex::new(yerd_php::PhpManager::new(
            yerd_php::TokioProcessSpawner,
            yerd_php::SystemClock,
            yerd_php::io::FastCgiProbe,
            dirs.clone(),
            yerd_platform::ActivePortBinder::new(),
            std::process::id(),
            std::collections::BTreeMap::new(),
        )));
        DaemonState {
            config: Mutex::new(yerd_config::Config::default()),
            router: Arc::new(TokioRwLock::new(router)),
            config_path: dirs.config.join("yerd.toml"),
            dirs,
            dns_addr: "127.0.0.1:1053".parse().unwrap(),
            ca_path,
            ca_fingerprint: yerd_platform::CaFingerprint::new([0u8; 32]),
            php_ca_bundle: None,
            php_updates: TokioRwLock::new(std::collections::HashMap::new()),
            yerd_update: TokioRwLock::new(Vec::new()),
            update_snapshot: TokioRwLock::new(None),
            php_manager,
            service_manager: Arc::new(Mutex::new(crate::services::new_manager(dirs_in(tmp)))),
            mail_store: Arc::new(yerd_mail::Store::open(tmp.join("mail")).unwrap()),
            mail: crate::state::MailRuntime { listening: false },
            http: yerd_ipc::PortStatus {
                requested: 80,
                bound: 8080,
                fell_back: true,
            },
            https: yerd_ipc::PortStatus {
                requested: 443,
                bound: 8443,
                fell_back: true,
            },
            redirect_https_port: std::sync::Arc::new(std::sync::atomic::AtomicU16::new(8443)),
            web_unbound: None,
            dns_unbound: None,
            boot_id: 1,
            started_at: std::time::Instant::now(),
            shutdown_tx: tokio::sync::watch::channel(false).0,
            restart_requested: std::sync::atomic::AtomicBool::new(false),
            detect_cache: Arc::new(crate::detect_cache::DetectCache::new()),
            watch_dirty: tokio::sync::Notify::new(),
            dumps: Arc::new(crate::dump_server::DumpStore::new()),
            shim_reconcile: Mutex::new(()),
            tunnel_manager: Arc::new(Mutex::new(crate::tunnel::new_manager())),
            cloudflared_resolution: TokioRwLock::new(None),
            tool_mutate: Mutex::new(()),
            tunnel_mutate: Mutex::new(()),
            php_mutate: Mutex::new(()),
            jobs: crate::jobs::JobRegistry::default(),
            reserved_names: Mutex::new(std::collections::HashSet::new()),
            wordpress_versions: TokioRwLock::new(None),
            wordpress_login_tokens: Arc::new(crate::wordpress_login::LoginTokenRegistry::new()),
            wordpress_login_prepend_script: None,
        }
    }

    struct FakeDl(Result<&'static str, &'static str>);

    #[async_trait::async_trait]
    impl Downloader for FakeDl {
        async fn download(&self, url: &str) -> Result<Vec<u8>, yerd_php::DownloadError> {
            match self.0 {
                Ok(body) => Ok(body.as_bytes().to_vec()),
                Err(reason) => Err(yerd_php::DownloadError::Transport {
                    url: url.to_owned(),
                    reason: reason.to_owned(),
                }),
            }
        }
    }

    fn sample_versions() -> Vec<WordPressVersionInfo> {
        parse_meta(SAMPLE_BODY).unwrap()
    }

    fn versions_of(r: Response) -> Vec<WordPressVersionInfo> {
        match r {
            Response::WordpressVersions { versions } => versions,
            other => panic!("expected Response::WordpressVersions, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn available_versions_serves_fresh_cache_without_fetching() {
        let tmp = tempfile::tempdir().unwrap();
        let state = state_in(tmp.path());
        *state.wordpress_versions.write().await = Some((Instant::now(), sample_versions()));

        // A downloader that errors proves the fresh cache path never calls it.
        let dl = FakeDl(Err("must not be called"));
        let versions = versions_of(available_versions(&state, &dl).await);
        assert_eq!(versions, sample_versions());
    }

    #[tokio::test]
    async fn available_versions_fetches_and_populates_empty_cache() {
        let tmp = tempfile::tempdir().unwrap();
        let state = state_in(tmp.path());

        let dl = FakeDl(Ok(SAMPLE_BODY));
        let versions = versions_of(available_versions(&state, &dl).await);
        assert_eq!(versions, sample_versions());

        let cached = state.wordpress_versions.read().await;
        let (fetched_at, cached_versions) = cached.as_ref().unwrap();
        assert!(fetched_at.elapsed() < Duration::from_secs(1));
        assert_eq!(*cached_versions, sample_versions());
    }

    #[tokio::test]
    async fn available_versions_falls_back_to_stale_cache_on_fetch_error() {
        let tmp = tempfile::tempdir().unwrap();
        let state = state_in(tmp.path());
        let stale_at = Instant::now()
            .checked_sub(CACHE_TTL + Duration::from_secs(1))
            .unwrap();
        *state.wordpress_versions.write().await = Some((stale_at, sample_versions()));

        let dl = FakeDl(Err("network down"));
        let versions = versions_of(available_versions(&state, &dl).await);
        assert_eq!(
            versions,
            sample_versions(),
            "a fetch failure must serve the last-known-good cache, however stale"
        );
    }

    #[tokio::test]
    async fn available_versions_returns_empty_when_fetch_fails_with_no_cache() {
        let tmp = tempfile::tempdir().unwrap();
        let state = state_in(tmp.path());

        let dl = FakeDl(Err("network down"));
        let versions = versions_of(available_versions(&state, &dl).await);
        assert!(versions.is_empty());
    }
}
