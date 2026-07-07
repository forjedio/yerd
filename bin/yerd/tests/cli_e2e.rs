//! End-to-end: drive the CLI client transport against a real daemon booted on
//! a tempdir, exercising every command through the socket. Only the IPC task is
//! spawned (no proxy/DNS) - none of the shipped commands touch them, and
//! skipping them keeps the test fast and CI-stable.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

#[cfg(unix)]
mod tests {
    use std::time::Duration;

    use tokio::sync::watch;

    use yerd::cli::Command;
    use yerd::{map, transport};
    use yerd_ipc::Response;

    fn make_dirs(tmp: &std::path::Path) -> yerd_platform::PlatformDirs {
        yerd_platform::PlatformDirs {
            config: tmp.join("c"),
            data: tmp.join("d"),
            state: tmp.join("s"),
            cache: tmp.join("ca"),
            runtime: tmp.join("r"),
        }
    }

    /// Two distinct, currently-free, non-zero ports - required because a
    /// mutation persists the config and `validate()` rejects port 0 / equal.
    fn valid_config() -> yerd_config::Config {
        let a = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let b = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let (pa, pb) = (
            a.local_addr().unwrap().port(),
            b.local_addr().unwrap().port(),
        );
        drop(a);
        drop(b);
        assert_ne!(pa, pb);
        let mut cfg = yerd_config::Config::default();
        cfg.ports.http = pa;
        cfg.ports.https = pb;
        cfg.dns_port = 0;
        cfg
    }

    async fn send(sock: &std::path::Path, cmd: &Command) -> Response {
        let req = map::to_request(cmd).expect("map command");
        transport::exchange_at(sock, &req).await.expect("exchange")
    }

    /// Drives `Command::Link`'s CLI-side resolution (`resolve_link`) directly
    /// and exchanges the resulting `Request::Link` with the daemon -
    /// `Command::Link` never reaches `map::to_request`, so `send()` can't be
    /// used for it.
    async fn link(sock: &std::path::Path, name: &str, path: &std::path::Path) -> Response {
        let req = yerd::resolve_link(Some(name), Some(path)).expect("resolve_link");
        transport::exchange_at(sock, &req).await.expect("exchange")
    }

    fn site_names(resp: &Response) -> Vec<String> {
        match resp {
            Response::Sites { sites } => sites.iter().map(|s| s.site.name().to_owned()).collect(),
            other => panic!("expected Sites, got {other:?}"),
        }
    }

    async fn blog_is_secure(sock: &std::path::Path) -> bool {
        match send(sock, &Command::Sites).await {
            Response::Sites { sites } => sites
                .iter()
                .find(|s| s.site.name() == "blog")
                .expect("blog present")
                .site
                .secure(),
            other => panic!("expected Sites, got {other:?}"),
        }
    }

    /// `secure` then `unsecure` the already-promoted `blog` site, asserting the
    /// flag flips on and back off.
    async fn exercise_secure_toggle(sock: &std::path::Path) {
        assert!(matches!(
            send(
                sock,
                &Command::Secure {
                    name: "blog".into()
                }
            )
            .await,
            Response::Ok
        ));
        assert!(blog_is_secure(sock).await);
        assert!(matches!(
            send(
                sock,
                &Command::Unsecure {
                    name: "blog".into()
                }
            )
            .await,
            Response::Ok
        ));
        assert!(!blog_is_secure(sock).await);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    #[allow(clippy::too_many_lines)]
    async fn cli_commands_round_trip_against_daemon() {
        let tmp = tempfile::tempdir().unwrap();
        let dirs = make_dirs(tmp.path());
        let cfg_path = dirs.config.join("yerd.toml");

        let sites_root = tmp.path().join("Sites");
        std::fs::create_dir_all(sites_root.join("blog")).unwrap();
        let linked_dir = tmp.path().join("standalone");
        std::fs::create_dir_all(linked_dir.join("public")).unwrap();
        std::fs::write(linked_dir.join("artisan"), b"").unwrap();
        std::fs::write(linked_dir.join("public/index.php"), b"").unwrap();

        let daemon =
            yerdd::startup::bring_up_with_dirs(dirs.clone(), valid_config(), cfg_path.clone())
                .await
                .expect("bring_up_with_dirs");
        let sock = dirs.runtime.join("yerd.sock");

        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let state = daemon.state.clone();
        let ipc_task = tokio::spawn(yerdd::ipc_server::run(
            daemon.ipc_listener,
            state,
            shutdown_rx,
        ));
        let keep_alive = (
            daemon.lock,
            daemon.dns_bound,
            daemon.http_listener,
            daemon.https_listener,
            daemon.php_manager,
        );
        tokio::time::sleep(Duration::from_millis(100)).await;

        assert!(matches!(send(&sock, &Command::Ping).await, Response::Pong));

        assert!(matches!(
            send(
                &sock,
                &Command::Park {
                    path: sites_root.clone()
                }
            )
            .await,
            Response::Ok
        ));
        assert!(site_names(&send(&sock, &Command::Sites).await).contains(&"blog".to_owned()));

        assert!(matches!(
            link(&sock, "app", &linked_dir).await,
            Response::Ok
        ));
        assert!(site_names(&send(&sock, &Command::Sites).await).contains(&"app".to_owned()));
        match send(&sock, &Command::Sites).await {
            Response::Sites { sites } => {
                let app = sites.iter().find(|s| s.site.name() == "app").unwrap();
                assert_eq!(
                    app.site.web_subpath(),
                    std::path::Path::new("public"),
                    "linking a Laravel-shaped dir should auto-detect its web root"
                );
            }
            other => panic!("expected Sites, got {other:?}"),
        }

        assert!(matches!(
            send(
                &sock,
                &Command::Use {
                    first: "blog".into(),
                    version: Some("8.4".into())
                }
            )
            .await,
            Response::Ok
        ));
        match send(&sock, &Command::Sites).await {
            Response::Sites { sites } => {
                let blog = sites.iter().find(|s| s.site.name() == "blog").unwrap();
                assert_eq!(blog.site.php(), yerd_core::PhpVersion::new(8, 4));
                assert_eq!(blog.site.kind(), yerd_core::SiteKind::Parked);
            }
            other => panic!("expected Sites, got {other:?}"),
        }

        exercise_secure_toggle(&sock).await;

        match send(&sock, &Command::Status).await {
            Response::Status { report } => {
                assert_eq!(report.tld, "test");
                assert_eq!(report.daemon_pid, std::process::id());
                assert!(report.sites.linked >= 1);
                assert!(report.php.is_empty());
            }
            other => panic!("expected Status, got {other:?}"),
        }

        let diag = send(&sock, &Command::Doctor { action: None }).await;
        match &diag {
            Response::Diagnoses { items } => {
                assert!(items
                    .iter()
                    .any(|d| d.code == yerd_ipc::DiagnosisCode::NoPhpInstalled));
            }
            other => panic!("expected Diagnoses, got {other:?}"),
        }
        assert_eq!(map::render(&diag, false).code, 1, "FAIL → exit 1");
        assert_eq!(
            map::render(&diag, true).code,
            1,
            "JSON path agrees on exit 1"
        );

        match send(
            &sock,
            &Command::Doctor {
                action: Some(yerd::cli::DoctorAction::Fix),
            },
        )
        .await
        {
            Response::DoctorFix { report } => {
                assert!(report.performed.is_empty());
                assert!(report
                    .manual
                    .iter()
                    .any(|d| d.severity == yerd_ipc::Severity::Fail));
            }
            other => panic!("expected DoctorFix, got {other:?}"),
        }

        assert!(matches!(
            send(&sock, &Command::Unlink { name: "app".into() }).await,
            Response::Ok
        ));
        assert!(!site_names(&send(&sock, &Command::Sites).await).contains(&"app".to_owned()));

        match send(
            &sock,
            &Command::Unlink {
                name: "ghost".into(),
            },
        )
        .await
        {
            Response::Error { code, .. } => {
                assert_eq!(code, yerd_ipc::ErrorCode::NotFound);
            }
            other => panic!("expected Error, got {other:?}"),
        }

        let on_disk = std::fs::read_to_string(&cfg_path).expect("config written");
        let canonical = std::fs::canonicalize(&sites_root).unwrap();
        let canonical_str = canonical.to_string_lossy().into_owned();
        assert!(on_disk.contains(&canonical_str));

        match send(
            &sock,
            &Command::List {
                target: yerd::cli::ListTarget::Parked,
            },
        )
        .await
        {
            Response::Parked { paths } => {
                assert!(paths.contains(&canonical_str), "parked roots: {paths:?}");
            }
            other => panic!("expected Parked, got {other:?}"),
        }

        assert!(matches!(
            send(
                &sock,
                &Command::Unpark {
                    path: canonical.clone()
                }
            )
            .await,
            Response::Ok
        ));
        match send(
            &sock,
            &Command::List {
                target: yerd::cli::ListTarget::Parked,
            },
        )
        .await
        {
            Response::Parked { paths } => {
                assert!(!paths.contains(&canonical_str), "parked roots: {paths:?}");
            }
            other => panic!("expected Parked, got {other:?}"),
        }

        assert!(matches!(
            send(&sock, &Command::Unpark { path: canonical }).await,
            Response::Ok
        ));

        shutdown_tx.send_replace(true);
        let _ = tokio::time::timeout(Duration::from_secs(5), ipc_task).await;
        drop(keep_alive);
    }
}
