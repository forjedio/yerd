//! End-to-end: drive the CLI client transport against a real daemon booted on
//! a tempdir, exercising every command through the socket. Only the IPC task is
//! spawned (no proxy/DNS) — none of the shipped commands touch them, and
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

    /// Two distinct, currently-free, non-zero ports — required because a
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
        cfg
    }

    async fn send(sock: &std::path::Path, cmd: &Command) -> Response {
        let req = map::to_request(cmd).expect("map command");
        transport::exchange_at(sock, &req).await.expect("exchange")
    }

    fn site_names(resp: &Response) -> Vec<String> {
        match resp {
            Response::Sites { sites } => sites.iter().map(|s| s.name().to_owned()).collect(),
            other => panic!("expected Sites, got {other:?}"),
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn cli_commands_round_trip_against_daemon() {
        let tmp = tempfile::tempdir().unwrap();
        let dirs = make_dirs(tmp.path());
        let cfg_path = dirs.config.join("yerd.toml");

        // A parked root with one child, and a separate dir to `link`.
        let sites_root = tmp.path().join("Sites");
        std::fs::create_dir_all(sites_root.join("blog")).unwrap();
        let linked_dir = tmp.path().join("standalone");
        std::fs::create_dir_all(&linked_dir).unwrap();

        let daemon =
            yerdd::startup::bring_up_with_dirs(dirs.clone(), valid_config(), cfg_path.clone())
                .await
                .expect("bring_up_with_dirs");
        let sock = dirs.runtime.join("yerd.sock");

        // Spawn ONLY the IPC task. Keep `daemon` alive (holds the instance lock
        // + bound listeners) until the test ends.
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let state = daemon.state.clone();
        let ipc_task = tokio::spawn(yerdd::ipc_server::run(
            daemon.ipc_listener,
            state,
            shutdown_rx,
        ));
        // Remaining daemon fields stay owned by this binding (not dropped).
        let keep_alive = (
            daemon.lock,
            daemon.dns_bound,
            daemon.http_listener,
            daemon.https_listener,
            daemon.php_manager,
        );
        tokio::time::sleep(Duration::from_millis(100)).await;

        // ping
        assert!(matches!(send(&sock, &Command::Ping).await, Response::Pong));

        // park → the child becomes a site
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

        // link a standalone dir
        assert!(matches!(
            send(
                &sock,
                &Command::Link {
                    name: "app".into(),
                    path: linked_dir.clone()
                }
            )
            .await,
            Response::Ok
        ));
        assert!(site_names(&send(&sock, &Command::Sites).await).contains(&"app".to_owned()));

        // use → promotes the parked `blog` to a linked entry at 8.4
        assert!(matches!(
            send(
                &sock,
                &Command::Use {
                    name: "blog".into(),
                    version: "8.4".into()
                }
            )
            .await,
            Response::Ok
        ));
        match send(&sock, &Command::Sites).await {
            Response::Sites { sites } => {
                let blog = sites.iter().find(|s| s.name() == "blog").unwrap();
                assert_eq!(blog.php(), yerd_core::PhpVersion::new(8, 4));
                assert_eq!(blog.kind(), yerd_core::SiteKind::Linked);
            }
            other => panic!("expected Sites, got {other:?}"),
        }

        // unlink the linked app
        assert!(matches!(
            send(&sock, &Command::Unlink { name: "app".into() }).await,
            Response::Ok
        ));
        assert!(!site_names(&send(&sock, &Command::Sites).await).contains(&"app".to_owned()));

        // unlink unknown → NotFound error response (exit-code 1 via render)
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

        // The config was persisted with the parked path.
        let on_disk = std::fs::read_to_string(&cfg_path).expect("config written");
        let canonical = std::fs::canonicalize(&sites_root).unwrap();
        assert!(on_disk.contains(&canonical.to_string_lossy().into_owned()));

        shutdown_tx.send_replace(true);
        let _ = tokio::time::timeout(Duration::from_secs(5), ipc_task).await;
        drop(keep_alive);
    }
}
