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

mod tests {
    use std::path::{Path, PathBuf};
    use std::time::Duration;

    use tokio::sync::watch;

    use yerd::exec_cmd::{select_php, PhpSelection};
    use yerd_core::PhpVersion;
    use yerd_ipc::{Request, Response};
    use yerd_platform::PlatformDirs;

    fn make_dirs(tmp: &Path) -> yerd_platform::PlatformDirs {
        yerd_platform::PlatformDirs {
            config: tmp.join("c"),
            data: tmp.join("d"),
            state: tmp.join("s"),
            cache: tmp.join("ca"),
            runtime: tmp.join("r"),
        }
    }

    /// Exchange one request over the daemon's platform-native transport (Unix
    /// socket, or the SID-derived named pipe on Windows), asserting `Ok`. See
    /// `wp_shim_e2e.rs`'s identical helper.
    async fn exchange_ok(dirs: &PlatformDirs, req: &Request) {
        let resp = {
            #[cfg(unix)]
            {
                yerd::transport::exchange_at(&dirs.runtime.join("yerd.sock"), req)
                    .await
                    .unwrap()
            }
            #[cfg(windows)]
            {
                let name = yerd_platform::daemon_pipe_name(dirs).expect("derive pipe name");
                yerd::transport::exchange_at_name(&name, req).await.unwrap()
            }
        };
        assert!(matches!(resp, Response::Ok), "expected Ok, got {resp:?}");
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
        let ver_root = dirs
            .data
            .join("php")
            .join(format!("php-{}.{}", version.major, version.minor));
        #[cfg(unix)]
        {
            let bin = ver_root.join("bin");
            std::fs::create_dir_all(&bin).unwrap();
            std::fs::write(bin.join("php"), b"#!/bin/sh\n").unwrap();
        }
        #[cfg(windows)]
        {
            std::fs::create_dir_all(&ver_root).unwrap();
            std::fs::write(ver_root.join("php.exe"), b"x").unwrap();
        }
    }

    /// Run `select_php` on a blocking-pool thread - the site lookup builds its
    /// own ad-hoc tokio runtime internally, which panics if called from inside
    /// this test's own async runtime.
    async fn scoped_select(
        dirs: yerd_platform::PlatformDirs,
        cwd: Option<PathBuf>,
        site: Option<String>,
    ) -> Result<PhpSelection, yerd::exec_cmd::SelectError> {
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
        exchange_ok(&dirs, &req).await;
        exchange_ok(
            &dirs,
            &Request::SetPhp {
                name: "blog".into(),
                version: pinned_php,
            },
        )
        .await;

        // Link "legacy", pinned to a version with no binary laid down.
        let req = yerd::resolve_link(Some("legacy"), Some(&missing_php_dir)).expect("resolve_link");
        exchange_ok(&dirs, &req).await;
        exchange_ok(
            &dirs,
            &Request::SetPhp {
                name: "legacy".into(),
                version: missing_php,
            },
        )
        .await;

        // cwd inside the site (a subdirectory, not the root itself) -> its
        // pinned version, not the global default.
        let cwd = std::fs::canonicalize(site_dir.join("app")).unwrap();
        match scoped_select(dirs.clone(), Some(cwd), None).await.unwrap() {
            PhpSelection::Site(scope) => {
                assert_eq!(scope.site_name, "blog");
                assert_eq!(scope.php_minor, "8.3");
                #[cfg(unix)]
                assert!(scope.php_bin.ends_with("php-8.3/bin/php"));
                #[cfg(windows)]
                assert!(scope.php_bin.ends_with(r"php-8.3\php.exe"));
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
                #[cfg(unix)]
                assert!(php_bin.ends_with(format!("php-{config_default}/bin/php")));
                #[cfg(windows)]
                assert!(php_bin.ends_with(format!(r"php-{config_default}\php.exe")));
            }
            other => panic!("expected Default, got {other:?}"),
        }

        // cwd inside a site pinned to an uninstalled version -> a loud failure,
        // not a silent fall-through to the default PHP.
        let cwd = std::fs::canonicalize(&missing_php_dir).unwrap();
        let err = scoped_select(dirs.clone(), Some(cwd), None)
            .await
            .unwrap_err();
        assert!(err.message.contains("pinned to PHP 7.4"), "got: {err:?}");
        assert!(err.message.contains("yerd install php 7.4"), "got: {err:?}");
        assert_eq!(err.code, 2, "a pinned-but-uninstalled version is exit 2");

        // ...and the same via `--site`, from outside the site.
        let cwd = std::fs::canonicalize(&outside_dir).unwrap();
        let err = scoped_select(dirs.clone(), Some(cwd.clone()), Some("legacy".to_owned()))
            .await
            .unwrap_err();
        assert!(err.message.contains("pinned to PHP 7.4"), "got: {err:?}");
        assert_eq!(err.code, 2);

        // An unknown `--site` name is an error, never a default fallback - and
        // a *usage* error (exit 2), not the exit 1 every shim failure returns.
        // With the daemon up, this must not be mistaken for exit 69 either.
        let err = scoped_select(dirs.clone(), Some(cwd), Some("nope".to_owned()))
            .await
            .unwrap_err();
        assert!(err.message.contains("no site named 'nope'"), "got: {err:?}");
        assert_eq!(err.code, 2, "an unknown site name is a usage error");

        shutdown_tx.send_replace(true);
        let _ = tokio::time::timeout(Duration::from_secs(5), ipc_task).await;
        drop(keep_alive);
    }
}

/// End-to-end on Windows: drive the real `yerd.exe` and observe the one
/// behaviour that is genuinely new there, namely that `yerd exec` spawns the
/// tool, waits, and propagates its exit code rather than replacing itself.
///
/// No daemon is started, so both tests take the `NoScope` fallback to the
/// configured default PHP, which is the deterministic shape on a bare runner.
/// Scaffolding mirrors `cover_shim_e2e.rs`'s `win_tests`.
#[cfg(windows)]
mod win_tests {
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::process::Command;

    /// A Rust stand-in for `php.exe`, compiled with `rustc` at test start. It
    /// exits `3` on `--fail`, so a propagated code is distinguishable from both
    /// success and the flat `1`/`2` a swallowed failure would produce.
    const STUB_PHP_RS: &str = r#"
fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "--fail") {
        std::process::exit(3);
    }
    println!("args={}", args.join(" "));
}
"#;

    /// `%LOCALAPPDATA%\yerd\data` for a fake root, matching `WindowsPaths`.
    fn data_dir(root: &Path) -> PathBuf {
        root.join("local").join("yerd").join("data")
    }

    fn compile_stub_php(dest: &Path) {
        let src = dest
            .parent()
            .expect("dest has a parent")
            .join("php_stub_main.rs");
        fs::write(&src, STUB_PHP_RS).unwrap();
        let out = Command::new("rustc")
            .arg("-O")
            .arg("--crate-name")
            .arg("php_stub")
            .arg("-o")
            .arg(dest)
            .arg(&src)
            .output()
            .expect("run rustc to build the stub php.exe");
        assert!(
            out.status.success(),
            "rustc failed to build the stub php.exe: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    /// A faked Windows root with PHP 8.4 as the only installed version.
    ///
    /// No config file is written: with none present, `resolve_default_php`
    /// falls through to the highest installed version, which is 8.4. That is
    /// the same shape `cover_shim_e2e.rs`'s `win_tests` relies on.
    fn faked_default_php_layout() -> (tempfile::TempDir, PathBuf) {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_path_buf();
        for sub in ["appdata", "local", "temp"] {
            fs::create_dir_all(root.join(sub)).unwrap();
        }
        let ver = data_dir(&root).join("php").join("php-8.4");
        fs::create_dir_all(&ver).unwrap();
        compile_stub_php(&ver.join("php.exe"));
        (tmp, root)
    }

    /// Run `program` under the faked root's Windows environment, with the rest
    /// inherited so `SystemRoot` stays intact.
    fn run_in_root(program: &Path, args: &[&str], root: &Path) -> std::process::Output {
        Command::new(program)
            .args(args)
            .env("APPDATA", root.join("appdata"))
            .env("LOCALAPPDATA", root.join("local"))
            .env("TEMP", root.join("temp"))
            .env("TMP", root.join("temp"))
            .output()
            .expect("run yerd")
    }

    fn yerd_exe() -> &'static Path {
        Path::new(env!("CARGO_BIN_EXE_yerd"))
    }

    /// The child's own exit code must reach the caller. Before `exec` worked on
    /// Windows this was a flat `2` from the declining arm.
    #[test]
    fn exec_propagates_the_childs_exit_code() {
        let (_tmp, root) = faked_default_php_layout();
        let out = run_in_root(yerd_exe(), &["exec", "php", "--fail"], &root);
        assert_eq!(
            out.status.code(),
            Some(3),
            "expected the stub's own code 3, got {:?}; stderr: {}",
            out.status.code(),
            String::from_utf8_lossy(&out.stderr)
        );
    }

    /// `yerd which php` resolves to the default version's Windows binary path.
    #[test]
    fn which_reports_the_default_php_binary() {
        let (_tmp, root) = faked_default_php_layout();
        let out = run_in_root(yerd_exe(), &["which", "php"], &root);
        assert!(
            out.status.success(),
            "which failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(
            stdout.trim_end().ends_with(r"php-8.4\php.exe"),
            "unexpected which output: {stdout}"
        );
    }
}
