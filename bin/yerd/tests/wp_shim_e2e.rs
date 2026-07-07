//! End-to-end: exercise `wp_shim::site_scope` against a real daemon booted on
//! a tempdir, mirroring `cli_e2e.rs`'s pattern. Covers the site-scoping
//! behavior the unit tests in `wp_shim.rs` can't reach without a real
//! `ListSites` response: a cwd inside a registered site whose pinned PHP is
//! (or isn't) installed.

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

    use yerd::wp_shim::{site_scope, ScopeResolution};
    use yerd_core::PhpVersion;
    use yerd_ipc::Request;

    fn make_dirs(tmp: &std::path::Path) -> yerd_platform::PlatformDirs {
        yerd_platform::PlatformDirs {
            config: tmp.join("c"),
            data: tmp.join("d"),
            state: tmp.join("s"),
            cache: tmp.join("ca"),
            runtime: tmp.join("r"),
        }
    }

    /// Two distinct, currently-free, non-zero ports (see `cli_e2e.rs`'s
    /// identical helper for why: `validate()` rejects port 0 / equal ports).
    fn valid_config() -> yerd_config::Config {
        let a = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let b = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let (pa, pb) = (
            a.local_addr().unwrap().port(),
            b.local_addr().unwrap().port(),
        );
        drop(a);
        drop(b);
        let mut cfg = yerd_config::Config::default();
        cfg.ports.http = pa;
        cfg.ports.https = pb;
        cfg.dns_port = 0;
        cfg
    }

    /// Lay down a fake, executable-looking PHP CLI binary at the path
    /// `shim::cli_binary` expects, so `site_scope` sees the pinned version as
    /// "installed" without needing a real PHP build.
    fn fake_php_cli(dirs: &yerd_platform::PlatformDirs, version: PhpVersion) {
        let bin = dirs
            .data
            .join("php")
            .join(format!("php-{}.{}", version.major, version.minor))
            .join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::write(bin.join("php"), b"#!/bin/sh\n").unwrap();
    }

    /// Run `site_scope` on a blocking-pool thread - it builds its own ad-hoc
    /// tokio runtime internally, which panics if called from inside this
    /// test's own async runtime.
    async fn scoped_site_scope(
        dirs: yerd_platform::PlatformDirs,
        cwd: std::path::PathBuf,
    ) -> ScopeResolution {
        tokio::task::spawn_blocking(move || site_scope(&dirs, &cwd))
            .await
            .unwrap()
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn site_scope_against_a_real_daemon() {
        let tmp = tempfile::tempdir().unwrap();
        let dirs = make_dirs(tmp.path());
        let cfg_path = dirs.config.join("yerd.toml");

        let installed_php = PhpVersion::new(8, 3);
        let missing_php = PhpVersion::new(7, 4);
        fake_php_cli(&dirs, installed_php);

        let scoped_dir = tmp.path().join("blog");
        std::fs::create_dir_all(scoped_dir.join("wp-content")).unwrap();
        let unscoped_dir = tmp.path().join("elsewhere");
        std::fs::create_dir_all(&unscoped_dir).unwrap();
        let missing_php_dir = tmp.path().join("legacy");
        std::fs::create_dir_all(&missing_php_dir).unwrap();

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

        // Link "blog" and pin it to the installed PHP version.
        let req = yerd::resolve_link(Some("blog"), Some(&scoped_dir)).expect("resolve_link");
        assert!(matches!(
            yerd::transport::exchange_at(&sock, &req).await.unwrap(),
            yerd_ipc::Response::Ok
        ));
        assert!(matches!(
            yerd::transport::exchange_at(
                &sock,
                &Request::SetPhp {
                    name: "blog".into(),
                    version: installed_php,
                },
            )
            .await
            .unwrap(),
            yerd_ipc::Response::Ok
        ));

        // Link "legacy" and pin it to a version with no fake binary laid down.
        let req = yerd::resolve_link(Some("legacy"), Some(&missing_php_dir)).expect("resolve_link");
        assert!(matches!(
            yerd::transport::exchange_at(&sock, &req).await.unwrap(),
            yerd_ipc::Response::Ok
        ));
        assert!(matches!(
            yerd::transport::exchange_at(
                &sock,
                &Request::SetPhp {
                    name: "legacy".into(),
                    version: missing_php,
                },
            )
            .await
            .unwrap(),
            yerd_ipc::Response::Ok
        ));

        // cwd inside the scoped site (a subdirectory, not the root itself) ->
        // Scoped, with the site's own PHP and canonical document root.
        // `site_scope` builds its own ad-hoc tokio runtime internally (the
        // production caller, `run()`, has none of its own); run it on a
        // blocking-pool thread so it isn't nested inside this test's own
        // runtime.
        let cwd = std::fs::canonicalize(scoped_dir.join("wp-content")).unwrap();
        match scoped_site_scope(dirs.clone(), cwd).await {
            ScopeResolution::Scoped(scope) => {
                assert_eq!(
                    scope.document_root,
                    std::fs::canonicalize(&scoped_dir).unwrap()
                );
                assert!(scope.php_bin.ends_with("php-8.3/bin/php"));
            }
            other => panic!("expected Scoped, got {other:?}"),
        }

        // cwd inside a site pinned to a PHP version with no installed binary
        // -> MatchedPhpMissing, not a silent fall-through to NoScope.
        let cwd = std::fs::canonicalize(&missing_php_dir).unwrap();
        match scoped_site_scope(dirs.clone(), cwd).await {
            ScopeResolution::MatchedPhpMissing { php_version } => {
                assert_eq!(php_version, missing_php);
            }
            other => panic!("expected MatchedPhpMissing, got {other:?}"),
        }

        // cwd outside every registered site -> NoScope.
        let cwd = std::fs::canonicalize(&unscoped_dir).unwrap();
        assert!(matches!(
            scoped_site_scope(dirs.clone(), cwd).await,
            ScopeResolution::NoScope
        ));

        shutdown_tx.send_replace(true);
        let _ = tokio::time::timeout(Duration::from_secs(5), ipc_task).await;
        drop(keep_alive);
    }
}
