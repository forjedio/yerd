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
        let ipc_sock = dirs.runtime.join("yerd.sock");

        let park = Request::Park {
            path: sites_root.clone(),
        };
        let resp = round_trip(&ipc_sock, &park).await;
        assert!(matches!(resp, Response::Ok), "park got {resp:?}");

        let resp = round_trip(&ipc_sock, &Request::ListSites).await;
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

        match round_trip(&ipc_sock, &Request::ListSites).await {
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

    /// Parked folders ending in a public suffix are served under the stripped
    /// name, with the cross-root duplicate rule, and boot moves the name-keyed
    /// settings of a renamed site to its new name (persisted to disk, and
    /// reflected in the first router: the migrated suppressed apex applies)
    /// while leaving a still-live legacy name alone.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn boot_strips_public_suffixes_and_migrates_name_keyed_settings() {
        let tmp = tempfile::tempdir().unwrap();
        let dirs = make_dirs(tmp.path());
        let cfg_path = dirs.config.join("yerd.toml");

        let first_root = tmp.path().join("a-sites");
        let second_root = tmp.path().join("b-sites");
        for folder in ["laravel.com", "acme.org", "example.mysite.com", "shop.com"] {
            std::fs::create_dir_all(first_root.join(folder)).unwrap();
        }
        std::fs::create_dir_all(second_root.join("laravel.com")).unwrap();
        std::fs::create_dir_all(tmp.path().join("shop-linked")).unwrap();

        let mut cfg = valid_config();
        for root in [&first_root, &second_root] {
            cfg.parked.paths.insert(root.to_string_lossy().into_owned());
        }
        cfg.linked.push(
            yerd_core::Site::linked(
                "shop-com",
                tmp.path().join("shop-linked"),
                yerd_core::PhpVersion::new(8, 3),
            )
            .unwrap(),
        );
        cfg.favourites.sites.insert("laravel-com".to_owned());
        cfg.favourites.sites.insert("shop-com".to_owned());
        cfg.groups.order.push("Work".to_owned());
        cfg.groups
            .members
            .insert("laravel-com".to_owned(), "Work".to_owned());
        cfg.domains.parked.insert(
            first_root
                .join("laravel.com")
                .to_string_lossy()
                .into_owned(),
            yerd_config::DomainDelta {
                added: vec![yerd_core::Domain::parse_subpart("www.laravel").unwrap()],
                suppressed: vec![yerd_core::Domain::apex("laravel-com")],
                primary: None,
            },
        );
        cfg.save(&cfg_path).unwrap();

        let daemon = yerdd::startup::bring_up_with_dirs(dirs.clone(), cfg, cfg_path.clone())
            .await
            .expect("bring_up_with_dirs");

        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let daemon_task = tokio::spawn(async move { drive_subsystems(daemon, shutdown_rx).await });

        tokio::time::sleep(Duration::from_millis(100)).await;
        let ipc_sock = dirs.runtime.join("yerd.sock");

        match round_trip(&ipc_sock, &Request::ListSites).await {
            Response::Sites { sites } => {
                let mut names: Vec<&str> = sites.iter().map(|s| s.site.name()).collect();
                names.sort_unstable();
                assert_eq!(
                    names,
                    ["acme", "example-mysite", "laravel", "shop", "shop-com"]
                );
                let laravel = sites
                    .iter()
                    .find(|s| s.site.name() == "laravel")
                    .expect("laravel present");
                assert_eq!(
                    laravel.site.document_root(),
                    first_root.join("laravel.com").as_path()
                );
                assert_eq!(laravel.domains, ["www.laravel.test"]);
            }
            other => panic!("expected Sites, got {other:?}"),
        }

        let on_disk = yerd_config::Config::load(&cfg_path).unwrap();
        assert!(on_disk.favourites.sites.contains("laravel"));
        assert!(!on_disk.favourites.sites.contains("laravel-com"));
        assert!(on_disk.favourites.sites.contains("shop-com"));
        assert_eq!(
            on_disk.groups.members.get("laravel").map(String::as_str),
            Some("Work")
        );
        let claim_of = |folder: &str| {
            on_disk
                .parked
                .claims
                .get(&first_root.join(folder).to_string_lossy().into_owned())
                .map(|c| (c.name.as_str(), c.url_synced))
        };
        assert_eq!(claim_of("laravel.com"), Some(("laravel", false)));
        assert_eq!(claim_of("acme.org"), Some(("acme", false)));
        assert_eq!(claim_of("shop.com"), Some(("shop", true)));
        assert_eq!(on_disk.parked.claims.len(), 4);

        shutdown_tx.send_replace(true);
        let _ = tokio::time::timeout(Duration::from_secs(10), daemon_task).await;
    }

    /// The loopback tunnel origin listener routes by public `Host` through the
    /// route map the daemon rebuilds on `SetSiteTunnel`/`SetTunnelPassword`: a
    /// protected mapped host challenges for the share password, an unmapped
    /// host is a 404.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn tunnel_listener_routes_by_public_host() {
        let tmp = tempfile::tempdir().unwrap();
        let dirs = make_dirs(tmp.path());
        let cfg = valid_config();
        let cfg_path = dirs.config.join("yerd.toml");

        let sites_root = tmp.path().join("Sites");
        std::fs::create_dir_all(sites_root.join("blog")).unwrap();

        let daemon = yerdd::startup::bring_up_with_dirs(dirs.clone(), cfg, cfg_path.clone())
            .await
            .expect("bring_up_with_dirs");
        let tunnel_port = daemon
            .state
            .tunnel_origin_port
            .expect("test daemon binds its tunnel listener");

        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let daemon_task = tokio::spawn(async move { drive_subsystems(daemon, shutdown_rx).await });

        tokio::time::sleep(Duration::from_millis(100)).await;
        let ipc_sock = dirs.runtime.join("yerd.sock");

        for req in [
            Request::Park {
                path: sites_root.clone(),
            },
            Request::SetSiteTunnel {
                site: "blog".into(),
                hostname: Some("blog.example.com".into()),
            },
            Request::SetTunnelPassword {
                site: "blog".into(),
                password: Some("secret".into()),
            },
        ] {
            let resp = round_trip(&ipc_sock, &req).await;
            assert!(matches!(resp, Response::Ok), "{req:?} got {resp:?}");
        }

        let protected = raw_http_status(tunnel_port, "blog.example.com").await;
        assert_eq!(protected, 401);
        let unmapped = raw_http_status(tunnel_port, "nope.example.com").await;
        assert_eq!(unmapped, 404);

        shutdown_tx.send_replace(true);
        let _ = tokio::time::timeout(Duration::from_secs(10), daemon_task).await;
    }

    /// The provider setting requests are dispatched and persisted: the saved
    /// config holds the ngrok token, the normalised domain and the default.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn tunnel_provider_settings_round_trip_over_ipc() {
        let tmp = tempfile::tempdir().unwrap();
        let dirs = make_dirs(tmp.path());
        let cfg_path = dirs.config.join("yerd.toml");
        let daemon =
            yerdd::startup::bring_up_with_dirs(dirs.clone(), valid_config(), cfg_path.clone())
                .await
                .expect("bring_up_with_dirs");
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let daemon_task = tokio::spawn(async move { drive_subsystems(daemon, shutdown_rx).await });
        tokio::time::sleep(Duration::from_millis(100)).await;
        let ipc_sock = dirs.runtime.join("yerd.sock");

        let token = "2fXkAbCdEfGhIjKlMnOpQrStUvWxYz0123456789abcdQa";
        for req in [
            Request::SetNgrokAuthtoken {
                authtoken: Some(token.into()),
            },
            Request::SetNgrokDomain {
                domain: Some("https://Demo.ngrok-free.app/".into()),
            },
            Request::SetDefaultTunnelProvider {
                provider: yerd_core::tunnel_provider::TunnelProvider::Ngrok,
            },
        ] {
            let resp = round_trip(&ipc_sock, &req).await;
            assert!(matches!(resp, Response::Ok), "{resp:?}");
        }

        let saved = yerd_config::Config::load(&cfg_path).unwrap();
        assert_eq!(saved.tunnel.ngrok.authtoken.as_deref(), Some(token));
        assert_eq!(
            saved.tunnel.ngrok.domain.as_deref(),
            Some("demo.ngrok-free.app")
        );
        assert_eq!(
            saved.tunnel.default_provider,
            yerd_core::tunnel_provider::TunnelProvider::Ngrok
        );

        shutdown_tx.send_replace(true);
        let _ = tokio::time::timeout(Duration::from_secs(10), daemon_task).await;
    }

    /// Send one raw HTTP/1.1 `GET /` with `host` to the loopback `port` and
    /// return the response status code.
    async fn raw_http_status(port: u16, host: &str) -> u16 {
        use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
        let mut stream = tokio::net::TcpStream::connect(("127.0.0.1", port))
            .await
            .expect("connect tunnel listener");
        let request = format!("GET / HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n");
        stream.write_all(request.as_bytes()).await.unwrap();
        let mut buf = Vec::new();
        tokio::time::timeout(Duration::from_secs(5), stream.read_to_end(&mut buf))
            .await
            .expect("tunnel listener answered in time")
            .unwrap();
        let text = String::from_utf8_lossy(&buf);
        let status_line = text.lines().next().expect("status line");
        status_line
            .split_whitespace()
            .nth(1)
            .expect("status code")
            .parse()
            .expect("numeric status")
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

        let daemon = yerdd::startup::bring_up_with_dirs(dirs.clone(), cfg, cfg_path.clone())
            .await
            .expect("bring_up_with_dirs");

        let ipc_sock = dirs.runtime.join("yerd.sock");
        assert!(ipc_sock.exists(), "IPC socket should be bound");

        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let daemon_for_task = daemon;
        let daemon_task =
            tokio::spawn(async move { drive_subsystems(daemon_for_task, shutdown_rx).await });

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
        let dns_handle = {
            let bound = daemon.dns_bound.expect("test daemon binds its DNS sockets");
            let responder = yerd_dns::Responder::new(daemon.dns_tld.clone());
            let mut rx = shutdown_rx.clone();
            tokio::spawn(async move {
                bound
                    .serve(responder, yerd_dns::AnswerAddrs::loopback(), async move {
                        let _ = rx.changed().await;
                    })
                    .await
            })
        };
        let proxy_handle = {
            let resolver = Arc::new(yerdd::backend_resolver::DaemonBackendResolver {
                php_manager: daemon.php_manager.clone(),
                wordpress_sites: daemon.state.wordpress_sites.clone(),
            });
            let https = yerd_proxy::HttpsBinding {
                listener: daemon
                    .https_listener
                    .expect("test daemon binds its https listener"),
                public_port: daemon.state.redirect_https_port.clone(),
                cert_store: daemon.cert_store.clone(),
            };
            let router = daemon.state.router.clone();
            let login_tokens = daemon.state.wordpress_login_tokens.clone();
            let login_prepend_script = daemon.state.wordpress_login_prepend_script.clone();
            let mut rx = shutdown_rx.clone();
            tokio::spawn(yerd_proxy::ProxyServer::serve(
                daemon
                    .http_listener
                    .expect("test daemon binds its http listener"),
                Some(https),
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
        };
        let tunnel_handle = daemon.tunnel_listener.map(|listener| {
            let resolver = Arc::new(yerdd::backend_resolver::DaemonBackendResolver {
                php_manager: daemon.php_manager.clone(),
                wordpress_sites: daemon.state.wordpress_sites.clone(),
            });
            let binding = yerd_proxy::TunnelBinding {
                listener,
                routes: daemon.state.tunnel_routes.clone(),
                public_url_prepend: daemon.tunnel_wp_prepend.clone(),
            };
            let mut rx = shutdown_rx.clone();
            tokio::spawn(yerd_proxy::ProxyServer::serve_tunnel(
                binding,
                daemon.state.router.clone(),
                resolver,
                daemon.state.symlink_protection.clone(),
                std::sync::Arc::new(yerd_proxy::ProxyClientTls::new(
                    yerd_proxy::ProxyClientTls::no_verify_config().unwrap(),
                    yerd_proxy::ProxyClientTls::no_verify_config().unwrap(),
                )),
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

        let _ = tokio::time::timeout(Duration::from_secs(10), dns_handle).await;
        let _ = tokio::time::timeout(Duration::from_secs(10), proxy_handle).await;
        if let Some(tunnel_handle) = tunnel_handle {
            let _ = tokio::time::timeout(Duration::from_secs(10), tunnel_handle).await;
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
