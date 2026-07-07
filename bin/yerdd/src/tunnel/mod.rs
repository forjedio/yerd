//! Cloudflare Tunnel wiring: the daemon's `TunnelManager` type, the IPC handlers
//! for the tunnel requests, the `cloudflared` install job, and the
//! snapshot→wire mapping.
//!
//! Lock discipline mirrors the services path: the site facts are read under the
//! router lock, which is released before the (slow) `tunnel_manager` `ensure`;
//! the config/router lock and the tunnel-manager lock are never held
//! simultaneously across an `.await`. The `cloudflared` install runs under
//! `tunnel_mutate` only.

pub mod credentials;
pub mod install;
pub mod named;

use std::path::PathBuf;
use std::sync::Arc;

use yerd_ipc::{
    CloudflaredStatus, ErrorCode, Response, TunnelInfo, TunnelKind as WireKind, TunnelRunState,
};
use yerd_platform::PlatformDirs;
use yerd_supervise::{SystemClock, TokioProcessSpawner};
use yerd_tunnel::manager::{TunnelSnapshot, TunnelState};
use yerd_tunnel::{OriginTarget, Step, TunnelKind, TunnelManager};

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
/// Registers + spawns the child under a brief manager lock, then drives
/// readiness with the lock released between ticks (see [`yerd_tunnel`]'s tick
/// model) so `StopTunnel`/`TunnelStatus`/shutdown stay responsive and a stuck
/// connect can be cancelled.
pub async fn start_quick_tunnel(site: &str, state: &DaemonState) -> Response {
    let Some(resolved) = resolved_cloudflared(state).await else {
        return Response::Error {
            code: ErrorCode::NotFound,
            message: "cloudflared is not installed - install it from Integrations first".into(),
        };
    };

    let Some((name, secure, tld)) = resolve_site(state, site).await else {
        return Response::Error {
            code: ErrorCode::NotFound,
            message: format!("no site named {site:?}"),
        };
    };

    let origin = OriginTarget::for_site(&name, &tld, secure, state.http.bound, state.https.bound);
    let args = yerd_tunnel::args::quick_tunnel_args(&origin);
    run_to_ready(
        state,
        &name,
        resolved.binary,
        args,
        pinned_home_env(state),
        TunnelKind::Quick,
        None,
    )
    .await
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

/// Resolve a site by name or `.test` host to its `(name, secure, tld)`, reading
/// the router under a brief lock that is released before the caller takes any
/// other lock.
pub(super) async fn resolve_site(
    state: &DaemonState,
    site: &str,
) -> Option<(String, bool, String)> {
    let router = state.router.read().await;
    router.resolve(site).or_else(|| router.get(site)).map(|s| {
        (
            s.name().to_owned(),
            s.secure(),
            router.config().tld().to_owned(),
        )
    })
}

/// Register + spawn a tunnel for `name` under a brief manager lock, then drive
/// readiness with the lock released between ticks (see [`yerd_tunnel`]'s tick
/// model). Shared by the Quick and Named start paths. `binary` is the
/// already-resolved `cloudflared` to spawn (see `resolved_cloudflared`).
/// Returns the live `Response::Tunnels` on readiness, or a `Response::Error`
/// on failure.
pub(super) async fn run_to_ready(
    state: &DaemonState,
    name: &str,
    binary: PathBuf,
    args: Vec<std::ffi::OsString>,
    env: Vec<(std::ffi::OsString, std::ffi::OsString)>,
    kind: TunnelKind,
    hostname: Option<String>,
) -> Response {
    let logfile = logfile(&state.dirs, name);
    if let Some(parent) = logfile.parent() {
        if let Err(e) = crate::secure_fs::create_private_dir(parent) {
            return Response::Error {
                code: ErrorCode::Internal,
                message: format!("could not prepare tunnel log dir: {e}"),
            };
        }
    }

    let spec = yerd_tunnel::LaunchSpec {
        binary,
        args,
        env,
        logfile,
    };
    let started = {
        let mut mgr = state.tunnel_manager.lock().await;
        mgr.begin(name, spec, kind, hostname).await
    };
    match started {
        Ok(false) => return tunnels_response(state).await,
        Ok(true) => {}
        Err(e) => {
            return Response::Error {
                code: ErrorCode::Internal,
                message: format!("could not start tunnel for {name}: {e}"),
            }
        }
    }

    loop {
        let step = {
            let mut mgr = state.tunnel_manager.lock().await;
            mgr.advance(name).await
        };
        match step {
            Ok(Step::Continue) => {}
            Ok(Step::Sleep(d)) => tokio::time::sleep(d).await,
            Ok(Step::Ready | Step::Gone) => return tunnels_response(state).await,
            Err(e) => {
                let _ = state.tunnel_manager.lock().await.stop(name).await;
                return Response::Error {
                    code: ErrorCode::Internal,
                    message: format!("could not start tunnel for {name}: {e}"),
                };
            }
        }
    }
}

/// `StopTunnel`: tear down a site's tunnel (no-op if none).
pub async fn stop_tunnel(site: &str, state: &DaemonState) -> Response {
    {
        let mut mgr = state.tunnel_manager.lock().await;
        if let Err(e) = mgr.stop(site).await {
            return Response::Error {
                code: ErrorCode::Internal,
                message: format!("could not stop tunnel for {site}: {e}"),
            };
        }
    }
    tunnels_response(state).await
}

/// `TunnelStatus`: the live tunnels plus `cloudflared` install status.
pub async fn tunnel_status(state: &DaemonState) -> Response {
    tunnels_response(state).await
}

/// Number of distinct sites currently shared to the public internet: sites with
/// a live quick tunnel, plus the sites the named tunnel exposes when it is
/// running. A site shared via both is counted once. Surfaced in `StatusReport`
/// so the GUI can badge the count.
pub async fn shared_site_count(state: &DaemonState) -> u32 {
    let (mut shared, named_running) = {
        let mut mgr = state.tunnel_manager.lock().await;
        let snaps = mgr.snapshots();
        let shared: std::collections::BTreeSet<String> = snaps
            .iter()
            .filter(|s| matches!(s.kind, TunnelKind::Quick) && s.state == TunnelState::Running)
            .map(|s| s.site.clone())
            .collect();
        let named_running = snaps
            .iter()
            .any(|s| matches!(s.kind, TunnelKind::Named) && s.state == TunnelState::Running);
        (shared, named_running)
    };
    if named_running {
        for site in state.config.lock().await.tunnel.sites.keys() {
            shared.insert(site.clone());
        }
    }
    u32::try_from(shared.len()).unwrap_or(u32::MAX)
}

/// Shared `Response::Tunnels` builder.
pub(super) async fn tunnels_response(state: &DaemonState) -> Response {
    let tunnels = {
        let mut mgr = state.tunnel_manager.lock().await;
        mgr.snapshots().into_iter().map(to_wire).collect()
    };
    Response::Tunnels {
        tunnels,
        cloudflared: cloudflared_status(state).await,
    }
}

/// Map a manager snapshot to its wire form.
fn to_wire(s: TunnelSnapshot) -> TunnelInfo {
    TunnelInfo {
        site: s.site,
        kind: match s.kind {
            TunnelKind::Quick => WireKind::Quick,
            TunnelKind::Named => WireKind::Named,
        },
        state: match s.state {
            TunnelState::Running => TunnelRunState::Running,
            TunnelState::Failed => TunnelRunState::Failed,
        },
        url: s.url,
        hostname: s.hostname,
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
        let dl = crate::php_install::ReqwestDownloader::new();
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
#[allow(clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;
    use crate::test_support::state_in;

    #[tokio::test]
    async fn resolved_cloudflared_reflects_a_fresh_install_without_restart() {
        let tmp = tempfile::tempdir().unwrap();
        let state = state_in(tmp.path());

        // Nothing installed, and no `cloudflared` on this test's real `PATH`
        // (near-certain in CI): resolves to `None`.
        assert!(resolved_cloudflared(&state).await.is_none());

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
