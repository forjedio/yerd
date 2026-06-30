//! Cloudflare Tunnel wiring: the daemon's `TunnelManager` type, the IPC handlers
//! for the tunnel requests, the `cloudflared` install job, and the
//! snapshot→wire mapping.
//!
//! Lock discipline mirrors the services path: the site facts are read under the
//! router lock, which is released before the (slow) `tunnel_manager` `ensure`;
//! the config/router lock and the tunnel-manager lock are never held
//! simultaneously across an `.await`. The `cloudflared` install runs under
//! `tunnel_mutate` only.

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
#[must_use]
pub fn cloudflared_status(dirs: &PlatformDirs) -> CloudflaredStatus {
    CloudflaredStatus {
        installed: install::is_installed(dirs),
        version: install::installed_version(dirs),
        logged_in: named::is_logged_in(dirs),
    }
}

/// `StartQuickTunnel`: publish a site at a random `*.trycloudflare.com` URL.
///
/// Registers + spawns the child under a brief manager lock, then drives
/// readiness with the lock released between ticks (see [`yerd_tunnel`]'s tick
/// model) so `StopTunnel`/`TunnelStatus`/shutdown stay responsive and a stuck
/// connect can be cancelled.
pub async fn start_quick_tunnel(site: &str, state: &DaemonState) -> Response {
    if !install::is_installed(&state.dirs) {
        return Response::Error {
            code: ErrorCode::NotFound,
            message: "cloudflared is not installed - install it from Integrations first".into(),
        };
    }

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
        args,
        pinned_home_env(state),
        TunnelKind::Quick,
        None,
    )
    .await
}

/// The minimal pinned environment for a `cloudflared` subprocess: `HOME` points
/// at the daemon-owned `{data}/tunnel` dir so `cloudflared` never reads or writes
/// `~/.cloudflared`. Named runs add `TUNNEL_ORIGIN_CERT` on top of this.
pub(super) fn pinned_home_env(
    state: &DaemonState,
) -> Vec<(std::ffi::OsString, std::ffi::OsString)> {
    vec![(
        "HOME".into(),
        install::tunnel_dir(&state.dirs).into_os_string(),
    )]
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
/// model). Shared by the Quick and Named start paths. Returns the live
/// `Response::Tunnels` on readiness, or a `Response::Error` on failure.
pub(super) async fn run_to_ready(
    state: &DaemonState,
    name: &str,
    args: Vec<std::ffi::OsString>,
    env: Vec<(std::ffi::OsString, std::ffi::OsString)>,
    kind: TunnelKind,
    hostname: Option<String>,
) -> Response {
    let binary = install::binary_path(&state.dirs);
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

/// Shared `Response::Tunnels` builder.
pub(super) async fn tunnels_response(state: &DaemonState) -> Response {
    let tunnels = {
        let mut mgr = state.tunnel_manager.lock().await;
        mgr.snapshots().into_iter().map(to_wire).collect()
    };
    Response::Tunnels {
        tunnels,
        cloudflared: cloudflared_status(&state.dirs),
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
        let guard = state.tunnel_mutate.lock().await;
        let result = tokio::select! {
            r = install::install(&state.dirs, &dl, Some(&tx)) => Some(r),
            _ = cancel.changed() => None,
        };
        drop(guard);
        drop(tx);
        let _ = drain.await;

        match result {
            Some(Ok(())) => {
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
