//! Cloudflare Tunnel wiring: the daemon's `TunnelManager` type, the IPC handlers
//! for the tunnel requests, the `cloudflared` install job, and the
//! snapshot→wire mapping.
//!
//! Every `cloudflared` points at the daemon's loopback tunnel listener
//! (`state.tunnel_origin_port`), which routes by the public `Host` through
//! [`rebuild_routes`]'s map. Each running instance is supervised by a background
//! driver task (see [`supervise`]); start requests only launch and then wait for
//! the instance to settle.
//!
//! Lock discipline mirrors the services path: the site facts are read under the
//! router lock, which is released before the `tunnel_manager` is taken; the
//! config/router lock and the tunnel-manager lock are never held simultaneously
//! across an `.await`. The `cloudflared` install runs under `tunnel_mutate` only.
//!
//! The tunnel origin listener's route map is rebuilt by [`rebuild_routes`] with
//! a fixed lock order: `tunnel_routes_rebuild`, then `tunnel_manager`
//! (released), then the config lock (released), then the router read guard
//! (released), then the `tunnel_routes` write guard. The
//! routes lock is a leaf and is never held across another lock. tokio's `Mutex`
//! is not reentrant, so every caller drops its own config and manager guards
//! before calling [`rebuild_routes`].

pub mod credentials;
pub mod install;
pub mod named;
pub mod orphans;
pub mod supervise;

use std::collections::{BTreeMap, HashMap};
use std::ffi::OsString;
use std::net::{Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use std::sync::Arc;

use yerd_ipc::{
    CloudflaredStatus, ErrorCode, Response, TunnelInfo, TunnelKind as WireKind, TunnelRunState,
};
use yerd_platform::PlatformDirs;
use yerd_supervise::{SystemClock, TokioProcessSpawner};
use yerd_tunnel::manager::{TunnelSnapshot, TunnelState};
use yerd_tunnel::{LaunchSpec, TunnelKind, TunnelManager};

use self::supervise::Settled;
use crate::state::DaemonState;

/// A sink for streamed install output (one line per send), drained into the job
/// log by the streamed-install job.
pub type ProgressTx = tokio::sync::mpsc::UnboundedSender<String>;

/// Concrete `TunnelManager` the daemon uses.
pub type DaemonTunnelManager = TunnelManager<TokioProcessSpawner, SystemClock>;

/// Build the daemon's tunnel manager.
#[must_use]
pub fn new_manager() -> DaemonTunnelManager {
    TunnelManager::new(TokioProcessSpawner, SystemClock)
}

/// `{state}/tunnel/<site>.log`, where a tunnel's `cloudflared` output is
/// captured for readiness parsing and `yerd tunnel logs`.
pub(super) fn logfile(dirs: &PlatformDirs, site: &str) -> PathBuf {
    dirs.state.join("tunnel").join(format!("{site}.log"))
}

/// `{state}/tunnel/<key>.pid`, the live `cloudflared` PID for `key`, read by the
/// orphan reaper after a daemon crash.
pub(super) fn pidfile(dirs: &PlatformDirs, key: &str) -> PathBuf {
    dirs.state.join("tunnel").join(format!("{key}.pid"))
}

/// The production metrics address source: bind `127.0.0.1:0`, read the port the
/// OS picked, and release it for `cloudflared` to bind. A port grabbed in
/// between costs one failed spawn, which the supervisor retries with a fresh
/// address.
///
/// # Errors
///
/// Binding or reading the loopback socket failed.
pub fn pick_metrics_addr() -> std::io::Result<SocketAddr> {
    let listener = std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
    listener.local_addr()
}

/// The [`LaunchSpec`] for `key`: its logfile and pidfile under
/// `{state}/tunnel` (created 0700 here), and the daemon's metrics address
/// source.
pub(super) fn launch_spec(
    state: &DaemonState,
    key: &str,
    binary: PathBuf,
    args: Vec<OsString>,
    env: Vec<(OsString, OsString)>,
) -> Result<LaunchSpec, Response> {
    let logfile = logfile(&state.dirs, key);
    if let Some(parent) = logfile.parent() {
        if let Err(e) = crate::secure_fs::create_private_dir(parent) {
            return Err(Response::Error {
                code: ErrorCode::Internal,
                message: format!("could not prepare tunnel log dir: {e}"),
            });
        }
    }
    Ok(LaunchSpec {
        binary,
        args,
        env,
        logfile,
        metrics_addr: Arc::clone(&state.tunnel_metrics_addr),
        pidfile: pidfile(&state.dirs, key),
    })
}

/// Write `{data}/tunnel/quick.yml` (0600, atomically), the explicit ingress-free
/// config every quick tunnel runs with, and return its path.
fn write_quick_config(dirs: &PlatformDirs) -> Result<PathBuf, String> {
    let dir = install::tunnel_dir(dirs);
    crate::secure_fs::create_private_dir(&dir)
        .map_err(|e| format!("could not prepare tunnel dir: {e}"))?;
    let path = dir.join("quick.yml");
    yerd_php::io::atomic_write::write(&path, yerd_tunnel::config::render_quick_config().as_bytes())
        .map_err(|e| format!("write quick tunnel config: {e}"))?;
    Ok(path)
}

/// `cloudflared` install + account status for the wire.
pub async fn cloudflared_status(state: &DaemonState) -> CloudflaredStatus {
    let resolved = resolved_cloudflared(state).await;
    CloudflaredStatus {
        installed: resolved.is_some(),
        version: resolved.as_ref().and_then(|r| r.version.clone()),
        source: resolved.map(|r| source_to_wire(r.source)),
        logged_in: named::is_logged_in(&state.dirs),
    }
}

/// Map the daemon-internal source to its wire form.
fn source_to_wire(source: install::CloudflaredSource) -> yerd_ipc::CloudflaredSource {
    match source {
        install::CloudflaredSource::Managed => yerd_ipc::CloudflaredSource::Managed,
        install::CloudflaredSource::System => yerd_ipc::CloudflaredSource::System,
    }
}

/// Resolve which `cloudflared` binary to use, maintaining
/// `state.cloudflared_resolution` as a cache so a `PATH`-found system
/// binary's `--version` probe runs at most once per resolution rather than on
/// every tunnel action or status poll.
///
/// A cached `System` entry is re-checked for existence (cheap, no re-spawn)
/// before use, so a binary removed or replaced out from under a live session
/// is detected and dropped rather than silently spawned from a dangling path.
/// A cached `Managed` entry is trusted as-is (it's invalidated explicitly by a
/// fresh install instead, see `install_cloudflared_streamed`). On a cache
/// miss this recomputes via `install::resolve` and writes the result back
/// under a no-downgrade rule: a fresh `Managed` result always overwrites, but
/// a `System` (or absent) result never overwrites an existing `Managed`
/// entry, since a concurrent successful install may have raced ahead of a
/// slow `PATH` probe that started before it committed.
pub(super) async fn resolved_cloudflared(state: &DaemonState) -> Option<install::Resolved> {
    if let Some(cached) = cached_cloudflared_if_valid(state).await {
        return Some(cached);
    }

    let fresh = install::resolve(
        &state.dirs,
        &install::RealVersionProbe,
        &install::RealPathSearch,
    )
    .await;

    let mut guard = state.cloudflared_resolution.write().await;
    if is_downgrade(guard.as_ref(), fresh.as_ref()) {
        return guard.clone();
    }
    guard.clone_from(&fresh);
    fresh
}

/// The cached resolution, if present and (for a `System` entry) still
/// pointing at an existing executable file.
async fn cached_cloudflared_if_valid(state: &DaemonState) -> Option<install::Resolved> {
    let cached = state.cloudflared_resolution.read().await;
    let resolved = cached.as_ref()?;
    match resolved.source {
        install::CloudflaredSource::Managed => Some(resolved.clone()),
        install::CloudflaredSource::System => {
            install::is_executable(&resolved.binary).then(|| resolved.clone())
        }
    }
}

/// Whether writing `fresh` over `existing` in the cache would downgrade an
/// established `Managed` resolution to `System` or to nothing. Pure decision
/// logic (no locks, no I/O) so it's directly unit-testable; `resolved_cloudflared`
/// is the only caller, and always calls this under the cache's write lock so
/// the check and the write it gates are atomic.
fn is_downgrade(existing: Option<&install::Resolved>, fresh: Option<&install::Resolved>) -> bool {
    let existing_is_managed = matches!(
        existing.map(|r| r.source),
        Some(install::CloudflaredSource::Managed)
    );
    let fresh_is_managed = matches!(
        fresh.map(|r| r.source),
        Some(install::CloudflaredSource::Managed)
    );
    existing_is_managed && !fresh_is_managed
}

/// `StartQuickTunnel`: publish a site at a random `*.trycloudflare.com` URL.
///
/// Launches a supervised instance pointed at the tunnel listener, then waits for
/// it to settle. A fresh start that never comes up is stopped (its own
/// generation only) and reported as an error, so a share reported as failed
/// never goes live later. An existing instance (say, one retrying after a crash)
/// is left alone and reported as-is. Once a quick tunnel has been online it
/// keeps restarting until stopped. The site's share preflight warnings are
/// recomputed and cached on every call; they never block the start.
pub async fn start_quick_tunnel(site: &str, state: &Arc<DaemonState>) -> Response {
    let Some(resolved) = resolved_cloudflared(state).await else {
        return Response::Error {
            code: ErrorCode::NotFound,
            message: "cloudflared is not installed - install it from Integrations first".into(),
        };
    };

    let Some((name, _host)) = resolve_site(state, site).await else {
        return Response::Error {
            code: ErrorCode::NotFound,
            message: format!("no site named {site:?}"),
        };
    };

    let Some(port) = state.tunnel_origin_port else {
        return origin_not_running();
    };
    refresh_share_warnings(state, &name).await;
    let config_path = match write_quick_config(&state.dirs) {
        Ok(path) => path,
        Err(message) => {
            return Response::Error {
                code: ErrorCode::Internal,
                message,
            }
        }
    };
    let args = yerd_tunnel::args::quick_tunnel_args(&yerd_tunnel::origin_url(port), &config_path);
    let spec = match launch_spec(state, &name, resolved.binary, args, pinned_home_env(state)) {
        Ok(spec) => spec,
        Err(resp) => return resp,
    };

    let launched = match supervise::launch(state, &name, spec, TunnelKind::Quick).await {
        Ok(launched) => launched,
        Err(e) => {
            return Response::Error {
                code: ErrorCode::Internal,
                message: format!("could not start tunnel for {name}: {e}"),
            }
        }
    };
    let generation = match &launched {
        Some(l) => l.generation,
        None => match supervise::current_generation(state, &name).await {
            Some(generation) => generation,
            None => return tunnels_response(state).await,
        },
    };

    match supervise::wait_settled(state, &name, generation, supervise::SETTLE_TIMEOUT).await {
        Settled::Running | Settled::Gone => tunnels_response(state).await,
        Settled::NotRunning { .. } if launched.is_none() => tunnels_response(state).await,
        Settled::NotRunning { last_error } => {
            let stopped = state
                .tunnel_manager
                .lock()
                .await
                .stop_if_generation(&name, generation)
                .await;
            rebuild_routes(state).await;
            if let Err(e) = stopped {
                tracing::warn!(site = %name, error = %e, "could not stop a failed quick tunnel");
            }
            let reason = last_error.unwrap_or_else(|| "it did not come online in time".to_owned());
            Response::Error {
                code: ErrorCode::Internal,
                message: format!("could not start tunnel for {name}: {reason}"),
            }
        }
    }
}

/// The reply when the tunnel origin listener failed to bind at startup.
pub(super) fn origin_not_running() -> Response {
    Response::Error {
        code: ErrorCode::Internal,
        message: "tunnel origin listener is not running".into(),
    }
}

/// Daemon variables forwarded into a `cloudflared` child after the manager
/// clears its inherited environment. The child's env is otherwise fully pinned
/// (see [`yerd_tunnel::LaunchSpec::env`]); these are the few variables
/// `cloudflared` legitimately needs and that are safe to pass through. `PATH`
/// and `TMPDIR` keep subprocess/temp behaviour sane, and `SSL_CERT_FILE`/
/// `SSL_CERT_DIR` let edge-TLS trust-store discovery work on Linux distros that
/// rely on them. Secrets and unrelated daemon config are deliberately excluded.
const FORWARDED_ENV: [&str; 4] = ["PATH", "TMPDIR", "SSL_CERT_FILE", "SSL_CERT_DIR"];

/// The pinned environment for a `cloudflared` subprocess: `HOME` points at the
/// daemon-owned `{data}/tunnel` dir so `cloudflared` never reads or writes
/// `~/.cloudflared`, plus the [`FORWARDED_ENV`] allowlist the cleared child still
/// needs. Named runs add `TUNNEL_ORIGIN_CERT` on top of this.
pub(super) fn pinned_home_env(
    state: &DaemonState,
) -> Vec<(std::ffi::OsString, std::ffi::OsString)> {
    let mut env = vec![(
        "HOME".into(),
        install::tunnel_dir(&state.dirs).into_os_string(),
    )];
    for key in FORWARDED_ENV {
        if let Some(val) = std::env::var_os(key) {
            env.push((key.into(), val));
        }
    }
    env
}

/// Resolve a site by name or `.test` host to its `(name, host)`, where `host` is
/// the site's primary domain FQDN. Reads the router under a brief lock released
/// before the caller takes any other lock. Returns `None` when the site's
/// primary host does not resolve back to it - a shadowed apex whose exact label
/// is claimed by another site (so its effective set is wildcard-only) has no
/// concrete host that routes to it and cannot be tunneled; `yerd doctor` flags
/// the shadow.
pub(super) async fn resolve_site(state: &DaemonState, site: &str) -> Option<(String, String)> {
    let router = state.router.read().await;
    let s = router.resolve(site).or_else(|| router.get(site))?;
    let host = router.primary_fqdn(s.name());
    if router.resolve(&host).map(yerd_core::Site::name) != Some(s.name()) {
        return None;
    }
    Some((s.name().to_owned(), host))
}

/// `StopTunnel`: tear down a site's tunnel (no-op if none) and drop its host
/// from the route map. The instance's driver sees it gone and exits. `site` may
/// be a site name or `.test` host; one that no longer resolves (a removed site)
/// is used as given.
pub async fn stop_tunnel(site: &str, state: &DaemonState) -> Response {
    let key = canonical_site_name(state, site)
        .await
        .unwrap_or_else(|| site.to_owned());
    let stopped = {
        let mut mgr = state.tunnel_manager.lock().await;
        mgr.stop(&key).await
    };
    rebuild_routes(state).await;
    if let Err(e) = stopped {
        return Response::Error {
            code: ErrorCode::Internal,
            message: format!("could not stop tunnel for {site}: {e}"),
        };
    }
    tunnels_response(state).await
}

/// `TunnelStatus`: the live tunnels plus `cloudflared` install status.
pub async fn tunnel_status(state: &DaemonState) -> Response {
    tunnels_response(state).await
}

/// Number of distinct sites currently shared to the public internet: sites with
/// a live quick tunnel, plus the sites the named tunnel exposes when it is live.
/// Live means connecting, online, or reconnecting. A site shared via both is
/// counted once. Surfaced in `StatusReport` so the GUI can badge the count.
pub async fn shared_site_count(state: &DaemonState) -> u32 {
    let (mut shared, named_running) = {
        let mut mgr = state.tunnel_manager.lock().await;
        let snaps = mgr.snapshots();
        let shared: std::collections::BTreeSet<String> = snaps
            .iter()
            .filter(|s| matches!(s.kind, TunnelKind::Quick) && is_live(s.state))
            .map(|s| s.site.clone())
            .collect();
        let named_running = snaps
            .iter()
            .any(|s| matches!(s.kind, TunnelKind::Named) && is_live(s.state));
        (shared, named_running)
    };
    if named_running {
        shared.extend(canonical_tunnel_config(state).await.0.into_keys());
    }
    u32::try_from(shared.len()).unwrap_or(u32::MAX)
}

/// Whether a reported state counts as an active share.
fn is_live(state: TunnelState) -> bool {
    matches!(
        state,
        TunnelState::Connecting | TunnelState::Running | TunnelState::Reconnecting
    )
}

/// Shared `Response::Tunnels` builder. Reads the cached share warnings only
/// (never the filesystem): a quick entry gets its own site's, the named entry
/// gets every currently mapped site's, prefixed `"<site>: "`.
pub(super) async fn tunnels_response(state: &DaemonState) -> Response {
    let snapshots = {
        let mut mgr = state.tunnel_manager.lock().await;
        mgr.snapshots()
    };
    let (sites, passwords) = canonical_tunnel_config(state).await;
    let mapped: Vec<String> = sites.into_keys().collect();
    let tunnels = {
        let cache = state.share_warnings.lock().await;
        snapshots
            .into_iter()
            .map(|s| {
                let (protected, warnings) = share_extras(&s, &mapped, &passwords, &cache);
                to_wire(s, protected, warnings)
            })
            .collect()
    };
    Response::Tunnels {
        tunnels,
        cloudflared: cloudflared_status(state).await,
    }
}

/// The `protected` flag and cached warnings reported for one snapshot. A quick
/// tunnel is protected when its site has a share password; the named tunnel's
/// protection is reported per site (see `named::list`), so it is never flagged
/// here.
fn share_extras(
    snapshot: &TunnelSnapshot,
    mapped: &[String],
    passwords: &BTreeMap<String, String>,
    cache: &HashMap<String, Vec<String>>,
) -> (bool, Vec<String>) {
    match snapshot.kind {
        TunnelKind::Quick => (
            passwords.contains_key(&snapshot.site),
            cache.get(&snapshot.site).cloned().unwrap_or_default(),
        ),
        TunnelKind::Named => (
            false,
            mapped
                .iter()
                .flat_map(|site| {
                    cache
                        .get(site)
                        .into_iter()
                        .flatten()
                        .map(move |w| format!("{site}: {w}"))
                })
                .collect(),
        ),
    }
}

/// Compute `site`'s share preflight warnings: the router facts are read under a
/// brief read guard, then the `hot` file check and the `.env` read run on the
/// blocking pool. An unknown site, or a failed read task, yields no warnings.
pub(super) async fn compute_share_warnings(state: &DaemonState, site: &str) -> Vec<String> {
    let facts = {
        let router = state.router.read().await;
        router.get(site).map(|s| {
            (
                s.document_root().to_path_buf(),
                s.served_root(),
                router.config().tld().to_owned(),
            )
        })
    };
    let Some((document_root, served_root, tld)) = facts else {
        return Vec::new();
    };
    let read = tokio::task::spawn_blocking(move || {
        let vite_hot = served_root.join("hot").is_file();
        let dotenv = std::fs::read_to_string(document_root.join(".env")).ok();
        (vite_hot, dotenv)
    })
    .await;
    let Ok((vite_hot, dotenv)) = read else {
        return Vec::new();
    };
    yerd_tunnel::preflight(&yerd_tunnel::ShareFacts {
        vite_hot,
        dotenv: dotenv.as_deref(),
        tld: &tld,
    })
}

/// Recompute and cache `site`'s share warnings. Called only when a share
/// starts; a new start overwrites the entry.
pub(super) async fn refresh_share_warnings(state: &DaemonState, site: &str) {
    let warnings = compute_share_warnings(state, site).await;
    state
        .share_warnings
        .lock()
        .await
        .insert(site.to_owned(), warnings);
}

/// Refresh the cached share warnings of every site mapped on the named tunnel.
pub(super) async fn refresh_named_share_warnings(state: &DaemonState) {
    let mapped = canonical_tunnel_config(state).await.0;
    for site in mapped.into_keys() {
        refresh_share_warnings(state, &site).await;
    }
}

/// Rebuild the tunnel origin listener's `public host -> site` route map from
/// config (named-tunnel hostnames and share passwords, keyed by canonical site
/// name, see [`canonical_tunnel_config`]) plus the URLs of the live quick
/// tunnels, then swap it in. See the module doc for the lock order.
///
/// A quick tunnel whose site is missing from the router snapshot the passwords
/// were resolved against gets no route: its password would have been dropped
/// with the site, so routing it could serve the site unprotected once the
/// router has it back. The next rebuild restores it with its password.
pub async fn rebuild_routes(state: &DaemonState) {
    let _rebuild = state.tunnel_routes_rebuild.lock().await;
    let quick: Vec<(String, String)> = {
        let mut mgr = state.tunnel_manager.lock().await;
        mgr.snapshots()
            .into_iter()
            .filter(|s| matches!(s.kind, TunnelKind::Quick))
            .filter_map(|s| s.url.map(|url| (s.site, url)))
            .collect()
    };
    let (raw_sites, raw_passwords) = {
        let cfg = state.config.lock().await;
        (cfg.tunnel.sites.clone(), cfg.tunnel.passwords.clone())
    };
    let (sites, passwords, quick) = {
        let router = state.router.read().await;
        let resolve = |key: &str| canonical_in(&router, key);
        let quick: Vec<(String, String)> = quick
            .into_iter()
            .filter(|(site, _)| router.get(site).is_some())
            .collect();
        (
            canonicalize_keys(&raw_sites, resolve),
            canonicalize_keys(&raw_passwords, resolve),
            quick,
        )
    };
    let routes = build_routes(&sites, &passwords, &quick);
    *state.tunnel_routes.write().await = routes;
}

/// The canonical site name for a tunnel config key: an exact site name, or a
/// `.test` host that routes to a site. `None` when it names no site.
pub(super) fn canonical_in(router: &yerd_core::SiteRouter, key: &str) -> Option<String> {
    router
        .get(key)
        .or_else(|| router.resolve(key))
        .map(|s| s.name().to_owned())
}

/// [`canonical_in`] against the live router, under a brief read guard.
pub(super) async fn canonical_site_name(state: &DaemonState, key: &str) -> Option<String> {
    canonical_in(&*state.router.read().await, key)
}

/// Every `[tunnel.sites]` or `[tunnel.passwords]` key that resolves to the site
/// `name`: the canonical key itself plus any legacy `.test` host aliases. The
/// config lock is released before the router read guard is taken.
pub(super) async fn tunnel_keys_for(state: &DaemonState, name: &str) -> Vec<String> {
    let keys: std::collections::BTreeSet<String> = {
        let cfg = state.config.lock().await;
        cfg.tunnel
            .sites
            .keys()
            .chain(cfg.tunnel.passwords.keys())
            .cloned()
            .collect()
    };
    let router = state.router.read().await;
    keys.into_iter()
        .filter(|key| key == name || canonical_in(&router, key).as_deref() == Some(name))
        .collect()
}

/// `[tunnel.sites]` and `[tunnel.passwords]` rekeyed by canonical site name.
///
/// Older builds accepted a site's `.test` host as the key, and such entries may
/// still be on disk; resolving them here keeps those shares routing. Keys that
/// resolve to no site are dropped. The config lock is taken and released first,
/// then the router read guard.
pub(super) async fn canonical_tunnel_config(
    state: &DaemonState,
) -> (BTreeMap<String, String>, BTreeMap<String, String>) {
    let (sites, passwords) = {
        let cfg = state.config.lock().await;
        (cfg.tunnel.sites.clone(), cfg.tunnel.passwords.clone())
    };
    let router = state.router.read().await;
    let resolve = |key: &str| canonical_in(&router, key);
    (
        canonicalize_keys(&sites, resolve),
        canonicalize_keys(&passwords, resolve),
    )
}

/// Rekey `map` through `resolve`, dropping keys it cannot resolve. When a
/// legacy key and the canonical key both map to the same site, the entry stored
/// under the canonical key wins.
fn canonicalize_keys(
    map: &BTreeMap<String, String>,
    resolve: impl Fn(&str) -> Option<String>,
) -> BTreeMap<String, String> {
    let resolved: Vec<(String, &String, bool)> = map
        .iter()
        .filter_map(|(key, value)| resolve(key).map(|name| (name.clone(), value, name == *key)))
        .collect();
    let mut out = BTreeMap::new();
    for (name, value, _) in resolved.iter().filter(|(_, _, exact)| *exact) {
        out.insert(name.clone(), (*value).clone());
    }
    for (name, value, _) in resolved.iter().filter(|(_, _, exact)| !*exact) {
        out.entry(name.clone()).or_insert_with(|| (*value).clone());
    }
    out
}

/// Pure route-map builder behind [`rebuild_routes`]. `sites` is the named
/// tunnel's `site -> hostname` map, `passwords` the `site -> password` map, and
/// `quick` the live quick tunnels as `(site, public URL)`. Keys are normalized
/// hosts. Named entries go in first and a quick entry never overrides a named
/// key. A site's password protects it on every tunnel kind.
fn build_routes(
    sites: &BTreeMap<String, String>,
    passwords: &BTreeMap<String, String>,
    quick: &[(String, String)],
) -> HashMap<String, yerd_proxy::TunnelRoute> {
    let route_for = |site: &str| yerd_proxy::TunnelRoute {
        site: site.to_owned(),
        password: passwords.get(site).cloned(),
    };
    let mut routes = HashMap::new();
    for (site, hostname) in sites {
        if let Some(host) = yerd_proxy::pure::tunnel::normalize_host(hostname) {
            routes.insert(host, route_for(site));
        }
    }
    for (site, url) in quick {
        if let Some(host) = url_host(url) {
            routes.entry(host).or_insert_with(|| route_for(site));
        }
    }
    routes
}

/// The normalized host of an `http(s)://host[:port][/path]` URL.
fn url_host(url: &str) -> Option<String> {
    let rest = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))?;
    let authority = rest.split(['/', '?', '#']).next()?;
    yerd_proxy::pure::tunnel::normalize_host(authority)
}

/// Map a manager snapshot, plus its share extras, to its wire form.
fn to_wire(s: TunnelSnapshot, protected: bool, warnings: Vec<String>) -> TunnelInfo {
    TunnelInfo {
        site: s.site,
        kind: match s.kind {
            TunnelKind::Quick => WireKind::Quick,
            TunnelKind::Named => WireKind::Named,
        },
        state: match s.state {
            TunnelState::Connecting => TunnelRunState::Connecting,
            TunnelState::Running => TunnelRunState::Running,
            TunnelState::Reconnecting => TunnelRunState::Reconnecting,
            TunnelState::Failed => TunnelRunState::Failed,
        },
        url: s.url,
        hostname: None,
        last_error: s.last_error,
        protected,
        warnings,
    }
}

/// Overwrite the `cloudflared`-resolution cache to the freshly installed
/// managed binary right after a successful install, so `TunnelStatus` and the
/// next tunnel action reflect it immediately rather than waiting on a stale
/// cached `System` entry that hasn't yet failed its existence re-check (see
/// `resolved_cloudflared`). Writing `Managed` here is never a downgrade, so it
/// doesn't need the no-downgrade check `resolved_cloudflared` itself applies.
async fn refresh_cloudflared_cache_after_install(state: &DaemonState) {
    let resolved = install::Resolved {
        binary: install::binary_path(&state.dirs),
        source: install::CloudflaredSource::Managed,
        version: install::installed_version(&state.dirs),
    };
    *state.cloudflared_resolution.write().await = Some(resolved);
}

/// `InstallCloudflaredStreamed`: download `cloudflared` as a background job,
/// streaming progress into the job log. Returns `JobStarted` immediately; the
/// client polls `JobStatus`.
pub async fn install_cloudflared_streamed(state: Arc<DaemonState>) -> Response {
    let (job_id, mut cancel) = state.jobs.create().await;
    let id = job_id.clone();
    tokio::spawn(async move {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<String>();
        let drain = {
            let state = state.clone();
            let id = id.clone();
            tokio::spawn(async move {
                while let Some(line) = rx.recv().await {
                    state.jobs.push_log(&id, line).await;
                }
            })
        };

        state.jobs.set_phase(&id, "Installing cloudflared").await;
        let dl = crate::network::downloader(&state).await;
        let guard = tokio::select! {
            g = state.tunnel_mutate.lock() => g,
            _ = cancel.changed() => {
                drop(tx);
                let _ = drain.await;
                state
                    .jobs
                    .finish(&id, yerd_ipc::JobState::Cancelled, None)
                    .await;
                return;
            }
        };
        let result = tokio::select! {
            r = install::install(&state.dirs, &dl, Some(&tx)) => Some(r),
            _ = cancel.changed() => None,
        };
        drop(guard);
        drop(tx);
        let _ = drain.await;

        match result {
            Some(Ok(())) => {
                refresh_cloudflared_cache_after_install(&state).await;
                crate::ipc_server::reconcile_client_shims_now(&state).await;
                state
                    .jobs
                    .finish(&id, yerd_ipc::JobState::Succeeded, None)
                    .await;
            }
            Some(Err(e)) => {
                state
                    .jobs
                    .finish(&id, yerd_ipc::JobState::Failed, Some(e.to_string()))
                    .await;
            }
            None => {
                state
                    .jobs
                    .finish(&id, yerd_ipc::JobState::Cancelled, None)
                    .await;
            }
        }
    });
    Response::JobStarted { job_id }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::panic, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use crate::test_support::state_in;

    fn route(site: &str, password: Option<&str>) -> yerd_proxy::TunnelRoute {
        yerd_proxy::TunnelRoute {
            site: site.to_owned(),
            password: password.map(str::to_owned),
        }
    }

    fn map(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect()
    }

    #[test]
    fn build_routes_maps_named_hostnames() {
        let routes = build_routes(&map(&[("blog", "Blog.Example.com.")]), &map(&[]), &[]);
        assert_eq!(
            routes,
            HashMap::from([("blog.example.com".to_owned(), route("blog", None))])
        );
    }

    #[test]
    fn build_routes_maps_quick_url_hosts() {
        let quick = vec![(
            "app".to_owned(),
            "https://calm-river-1234.trycloudflare.com".to_owned(),
        )];
        let routes = build_routes(&map(&[]), &map(&[]), &quick);
        assert_eq!(
            routes,
            HashMap::from([(
                "calm-river-1234.trycloudflare.com".to_owned(),
                route("app", None)
            )])
        );
    }

    #[test]
    fn build_routes_attaches_passwords_to_every_tunnel_kind() {
        let quick = vec![("blog".to_owned(), "https://q.trycloudflare.com/".to_owned())];
        let routes = build_routes(
            &map(&[("blog", "blog.example.com"), ("shop", "shop.example.com")]),
            &map(&[("blog", "secret")]),
            &quick,
        );
        assert_eq!(
            routes.get("blog.example.com"),
            Some(&route("blog", Some("secret")))
        );
        assert_eq!(
            routes.get("q.trycloudflare.com"),
            Some(&route("blog", Some("secret")))
        );
        assert_eq!(routes.get("shop.example.com"), Some(&route("shop", None)));
    }

    #[test]
    fn build_routes_never_lets_a_quick_entry_override_a_named_one() {
        let quick = vec![("other".to_owned(), "https://blog.example.com".to_owned())];
        let routes = build_routes(&map(&[("blog", "blog.example.com")]), &map(&[]), &quick);
        assert_eq!(routes.len(), 1);
        assert_eq!(routes.get("blog.example.com"), Some(&route("blog", None)));
    }

    #[test]
    fn url_host_table() {
        for (url, expected) in [
            ("https://a.trycloudflare.com", Some("a.trycloudflare.com")),
            (
                "https://A.trycloudflare.com:443/path?q",
                Some("a.trycloudflare.com"),
            ),
            ("http://a.example.com/", Some("a.example.com")),
            ("a.example.com", None),
            ("https://", None),
        ] {
            assert_eq!(url_host(url).as_deref(), expected, "{url}");
        }
    }

    fn snapshot(site: &str, kind: TunnelKind) -> TunnelSnapshot {
        TunnelSnapshot {
            site: site.to_owned(),
            kind,
            state: TunnelState::Running,
            pid: None,
            url: None,
            generation: 1,
            last_error: None,
            failures: 0,
        }
    }

    #[test]
    fn share_extras_table() {
        let mapped = vec!["blog".to_owned(), "shop".to_owned()];
        let passwords = map(&[("app", "pw"), ("blog", "pw")]);
        let cache = HashMap::from([
            ("app".to_owned(), vec!["a1".to_owned()]),
            ("blog".to_owned(), vec!["b1".to_owned(), "b2".to_owned()]),
            ("shop".to_owned(), Vec::new()),
            ("other".to_owned(), vec!["o1".to_owned()]),
        ]);
        let cases: &[(TunnelSnapshot, bool, Vec<&str>)] = &[
            (snapshot("app", TunnelKind::Quick), true, vec!["a1"]),
            (snapshot("shop", TunnelKind::Quick), false, vec![]),
            (snapshot("new", TunnelKind::Quick), false, vec![]),
            (
                snapshot("@named", TunnelKind::Named),
                false,
                vec!["blog: b1", "blog: b2"],
            ),
        ];
        for (snap, protected, warnings) in cases {
            assert_eq!(
                share_extras(snap, &mapped, &passwords, &cache),
                (
                    *protected,
                    warnings.iter().map(|w| (*w).to_owned()).collect()
                ),
                "{}",
                snap.site
            );
        }
    }

    async fn state_with_site_root(tmp: &std::path::Path) -> (DaemonState, PathBuf) {
        let state = state_in(tmp);
        let root = tmp.join("app");
        std::fs::create_dir_all(root.join("public")).unwrap();
        let mut site =
            yerd_core::Site::linked("app", &root, yerd_core::PhpVersion::new(8, 3)).unwrap();
        site.set_web_subpath("public");
        state.router.write().await.insert(site).unwrap();
        (state, root)
    }

    #[tokio::test]
    async fn compute_share_warnings_reads_the_hot_file_and_env() {
        let tmp = tempfile::tempdir().unwrap();
        let (state, root) = state_with_site_root(tmp.path()).await;
        assert!(compute_share_warnings(&state, "app").await.is_empty());

        std::fs::write(root.join("public/hot"), "http://localhost:5173").unwrap();
        std::fs::write(root.join(".env"), "APP_NAME=x\nSESSION_DOMAIN=.app.test\n").unwrap();
        let warnings = compute_share_warnings(&state, "app").await;

        assert_eq!(warnings.len(), 2, "{warnings:?}");
        assert_eq!(warnings[0], yerd_tunnel::preflight::VITE_HOT_WARNING);
        assert!(warnings[1].contains("SESSION_DOMAIN"), "{warnings:?}");
        assert!(compute_share_warnings(&state, "nope").await.is_empty());
    }

    #[tokio::test]
    async fn refresh_share_warnings_overwrites_the_cached_entry() {
        let tmp = tempfile::tempdir().unwrap();
        let (state, root) = state_with_site_root(tmp.path()).await;
        std::fs::write(root.join("public/hot"), "").unwrap();
        refresh_share_warnings(&state, "app").await;
        assert_eq!(state.share_warnings.lock().await["app"].len(), 1);

        std::fs::remove_file(root.join("public/hot")).unwrap();
        refresh_share_warnings(&state, "app").await;

        assert!(state.share_warnings.lock().await["app"].is_empty());
    }

    type Pairs<'a> = &'a [(&'a str, &'a str)];

    #[test]
    fn canonicalize_keys_table() {
        let resolve = |key: &str| match key {
            "app" | "app.test" | "www.app.test" => Some("app".to_owned()),
            "blog" => Some("blog".to_owned()),
            _ => None,
        };
        let cases: &[(Pairs, Pairs)] = &[
            (&[("app", "a")], &[("app", "a")]),
            (&[("app.test", "a")], &[("app", "a")]),
            (&[("app", "new"), ("app.test", "old")], &[("app", "new")]),
            (
                &[("www.app.test", "old"), ("app", "new")],
                &[("app", "new")],
            ),
            (&[("gone.test", "x"), ("blog", "b")], &[("blog", "b")]),
        ];
        for (input, expected) in cases {
            assert_eq!(
                canonicalize_keys(&map(input), resolve),
                map(expected),
                "{input:?}"
            );
        }
    }

    #[tokio::test]
    async fn rebuild_routes_resolves_legacy_host_keys_and_skips_unknown_sites() {
        let tmp = tempfile::tempdir().unwrap();
        let state = state_in(tmp.path());
        let site =
            yerd_core::Site::linked("blog", "/srv/www/blog", yerd_core::PhpVersion::new(8, 3))
                .unwrap();
        state.router.write().await.insert(site).unwrap();
        {
            let mut cfg = state.config.lock().await;
            cfg.tunnel
                .sites
                .insert("blog.test".to_owned(), "blog.example.com".to_owned());
            cfg.tunnel
                .sites
                .insert("gone".to_owned(), "gone.example.com".to_owned());
            cfg.tunnel
                .passwords
                .insert("blog.test".to_owned(), "secret".to_owned());
        }

        rebuild_routes(&state).await;

        assert_eq!(
            *state.tunnel_routes.read().await,
            HashMap::from([("blog.example.com".to_owned(), route("blog", Some("secret")))])
        );
    }

    #[tokio::test]
    async fn rebuild_routes_reads_config_and_swaps_the_map() {
        let tmp = tempfile::tempdir().unwrap();
        let state = state_in(tmp.path());
        let site =
            yerd_core::Site::linked("blog", "/srv/www/blog", yerd_core::PhpVersion::new(8, 3))
                .unwrap();
        state.router.write().await.insert(site).unwrap();
        {
            let mut cfg = state.config.lock().await;
            cfg.tunnel
                .sites
                .insert("blog".to_owned(), "blog.example.com".to_owned());
            cfg.tunnel
                .passwords
                .insert("blog".to_owned(), "secret".to_owned());
        }
        state
            .tunnel_routes
            .write()
            .await
            .insert("stale.example.com".to_owned(), route("gone", None));

        rebuild_routes(&state).await;

        assert_eq!(
            *state.tunnel_routes.read().await,
            HashMap::from([("blog.example.com".to_owned(), route("blog", Some("secret")))])
        );
    }

    #[tokio::test]
    async fn resolved_cloudflared_reflects_a_fresh_install_without_restart() {
        let tmp = tempfile::tempdir().unwrap();
        let state = state_in(tmp.path());

        install::install_binary_for_test(&state.dirs, "2026.6.1", b"#!/bin/sh\n");
        refresh_cloudflared_cache_after_install(&state).await;

        let resolved = resolved_cloudflared(&state).await.unwrap();
        assert_eq!(resolved.source, install::CloudflaredSource::Managed);
        assert_eq!(resolved.version.as_deref(), Some("2026.6.1"));
    }

    fn managed(version: &str) -> install::Resolved {
        install::Resolved {
            binary: PathBuf::from("/managed/cloudflared"),
            source: install::CloudflaredSource::Managed,
            version: Some(version.to_owned()),
        }
    }

    fn system(version: &str) -> install::Resolved {
        install::Resolved {
            binary: PathBuf::from("/usr/local/bin/cloudflared"),
            source: install::CloudflaredSource::System,
            version: Some(version.to_owned()),
        }
    }

    /// Exhaustive, pure-logic coverage of the write-time no-downgrade guard
    /// itself, independent of locks/timing - this is the actual regression
    /// protection for "a stale System/None write must never clobber an
    /// established Managed cache entry".
    #[test]
    fn is_downgrade_covers_every_existing_fresh_combination() {
        // An established Managed entry must never be replaced by System or by
        // nothing (a failed re-resolution).
        assert!(is_downgrade(Some(&managed("1")), Some(&system("2"))));
        assert!(is_downgrade(Some(&managed("1")), None));
        // Managed may always be refreshed by a fresh Managed (e.g. a version
        // bump from a second install).
        assert!(!is_downgrade(Some(&managed("1")), Some(&managed("2"))));
        // Nothing else is ever a downgrade: no existing entry, or an existing
        // System entry, can freely be overwritten by any fresh result.
        assert!(!is_downgrade(None, Some(&system("1"))));
        assert!(!is_downgrade(None, Some(&managed("1"))));
        assert!(!is_downgrade(None, None));
        assert!(!is_downgrade(Some(&system("1")), Some(&system("2"))));
        assert!(!is_downgrade(Some(&system("1")), Some(&managed("1"))));
        assert!(!is_downgrade(Some(&system("1")), None));
    }

    #[tokio::test]
    async fn resolved_cloudflared_never_downgrades_a_cached_managed_entry() {
        let tmp = tempfile::tempdir().unwrap();
        let state = state_in(tmp.path());

        install::install_binary_for_test(&state.dirs, "2026.6.1", b"#!/bin/sh\n");
        refresh_cloudflared_cache_after_install(&state).await;
        assert_eq!(
            resolved_cloudflared(&state).await.unwrap().source,
            install::CloudflaredSource::Managed
        );

        // A cached Managed entry short-circuits in `cached_cloudflared_if_valid`
        // before any fresh resolution (and thus the no-downgrade write) is even
        // attempted, so repeated calls keep reporting Managed regardless of
        // what the real PATH/managed-dir state is.
        for _ in 0..3 {
            assert_eq!(
                resolved_cloudflared(&state).await.unwrap().source,
                install::CloudflaredSource::Managed
            );
        }
    }

    #[tokio::test]
    async fn a_removed_system_binary_is_dropped_from_the_cache_on_next_read() {
        let tmp = tempfile::tempdir().unwrap();
        let state = state_in(tmp.path());
        let candidate = tmp.path().join("cloudflared");
        std::fs::write(&candidate, b"#!/bin/sh\n").unwrap();
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(&candidate, std::fs::Permissions::from_mode(0o755)).unwrap();
        }

        *state.cloudflared_resolution.write().await = Some(install::Resolved {
            binary: candidate.clone(),
            source: install::CloudflaredSource::System,
            version: Some("2024.6.1".into()),
        });
        assert!(
            cached_cloudflared_if_valid(&state).await.is_some(),
            "precondition: the cached System entry should still be valid"
        );

        std::fs::remove_file(&candidate).unwrap();
        assert!(
            cached_cloudflared_if_valid(&state).await.is_none(),
            "a System entry whose binary has disappeared must not be served from cache"
        );
    }
}
