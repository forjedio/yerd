//! End-to-end lifecycle test: bring the daemon up against a tempdir
//! `PlatformDirs`, exchange one `Ping` over IPC, signal shutdown,
//! assert clean exit.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

mod tests {
    use std::sync::Arc;
    use std::time::Duration;

    use interprocess::local_socket::tokio::Stream as IpcStream;
    use tokio::sync::watch;

    use yerd_ipc::{
        read_message, write_message, FrameDecoder, Request, Response, DEFAULT_MAX_FRAME,
    };
    use yerd_platform::PlatformDirs;

    /// Connect to the daemon over its platform-native transport, derived from the
    /// same `PlatformDirs` the daemon bound on: the Unix socket path, or the
    /// Windows named pipe (`yerd_platform::daemon_pipe_name`, which the daemon's
    /// listener also derives). The runtime-dir hash in the pipe name keeps each
    /// tempdir test's pipe distinct, so the three tests run in parallel.
    async fn connect_ipc(dirs: &PlatformDirs) -> IpcStream {
        use interprocess::local_socket::traits::tokio::Stream as _;
        #[cfg(unix)]
        {
            use interprocess::local_socket::{GenericFilePath, ToFsName};
            let sock = dirs.runtime.join("yerd.sock");
            let name = sock.to_fs_name::<GenericFilePath>().unwrap();
            IpcStream::connect(name).await.expect("connect IPC socket")
        }
        #[cfg(windows)]
        {
            use interprocess::local_socket::{GenericNamespaced, ToNsName};
            let pipe = yerd_platform::daemon_pipe_name(dirs).expect("derive pipe name");
            let name = pipe.to_ns_name::<GenericNamespaced>().unwrap();
            IpcStream::connect(name).await.expect("connect IPC pipe")
        }
    }

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
        let mut cfg = yerd_config::Config::default();
        cfg.ports.http = 0;
        cfg.ports.https = 0;
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
        cfg.dns_port = 0;
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

        let sites_root = tmp.path().join("Sites");
        std::fs::create_dir_all(sites_root.join("blog")).unwrap();

        let daemon = yerdd::startup::bring_up_with_dirs(dirs.clone(), cfg, cfg_path.clone())
            .await
            .expect("bring_up_with_dirs");

        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let daemon_task = tokio::spawn(async move { drive_subsystems(daemon, shutdown_rx).await });

        tokio::time::sleep(Duration::from_millis(100)).await;

        let park = Request::Park {
            path: sites_root.clone(),
        };
        let resp = round_trip(&dirs, &park).await;
        assert!(matches!(resp, Response::Ok), "park got {resp:?}");

        let resp = round_trip(&dirs, &Request::ListSites).await;
        match resp {
            Response::Sites { sites } => {
                assert!(
                    sites.iter().any(|s| s.site.name() == "blog"),
                    "expected 'blog' in {sites:?}"
                );
            }
            other => panic!("expected Sites, got {other:?}"),
        }

        let on_disk = std::fs::read_to_string(&cfg_path).expect("config file written");
        let canonical = std::fs::canonicalize(&sites_root).unwrap();
        let needle = strip_verbatim(&canonical.to_string_lossy());
        assert!(
            strip_verbatim(&on_disk).contains(&needle),
            "parked path {needle} missing from {on_disk}"
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

        let resp = round_trip(
            &dirs,
            &Request::Park {
                path: sites_root.clone(),
            },
        )
        .await;
        assert!(matches!(resp, Response::Ok), "park got {resp:?}");

        let resp = round_trip(
            &dirs,
            &Request::SetSecure {
                name: "blog".into(),
                secure: true,
            },
        )
        .await;
        assert!(matches!(resp, Response::Ok), "set_secure got {resp:?}");

        match round_trip(&dirs, &Request::ListSites).await {
            Response::Sites { sites } => {
                let blog = sites
                    .iter()
                    .find(|s| s.site.name() == "blog")
                    .expect("blog present");
                assert!(blog.site.secure(), "blog should be secure");
                assert_eq!(
                    blog.site.kind(),
                    yerd_core::SiteKind::Parked,
                    "blog must stay parked"
                );
            }
            other => panic!("expected Sites, got {other:?}"),
        }

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

    /// Normalise away the Windows extended-length `\\?\` verbatim prefix that
    /// `std::fs::canonicalize` adds, so the disk-vs-canonicalize path comparison
    /// holds regardless of whether the Park handler recorded a verbatim path.
    /// A no-op on Unix (no such prefix). Test-only: daemon behaviour is
    /// unchanged.
    fn strip_verbatim(s: &str) -> String {
        s.replace(r"\\?\", "")
    }

    /// Open a fresh connection, send one request, read one response.
    async fn round_trip(dirs: &PlatformDirs, req: &Request) -> Response {
        use interprocess::local_socket::traits::tokio::Stream as _;
        let stream = connect_ipc(dirs).await;
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
        use interprocess::local_socket::traits::tokio::Stream as _;
        let tmp = tempfile::tempdir().unwrap();
        let dirs = make_dirs(tmp.path());
        let cfg = default_config();
        let cfg_path = dirs.config.join("yerd.toml");

        let daemon = yerdd::startup::bring_up_with_dirs(dirs.clone(), cfg, cfg_path.clone())
            .await
            .expect("bring_up_with_dirs");

        // On Unix the bound socket is filesystem-visible; on Windows the named
        // pipe is not, so its bind success plus the connect below is the
        // assertion (the daemon errors out of `bring_up_with_dirs` if it can't
        // bind, so reaching here means the listener is up).
        #[cfg(unix)]
        assert!(
            dirs.runtime.join("yerd.sock").exists(),
            "IPC socket should be bound"
        );

        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let daemon_for_task = daemon;
        let daemon_task =
            tokio::spawn(async move { drive_subsystems(daemon_for_task, shutdown_rx).await });

        tokio::time::sleep(Duration::from_millis(100)).await;

        let stream = connect_ipc(&dirs).await;
        let (reader, writer) = stream.split();
        let mut reader = reader;
        let mut writer = writer;

        write_message(&mut writer, &Request::Ping, DEFAULT_MAX_FRAME)
            .await
            .expect("write Ping");
        let mut decoder = FrameDecoder::new();
        let resp: Option<Response> = read_message(&mut reader, &mut decoder).await.unwrap();
        assert!(matches!(resp, Some(Response::Pong)));

        shutdown_tx.send_replace(true);
        let exit_result = tokio::time::timeout(Duration::from_secs(10), daemon_task)
            .await
            .expect("daemon should shut down within 10s")
            .expect("daemon task panicked");
        assert!(exit_result.is_ok(), "daemon exit was Err: {exit_result:?}");
    }

    /// Spawn whichever subsystems came up. The DNS and web listeners are
    /// `Option`s: `bring_up_with_dirs` leaves them `None` when the ports can't
    /// be bound (a busy port on a CI runner, say) and runs degraded rather than
    /// aborting, so each is driven only when present. The IPC listener - the one
    /// these tests exercise - is always up.
    async fn drive_subsystems(
        daemon: yerdd::startup::Daemon,
        shutdown_rx: watch::Receiver<bool>,
    ) -> Result<(), yerdd::error::DaemonError> {
        let dns_handle = daemon.dns_bound.map(|bound| {
            let responder = yerd_dns::Responder::new(daemon.dns_tld.clone());
            let mut rx = shutdown_rx.clone();
            tokio::spawn(async move {
                bound
                    .serve(responder, yerd_dns::AnswerAddrs::loopback(), async move {
                        let _ = rx.changed().await;
                    })
                    .await
            })
        });
        let proxy_handle = daemon.http_listener.map(|http_listener| {
            let resolver = Arc::new(yerdd::backend_resolver::DaemonBackendResolver {
                php_manager: daemon.php_manager.clone(),
                wordpress_sites: daemon.state.wordpress_sites.clone(),
            });
            let https = daemon
                .https_listener
                .map(|listener| yerd_proxy::HttpsBinding {
                    listener,
                    public_port: daemon.state.redirect_https_port.clone(),
                    cert_store: daemon.cert_store.clone(),
                });
            let router = daemon.state.router.clone();
            let login_tokens = daemon.state.wordpress_login_tokens.clone();
            let login_prepend_script = daemon.state.wordpress_login_prepend_script.clone();
            let mut rx = shutdown_rx.clone();
            tokio::spawn(yerd_proxy::ProxyServer::serve(
                http_listener,
                https,
                router,
                resolver,
                login_tokens,
                login_prepend_script,
                daemon.state.symlink_protection.clone(),
                std::sync::Arc::new(yerd_proxy::ProxyClientTls::new(
                    yerd_proxy::ProxyClientTls::no_verify_config().unwrap(),
                    yerd_proxy::ProxyClientTls::no_verify_config().unwrap(),
                )),
                false,
                async move {
                    let _ = rx.changed().await;
                },
            ))
        });
        let ipc_handle = tokio::spawn(yerdd::ipc_server::run(
            daemon.ipc_listener,
            daemon.state.clone(),
            shutdown_rx.clone(),
        ));

        if let Some(dns_handle) = dns_handle {
            let _ = tokio::time::timeout(Duration::from_secs(10), dns_handle).await;
        }
        if let Some(proxy_handle) = proxy_handle {
            let _ = tokio::time::timeout(Duration::from_secs(10), proxy_handle).await;
        }
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
