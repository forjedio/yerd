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
        // Ephemeral DNS too: the default `dns_port` (1053) is a fixed port that
        // can be busy (a concurrent test, a stray binder), in which case the
        // soft-fail `bring_up_with_dirs` returns `dns_bound: None` and the
        // `drive_subsystems` setup below would panic. `0` binds `127.0.0.1:0`,
        // which the OS always satisfies, so `dns_bound` is reliably `Some`.
        cfg.dns_port = 0;
        cfg
    }

    /// Two distinct, currently-free, non-zero TCP ports. Required by any test
    /// that triggers `Config::save`: `validate()` rejects http==0 / https==0 /
    /// http==https, so the ports-0 trick above is un-persistable.
    fn free_ports() -> (u16, u16) {
        let a = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let b = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let pa = a.local_addr().unwrap().port();
        let pb = b.local_addr().unwrap().port();
        drop(a);
        drop(b);
        assert_ne!(pa, pb);
        (pa, pb)
    }

    fn valid_config() -> yerd_config::Config {
        let (http, https) = free_ports();
        let mut cfg = yerd_config::Config::default();
        cfg.ports.http = http;
        cfg.ports.https = https;
        cfg.dns_port = 0; // ephemeral DNS — avoid colliding on the fixed default across tests
        cfg
    }

    /// A mutation (`Park`) over the real socket persists the config and is
    /// reflected by a follow-up `ListSites`. Uses valid (persistable) ports.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn park_round_trip_persists() {
        let tmp = tempfile::tempdir().unwrap();
        let dirs = make_dirs(tmp.path());
        let cfg = valid_config();
        let cfg_path = dirs.config.join("yerd.toml");

        // A parked root containing one child directory (the routable site).
        let sites_root = tmp.path().join("Sites");
        std::fs::create_dir_all(sites_root.join("blog")).unwrap();

        let daemon = yerdd::startup::bring_up_with_dirs(dirs.clone(), cfg, cfg_path.clone())
            .await
            .expect("bring_up_with_dirs");

        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let daemon_task = tokio::spawn(async move { drive_subsystems(daemon, shutdown_rx).await });

        tokio::time::sleep(Duration::from_millis(100)).await;
        let ipc_sock = dirs.runtime.join("yerd.sock");

        // Park the root.
        let park = Request::Park {
            path: sites_root.clone(),
        };
        let resp = round_trip(&ipc_sock, &park).await;
        assert!(matches!(resp, Response::Ok), "park got {resp:?}");

        // ListSites must show the *child* directory as a site.
        let resp = round_trip(&ipc_sock, &Request::ListSites).await;
        match resp {
            Response::Sites { sites } => {
                assert!(
                    sites.iter().any(|s| s.name() == "blog"),
                    "expected 'blog' in {sites:?}"
                );
            }
            other => panic!("expected Sites, got {other:?}"),
        }

        // Config persisted to disk with the parked path.
        let on_disk = std::fs::read_to_string(&cfg_path).expect("config file written");
        let canonical = std::fs::canonicalize(&sites_root).unwrap();
        assert!(
            on_disk.contains(&canonical.to_string_lossy().into_owned()),
            "parked path missing from {on_disk}"
        );

        shutdown_tx.send_replace(true);
        let _ = tokio::time::timeout(Duration::from_secs(10), daemon_task).await;
    }

    /// `SetSecure` over the real socket records a per-site override for the
    /// parked site (keeping it parked), sets the flag, and persists it under an
    /// `[[overrides]]` table on disk.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn set_secure_round_trip_persists() {
        let tmp = tempfile::tempdir().unwrap();
        let dirs = make_dirs(tmp.path());
        let cfg = valid_config();
        let cfg_path = dirs.config.join("yerd.toml");

        let sites_root = tmp.path().join("Sites");
        std::fs::create_dir_all(sites_root.join("blog")).unwrap();

        let daemon = yerdd::startup::bring_up_with_dirs(dirs.clone(), cfg, cfg_path.clone())
            .await
            .expect("bring_up_with_dirs");

        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let daemon_task = tokio::spawn(async move { drive_subsystems(daemon, shutdown_rx).await });

        tokio::time::sleep(Duration::from_millis(100)).await;
        let ipc_sock = dirs.runtime.join("yerd.sock");

        // Park, then secure the discovered `blog` site.
        let resp = round_trip(
            &ipc_sock,
            &Request::Park {
                path: sites_root.clone(),
            },
        )
        .await;
        assert!(matches!(resp, Response::Ok), "park got {resp:?}");

        let resp = round_trip(
            &ipc_sock,
            &Request::SetSecure {
                name: "blog".into(),
                secure: true,
            },
        )
        .await;
        assert!(matches!(resp, Response::Ok), "set_secure got {resp:?}");

        // ListSites reflects the secured site — still PARKED (no promotion).
        match round_trip(&ipc_sock, &Request::ListSites).await {
            Response::Sites { sites } => {
                let blog = sites
                    .iter()
                    .find(|s| s.name() == "blog")
                    .expect("blog present");
                assert!(blog.secure(), "blog should be secure");
                assert_eq!(
                    blog.kind(),
                    yerd_core::SiteKind::Parked,
                    "blog must stay parked"
                );
            }
            other => panic!("expected Sites, got {other:?}"),
        }

        // Persisted to disk under an `[[overrides]]` table (not promoted to
        // `[[linked]]`).
        let on_disk = std::fs::read_to_string(&cfg_path).expect("config file written");
        assert!(
            on_disk.contains("[[overrides]]"),
            "expected an `[[overrides]]` table in {on_disk}"
        );
        assert!(
            on_disk.contains("secure = true"),
            "expected `secure = true` in {on_disk}"
        );
        assert!(
            !on_disk.contains("[[linked]]"),
            "blog must not be promoted to a linked site: {on_disk}"
        );

        shutdown_tx.send_replace(true);
        let _ = tokio::time::timeout(Duration::from_secs(10), daemon_task).await;
    }

    /// Open a fresh connection, send one request, read one response.
    async fn round_trip(sock: &std::path::Path, req: &Request) -> Response {
        let name = sock.to_fs_name::<GenericFilePath>().unwrap();
        let stream = IpcStream::connect(name).await.expect("connect");
        let (reader, writer) = stream.split();
        let mut reader = reader;
        let mut writer = writer;
        write_message(&mut writer, req, DEFAULT_MAX_FRAME)
            .await
            .expect("write");
        let mut decoder = FrameDecoder::new();
        read_message(&mut reader, &mut decoder)
            .await
            .expect("read")
            .expect("response")
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
            // The test daemon always binds DNS (ephemeral `127.0.0.1:0`), so the
            // soft-fail `Option` is always `Some` here.
            let bound = daemon.dns_bound.expect("test daemon binds its DNS sockets");
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
        let proxy_handle = {
            let resolver = Arc::new(yerdd::backend_resolver::DaemonBackendResolver {
                php_manager: daemon.php_manager.clone(),
            });
            // The test daemon always binds its ports, so the listeners are `Some`.
            let https = yerd_proxy::HttpsBinding {
                listener: daemon
                    .https_listener
                    .expect("test daemon binds its https listener"),
                public_port: daemon.https_port,
                cert_store: daemon.cert_store.clone(),
            };
            let router = daemon.state.router.clone();
            let mut rx = shutdown_rx.clone();
            tokio::spawn(yerd_proxy::ProxyServer::serve(
                daemon
                    .http_listener
                    .expect("test daemon binds its http listener"),
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
            daemon.state.clone(),
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
            daemon.state,
        );
        Ok(())
    }
}
