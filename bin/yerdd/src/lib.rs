//! Yerd daemon - library shim.
//!
//! Binary-only crates don't expose a Rust API to integration tests
//! under `tests/`. This lib publishes the daemon's modules as a normal
//! crate so the lifecycle test can reach `bring_up_with_dirs`,
//! `DaemonError`, etc. All real logic lives in the individual modules;
//! this file is just `pub mod`s and a `run` entry point shared between
//! `main.rs` and the tests.

#![forbid(unsafe_code)]
#![allow(clippy::doc_markdown)]

pub mod ansi;
pub mod args;
pub mod backend_resolver;
pub mod cert_store;
pub mod create_site;
pub mod db_admin;
pub mod detect_cache;
pub mod dump_server;
pub mod error;
pub mod ext_install;
pub mod fs_watch;
pub mod ipc_server;
pub mod jobs;
pub mod mutate;
pub mod php_install;
pub mod php_updates;
pub mod secure_fs;
pub mod self_update;
pub mod service_install;
pub mod services;
pub mod signals;
pub mod single_instance;
pub mod startup;
pub mod state;
pub mod tools;
pub mod tracing_init;
pub mod tunnel;

#[cfg(test)]
pub mod test_support;

use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::watch;

use crate::args::ServeArgs;
use crate::backend_resolver::DaemonBackendResolver;
use crate::error::DaemonError;
use crate::startup::Daemon;

/// What the run loop wants `main` to do after a graceful teardown: exit the
/// process, or re-exec it in place (a `RestartDaemon` request).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// Normal shutdown - the process should exit.
    Exit,
    /// A restart was requested - `main` should re-exec the binary.
    Restart,
}

/// Run the daemon to completion (until a shutdown signal or restart request).
///
/// `main` calls this inside a tokio runtime; integration tests call
/// `run_with_daemon` after seeding a `Daemon` via `bring_up_with_dirs`.
pub async fn run(args: ServeArgs) -> Result<Outcome, DaemonError> {
    let daemon = startup::bring_up(&args).await?;
    run_with_daemon(daemon).await
}

/// Drive an already-bootstrapped `Daemon` to completion.
#[doc(hidden)]
pub async fn run_with_daemon(daemon: Daemon) -> Result<Outcome, DaemonError> {
    let shutdown_tx = daemon.state.shutdown_tx.clone();
    let shutdown_rx = shutdown_tx.subscribe();
    let signal_task = tokio::spawn(signals::wait_for_shutdown(shutdown_tx));
    let result = run_until_shutdown(daemon, shutdown_rx).await;
    signal_task.abort();
    let _ = signal_task.await;
    result
}

/// Self-update poll wake interval. Short (15 min) so the wall-clock due-check
/// in `self_update::poll_if_due` recovers quickly after the process was
/// suspended. The default `MissedTickBehavior::Burst` means missed ticks from
/// runtime starvation (not suspend itself: a stalled monotonic clock accrues
/// no missed ticks) fire back-to-back on resume. Harmless either way: each is
/// awaited serially and `poll_if_due` wall-clock-gates every tick, so only the
/// first (or, on repeated fetch failures, the first few) actually fetch.
const SELF_UPDATE_WAKE: Duration = Duration::from_secs(15 * 60);

#[allow(clippy::too_many_lines)]
async fn run_until_shutdown(
    daemon: Daemon,
    shutdown_rx: watch::Receiver<bool>,
) -> Result<Outcome, DaemonError> {
    let dns_handle = if let Some(bound) = daemon.dns_bound {
        let responder = yerd_dns::Responder::new(daemon.dns_tld.clone());
        let mut rx = shutdown_rx.clone();
        Some(tokio::spawn(async move {
            bound
                .serve(responder, async move {
                    let _ = rx.changed().await;
                })
                .await
        }))
    } else {
        tracing::warn!(
            "DNS responder disabled (degraded): dns_port couldn't bind - .test names won't resolve until the port is fixed and the daemon restarts"
        );
        None
    };

    let proxy_handle = if let (Some(http_listener), Some(tls_listener)) =
        (daemon.http_listener, daemon.https_listener)
    {
        let router = daemon.state.router.clone();
        let resolver = Arc::new(DaemonBackendResolver {
            php_manager: daemon.php_manager.clone(),
        });
        let https = yerd_proxy::HttpsBinding {
            listener: tls_listener,
            public_port: daemon.state.redirect_https_port.clone(),
            cert_store: daemon.cert_store.clone(),
        };
        let mut rx = shutdown_rx.clone();
        Some(tokio::spawn(yerd_proxy::ProxyServer::serve(
            http_listener,
            Some(https),
            router,
            resolver,
            async move {
                let _ = rx.changed().await;
            },
        )))
    } else {
        tracing::warn!(
            "web proxy disabled (degraded): no HTTP/HTTPS listeners - sites won't be served until the fallback ports are fixed and the daemon restarts"
        );
        None
    };

    let redirect_probe_handle =
        spawn_redirect_probe(proxy_handle.is_some(), &daemon.state, shutdown_rx.clone());

    let ipc_handle = tokio::spawn(ipc_server::run(
        daemon.ipc_listener,
        daemon.state.clone(),
        shutdown_rx.clone(),
    ));

    let dump_handle = {
        let state = daemon.state.clone();
        tokio::spawn(crate::dump_server::run(state, shutdown_rx.clone()))
    };

    let update_check_handle = {
        let state = daemon.state.clone();
        let mut rx = shutdown_rx.clone();
        tokio::spawn(async move {
            let dl = crate::php_install::ReqwestDownloader::new();
            let mut php_tick = tokio::time::interval(Duration::from_secs(12 * 60 * 60));
            let mut self_tick = tokio::time::interval(SELF_UPDATE_WAKE);
            loop {
                tokio::select! {
                    _ = php_tick.tick() => {
                        crate::php_updates::poll_and_refresh(
                            &state,
                            &dl,
                            yerd_update::PHP_LISTING_PUBLIC_KEY,
                        )
                        .await;
                    }
                    _ = self_tick.tick() => {
                        crate::self_update::poll_if_due(&state, &dl).await;
                    }
                    _ = rx.changed() => break,
                }
            }
        })
    };

    let watch_handle = {
        let state = daemon.state.clone();
        let rx = shutdown_rx.clone();
        tokio::spawn(crate::fs_watch::run(state, rx))
    };

    let mail_handle = daemon.mail_listener.map(|listener| {
        let store = daemon.state.mail_store.clone();
        let mut rx = shutdown_rx.clone();
        tokio::spawn(yerd_mail::serve(listener, store, async move {
            let _ = rx.changed().await;
        }))
    });

    let _autostart = tokio::spawn(crate::services::auto_start_installed(daemon.state.clone()));

    let _ext_install = {
        let state = daemon.state.clone();
        tokio::spawn(async move {
            if state.config.lock().await.dumps.enabled {
                let dl = crate::php_install::ReqwestDownloader::new();
                crate::ext_install::ensure_for_installed(&state.dirs, &dl).await;
            }
        })
    };

    let _pcov_shims = {
        let state = daemon.state.clone();
        tokio::spawn(async move {
            crate::ipc_server::refresh_pcov_and_shims(&state).await;
            crate::ipc_server::write_cli_ini_now(&state).await;
        })
    };

    let _tool_shims = {
        let state = daemon.state.clone();
        tokio::spawn(async move {
            crate::ipc_server::reconcile_tool_shims_now(&state).await;
        })
    };

    let mut wait_rx = shutdown_rx;
    let _ = wait_rx.changed().await;

    if let Some(dns_handle) = dns_handle {
        let _ = tokio::time::timeout(Duration::from_secs(10), dns_handle).await;
    }
    if let Some(proxy_handle) = proxy_handle {
        let _ = tokio::time::timeout(Duration::from_secs(10), proxy_handle).await;
    }
    if let Some(redirect_probe_handle) = redirect_probe_handle {
        let _ = tokio::time::timeout(Duration::from_secs(5), redirect_probe_handle).await;
    }
    let _ = tokio::time::timeout(Duration::from_secs(5), ipc_handle).await;
    let _ = tokio::time::timeout(Duration::from_secs(5), dump_handle).await;
    let _ = tokio::time::timeout(Duration::from_secs(5), update_check_handle).await;
    let _ = tokio::time::timeout(Duration::from_secs(5), watch_handle).await;
    if let Some(mail_handle) = mail_handle {
        let _ = tokio::time::timeout(Duration::from_secs(5), mail_handle).await;
    }

    {
        let mut mgr = daemon.php_manager.lock().await;
        let _ = mgr.shutdown().await;
    }
    {
        let mut mgr = daemon.state.service_manager.lock().await;
        let _ = mgr.shutdown().await;
    }
    {
        let mut mgr = daemon.state.tunnel_manager.lock().await;
        let _ = mgr.shutdown().await;
    }

    let outcome = if daemon.state.restart_requested.load(Ordering::Acquire) {
        Outcome::Restart
    } else {
        Outcome::Exit
    };
    drop(daemon.lock);
    Ok(outcome)
}

/// Spawns the background task that keeps
/// [`crate::state::DaemonState::redirect_https_port`] in sync with a live
/// privileged-port redirect (macOS `pf`, installed by `yerd elevate ports`) -
/// the only case where the daemon's own bound HTTPS port can diverge from
/// what's actually reachable without a restart. Returns `None` (spawning
/// nothing) when the proxy isn't running or the HTTPS listener bound its
/// well-known port directly, since neither case has anything to detect.
fn spawn_redirect_probe(
    proxy_running: bool,
    state: &Arc<crate::state::DaemonState>,
    mut shutdown_rx: watch::Receiver<bool>,
) -> Option<tokio::task::JoinHandle<()>> {
    if !proxy_running || !state.https.fell_back {
        return None;
    }
    let state = state.clone();
    Some(tokio::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_secs(5));
        loop {
            tokio::select! {
                _ = tick.tick() => {
                    let active = tokio::task::spawn_blocking(|| {
                        use yerd_platform::PortRedirector;
                        yerd_platform::ActivePortRedirector::new().is_active()
                    })
                    .await
                    .unwrap_or(None);
                    let port = effective_redirect_port(state.https, active);
                    state.redirect_https_port.store(port, Ordering::Relaxed);
                }
                _ = shutdown_rx.changed() => break,
            }
        }
    }))
}

/// Port the HTTP→HTTPS redirect `Location` header should advertise, given the
/// HTTPS listener's bind status and a live
/// [`yerd_platform::PortRedirector::is_active`] probe result.
///
/// When the daemon bound the well-known port directly there's nothing to
/// correct. When it fell back to a rootless port, a live privileged-port
/// redirect (macOS `pf`, installed by `yerd elevate ports`) makes the
/// well-known port reachable too, so the redirect should advertise it instead
/// of leaking the internal fallback port into the browser's address bar.
fn effective_redirect_port(https: yerd_ipc::PortStatus, redirect_active: Option<bool>) -> u16 {
    if !https.fell_back || redirect_active == Some(true) {
        https.requested
    } else {
        https.bound
    }
}

#[cfg(test)]
mod redirect_port_tests {
    use super::effective_redirect_port;

    fn status(requested: u16, bound: u16) -> yerd_ipc::PortStatus {
        yerd_ipc::PortStatus {
            requested,
            bound,
            fell_back: requested != bound,
        }
    }

    #[test]
    fn bound_on_well_known_port_ignores_the_probe() {
        assert_eq!(effective_redirect_port(status(443, 443), None), 443);
        assert_eq!(effective_redirect_port(status(443, 443), Some(false)), 443);
        assert_eq!(effective_redirect_port(status(443, 443), Some(true)), 443);
    }

    #[test]
    fn fallback_with_a_live_redirect_advertises_the_well_known_port() {
        assert_eq!(effective_redirect_port(status(443, 8443), Some(true)), 443);
    }

    #[test]
    fn fallback_without_a_live_redirect_advertises_the_bound_port() {
        assert_eq!(
            effective_redirect_port(status(443, 8443), Some(false)),
            8443
        );
        assert_eq!(effective_redirect_port(status(443, 8443), None), 8443);
    }
}
