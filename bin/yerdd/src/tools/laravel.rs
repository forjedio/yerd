//! Laravel installer — `composer create-project laravel/installer` into
//! `{data}/tools/laravel/`.
//!
//! Unlike the download-and-unpack tools (Composer/Node/Bun), the Laravel
//! installer is a Composer package: we run the managed Composer to build it into
//! a staging dir, then atomically swap it into place. The `laravel` command is a
//! multi-call shim into the `yerd` binary (like `composer`), which execs
//! `php …/installer/bin/laravel`.

use std::path::{Path, PathBuf};
use std::process::Stdio;

use tokio::io::{AsyncBufReadExt, BufReader};
use yerd_platform::PlatformDirs;

use super::{stage_and_swap, tool_dir, ProgressTx, Tool, ToolError};
use crate::ext_install::installed_versions;

/// The Composer package providing `laravel new`.
const PACKAGE: &str = "laravel/installer";

/// `{data}/tools/laravel/bin/laravel` — the installer binary the `laravel` shim
/// and the site-creation handler exec. `composer create-project` installs the
/// package *as the root project*, so its `bin/` is at the tool-dir root (not
/// under `vendor/`).
#[must_use]
pub fn installer_bin(dirs: &PlatformDirs) -> PathBuf {
    tool_dir(dirs, Tool::Laravel).join("bin").join("laravel")
}

/// Writable `COMPOSER_HOME` for every yerd-driven Composer run. The daemon sets
/// no `HOME` of its own, so Composer must be told where to keep its cache/config
/// or it falls back to `$HOME/.composer`, which can be unset/unwritable.
#[must_use]
pub fn composer_home(dirs: &PlatformDirs) -> PathBuf {
    dirs.cache.join("composer")
}

/// Build + install the latest Laravel installer via the managed Composer,
/// streaming Composer's output to `progress` when attached.
pub async fn install(dirs: &PlatformDirs, progress: Option<&ProgressTx>) -> Result<(), ToolError> {
    let Some(php_version) = installed_versions(dirs)
        .into_iter()
        .max_by_key(|v| (v.major, v.minor))
    else {
        return Err(ToolError::UnsupportedHost(
            "Laravel installer (requires an installed PHP)",
        ));
    };
    let php = crate::php_install::cli_binary_path(dirs, php_version);
    let phar = super::composer::phar_path(dirs);
    if !phar.is_file() {
        return Err(ToolError::UnsupportedHost(
            "Laravel installer (install Composer first)",
        ));
    }

    let home = composer_home(dirs);
    std::fs::create_dir_all(&home)
        .map_err(|e| ToolError::Io(format!("{}: {e}", home.display())))?;

    let tools_root = dirs.data.join("tools");
    std::fs::create_dir_all(&tools_root)
        .map_err(|e| ToolError::Io(format!("{}: {e}", tools_root.display())))?;
    let build = tools_root.join(format!(".laravel-build-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&build);

    // `php composer.phar create-project --prefer-dist --no-interaction
    //  laravel/installer <build>`. stdin is null so a stray prompt can't hang us;
    // stdout/stderr are piped and streamed line-by-line to `progress`.
    let mut child = tokio::process::Command::new(&php)
        .arg(&phar)
        .arg("create-project")
        .arg("--prefer-dist")
        .arg("--no-interaction")
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

    // Drain both pipes concurrently with the wait (draining one while the other
    // fills would deadlock the child).
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let ((), (), status) = tokio::join!(
        drain(stdout, progress.cloned()),
        drain(stderr, progress.cloned()),
        child.wait(),
    );
    let status = status.map_err(|e| ToolError::Io(format!("await composer: {e}")))?;
    if !status.success() {
        let _ = std::fs::remove_dir_all(&build);
        return Err(ToolError::Download(format!(
            "composer create-project {PACKAGE} failed (exit {status})"
        )));
    }

    let version = read_installer_version(&build).unwrap_or_else(|| "installed".to_owned());
    // Move the freshly-built tree into the tool's staging dir (same filesystem,
    // so the renames are atomic), then let `stage_and_swap` mark + swap it in.
    let swapped = stage_and_swap(dirs, Tool::Laravel, &version, |staging| {
        move_dir_contents(&build, staging)
    });
    let _ = std::fs::remove_dir_all(&build);
    swapped?;
    tracing::info!(version = %version, "installed Laravel installer");
    Ok(())
}

/// Stream a child pipe line-by-line into `progress` (no-op if either is absent).
async fn drain<R: tokio::io::AsyncRead + Unpin>(pipe: Option<R>, progress: Option<ProgressTx>) {
    let Some(pipe) = pipe else { return };
    let mut lines = BufReader::new(pipe).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        if let Some(tx) = &progress {
            let _ = tx.send(line);
        }
    }
}

/// Move every entry of `from` into `to`. Both live under `{data}/tools`, so the
/// renames stay on one filesystem and are atomic + instant.
fn move_dir_contents(from: &Path, to: &Path) -> Result<(), ToolError> {
    let entries =
        std::fs::read_dir(from).map_err(|e| ToolError::Io(format!("{}: {e}", from.display())))?;
    for entry in entries {
        let entry = entry.map_err(|e| ToolError::Io(e.to_string()))?;
        let dest = to.join(entry.file_name());
        std::fs::rename(entry.path(), &dest)
            .map_err(|e| ToolError::Io(format!("{}: {e}", dest.display())))?;
    }
    Ok(())
}

/// Pull `laravel/installer`'s resolved version out of the built `composer.lock`.
fn read_installer_version(build: &Path) -> Option<String> {
    let text = std::fs::read_to_string(build.join("composer.lock")).ok()?;
    let lock: serde_json::Value = serde_json::from_str(&text).ok()?;
    for pkg in lock.get("packages")?.as_array()? {
        if pkg.get("name").and_then(serde_json::Value::as_str) == Some(PACKAGE) {
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
    fn read_installer_version_parses_lock() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(
            tmp.path().join("composer.lock"),
            r#"{"packages":[{"name":"laravel/installer","version":"v5.6.1"}]}"#,
        )
        .unwrap();
        assert_eq!(
            read_installer_version(tmp.path()).as_deref(),
            Some("v5.6.1")
        );
    }

    #[test]
    fn read_installer_version_absent_is_none() {
        let tmp = tempfile::tempdir().unwrap();
        assert_eq!(read_installer_version(tmp.path()), None);
    }

    #[test]
    fn move_dir_contents_moves_all_entries() {
        let tmp = tempfile::tempdir().unwrap();
        let from = tmp.path().join("from");
        let to = tmp.path().join("to");
        std::fs::create_dir_all(from.join("vendor")).unwrap();
        std::fs::write(from.join("composer.json"), b"{}").unwrap();
        std::fs::create_dir_all(&to).unwrap();
        move_dir_contents(&from, &to).unwrap();
        assert!(to.join("vendor").is_dir());
        assert!(to.join("composer.json").is_file());
    }
}
