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

    match run_user_list(&php_cli, &boot_fs, &served_root).await {
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
) -> Result<Vec<WordPressAdminUser>, String> {
    let Some((boot_dir, boot_name, args)) = user_list_invocation(boot_fs, served_root) else {
        return Err(format!("{}: not a valid file path", boot_fs.display()));
    };
    let output = tokio::process::Command::new(php_cli)
        .args(crate::tools::wp_cli::QUIET_DEPRECATIONS)
        .arg(&boot_name)
        .args(&args)
        .current_dir(&boot_dir)
        .env("NO_COLOR", "1")
        .stdin(Stdio::null())
        .output()
        .await
        .map_err(|e| e.to_string())?;
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
