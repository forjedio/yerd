//! Yerd daemon — library shim.
//!
//! Binary-only crates don't expose a Rust API to integration tests
//! under `tests/`. This lib publishes the daemon's modules as a normal
//! crate so the lifecycle test can reach `bring_up_with_dirs`,
//! `DaemonError`, etc. All real logic lives in the individual modules;
//! this file is just `pub mod`s and a `run` entry point shared between
//! `main.rs` and the tests.

#![forbid(unsafe_code)]
// Sysexits constants, kernel symbols, and OS shorthand show up
// throughout daemon docs; backticking every one is noise.
#![allow(clippy::doc_markdown)]

pub mod args;
pub mod backend_resolver;
pub mod cert_store;
pub mod db_admin;
pub mod detect_cache;
pub mod dump_server;
pub mod error;
pub mod fs_watch;
pub mod ipc_server;
pub mod mutate;
pub mod php_install;
pub mod php_updates;
pub mod secure_fs;
pub mod service_install;
pub mod services;
pub mod signals;
pub mod single_instance;
pub mod startup;
pub mod state;
pub mod tracing_init;

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
    /// Normal shutdown — the process should exit.
    Exit,
    /// A restart was requested — `main` should re-exec the binary.
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
    // The shutdown channel lives in `DaemonState` so the IPC `RestartDaemon`
    // handler can trip it; the signal task and every subsystem share it.
    let shutdown_tx = daemon.state.shutdown_tx.clone();
    let shutdown_rx = shutdown_tx.subscribe();
    let signal_task = tokio::spawn(signals::wait_for_shutdown(shutdown_tx));
    let result = run_until_shutdown(daemon, shutdown_rx).await;
    // The signal task only finishes when it actually receives SIGTERM/Ctrl-C.
    // When shutdown was triggered another way (a `RestartDaemon` IPC trips the
    // channel directly, no signal), the task is still parked — abort it rather
    // than awaiting it forever, which would otherwise hang the restart.
    signal_task.abort();
    let _ = signal_task.await;
    result
}

async fn run_until_shutdown(
    daemon: Daemon,
    shutdown_rx: watch::Receiver<bool>,
) -> Result<Outcome, DaemonError> {
    // DNS task. The socket pair was bound during `bring_up`, so the daemon
    // owns it here — we just consume it into the serve loop.
    let dns_handle = {
        let bound = daemon.dns_bound;
        let responder = yerd_dns::Responder::new(daemon.dns_tld.clone());
        let mut rx = shutdown_rx.clone();
        tokio::spawn(async move {
            bound
                .serve(responder, async move {
                    let _ = rx.changed().await;
                })
                .await
        })
    };

    // Proxy task.
    let proxy_handle = {
        let router = daemon.state.router.clone();
        let resolver = Arc::new(DaemonBackendResolver {
            php_manager: daemon.php_manager.clone(),
        });
        let https = yerd_proxy::HttpsBinding {
            listener: daemon.https_listener,
            public_port: daemon.https_port,
            cert_store: daemon.cert_store.clone(),
        };
        let mut rx = shutdown_rx.clone();
        tokio::spawn(yerd_proxy::ProxyServer::serve(
            daemon.http_listener,
            Some(https),
            router,
            resolver,
            async move {
                let _ = rx.changed().await;
            },
        ))
    };

    // IPC task.
    let ipc_handle = tokio::spawn(ipc_server::run(
        daemon.ipc_listener,
        daemon.state.clone(),
        shutdown_rx.clone(),
    ));

    // Dump-telemetry server: loopback TCP receiving frames from the PHP
    // extension. Rebinds on a port change; a bind failure is non-fatal.
    let dump_handle = {
        let state = daemon.state.clone();
        tokio::spawn(crate::dump_server::run(state, shutdown_rx.clone()))
    };

    // Periodic PHP update checker: poll once at startup, then every 12h, until
    // shutdown. Notify-only (logs available updates; never auto-installs).
    let update_check_handle = {
        let state = daemon.state.clone();
        let mut rx = shutdown_rx.clone();
        tokio::spawn(async move {
            let dl = crate::php_install::ReqwestDownloader::new();
            let mut tick = tokio::time::interval(Duration::from_secs(12 * 60 * 60));
            loop {
                tokio::select! {
                    _ = tick.tick() => crate::php_updates::poll_and_refresh(&state, &dl).await,
                    _ = rx.changed() => break,
                }
            }
        })
    };

    // Filesystem watcher: rebuilds the router as parked projects appear/change
    // (e.g. a project cloned into a parked folder), so detection stays fresh
    // without a manual refresh. Non-recursive; see `watch.rs`.
    let watch_handle = {
        let state = daemon.state.clone();
        let rx = shutdown_rx.clone();
        tokio::spawn(crate::fs_watch::run(state, rx))
    };

    // Auto-start enabled services in the background — deliberately NOT awaited,
    // so a slow/failing DB cold-boot never delays the listeners above. Each
    // engine's outcome is logged inside the task.
    let _autostart = tokio::spawn(crate::services::auto_start_enabled(daemon.state.clone()));

    // Serve until a shutdown is requested — a SIGTERM/Ctrl-C, or a
    // `RestartDaemon` IPC tripping the same channel. Without this wait the
    // daemon would fall straight through to the graceful-join timeouts below
    // and exit on its own ~30s after startup.
    let mut wait_rx = shutdown_rx;
    let _ = wait_rx.changed().await;

    // Now the subsystems are winding down (they watch the same channel); cap
    // each join so a stuck task can't hang the exit/restart.
    let _ = tokio::time::timeout(Duration::from_secs(10), dns_handle).await;
    let _ = tokio::time::timeout(Duration::from_secs(10), proxy_handle).await;
    let _ = tokio::time::timeout(Duration::from_secs(5), ipc_handle).await;
    let _ = tokio::time::timeout(Duration::from_secs(5), dump_handle).await;
    let _ = tokio::time::timeout(Duration::from_secs(5), update_check_handle).await;
    let _ = tokio::time::timeout(Duration::from_secs(5), watch_handle).await;

    {
        let mut mgr = daemon.php_manager.lock().await;
        let _ = mgr.shutdown().await;
    }
    // Stop supervised services cleanly (graceful SIGTERM→grace→SIGKILL); any
    // still-running children would otherwise be reaped by kill-on-drop.
    {
        let mut mgr = daemon.state.service_manager.lock().await;
        let _ = mgr.shutdown().await;
    }

    // Read the restart decision before releasing the lock; `main` re-execs on
    // `Restart` (never here — this path is also reached by the lifecycle test).
    let outcome = if daemon.state.restart_requested.load(Ordering::Acquire) {
        Outcome::Restart
    } else {
        Outcome::Exit
    };
    drop(daemon.lock);
    Ok(outcome)
}
