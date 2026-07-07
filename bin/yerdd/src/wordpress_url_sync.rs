//! Keeps a `WordPress` site's own `siteurl`/`home` options in sync with the
//! scheme yerd actually serves it on. Run after a `Request::SetSecure`
//! mutation succeeds (see `ipc_server::handle_mutation`'s call site) - a site
//! created through yerd's own wizard already gets the right scheme at
//! creation time (`create_site::wordpress::install_args`'s `--url=`), but
//! nothing previously kept `siteurl`/`home` in step when the secure flag was
//! toggled *after* creation, which left WordPress's own idea of its scheme
//! stale and defeated the [`crate::wordpress_login`] host/scheme guard.
//!
//! Best-effort and silent on failure: WP-CLI or PHP missing, or the `wp
//! option update` call itself failing, only logs a warning - it never fails
//! the secure toggle it's attached to.

use std::path::{Path, PathBuf};
use std::process::Stdio;

use yerd_core::Site;

use crate::state::DaemonState;

/// `Request::SetSecure` handler's post-mutation hook: if `site` is a
/// `WordPress` install, update its `siteurl`/`home` options to match
/// `site.secure()`'s new value.
pub async fn sync_site_url(site: &Site, state: &DaemonState) {
    let served_root = site.served_root();
    let is_wordpress = state
        .wordpress_sites
        .read()
        .await
        .get(site.name())
        .copied()
        .unwrap_or(false);
    if !is_wordpress {
        return;
    }

    let boot_fs = crate::tools::wp_cli::boot_path(&state.dirs);
    if !boot_fs.is_file() {
        return;
    }
    let php_cli = crate::php_install::cli_binary_path(&state.dirs, site.php());
    if !php_cli.is_file() {
        return;
    }

    let scheme = if site.secure() { "https" } else { "http" };
    let url = format!("{scheme}://{}.test", site.name());

    for option in ["siteurl", "home"] {
        if let Err(e) = run_option_update(&php_cli, &boot_fs, &served_root, option, &url).await {
            tracing::warn!(
                site = %site.name(),
                option,
                error = %e,
                "couldn't sync WordPress site URL after toggling secure"
            );
            return;
        }
    }
}

/// Pure - splits `boot_fs` into its own directory and bare file name, and
/// builds the `wp option update <option> <url> --path=<served_root>`
/// argument vector. `None` if `boot_fs` has no parent/file name (never true
/// for a real path). Invoked with `boot_fs`'s bare file name from its own
/// directory, not `served_root`, for the same reason
/// `create_site::wordpress::wp_step_invocation` does: WP-CLI's
/// `WP_CLI::launch_self()` re-invocation bug on macOS, triggered by a space
/// in the captured `argv[0]` path.
fn option_update_invocation(
    boot_fs: &Path,
    served_root: &Path,
    option: &str,
    url: &str,
) -> Option<(PathBuf, PathBuf, Vec<String>)> {
    let boot_dir = boot_fs.parent()?.to_path_buf();
    let boot_name = PathBuf::from(boot_fs.file_name()?);
    let args = vec![
        "option".to_owned(),
        "update".to_owned(),
        option.to_owned(),
        url.to_owned(),
        format!("--path={}", served_root.display()),
    ];
    Some((boot_dir, boot_name, args))
}

async fn run_option_update(
    php_cli: &Path,
    boot_fs: &Path,
    served_root: &Path,
    option: &str,
    url: &str,
) -> Result<(), String> {
    let Some((boot_dir, boot_name, args)) =
        option_update_invocation(boot_fs, served_root, option, url)
    else {
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
    if output.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&output.stderr).trim().to_owned())
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn option_update_invocation_splits_boot_fs_and_builds_args() {
        let boot_fs = Path::new("/Users/x/Library/Application Support/io.yerd.Yerd/boot-fs.php");
        let served_root = Path::new("/Users/x/Yerd/blog");
        let (boot_dir, boot_name, args) =
            option_update_invocation(boot_fs, served_root, "siteurl", "https://blog.test").unwrap();
        assert_eq!(
            boot_dir,
            Path::new("/Users/x/Library/Application Support/io.yerd.Yerd")
        );
        assert_eq!(boot_name, Path::new("boot-fs.php"));
        assert_eq!(
            args,
            vec![
                "option",
                "update",
                "siteurl",
                "https://blog.test",
                "--path=/Users/x/Yerd/blog",
            ]
        );
    }

    #[test]
    fn option_update_invocation_none_for_rootless_boot_fs() {
        assert!(
            option_update_invocation(Path::new("/"), Path::new("/x"), "home", "http://x.test")
                .is_none()
        );
    }
}
