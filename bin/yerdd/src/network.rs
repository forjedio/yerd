//! Outbound HTTP for the daemon: the one client factory every external request
//! goes through, the resolved proxy it applies, and the proxy environment for
//! network subprocesses and `{data}/bin` shims.
//!
//! [`NetworkRuntime`] (held in [`DaemonState::network`]) caches the
//! [`ResolvedProxy`] for [`RESOLVE_TTL`] so VPN and System Settings changes
//! apply within a minute. The user's login shell is only probed at daemon
//! start, on a settings save, and on a connection test; every other refresh
//! reuses that result. [`NetworkRuntime::refresh`] is the single place
//! `{data}/network.env` is written or removed.
//!
//! The decisions themselves live in [`yerd_core::network`]; OS reads sit
//! behind [`yerd_platform::SystemProxySource`].

use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::{Arc, Mutex as StdMutex};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use yerd_core::network::{
    child_env_plan as plan_for, from_env_lookup, render_env_file, resolve, scrub, ChildEnvPlan,
    ProxyUrl, ResolveInput, ResolvedProxy, SystemProxy,
};
use yerd_php::{DownloadError, Downloader};
use yerd_platform::SystemProxySource;

use crate::state::DaemonState;

/// How long a resolved snapshot is reused before env and OS sources are
/// re-read.
pub const RESOLVE_TTL: Duration = Duration::from_secs(60);

pub use yerd_core::network::ENV_FILE_NAME;

/// Prefix of the error every download returns when the client cannot be built.
pub const INVALID_CONFIG_PREFIX: &str = "outbound proxy configuration is invalid";

/// Whether a refresh may run the login-shell capture.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoginShell {
    /// Reuse the stored login-shell result ("never probed" reads as absent).
    UseCached,
    /// Run the capture and store its result, including "nothing found".
    Probe,
}

type CaptureFuture = Pin<Box<dyn Future<Output = Option<SystemProxy>> + Send>>;

/// The login-shell capture, injectable so tests never spawn a shell.
pub type LoginShellCapture = Arc<dyn Fn() -> CaptureFuture + Send + Sync>;

/// Reads the daemon's own environment.
#[must_use]
pub fn daemon_env(key: &str) -> Option<String> {
    std::env::var(key).ok()
}

/// An empty environment, for tests that must not see the real process env.
#[must_use]
pub fn no_env(_key: &str) -> Option<String> {
    None
}

/// The stored login-shell result. "Probed, nothing found" is kept apart from
/// "never probed" so a refresh never re-runs the shell just because it found
/// nothing.
#[derive(Debug, Clone)]
enum ShellReading {
    NeverProbed,
    Probed(Option<Box<SystemProxy>>),
}

/// Cached outbound-proxy state shared by every request path.
pub struct NetworkRuntime {
    source: Arc<dyn SystemProxySource + Send + Sync>,
    probe_login_shell: bool,
    env_lookup: fn(&str) -> Option<String>,
    capture: LoginShellCapture,
    login_shell: StdMutex<ShellReading>,
    cache: StdMutex<Option<(Instant, ResolvedProxy)>>,
    env_file_written: StdMutex<Option<String>>,
    refresh_lock: tokio::sync::Mutex<()>,
    ready: tokio::sync::watch::Sender<bool>,
}

impl NetworkRuntime {
    /// Build a runtime from its injected edges.
    #[must_use]
    pub fn new(
        source: Arc<dyn SystemProxySource + Send + Sync>,
        probe_login_shell: bool,
        env_lookup: fn(&str) -> Option<String>,
        capture: LoginShellCapture,
    ) -> Self {
        Self {
            source,
            probe_login_shell,
            env_lookup,
            capture,
            login_shell: StdMutex::new(ShellReading::NeverProbed),
            cache: StdMutex::new(None),
            env_file_written: StdMutex::new(None),
            refresh_lock: tokio::sync::Mutex::new(()),
            ready: tokio::sync::watch::channel(false).0,
        }
    }

    /// The production runtime: the active OS source, the real daemon env, and
    /// the real login-shell capture.
    #[must_use]
    pub fn production() -> Self {
        Self::new(
            Arc::new(yerd_platform::ActiveSystemProxySource::new()),
            true,
            daemon_env,
            Arc::new(|| Box::pin(crate::tools::external::capture_user_proxy_env())),
        )
    }

    /// Re-read every source, re-resolve, store the snapshot, and bring
    /// `{data}/network.env` in line with it (written 0600, or removed when
    /// there is nothing to export).
    ///
    /// Do not call while holding `state.config`; this takes that lock.
    pub async fn refresh(&self, state: &DaemonState, login: LoginShell) -> ResolvedProxy {
        let _serial = self.refresh_lock.lock().await;
        self.refresh_locked(state, login).await
    }

    async fn refresh_locked(&self, state: &DaemonState, login: LoginShell) -> ResolvedProxy {
        let (network, tld) = {
            let config = state.config.lock().await;
            (config.network.clone(), config.tld.as_str().to_owned())
        };
        let system = network.mode == yerd_core::network::OutboundProxyMode::System;
        let lookup = self.env_lookup;
        let env = from_env_lookup(&|k| lookup(k));
        let os = if system {
            let source = Arc::clone(&self.source);
            tokio::task::spawn_blocking(move || source.read())
                .await
                .ok()
                .flatten()
        } else {
            None
        };
        let login_shell = if system && login == LoginShell::Probe && self.probe_login_shell {
            let reading = (self.capture)().await;
            if let Ok(mut slot) = self.login_shell.lock() {
                *slot = ShellReading::Probed(reading.clone().map(Box::new));
            }
            reading
        } else {
            match self.login_shell.lock().ok().map(|slot| slot.clone()) {
                Some(ShellReading::Probed(reading)) => reading.map(|r| *r),
                _ => None,
            }
        };
        let resolved = resolve(&ResolveInput {
            mode: network.mode,
            manual_http: network.http_proxy.as_ref(),
            manual_https: network.https_proxy.as_ref(),
            user_bypass: &network.no_proxy,
            tld: &tld,
            env: &env,
            os: os.as_ref().map(|r| (self.source.kind(), r)),
            login_shell: login_shell.as_ref(),
        });
        if let Ok(mut cache) = self.cache.lock() {
            *cache = Some((Instant::now(), resolved.clone()));
        }
        self.sync_env_file(state, &resolved);
        resolved
    }

    fn sync_env_file(&self, state: &DaemonState, resolved: &ResolvedProxy) {
        let rendered = render_env_file(&plan_for(resolved));
        let unchanged = self
            .env_file_written
            .lock()
            .ok()
            .is_some_and(|w| w.as_deref() == Some(rendered.as_str()));
        if unchanged {
            return;
        }
        let path = env_file_path(state);
        let outcome = if rendered.is_empty() {
            match std::fs::remove_file(&path) {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
                _ => Ok(()),
            }
        } else {
            yerd_php::io::atomic_write::write(&path, rendered.as_bytes())
        };
        match outcome {
            Ok(()) => {
                if let Ok(mut w) = self.env_file_written.lock() {
                    *w = Some(rendered);
                }
                tracing::info!(
                    mode = resolved.mode.as_str(),
                    source = resolved.source.label(),
                    http = resolved.http.as_ref().map(ProxyUrl::redacted).as_deref(),
                    https = resolved.https.as_ref().map(ProxyUrl::redacted).as_deref(),
                    "outbound proxy resolved"
                );
            }
            Err(e) => tracing::warn!(path = %path.display(), "could not update network.env: {e}"),
        }
    }

    /// The cached snapshot when younger than [`RESOLVE_TTL`], else a refresh
    /// that reuses the cached login-shell result.
    ///
    /// Do not call while holding `state.config`; a refresh takes that lock.
    /// Concurrent callers that all miss serialise on the refresh lock and
    /// re-check the snapshot once they hold it, so only the first refreshes.
    pub async fn resolved(&self, state: &DaemonState) -> ResolvedProxy {
        if let Some(r) = self.fresh_snapshot() {
            return r;
        }
        let _serial = self.refresh_lock.lock().await;
        match self.fresh_snapshot() {
            Some(r) => r,
            None => self.refresh_locked(state, LoginShell::UseCached).await,
        }
    }

    fn fresh_snapshot(&self) -> Option<ResolvedProxy> {
        self.cache.lock().ok().and_then(|c| {
            c.as_ref()
                .filter(|(at, _)| at.elapsed() < RESOLVE_TTL)
                .map(|(_, r)| r.clone())
        })
    }

    /// Record that the startup login-shell probe has finished, releasing every
    /// [`Self::wait_ready_or_shutdown`] waiter.
    pub fn mark_ready(&self) {
        self.ready.send_replace(true);
    }

    /// Wait for [`Self::mark_ready`] so the first downloads after start see the
    /// login-shell result. Returns `false` when `shutdown` fires (or its sender
    /// is gone) first, so the caller can stop instead of starting work.
    pub async fn wait_ready_or_shutdown(
        &self,
        shutdown: &mut tokio::sync::watch::Receiver<bool>,
    ) -> bool {
        let mut ready = self.ready.subscribe();
        tokio::select! {
            r = ready.wait_for(|r| *r) => r.is_ok(),
            _ = shutdown.changed() => false,
        }
    }

    /// Drop the snapshot so the next [`Self::resolved`] re-reads the sources.
    /// The stored login-shell result is kept.
    pub fn invalidate(&self) {
        if let Ok(mut cache) = self.cache.lock() {
            *cache = None;
        }
    }

    /// Whether a snapshot is cached (tests use it to prove no client was built).
    #[cfg(test)]
    pub(crate) fn has_snapshot(&self) -> bool {
        self.cache.lock().is_ok_and(|c| c.is_some())
    }

    #[cfg(test)]
    fn backdate_snapshot(&self) -> bool {
        let Ok(mut cache) = self.cache.lock() else {
            return false;
        };
        let Some((at, _)) = cache.as_mut() else {
            return false;
        };
        match Instant::now().checked_sub(RESOLVE_TTL + Duration::from_secs(1)) {
            Some(past) => {
                *at = past;
                true
            }
            None => false,
        }
    }
}

/// `{data}/network.env`.
#[must_use]
pub fn env_file_path(state: &DaemonState) -> PathBuf {
    state.dirs.data.join(ENV_FILE_NAME)
}

fn proxy_urls(resolved: &ResolvedProxy) -> Vec<ProxyUrl> {
    resolved
        .http
        .iter()
        .chain(resolved.https.iter())
        .cloned()
        .collect()
}

/// Build a `reqwest` client for `resolved`.
///
/// `.no_proxy()` comes first so reqwest's implicit env proxy matcher never
/// applies; the resolved proxy is then added explicitly per scheme with the
/// bypass list. The error text is scrubbed of proxy passwords.
///
/// # Errors
/// A scrubbed message when a proxy URL or the client builder is rejected.
pub fn build_client(resolved: &ResolvedProxy) -> Result<reqwest::Client, String> {
    let urls = proxy_urls(resolved);
    let refs: Vec<&ProxyUrl> = urls.iter().collect();
    let scrubbed = |e: reqwest::Error| scrub(&e.to_string(), &refs);
    let bypass = reqwest::NoProxy::from_string(&resolved.bypass.join(","));
    let mut builder = reqwest::Client::builder()
        .user_agent(concat!("yerd/", env!("CARGO_PKG_VERSION")))
        .connect_timeout(Duration::from_secs(30))
        .read_timeout(Duration::from_secs(60))
        .no_proxy();
    if let Some(u) = &resolved.http {
        let proxy = reqwest::Proxy::http(u.as_str()).map_err(scrubbed)?;
        builder = builder.proxy(proxy.no_proxy(bypass.clone()));
    }
    if let Some(u) = &resolved.https {
        let proxy = reqwest::Proxy::https(u.as_str()).map_err(scrubbed)?;
        builder = builder.proxy(proxy.no_proxy(bypass));
    }
    builder.build().map_err(scrubbed)
}

/// A downloader built from the current resolved proxy.
///
/// Do not call while holding `state.config`; a refresh takes that lock.
pub async fn downloader(state: &DaemonState) -> ReqwestDownloader {
    let resolved = state.network.resolved(state).await;
    ReqwestDownloader::from_resolved(&resolved)
}

/// A raw client built from the current resolved proxy, for callers that need
/// more than a GET body (e.g. the Cloudflare API).
///
/// Do not call while holding `state.config`; a refresh takes that lock.
///
/// # Errors
/// A scrubbed message when the client cannot be built.
pub async fn client(state: &DaemonState) -> Result<reqwest::Client, String> {
    build_client(&state.network.resolved(state).await)
}

/// The proxy environment plan for a network subprocess.
///
/// Do not call while holding `state.config`; a refresh takes that lock.
pub async fn child_env_plan(state: &DaemonState) -> ChildEnvPlan {
    plan_for(&state.network.resolved(state).await)
}

/// Apply `plan` to `cmd`: the one place a child's proxy env is set.
pub fn apply_plan(cmd: &mut tokio::process::Command, plan: &ChildEnvPlan) {
    for key in &plan.remove {
        cmd.env_remove(key);
    }
    for (key, value) in &plan.set {
        cmd.env(key, value);
    }
}

/// Apply the current proxy environment to a network subprocess.
///
/// Do not call while holding `state.config`; a refresh takes that lock.
pub async fn apply_child_env(cmd: &mut tokio::process::Command, state: &DaemonState) {
    apply_plan(cmd, &child_env_plan(state).await);
}

/// `reqwest`-backed downloader (rustls, no OpenSSL; follows redirects), built
/// only through this module's factory.
pub struct ReqwestDownloader {
    client: Result<reqwest::Client, String>,
    secrets: Vec<ProxyUrl>,
}

impl ReqwestDownloader {
    /// Wrap a client build result. An `Err` makes every download fail with an
    /// [`INVALID_CONFIG_PREFIX`] transport error.
    #[must_use]
    pub fn from_build(client: Result<reqwest::Client, String>) -> Self {
        Self {
            client,
            secrets: Vec::new(),
        }
    }

    /// A downloader for `resolved`, scrubbing its proxy passwords from errors.
    #[must_use]
    pub fn from_resolved(resolved: &ResolvedProxy) -> Self {
        Self {
            client: build_client(resolved),
            secrets: proxy_urls(resolved),
        }
    }

    /// A direct (no proxy) downloader for tests.
    #[cfg(test)]
    #[must_use]
    pub fn direct() -> Self {
        Self::from_build(build_client(&ResolvedProxy::off()))
    }

    fn scrub(&self, text: &str) -> String {
        let refs: Vec<&ProxyUrl> = self.secrets.iter().collect();
        scrub(text, &refs)
    }
}

#[async_trait]
impl Downloader for ReqwestDownloader {
    async fn download(&self, url: &str) -> Result<Vec<u8>, DownloadError> {
        self.download_with_progress(url, &|_, _| {}).await
    }

    /// Streams the body chunk-by-chunk so progress can be reported and a
    /// mid-stream stall trips `read_timeout` rather than buffering forever. The
    /// `Content-Length` capacity hint is clamped (the header is server-controlled;
    /// a bogus huge value would otherwise abort the daemon on a failed
    /// allocation), and the `Vec` still grows past the cap as needed.
    async fn download_with_progress(
        &self,
        url: &str,
        progress: &(dyn Fn(u64, Option<u64>) + Send + Sync),
    ) -> Result<Vec<u8>, DownloadError> {
        let client = self.client.as_ref().map_err(|e| DownloadError::Transport {
            url: url.to_owned(),
            reason: format!("{INVALID_CONFIG_PREFIX}: {e}"),
        })?;
        let transport = |e: reqwest::Error| DownloadError::Transport {
            url: url.to_owned(),
            reason: self.scrub(&e.to_string()),
        };
        let mut resp = client
            .get(url)
            .send()
            .await
            .map_err(transport)?
            .error_for_status()
            .map_err(transport)?;
        let total = resp.content_length();
        let cap = total.map_or(0, |n| n.min(64 * 1024 * 1024) as usize);
        let mut buf = Vec::with_capacity(cap);
        progress(0, total);
        while let Some(chunk) = resp.chunk().await.map_err(transport)? {
            buf.extend_from_slice(&chunk);
            progress(buf.len() as u64, total);
        }
        Ok(buf)
    }
}

/// How long the Doctor reachability probe waits for a TCP connection.
const REACHABILITY_TIMEOUT: Duration = Duration::from_secs(3);

/// How long `TestNetwork` waits for the release manifest.
const TEST_TIMEOUT: Duration = Duration::from_secs(20);

/// A 3 s TCP connect to the proxy (HTTPS first, else HTTP). `None` when no
/// proxy resolved.
pub async fn probe_reachable(resolved: &ResolvedProxy) -> Option<bool> {
    let proxy = resolved.primary()?;
    let connect = tokio::net::TcpStream::connect((proxy.host(), proxy.port()));
    Some(matches!(
        tokio::time::timeout(REACHABILITY_TIMEOUT, connect).await,
        Ok(Ok(_))
    ))
}

/// The Doctor side-channel input for `resolved`. PAC is only flagged in
/// System mode when detection found PAC/WPAD and no static proxy.
#[must_use]
pub fn outbound_proxy_probe(
    resolved: &ResolvedProxy,
    reachable: Option<bool>,
) -> yerd_doctor::OutboundProxyProbe {
    yerd_doctor::OutboundProxyProbe {
        proxy: resolved
            .primary()
            .map(|u| format!("{} ({})", u.redacted(), resolved.source.label())),
        reachable,
        pac_unsupported: resolved.mode == yerd_core::network::OutboundProxyMode::System
            && resolved.pac_detected
            && !resolved.has_proxy(),
    }
}

/// Resolve (cached login shell), probe reachability, and build the Doctor
/// input for `Diagnose` / `DoctorFix`.
///
/// Do not call while holding `state.config`; a refresh takes that lock.
pub async fn doctor_probe(state: &DaemonState) -> yerd_doctor::OutboundProxyProbe {
    let resolved = state.network.resolved(state).await;
    let reachable = probe_reachable(&resolved).await;
    outbound_proxy_probe(&resolved, reachable)
}

fn effective(resolved: &ResolvedProxy) -> yerd_ipc::EffectiveProxy {
    yerd_ipc::EffectiveProxy {
        mode: resolved.mode,
        source: resolved.source,
        http_proxy: resolved.http.as_ref().map(ProxyUrl::redacted),
        https_proxy: resolved.https.as_ref().map(ProxyUrl::redacted),
        bypass: resolved.bypass.clone(),
    }
}

/// `GetNetworkSettings`: the saved settings (redacted) and what they resolve
/// to, using the cached login-shell result. Runs no probes.
pub async fn get_settings(state: &DaemonState) -> yerd_ipc::Response {
    let resolved = state.network.resolved(state).await;
    let network = state.config.lock().await.network.clone();
    yerd_ipc::Response::NetworkSettings {
        settings: Box::new(yerd_ipc::NetworkSettings {
            mode: network.mode,
            http_proxy: network.http_proxy.as_ref().map(ProxyUrl::redacted),
            https_proxy: network.https_proxy.as_ref().map(ProxyUrl::redacted),
            no_proxy: network.no_proxy,
            effective: Some(effective(&resolved)),
        }),
    }
}

fn invalid(message: String) -> yerd_ipc::Response {
    yerd_ipc::Response::Error {
        code: yerd_ipc::ErrorCode::InvalidPath,
        message,
    }
}

/// `SetNetworkSettings`: merge the submitted values (a redacted `***`
/// password keeps the stored one only for an unchanged endpoint), validate,
/// save, then re-detect with a login-shell probe, which also rewrites
/// `network.env`. Errors never echo a submitted URL.
pub async fn set_settings(
    state: &DaemonState,
    mode: yerd_core::network::OutboundProxyMode,
    for_http: Option<String>,
    for_https: Option<String>,
    no_proxy: Option<Vec<String>>,
) -> yerd_ipc::Response {
    {
        let mut guard = state.config.lock().await;
        let mut new = guard.clone();
        if let Some(submitted) = for_http {
            match yerd_core::network::merge_submitted(&submitted, new.network.http_proxy.as_ref()) {
                Ok(url) => new.network.http_proxy = url,
                Err(e) => return invalid(format!("HTTP proxy: {e}")),
            }
        }
        if let Some(submitted) = for_https {
            match yerd_core::network::merge_submitted(&submitted, new.network.https_proxy.as_ref())
            {
                Ok(url) => new.network.https_proxy = url,
                Err(e) => return invalid(format!("HTTPS proxy: {e}")),
            }
        }
        if let Some(entries) = no_proxy {
            new.network.no_proxy = yerd_core::network::normalize_bypass_entries(&entries);
        }
        new.network.mode = mode;
        if let Err(e) = new.validate() {
            return invalid(e.to_string());
        }
        if let Err(e) = new.save(&state.config_path) {
            return crate::ipc_server::internal(format!("config save failed: {e}"));
        }
        *guard = new;
    }
    state.network.invalidate();
    let resolved = state.network.refresh(state, LoginShell::Probe).await;
    tracing::info!(
        mode = mode.as_str(),
        source = resolved.source.label(),
        "set outbound proxy settings"
    );
    get_settings(state).await
}

/// `TestNetwork`: re-detect with a login-shell probe, then fetch Yerd's
/// release manifest through the resolved proxy.
pub async fn test_network(state: &DaemonState) -> yerd_ipc::Response {
    test_network_with(state, crate::self_update::LATEST_MANIFEST_URL).await
}

async fn test_network_with(state: &DaemonState, url: &str) -> yerd_ipc::Response {
    state.network.invalidate();
    let resolved = state.network.refresh(state, LoginShell::Probe).await;
    let urls = proxy_urls(&resolved);
    let refs: Vec<&ProxyUrl> = urls.iter().collect();
    let route = if url.starts_with("https://") {
        resolved.https.as_ref()
    } else {
        resolved.http.as_ref()
    };
    let mut result = yerd_ipc::NetworkTestResult {
        ok: false,
        url: url.to_owned(),
        status: None,
        error: None,
        via: route.map(ProxyUrl::redacted),
        source: if route.is_some() {
            resolved.source
        } else {
            yerd_core::network::ProxySource::None
        },
    };
    match build_client(&resolved) {
        Err(e) => result.error = Some(format!("{INVALID_CONFIG_PREFIX}: {e}")),
        Ok(client) => match client.get(url).timeout(TEST_TIMEOUT).send().await {
            Ok(resp) => {
                result.status = Some(resp.status().as_u16());
                result.ok = resp.status().is_success();
            }
            Err(e) => result.error = Some(scrub(&e.to_string(), &refs)),
        },
    }
    yerd_ipc::Response::NetworkTest {
        result: Box::new(result),
    }
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
    use yerd_core::network::{OutboundProxyMode, ProxySource};

    use super::*;

    struct SwitchSource(StdMutex<Option<SystemProxy>>);

    impl SystemProxySource for SwitchSource {
        fn kind(&self) -> ProxySource {
            ProxySource::MacosSystemSettings
        }

        fn read(&self) -> Option<SystemProxy> {
            self.0.lock().unwrap().clone()
        }
    }

    fn reading(url: &str) -> SystemProxy {
        SystemProxy {
            http: Some(ProxyUrl::parse(url).unwrap()),
            https: Some(ProxyUrl::parse(url).unwrap()),
            bypass: Vec::new(),
            pac_detected: false,
        }
    }

    fn counting_capture(reading: Option<SystemProxy>) -> (LoginShellCapture, Arc<AtomicUsize>) {
        let count = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&count);
        let capture: LoginShellCapture = Arc::new(move || {
            counter.fetch_add(1, Ordering::SeqCst);
            let r = reading.clone();
            Box::pin(async move { r })
        });
        (capture, count)
    }

    fn state_with(
        tmp: &std::path::Path,
        source: Arc<dyn SystemProxySource + Send + Sync>,
        capture: LoginShellCapture,
    ) -> DaemonState {
        let mut state = crate::test_support::state_in(tmp);
        std::fs::create_dir_all(&state.dirs.data).unwrap();
        state.network = NetworkRuntime::new(source, true, no_env, capture);
        state
    }

    async fn set_manual(state: &DaemonState, url: &str) {
        let mut c = state.config.lock().await;
        c.network.mode = OutboundProxyMode::Manual;
        c.network.http_proxy = Some(ProxyUrl::parse(url).unwrap());
    }

    #[tokio::test]
    async fn manual_refresh_writes_owner_only_env_file() {
        let tmp = tempfile::tempdir().unwrap();
        let (capture, _) = counting_capture(None);
        let state = state_with(
            tmp.path(),
            Arc::new(yerd_platform::FakeSystemProxySource::default()),
            capture,
        );
        set_manual(&state, "http://u:pw@proxy.corp:3128").await;
        let r = state.network.refresh(&state, LoginShell::UseCached).await;
        assert_eq!(r.source, ProxySource::Manual);
        let path = env_file_path(&state);
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(
            text.contains("HTTPS_PROXY=http://u:pw@proxy.corp:3128\n"),
            "{text}"
        );
        assert!(text.contains("NODE_USE_ENV_PROXY=1\n"), "{text}");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
        }

        std::fs::remove_file(&path).unwrap();
        state.network.refresh(&state, LoginShell::UseCached).await;
        assert!(
            !path.exists(),
            "an unchanged refresh must not rewrite the file"
        );

        state.config.lock().await.network.mode = OutboundProxyMode::Off;
        std::fs::write(&path, "stale").unwrap();
        let r = state.network.refresh(&state, LoginShell::UseCached).await;
        assert!(!r.has_proxy());
        assert!(!path.exists(), "off must remove network.env");
    }

    #[tokio::test]
    async fn ttl_expiry_picks_up_a_changed_os_reading() {
        let tmp = tempfile::tempdir().unwrap();
        let source = Arc::new(SwitchSource(StdMutex::new(Some(reading("http://a:1")))));
        let (capture, _) = counting_capture(None);
        let state = state_with(tmp.path(), source.clone(), capture);
        let first = state.network.resolved(&state).await;
        assert_eq!(first.source, ProxySource::MacosSystemSettings);
        let path = env_file_path(&state);
        assert!(std::fs::read_to_string(&path)
            .unwrap()
            .contains("http://a:1"));

        *source.0.lock().unwrap() = Some(reading("http://b:2"));
        assert_eq!(
            state
                .network
                .resolved(&state)
                .await
                .primary()
                .unwrap()
                .host(),
            "a",
            "a fresh snapshot is reused"
        );
        if !state.network.backdate_snapshot() {
            return;
        }
        let second = state.network.resolved(&state).await;
        assert_eq!(second.primary().unwrap().host(), "b");
        assert!(std::fs::read_to_string(&path)
            .unwrap()
            .contains("http://b:2"));
    }

    #[tokio::test]
    async fn startup_probe_is_seen_by_the_first_resolve_and_then_cached() {
        let tmp = tempfile::tempdir().unwrap();
        let (capture, count) = counting_capture(Some(reading("http://shell:3128")));
        let state = state_with(
            tmp.path(),
            Arc::new(yerd_platform::FakeSystemProxySource::default()),
            capture,
        );
        state.network.refresh(&state, LoginShell::Probe).await;
        assert_eq!(count.load(Ordering::SeqCst), 1);
        let r = state.network.resolved(&state).await;
        assert_eq!(r.source, ProxySource::LoginShell);
        assert_eq!(r.primary().unwrap().as_str(), "http://shell:3128");
        state.network.invalidate();
        let again = state.network.refresh(&state, LoginShell::UseCached).await;
        assert_eq!(again.source, ProxySource::LoginShell);
        assert_eq!(count.load(Ordering::SeqCst), 1);
    }

    struct CountingSource(AtomicUsize);

    impl SystemProxySource for CountingSource {
        fn kind(&self) -> ProxySource {
            ProxySource::MacosSystemSettings
        }

        fn read(&self) -> Option<SystemProxy> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Some(reading("http://os:1"))
        }
    }

    #[tokio::test]
    async fn concurrent_resolves_refresh_once() {
        let tmp = tempfile::tempdir().unwrap();
        let source = Arc::new(CountingSource(AtomicUsize::new(0)));
        let (capture, _) = counting_capture(None);
        let state = state_with(tmp.path(), source.clone(), capture);
        let (a, b, c, d) = tokio::join!(
            state.network.resolved(&state),
            state.network.resolved(&state),
            state.network.resolved(&state),
            state.network.resolved(&state),
        );
        assert!([&b, &c, &d].iter().all(|r| **r == a));
        assert_eq!(source.0.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn readiness_gates_waiters_and_yields_to_shutdown() {
        let tmp = tempfile::tempdir().unwrap();
        let (capture, _) = counting_capture(None);
        let state = state_with(
            tmp.path(),
            Arc::new(yerd_platform::FakeSystemProxySource::default()),
            capture,
        );
        let (tx, mut rx) = tokio::sync::watch::channel(false);
        let pending = tokio::time::timeout(
            Duration::from_millis(50),
            state.network.wait_ready_or_shutdown(&mut rx),
        )
        .await;
        assert!(pending.is_err(), "waits until the startup probe is done");
        tx.send_replace(true);
        assert!(!state.network.wait_ready_or_shutdown(&mut rx).await);
        let (_tx, mut rx) = tokio::sync::watch::channel(false);
        state.network.refresh(&state, LoginShell::Probe).await;
        state.network.mark_ready();
        assert!(state.network.wait_ready_or_shutdown(&mut rx).await);
    }

    #[tokio::test]
    async fn use_cached_never_probes_and_reuses_an_empty_result() {
        let tmp = tempfile::tempdir().unwrap();
        let (capture, count) = counting_capture(None);
        let state = state_with(
            tmp.path(),
            Arc::new(yerd_platform::FakeSystemProxySource::default()),
            capture,
        );
        state.network.refresh(&state, LoginShell::UseCached).await;
        assert_eq!(count.load(Ordering::SeqCst), 0);
        state.network.refresh(&state, LoginShell::Probe).await;
        assert_eq!(count.load(Ordering::SeqCst), 1);
        state.network.invalidate();
        let r = state.network.resolved(&state).await;
        assert_eq!(r.source, ProxySource::None);
        assert_eq!(count.load(Ordering::SeqCst), 1);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn apply_plan_sets_and_removes_proxy_env() {
        let manual = ResolvedProxy {
            mode: OutboundProxyMode::Manual,
            source: ProxySource::Manual,
            http: Some(ProxyUrl::parse("http://m:1").unwrap()),
            https: Some(ProxyUrl::parse("http://m:1").unwrap()),
            bypass: vec!["localhost".into(), ".test".into()],
            pac_detected: false,
        };
        let mut cmd = tokio::process::Command::new("/usr/bin/env");
        apply_plan(&mut cmd, &plan_for(&manual));
        let out = String::from_utf8(cmd.output().await.unwrap().stdout).unwrap();
        for line in [
            "http_proxy=http://m:1",
            "HTTP_PROXY=http://m:1",
            "https_proxy=http://m:1",
            "HTTPS_PROXY=http://m:1",
            "no_proxy=localhost,.test",
            "NO_PROXY=localhost,.test",
            "NODE_USE_ENV_PROXY=1",
        ] {
            assert!(out.lines().any(|l| l == line), "missing {line} in {out}");
        }

        let mut cmd = tokio::process::Command::new("/usr/bin/env");
        cmd.env("HTTPS_PROXY", "http://inherited:9")
            .env("all_proxy", "http://inherited:9");
        apply_plan(&mut cmd, &plan_for(&ResolvedProxy::off()));
        let out = String::from_utf8(cmd.output().await.unwrap().stdout).unwrap();
        for name in yerd_core::network::PROXY_ENV_NAMES {
            assert!(
                !out.lines().any(|l| l.starts_with(&format!("{name}="))),
                "{name} leaked into {out}"
            );
        }
    }

    type Seen = Arc<StdMutex<Vec<String>>>;

    async fn recording_proxy() -> (u16, Seen, tokio::task::JoinHandle<()>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let seen: Seen = Arc::new(StdMutex::new(Vec::new()));
        let log = Arc::clone(&seen);
        let task = tokio::spawn(async move {
            while let Ok((mut stream, _)) = listener.accept().await {
                let mut buf = vec![0u8; 4096];
                let n = stream.read(&mut buf).await.unwrap_or(0);
                let head = String::from_utf8_lossy(&buf[..n]).into_owned();
                let first = head.lines().next().unwrap_or_default().to_owned();
                log.lock().unwrap().push(first);
                let _ = stream
                    .write_all(
                        b"HTTP/1.1 200 OK\r\nContent-Length: 7\r\nConnection: close\r\n\r\nproxied",
                    )
                    .await;
                let _ = stream.shutdown().await;
            }
        });
        (port, seen, task)
    }

    async fn origin() -> (u16, tokio::task::JoinHandle<()>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let task = tokio::spawn(async move {
            while let Ok((mut stream, _)) = listener.accept().await {
                let mut buf = vec![0u8; 4096];
                let _ = stream.read(&mut buf).await;
                let _ = stream
                    .write_all(
                        b"HTTP/1.1 200 OK\r\nContent-Length: 6\r\nConnection: close\r\n\r\ndirect",
                    )
                    .await;
                let _ = stream.shutdown().await;
            }
        });
        (port, task)
    }

    fn manual_via(port: u16) -> ResolvedProxy {
        let url = ProxyUrl::parse(&format!("http://127.0.0.1:{port}")).unwrap();
        ResolvedProxy {
            mode: OutboundProxyMode::Manual,
            source: ProxySource::Manual,
            http: Some(url.clone()),
            https: Some(url),
            bypass: yerd_core::network::bypass_list("test", &[], &[]),
            pac_detected: false,
        }
    }

    #[tokio::test]
    async fn manual_proxy_routes_external_and_bypasses_local() {
        let (proxy_port, seen, proxy_task) = recording_proxy().await;
        let (origin_port, origin_task) = origin().await;
        let dl = ReqwestDownloader::from_resolved(&manual_via(proxy_port));

        let body = dl.download("http://example.invalid/x").await.unwrap();
        assert_eq!(body, b"proxied");
        assert_eq!(
            seen.lock().unwrap().clone(),
            vec!["GET http://example.invalid/x HTTP/1.1".to_owned()]
        );
        seen.lock().unwrap().clear();

        let _ =
            tokio::time::timeout(Duration::from_secs(10), dl.download("http://foo.test/")).await;
        assert!(
            seen.lock().unwrap().is_empty(),
            "a .test request reached the proxy"
        );

        let body = dl
            .download(&format!("http://127.0.0.1:{origin_port}/"))
            .await
            .unwrap();
        assert_eq!(body, b"direct");
        assert!(
            seen.lock().unwrap().is_empty(),
            "loopback reached the proxy"
        );

        proxy_task.abort();
        origin_task.abort();
    }

    #[test]
    fn off_mode_ignores_env_proxy_child() {
        let Ok(url) = std::env::var("YERD_TEST_OFF_URL") else {
            return;
        };
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let result = runtime.block_on(async {
            tokio::time::timeout(
                Duration::from_secs(10),
                ReqwestDownloader::from_resolved(&ResolvedProxy::off()).download(&url),
            )
            .await
        });
        if let Ok(Ok(body)) = result {
            assert_ne!(body, b"proxied", "off mode went through the env proxy");
        }
    }

    #[tokio::test]
    async fn off_mode_ignores_env_proxy() {
        let (proxy_port, seen, proxy_task) = recording_proxy().await;
        let proxy = format!("http://127.0.0.1:{proxy_port}");
        let output = tokio::time::timeout(
            Duration::from_secs(30),
            tokio::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "network::tests::off_mode_ignores_env_proxy_child",
                    "--nocapture",
                ])
                .env("YERD_TEST_OFF_URL", "http://example.invalid/y")
                .env("HTTP_PROXY", &proxy)
                .env("http_proxy", &proxy)
                .env("HTTPS_PROXY", &proxy)
                .env("ALL_PROXY", &proxy)
                .env_remove("NO_PROXY")
                .env_remove("no_proxy")
                .kill_on_drop(true)
                .output(),
        )
        .await
        .unwrap()
        .unwrap();
        assert!(
            output.status.success(),
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            seen.lock().unwrap().is_empty(),
            "off mode used the env proxy"
        );
        proxy_task.abort();
    }

    #[tokio::test]
    async fn invalid_config_fails_every_download_without_leaking() {
        let url = ProxyUrl::parse("http://u:hunter2@h:1").unwrap();
        let reason = scrub(&format!("builder error for {}", url.as_str()), &[&url]);
        let dl = ReqwestDownloader::from_build(Err(reason));
        match dl.download("http://example.invalid/").await {
            Err(DownloadError::Transport { reason, .. }) => {
                assert!(reason.starts_with(INVALID_CONFIG_PREFIX), "{reason}");
                assert!(!reason.contains("hunter2"), "{reason}");
            }
            other => panic!("expected Transport, got {other:?}"),
        }
    }

    #[test]
    fn outbound_proxy_probe_flags_pac_only_without_a_proxy() {
        let mut system = ResolvedProxy::off();
        system.mode = OutboundProxyMode::System;
        system.pac_detected = true;
        assert!(outbound_proxy_probe(&system, None).pac_unsupported);

        let mut with_proxy = manual_via(3128);
        with_proxy.mode = OutboundProxyMode::System;
        with_proxy.source = ProxySource::MacosSystemSettings;
        with_proxy.pac_detected = true;
        let probe = outbound_proxy_probe(&with_proxy, Some(false));
        assert!(!probe.pac_unsupported);
        assert_eq!(probe.reachable, Some(false));
        assert_eq!(
            probe.proxy.as_deref(),
            Some("http://127.0.0.1:3128 (macOS System Settings)")
        );

        let mut manual = manual_via(3128);
        manual.pac_detected = true;
        assert!(!outbound_proxy_probe(&manual, None).pac_unsupported);
        assert!(!outbound_proxy_probe(&ResolvedProxy::off(), None).pac_unsupported);
    }

    #[tokio::test]
    async fn probe_reachable_reports_listener_state() {
        assert_eq!(probe_reachable(&ResolvedProxy::off()).await, None);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        assert_eq!(probe_reachable(&manual_via(port)).await, Some(true));
        drop(listener);
        assert_eq!(probe_reachable(&manual_via(port)).await, Some(false));
    }

    #[tokio::test]
    async fn test_network_goes_through_the_resolved_proxy() {
        let tmp = tempfile::tempdir().unwrap();
        let (capture, _) = counting_capture(None);
        let state = state_with(
            tmp.path(),
            Arc::new(yerd_platform::FakeSystemProxySource::default()),
            capture,
        );
        let (port, seen, task) = recording_proxy().await;
        set_manual(&state, &format!("http://u:hunter2@127.0.0.1:{port}")).await;
        let resp = test_network_with(&state, "http://example.invalid/latest.json").await;
        let json = serde_json::to_string(&resp).unwrap();
        assert!(!json.contains("hunter2"), "{json}");
        let yerd_ipc::Response::NetworkTest { result } = resp else {
            panic!("expected NetworkTest");
        };
        assert!(result.ok);
        assert_eq!(result.status, Some(200));
        assert_eq!(
            result.via.as_deref(),
            Some(format!("http://u:***@127.0.0.1:{port}").as_str())
        );
        assert_eq!(result.source, ProxySource::Manual);
        assert_eq!(
            seen.lock().unwrap().clone(),
            vec!["GET http://example.invalid/latest.json HTTP/1.1".to_owned()]
        );
        task.abort();
    }

    #[test]
    fn build_client_accepts_every_resolved_shape() {
        assert!(build_client(&ResolvedProxy::off()).is_ok());
        assert!(build_client(&manual_via(3128)).is_ok());
        let mut https_proxy = manual_via(3128);
        https_proxy.https = Some(ProxyUrl::parse("https://u:p@secure:8443").unwrap());
        assert!(build_client(&https_proxy).is_ok());
    }
}
