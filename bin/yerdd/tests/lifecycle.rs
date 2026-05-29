//! End-to-end lifecycle test: bring the daemon up against a tempdir
//! `PlatformDirs`, exchange one `Ping` over IPC, signal shutdown,
//! assert clean exit.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

#[cfg(unix)]
mod tests {
    use std::sync::Arc;
    use std::time::Duration;

    use interprocess::local_socket::tokio::Stream as IpcStream;
    use interprocess::local_socket::traits::tokio::Stream as _;
    use interprocess::local_socket::{GenericFilePath, ToFsName};
    use tokio::sync::watch;

    use yerd_ipc::{
        read_message, write_message, FrameDecoder, Request, Response, DEFAULT_MAX_FRAME,
    };

    fn make_dirs(tmp: &std::path::Path) -> yerd_platform::PlatformDirs {
        yerd_platform::PlatformDirs {
            config: tmp.join("c"),
            data: tmp.join("d"),
            state: tmp.join("s"),
            cache: tmp.join("ca"),
            runtime: tmp.join("r"),
        }
    }

    fn default_config() -> yerd_config::Config {
        // `Config::default()` uses 80/443; replace with 0/0 so the
        // PortBinder picks ephemeral ports (the bind_pair fallback
        // pair 8080/8443 is also fine on a CI runner, but 0/0 is
        // safest).
        let mut cfg = yerd_config::Config::default();
        cfg.ports.http = 0;
        cfg.ports.https = 0;
        cfg
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn boot_ping_shutdown_round_trip() {
        let tmp = tempfile::tempdir().unwrap();
        let dirs = make_dirs(tmp.path());
        let cfg = default_config();
        let cfg_path = dirs.config.join("yerd.toml");

        // 1. Bring up the daemon (the tempdir variant).
        let daemon = yerdd::startup::bring_up_with_dirs(dirs.clone(), cfg, cfg_path.clone())
            .await
            .expect("bring_up_with_dirs");

        let ipc_sock = dirs.runtime.join("yerd.sock");
        assert!(ipc_sock.exists(), "IPC socket should be bound");

        // 2. Drive the daemon's tasks. We avoid `yerdd::run_with_daemon`
        //    because it installs signal handlers; here we want to drive
        //    shutdown via the watch channel manually.
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let daemon_for_task = daemon;
        let daemon_task =
            tokio::spawn(async move { drive_subsystems(daemon_for_task, shutdown_rx).await });

        // 3. Connect via interprocess + send a Ping.
        // Give the accept loop a moment to start polling.
        tokio::time::sleep(Duration::from_millis(100)).await;

        let name = ipc_sock.as_path().to_fs_name::<GenericFilePath>().unwrap();
        let stream = IpcStream::connect(name).await.expect("connect IPC socket");
        let (reader, writer) = stream.split();
        let mut reader = reader;
        let mut writer = writer;

        write_message(&mut writer, &Request::Ping, DEFAULT_MAX_FRAME)
            .await
            .expect("write Ping");
        let mut decoder = FrameDecoder::new();
        let resp: Option<Response> = read_message(&mut reader, &mut decoder).await.unwrap();
        assert!(matches!(resp, Some(Response::Pong)));

        // 4. Send shutdown and wait for the daemon to wind down.
        shutdown_tx.send_replace(true);
        let exit_result = tokio::time::timeout(Duration::from_secs(10), daemon_task)
            .await
            .expect("daemon should shut down within 10s")
            .expect("daemon task panicked");
        assert!(exit_result.is_ok(), "daemon exit was Err: {exit_result:?}");
    }

    async fn drive_subsystems(
        daemon: yerdd::startup::Daemon,
        shutdown_rx: watch::Receiver<bool>,
    ) -> Result<(), yerdd::error::DaemonError> {
        // DNS now binds ephemerally (`127.0.0.1:0`), so it no longer collides
        // with the host's mDNS responder on 5353 — drive it like production.
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
        let proxy_handle = {
            let resolver = Arc::new(yerdd::backend_resolver::DaemonBackendResolver {
                php_manager: daemon.php_manager.clone(),
            });
            let https = yerd_proxy::HttpsBinding {
                listener: daemon.https_listener,
                public_port: daemon.https_port,
                cert_store: daemon.cert_store.clone(),
            };
            let router = daemon.router.clone();
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
        let ipc_handle = tokio::spawn(yerdd::ipc_server::run(
            daemon.ipc_listener,
            daemon.router.clone(),
            shutdown_rx.clone(),
        ));

        let _ = tokio::time::timeout(Duration::from_secs(10), dns_handle).await;
        let _ = tokio::time::timeout(Duration::from_secs(10), proxy_handle).await;
        let _ = tokio::time::timeout(Duration::from_secs(5), ipc_handle).await;

        {
            let mut mgr = daemon.php_manager.lock().await;
            let _ = mgr.shutdown().await;
        }
        drop(daemon.lock);
        let _ = (
            daemon.config_path,
            daemon.dirs,
            daemon.dns_addr,
            daemon.config,
        );
        Ok(())
    }
}
