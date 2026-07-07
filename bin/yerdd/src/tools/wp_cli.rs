//! WP-CLI - `composer create-project wp-cli/wp-cli-bundle` into
//! `{data}/tools/wp-cli/`.
//!
//! Like the Laravel installer, WP-CLI is installed as a Composer package (not
//! the download-and-verify phar the upstream project also publishes, since its
//! GitHub release assets carry no checksum digest to verify against - only a
//! GPG signature, which this codebase has no precedent for checking). We run
//! the managed Composer to build it into a staging dir, then atomically swap it
//! into place. The `wp` command is a multi-call shim into the `yerd` binary
//! (like `composer`/`laravel`), which execs `php …/wp-cli/php/boot-fs.php`
//! directly - bypassing upstream's `bin/wp` shell wrapper (which only exists to
//! locate a `php` on `PATH`; we already know which PHP to use).

use std::path::{Path, PathBuf};
use std::process::Stdio;

use yerd_platform::PlatformDirs;

use super::{drain, move_dir_contents, stage_and_swap, tool_dir, ProgressTx, Tool, ToolError};
use crate::ext_install::installed_versions;

/// The Composer package providing the `wp` command (a root project depending
/// on `wp-cli/wp-cli`, not a package named `wp-cli/wp-cli` itself).
const PACKAGE: &str = "wp-cli/wp-cli-bundle";

/// `{data}/tools/wp-cli/vendor/wp-cli/wp-cli/php/boot-fs.php` - the filesystem
/// entry point the `wp` shim execs under the managed PHP. `wp-cli-bundle`
/// requires `wp-cli/wp-cli` as a regular dependency, so it lands under
/// `vendor/`, unlike the Laravel installer (which *is* the create-project
/// root). Kept in sync with `bin/yerd/src/wp_shim.rs`.
#[must_use]
pub fn boot_path(dirs: &PlatformDirs) -> PathBuf {
    tool_dir(dirs, Tool::WpCli)
        .join("vendor")
        .join("wp-cli")
        .join("wp-cli")
        .join("php")
        .join("boot-fs.php")
}

/// Build + install the latest WP-CLI via the managed Composer, streaming
/// Composer's output to `progress` when attached.
pub async fn install(dirs: &PlatformDirs, progress: Option<&ProgressTx>) -> Result<(), ToolError> {
    let Some(php_version) = installed_versions(dirs)
        .into_iter()
        .max_by_key(|v| (v.major, v.minor))
    else {
        return Err(ToolError::UnsupportedHost(
            "WP-CLI (requires an installed PHP)",
        ));
    };
    let php = crate::php_install::cli_binary_path(dirs, php_version);
    let phar = super::composer::phar_path(dirs);
    if !phar.is_file() {
        return Err(ToolError::UnsupportedHost(
            "WP-CLI (install Composer first)",
        ));
    }

    let home = super::laravel::composer_home(dirs);
    std::fs::create_dir_all(&home)
        .map_err(|e| ToolError::Io(format!("{}: {e}", home.display())))?;

    let tools_root = dirs.data.join("tools");
    std::fs::create_dir_all(&tools_root)
        .map_err(|e| ToolError::Io(format!("{}: {e}", tools_root.display())))?;
    let build = tools_root.join(format!(".wp-cli-build-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&build);

    let mut child = tokio::process::Command::new(&php)
        .arg(&phar)
        .arg("create-project")
        .arg("--prefer-dist")
        .arg("--no-interaction")
        .arg("--no-dev")
        .arg(PACKAGE)
        .arg(&build)
        .env("COMPOSER_HOME", &home)
        .env("COMPOSER_NO_INTERACTION", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| ToolError::Io(format!("spawn composer: {e}")))?;

    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let joined = tokio::time::timeout(std::time::Duration::from_secs(600), async {
        tokio::join!(
            drain(stdout, progress.cloned()),
            drain(stderr, progress.cloned()),
            child.wait(),
        )
    })
    .await;
    let Ok(((), (), status)) = joined else {
        let _ = std::fs::remove_dir_all(&build);
        return Err(ToolError::Download(format!(
            "composer create-project {PACKAGE} timed out"
        )));
    };
    let status = status.map_err(|e| ToolError::Io(format!("await composer: {e}")))?;
    if !status.success() {
        let _ = std::fs::remove_dir_all(&build);
        return Err(ToolError::Download(format!(
            "composer create-project {PACKAGE} failed (exit {status})"
        )));
    }

    let version = read_wp_cli_version(&build).unwrap_or_else(|| "installed".to_owned());
    let swapped = stage_and_swap(dirs, Tool::WpCli, &version, |staging| {
        move_dir_contents(&build, staging)
    });
    let _ = std::fs::remove_dir_all(&build);
    swapped?;
    tracing::info!(version = %version, "installed WP-CLI");
    Ok(())
}

/// Pull the `wp-cli/wp-cli` dependency's resolved version out of the built
/// `composer.lock`. Unlike the Laravel installer (which *is* the create-project
/// root and so has no self-entry in `packages`), `wp-cli-bundle` requires
/// `wp-cli/wp-cli` as a genuine non-dev dependency, so it's always present.
fn read_wp_cli_version(build: &Path) -> Option<String> {
    let text = std::fs::read_to_string(build.join("composer.lock")).ok()?;
    let lock: serde_json::Value = serde_json::from_str(&text).ok()?;
    for pkg in lock.get("packages")?.as_array()? {
        if pkg.get("name").and_then(serde_json::Value::as_str) == Some("wp-cli/wp-cli") {
            return pkg
                .get("version")
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned);
        }
    }
    None
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn read_wp_cli_version_parses_lock() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(
            tmp.path().join("composer.lock"),
            r#"{"packages":[
                {"name":"wp-cli/wp-cli-bundle","version":"dev-main"},
                {"name":"wp-cli/wp-cli","version":"v2.12.0"}
            ]}"#,
        )
        .unwrap();
        assert_eq!(read_wp_cli_version(tmp.path()).as_deref(), Some("v2.12.0"));
    }

    #[test]
    fn read_wp_cli_version_absent_is_none() {
        let tmp = tempfile::tempdir().unwrap();
        assert_eq!(read_wp_cli_version(tmp.path()), None);
    }
}
