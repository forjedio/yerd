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
pub mod error;
pub mod ipc_server;
pub mod secure_fs;
pub mod signals;
pub mod single_instance;
pub mod startup;
pub mod tracing_init;

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::watch;

use crate::args::ServeArgs;
use crate::backend_resolver::DaemonBackendResolver;
use crate::error::DaemonError;
use crate::startup::Daemon;

/// Run the daemon to completion (i.e. until a shutdown signal fires).
///
/// `main` calls this inside a tokio runtime; integration tests call
/// `run_with_daemon` after seeding a `Daemon` via `bring_up_with_dirs`.
pub async fn run(args: ServeArgs) -> Result<(), DaemonError> {
    let daemon = startup::bring_up(&args).await?;
    run_with_daemon(daemon).await
}

/// Drive an already-bootstrapped `Daemon` to completion.
#[doc(hidden)]
pub async fn run_with_daemon(daemon: Daemon) -> Result<(), DaemonError> {
    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let signal_task = tokio::spawn(signals::wait_for_shutdown(shutdown_tx.clone()));
    let result = run_until_shutdown(daemon, shutdown_rx).await;
    let _ = signal_task.await;
    result
}

async fn run_until_shutdown(
    daemon: Daemon,
    shutdown_rx: watch::Receiver<bool>,
) -> Result<(), DaemonError> {
    // DNS task. The socket pair was bound during `bring_up`, so the daemon
    // owns it here — we just consume it into the serve loop.
    let dns_handle = {
        let bound = daemon.dns_bound;
        let responder = yerd_dns::Responder::new(daemon.config.tld.clone());
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
        let router = daemon.router.clone();
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
        daemon.router.clone(),
        shutdown_rx.clone(),
    ));

    // Wait for any task to wind down — they all watch the same shutdown
    // channel, so they finish together once the signal fires.
    let _ = tokio::time::timeout(Duration::from_secs(10), dns_handle).await;
    let _ = tokio::time::timeout(Duration::from_secs(10), proxy_handle).await;
    let _ = tokio::time::timeout(Duration::from_secs(5), ipc_handle).await;

    {
        let mut mgr = daemon.php_manager.lock().await;
        let _ = mgr.shutdown().await;
    }

    drop(daemon.lock);
    Ok(())
}
