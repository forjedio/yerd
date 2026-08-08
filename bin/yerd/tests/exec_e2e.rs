//! End-to-end: exercise `exec_cmd::select_php` against a real daemon booted on
//! a tempdir, mirroring `wp_shim_e2e.rs`'s pattern. Covers the resolution the
//! unit tests in `exec_cmd.rs` can't reach without a real `ListSites`
//! response: a cwd inside a registered site, an explicit `--site` override
//! from outside every site, and both forms of the missing-pinned-PHP failure.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::match_wildcard_for_single_variants
)]

#[cfg(unix)]
mod tests {
    use std::path::{Path, PathBuf};
    use std::time::Duration;

    use tokio::sync::watch;

    use yerd::exec_cmd::{select_php, PhpSelection};
    use yerd_core::PhpVersion;
    use yerd_ipc::Request;

    fn make_dirs(tmp: &Path) -> yerd_platform::PlatformDirs {
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

    /// Lay down a fake, executable-looking PHP CLI binary where
    /// `shim::cli_binary` expects one, so a version counts as "installed"
    /// without needing a real PHP build.
    fn fake_php_cli(dirs: &yerd_platform::PlatformDirs, version: PhpVersion) {
        let bin = dirs
            .data
            .join("php")
            .join(format!("php-{}.{}", version.major, version.minor))
            .join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::write(bin.join("php"), b"#!/bin/sh\n").unwrap();
    }

    /// Run `select_php` on a blocking-pool thread - the site lookup builds its
    /// own ad-hoc tokio runtime internally, which panics if called from inside
    /// this test's own async runtime.
    async fn scoped_select(
        dirs: yerd_platform::PlatformDirs,
        cwd: Option<PathBuf>,
        site: Option<String>,
    ) -> Result<PhpSelection, String> {
        tokio::task::spawn_blocking(move || select_php(&dirs, cwd.as_deref(), site.as_deref()))
            .await
            .unwrap()
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    #[allow(clippy::too_many_lines)]
    async fn select_php_against_a_real_daemon() {
        let tmp = tempfile::tempdir().unwrap();
        let dirs = make_dirs(tmp.path());
        let cfg_path = dirs.config.join("yerd.toml");

        let pinned_php = PhpVersion::new(8, 3);
        let config_default = PhpVersion::new(8, 4);
        let missing_php = PhpVersion::new(7, 4);
        // The pinned version must differ from the global default, or "resolved
        // the site's version" and "fell back to the default" would be
        // indistinguishable.
        assert_ne!(pinned_php, config_default);
        let mut cfg = valid_config();
        cfg.php.default = config_default;
        fake_php_cli(&dirs, pinned_php);
        fake_php_cli(&dirs, config_default);

        let site_dir = tmp.path().join("blog");
        std::fs::create_dir_all(site_dir.join("app")).unwrap();
        let outside_dir = tmp.path().join("elsewhere");
        std::fs::create_dir_all(&outside_dir).unwrap();
        let missing_php_dir = tmp.path().join("legacy");
        std::fs::create_dir_all(&missing_php_dir).unwrap();

        let daemon = yerdd::startup::bring_up_with_dirs(dirs.clone(), cfg, cfg_path.clone())
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

        // Link "blog", pinned to an installed version.
        let req = yerd::resolve_link(Some("blog"), Some(&site_dir)).expect("resolve_link");
        assert!(matches!(
            yerd::transport::exchange_at(&sock, &req).await.unwrap(),
            yerd_ipc::Response::Ok
        ));
        assert!(matches!(
            yerd::transport::exchange_at(
                &sock,
                &Request::SetPhp {
                    name: "blog".into(),
                    version: pinned_php,
                },
            )
            .await
            .unwrap(),
            yerd_ipc::Response::Ok
        ));

        // Link "legacy", pinned to a version with no binary laid down.
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

        // cwd inside the site (a subdirectory, not the root itself) -> its
        // pinned version, not the global default.
        let cwd = std::fs::canonicalize(site_dir.join("app")).unwrap();
        match scoped_select(dirs.clone(), Some(cwd), None).await.unwrap() {
            PhpSelection::Site(scope) => {
                assert_eq!(scope.site_name, "blog");
                assert_eq!(scope.php_minor, "8.3");
                assert!(scope.php_bin.ends_with("php-8.3/bin/php"));
            }
            other => panic!("expected Site, got {other:?}"),
        }

        // `--site` resolves the named site even from outside every site.
        let cwd = std::fs::canonicalize(&outside_dir).unwrap();
        match scoped_select(dirs.clone(), Some(cwd.clone()), Some("blog".to_owned()))
            .await
            .unwrap()
        {
            PhpSelection::Site(scope) => {
                assert_eq!(scope.site_name, "blog");
                assert_eq!(scope.php_minor, "8.3");
            }
            other => panic!("expected Site, got {other:?}"),
        }

        // A site name is stored lowercased, so the lookup must be too.
        match scoped_select(dirs.clone(), Some(cwd.clone()), Some("BLOG".to_owned()))
            .await
            .unwrap()
        {
            PhpSelection::Site(scope) => assert_eq!(scope.site_name, "blog"),
            other => panic!("expected Site, got {other:?}"),
        }

        // cwd outside every site -> the global default (whichever version the
        // config names), no error.
        match scoped_select(dirs.clone(), Some(cwd), None).await.unwrap() {
            PhpSelection::Default { php_bin, minor } => {
                assert_eq!(minor, config_default.to_string());
                assert!(php_bin.ends_with(format!("php-{config_default}/bin/php")));
            }
            other => panic!("expected Default, got {other:?}"),
        }

        // cwd inside a site pinned to an uninstalled version -> a loud failure,
        // not a silent fall-through to the default PHP.
        let cwd = std::fs::canonicalize(&missing_php_dir).unwrap();
        let err = scoped_select(dirs.clone(), Some(cwd), None)
            .await
            .unwrap_err();
        assert!(err.contains("pinned to PHP 7.4"), "got: {err}");
        assert!(err.contains("yerd install php 7.4"), "got: {err}");

        // ...and the same via `--site`, from outside the site.
        let cwd = std::fs::canonicalize(&outside_dir).unwrap();
        let err = scoped_select(dirs.clone(), Some(cwd.clone()), Some("legacy".to_owned()))
            .await
            .unwrap_err();
        assert!(err.contains("pinned to PHP 7.4"), "got: {err}");

        // An unknown `--site` name is an error, never a default fallback.
        let err = scoped_select(dirs.clone(), Some(cwd), Some("nope".to_owned()))
            .await
            .unwrap_err();
        assert!(err.contains("no site named 'nope'"), "got: {err}");

        shutdown_tx.send_replace(true);
        let _ = tokio::time::timeout(Duration::from_secs(5), ipc_task).await;
        drop(keep_alive);
    }
}
