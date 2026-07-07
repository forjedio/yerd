//! `Request::WordpressAdminUsers` handler: lists a `WordPress` site's
//! administrator accounts for the auto-login user picker (see
//! `apps/yerd-gui/src/views/SitesView.vue`'s edit dialog). A thin, read-only
//! sibling of [`crate::wordpress_url_sync`] - same WP-CLI invocation pattern,
//! but captures and parses JSON stdout instead of just checking success.

use std::path::{Path, PathBuf};
use std::process::Stdio;

use yerd_ipc::{ErrorCode, Response, WordPressAdminUser};

use crate::state::DaemonState;

/// `Request::WordpressAdminUsers` handler. Checks `is_wordpress` via the
/// cache (same `NotFound` pattern `mint_wordpress_login_token` uses), then
/// runs `wp user list --role=administrator` and parses its JSON stdout.
/// Errors (WP-CLI/PHP missing, a non-zero exit, unparseable JSON) surface as
/// `Response::Error { code: Internal, .. }` - the GUI's picker shows an
/// empty/error state gracefully, never crashes.
pub async fn admin_users(site: &str, state: &DaemonState) -> Response {
    let (served_root, php) = {
        let guard = state.router.read().await;
        match guard.get(site) {
            Some(s) => (s.served_root(), s.php()),
            None => {
                return Response::Error {
                    code: ErrorCode::NotFound,
                    message: format!("no site named \"{site}\""),
                }
            }
        }
    };
    let is_wordpress = state
        .wordpress_sites
        .read()
        .await
        .get(site)
        .copied()
        .unwrap_or(false);
    if !is_wordpress {
        return Response::Error {
            code: ErrorCode::NotFound,
            message: format!("\"{site}\" is not a WordPress site"),
        };
    }

    let boot_fs = crate::tools::wp_cli::boot_path(&state.dirs);
    if !boot_fs.is_file() {
        return Response::Error {
            code: ErrorCode::Internal,
            message: "WP-CLI is not installed".into(),
        };
    }
    let php_cli = crate::php_install::cli_binary_path(&state.dirs, php);
    if !php_cli.is_file() {
        return Response::Error {
            code: ErrorCode::Internal,
            message: format!("PHP {php} is not installed"),
        };
    }

    match run_user_list(&php_cli, &boot_fs, &served_root, &state.dirs).await {
        Ok(users) => Response::WordpressAdminUsers { users },
        Err(e) => Response::Error {
            code: ErrorCode::Internal,
            message: format!("couldn't list WordPress admin users: {e}"),
        },
    }
}

/// Pure - splits `boot_fs` into its own directory and bare file name, and
/// builds the `wp user list` argument vector. `None` if `boot_fs` has no
/// parent/file name (never true for a real path). Same bare-filename/cwd
/// invocation as `wordpress_url_sync::option_update_invocation` - see its doc
/// for the macOS space-in-path bug this works around.
fn user_list_invocation(
    boot_fs: &Path,
    served_root: &Path,
) -> Option<(PathBuf, PathBuf, Vec<String>)> {
    let boot_dir = boot_fs.parent()?.to_path_buf();
    let boot_name = PathBuf::from(boot_fs.file_name()?);
    let args = vec![
        "user".to_owned(),
        "list".to_owned(),
        "--role=administrator".to_owned(),
        "--format=json".to_owned(),
        "--fields=user_login,display_name".to_owned(),
        format!("--path={}", served_root.display()),
    ];
    Some((boot_dir, boot_name, args))
}

/// One row of `wp user list --format=json --fields=user_login,display_name`.
#[derive(serde::Deserialize)]
struct WpCliUser {
    user_login: String,
    display_name: String,
}

async fn run_user_list(
    php_cli: &Path,
    boot_fs: &Path,
    served_root: &Path,
    dirs: &yerd_platform::PlatformDirs,
) -> Result<Vec<WordPressAdminUser>, String> {
    let Some((boot_dir, boot_name, args)) = user_list_invocation(boot_fs, served_root) else {
        return Err(format!("{}: not a valid file path", boot_fs.display()));
    };
    let mut cmd = tokio::process::Command::new(php_cli);
    cmd.args(crate::tools::wp_cli::QUIET_DEPRECATIONS)
        .arg(&boot_name)
        .args(&args)
        .current_dir(&boot_dir)
        .env("NO_COLOR", "1")
        .stdin(Stdio::null());
    if let Ok(dir) = crate::tools::wp_cli::ensure_quiet_deprecations_scan_dir(dirs) {
        cmd.env(
            "PHP_INI_SCAN_DIR",
            crate::tools::wp_cli::quiet_deprecations_scan_dir_env(&dir),
        );
    }
    let output = cmd.output().await.map_err(|e| e.to_string())?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_owned());
    }
    parse_user_list(&output.stdout)
}

/// Pure - parses `wp user list --format=json`'s stdout into
/// [`WordPressAdminUser`]s.
fn parse_user_list(stdout: &[u8]) -> Result<Vec<WordPressAdminUser>, String> {
    let rows: Vec<WpCliUser> = serde_json::from_slice(stdout).map_err(|e| e.to_string())?;
    Ok(rows
        .into_iter()
        .map(|row| WordPressAdminUser {
            login: row.user_login,
            display_name: row.display_name,
        })
        .collect())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use std::sync::Arc;

    use yerd_core::{PhpVersion, RouterConfig, Site, SiteRouter, Tld};
    use yerd_platform::PlatformDirs;

    fn dirs_in(tmp: &Path) -> PlatformDirs {
        PlatformDirs {
            config: tmp.join("c"),
            data: tmp.join("d"),
            state: tmp.join("s"),
            cache: tmp.join("ca"),
            runtime: tmp.join("r"),
        }
    }

    /// A [`DaemonState`] with an empty router and no `WordPress` sites,
    /// suitable for exercising [`admin_users`]'s NotFound branches. Tests
    /// that need a routable site call [`insert_wordpress_site`] on the
    /// result.
    fn state_in(tmp: &Path) -> DaemonState {
        let dirs = dirs_in(tmp);
        let router = SiteRouter::new(RouterConfig::with_tld(Tld::new("test").unwrap()));
        let ca_path = dirs.data.join("ca.cert.pem");
        let php_manager = Arc::new(tokio::sync::Mutex::new(yerd_php::PhpManager::new(
            yerd_php::TokioProcessSpawner,
            yerd_php::SystemClock,
            yerd_php::io::FastCgiProbe,
            dirs.clone(),
            yerd_platform::ActivePortBinder::new(),
            std::process::id(),
            std::collections::BTreeMap::new(),
        )));
        DaemonState {
            config: tokio::sync::Mutex::new(yerd_config::Config::default()),
            router: Arc::new(tokio::sync::RwLock::new(router)),
            config_path: dirs.config.join("yerd.toml"),
            dirs,
            dns_addr: "127.0.0.1:1053".parse().unwrap(),
            ca_path,
            ca_fingerprint: yerd_platform::CaFingerprint::new([0u8; 32]),
            php_ca_bundle: None,
            php_updates: tokio::sync::RwLock::new(std::collections::HashMap::new()),
            yerd_update: tokio::sync::RwLock::new(Vec::new()),
            update_snapshot: tokio::sync::RwLock::new(None),
            php_manager,
            service_manager: Arc::new(tokio::sync::Mutex::new(crate::services::new_manager(
                dirs_in(tmp),
            ))),
            mail_store: Arc::new(yerd_mail::Store::open(tmp.join("mail")).unwrap()),
            mail: crate::state::MailRuntime { listening: false },
            http: yerd_ipc::PortStatus {
                requested: 80,
                bound: 8080,
                fell_back: true,
            },
            https: yerd_ipc::PortStatus {
                requested: 443,
                bound: 8443,
                fell_back: true,
            },
            redirect_https_port: Arc::new(std::sync::atomic::AtomicU16::new(8443)),
            web_unbound: None,
            dns_unbound: None,
            boot_id: 1,
            started_at: std::time::Instant::now(),
            shutdown_tx: tokio::sync::watch::channel(false).0,
            restart_requested: std::sync::atomic::AtomicBool::new(false),
            detect_cache: Arc::new(crate::detect_cache::DetectCache::new()),
            watch_dirty: tokio::sync::Notify::new(),
            dumps: Arc::new(crate::dump_server::DumpStore::new()),
            shim_reconcile: tokio::sync::Mutex::new(()),
            tunnel_manager: Arc::new(tokio::sync::Mutex::new(crate::tunnel::new_manager())),
            cloudflared_resolution: tokio::sync::RwLock::new(None),
            tool_mutate: tokio::sync::Mutex::new(()),
            tunnel_mutate: tokio::sync::Mutex::new(()),
            php_mutate: tokio::sync::Mutex::new(()),
            jobs: crate::jobs::JobRegistry::default(),
            reserved_names: tokio::sync::Mutex::new(std::collections::HashSet::new()),
            wordpress_versions: tokio::sync::RwLock::new(None),
            wordpress_login_tokens: Arc::new(crate::wordpress_login::LoginTokenRegistry::new()),
            wordpress_login_prepend_script: Some(PathBuf::from("/opt/yerd/prepend.php")),
            wordpress_sites: Arc::new(tokio::sync::RwLock::new(std::collections::HashMap::new())),
        }
    }

    /// Inserts `name` into `state`'s router and marks it `WordPress` in
    /// `state.wordpress_sites`, so [`admin_users`] can resolve it past both
    /// its site-lookup and `is_wordpress` gates.
    async fn insert_wordpress_site(state: &DaemonState, name: &str) {
        let site = Site::linked(name, "/srv/www/blog", PhpVersion::new(8, 3)).unwrap();
        state.router.write().await.insert(site).unwrap();
        state
            .wordpress_sites
            .write()
            .await
            .insert(name.to_owned(), true);
    }

    #[tokio::test]
    async fn admin_users_not_found_for_unknown_site() {
        let tmp = tempfile::tempdir().unwrap();
        let state = state_in(tmp.path());
        let resp = admin_users("blog", &state).await;
        assert!(matches!(
            resp,
            Response::Error {
                code: ErrorCode::NotFound,
                ..
            }
        ));
    }

    #[tokio::test]
    async fn admin_users_not_found_when_not_wordpress() {
        let tmp = tempfile::tempdir().unwrap();
        let state = state_in(tmp.path());
        // Routed, but never marked in `wordpress_sites`.
        let site = Site::linked("blog", "/srv/www/blog", PhpVersion::new(8, 3)).unwrap();
        state.router.write().await.insert(site).unwrap();

        let resp = admin_users("blog", &state).await;
        assert!(matches!(
            resp,
            Response::Error {
                code: ErrorCode::NotFound,
                ..
            }
        ));
    }

    #[tokio::test]
    async fn admin_users_internal_error_when_wp_cli_not_installed() {
        let tmp = tempfile::tempdir().unwrap();
        let state = state_in(tmp.path());
        insert_wordpress_site(&state, "blog").await;

        // `state_in`'s `dirs` are a bare tmp dir with nothing installed under
        // it, so `wp_cli::boot_path` never exists - this exercises the
        // handler's own "WP-CLI is not installed" branch rather than
        // actually spawning a WP-CLI/PHP process.
        let resp = admin_users("blog", &state).await;
        assert!(matches!(
            resp,
            Response::Error {
                code: ErrorCode::Internal,
                ..
            }
        ));
    }

    #[test]
    fn user_list_invocation_splits_boot_fs_and_builds_args() {
        let boot_fs = Path::new("/Users/x/Library/Application Support/io.yerd.Yerd/boot-fs.php");
        let served_root = Path::new("/Users/x/Yerd/blog");
        let (boot_dir, boot_name, args) = user_list_invocation(boot_fs, served_root).unwrap();
        assert_eq!(
            boot_dir,
            Path::new("/Users/x/Library/Application Support/io.yerd.Yerd")
        );
        assert_eq!(boot_name, Path::new("boot-fs.php"));
        assert_eq!(
            args,
            vec![
                "user",
                "list",
                "--role=administrator",
                "--format=json",
                "--fields=user_login,display_name",
                "--path=/Users/x/Yerd/blog",
            ]
        );
    }

    #[test]
    fn user_list_invocation_none_for_rootless_boot_fs() {
        assert!(user_list_invocation(Path::new("/"), Path::new("/x")).is_none());
    }

    #[test]
    fn parse_user_list_maps_fields() {
        let stdout = br#"[{"user_login":"admin","display_name":"Admin"},{"user_login":"editor","display_name":"Editor Person"}]"#;
        let users = parse_user_list(stdout).unwrap();
        assert_eq!(users.len(), 2);
        assert_eq!(users[0].login, "admin");
        assert_eq!(users[0].display_name, "Admin");
        assert_eq!(users[1].login, "editor");
        assert_eq!(users[1].display_name, "Editor Person");
    }

    #[test]
    fn parse_user_list_empty_array() {
        assert!(parse_user_list(b"[]").unwrap().is_empty());
    }

    #[test]
    fn parse_user_list_rejects_malformed_json() {
        assert!(parse_user_list(b"not json").is_err());
    }
}
