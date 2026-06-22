//! Composer installer — fetch + verify `composer.phar` into `{data}/tools/composer/`.
//!
//! Composer is a PHP phar run via yerd's managed PHP (the `composer` multi-call
//! shim execs `php composer.phar …`). Integrity uses Composer's published
//! `composer.phar.sha256sum` sidecar (the `/versions` JSON carries no digest).
//! Installed on demand from the Tooling page — not auto-fetched.

use std::path::{Path, PathBuf};

use serde::Deserialize;

use yerd_core::PhpVersion;
use yerd_php::Downloader;
use yerd_platform::PlatformDirs;

use super::{stage_and_swap, verify_sha256, Tool, ToolError};
use crate::ext_install::installed_versions;

/// Composer's machine-readable version index.
const VERSIONS_URL: &str = "https://getcomposer.org/versions";
/// Per-version download root: `<base>/<version>/composer.phar[.sha256sum]`.
const DOWNLOAD_BASE: &str = "https://getcomposer.org/download";

#[derive(Debug, Deserialize)]
struct VersionEntry {
    version: String,
    /// `PHP_VERSION_ID` integer (e.g. `70205` = 7.2.5). Absent on some entries.
    #[serde(rename = "min-php")]
    min_php: Option<u32>,
}

#[derive(Debug, Deserialize)]
struct Versions {
    stable: Vec<VersionEntry>,
}

/// `{data}/tools/composer/composer.phar` — the path the `composer` shim execs.
/// Kept in sync with `bin/yerd/src/composer_shim.rs`.
#[must_use]
pub fn phar_path(dirs: &PlatformDirs) -> PathBuf {
    super::tool_dir(dirs, Tool::Composer).join("composer.phar")
}

/// `PHP_VERSION_ID` for `v` with patch 0 (`major*10000 + minor*100`).
fn version_id(v: PhpVersion) -> u32 {
    u32::from(v.major) * 10_000 + u32::from(v.minor) * 100
}

/// Digits-and-dots only, so a hostile `/versions` payload can't inject `../`.
fn valid_version(s: &str) -> bool {
    !s.is_empty()
        && s.bytes().all(|b| b.is_ascii_digit() || b == b'.')
        && !s.contains("..")
        && s.bytes().next().is_some_and(|b| b.is_ascii_digit())
}

/// Newest `stable` entry the host PHP can run (`min-php`), else newest overall.
fn choose_version(versions: &Versions, host_id: Option<u32>) -> Option<String> {
    let fits = |e: &VersionEntry| match (host_id, e.min_php) {
        (Some(h), Some(min)) => h >= min,
        _ => true,
    };
    versions
        .stable
        .iter()
        .find(|e| fits(e) && valid_version(&e.version))
        .map(|e| e.version.clone())
}

/// Download, verify, and install the latest Composer the installed PHP can run.
pub async fn install(dirs: &PlatformDirs, dl: &dyn Downloader) -> Result<(), ToolError> {
    let versions = fetch_versions(dl).await?;
    // Composer runs through yerd's managed PHP shim, so an install with no PHP
    // present would produce a non-runnable command — require one up front.
    let Some(host_id) = installed_versions(dirs).into_iter().map(version_id).max() else {
        return Err(ToolError::UnsupportedHost(
            "Composer (requires an installed PHP)",
        ));
    };
    let version =
        choose_version(&versions, Some(host_id)).ok_or(ToolError::UnsupportedHost("Composer"))?;

    let phar_url = format!("{DOWNLOAD_BASE}/{version}/composer.phar");
    let sha_url = format!("{phar_url}.sha256sum");

    let want_sha = fetch_sha256sum(dl, &sha_url).await?;
    let bytes = dl
        .download(&phar_url)
        .await
        .map_err(|e| ToolError::Download(format!("composer.phar: {e}")))?;
    verify_sha256(&bytes, &want_sha, "composer.phar")?;

    stage_and_swap(dirs, Tool::Composer, &version, |staging| {
        let dest = staging.join("composer.phar");
        std::fs::write(&dest, &bytes).map_err(|e| ToolError::Io(format!("{}: {e}", dest.display())))
    })?;
    tracing::info!(version = %version, "installed Composer");
    Ok(())
}

async fn fetch_versions(dl: &dyn Downloader) -> Result<Versions, ToolError> {
    let bytes = dl
        .download(VERSIONS_URL)
        .await
        .map_err(|e| ToolError::Download(format!("composer /versions: {e}")))?;
    serde_json::from_slice::<Versions>(&bytes)
        .map_err(|e| ToolError::Download(format!("composer /versions parse: {e}")))
}

/// Parse a `composer.phar.sha256sum` sidecar (a single `<hex>  composer.phar` line).
async fn fetch_sha256sum(dl: &dyn Downloader, url: &str) -> Result<String, ToolError> {
    let bytes = dl
        .download(url)
        .await
        .map_err(|e| ToolError::Download(format!("composer sha256sum: {e}")))?;
    let text = String::from_utf8_lossy(&bytes);
    let token = text.split_whitespace().next().unwrap_or("");
    if token.len() == 64 && token.bytes().all(|b| b.is_ascii_hexdigit()) {
        Ok(token.to_ascii_lowercase())
    } else {
        Err(ToolError::Download(
            "composer sha256sum: bad format".to_owned(),
        ))
    }
}

/// Unused locally (the `composer` shim target is the `yerd` binary, resolved in
/// `tools::shim_links`), but kept for symmetry with the other tools' modules.
#[cfg(unix)]
#[allow(dead_code)]
pub(crate) fn shim_source(_dirs: &PlatformDirs) -> Option<&'static Path> {
    None
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;

    fn versions_json() -> &'static str {
        r#"{"stable":[
            {"path":"/download/2.10.1/composer.phar","version":"2.10.1","min-php":70205},
            {"path":"/download/2.2.22/composer.phar","version":"2.2.22","min-php":50302}
        ]}"#
    }

    #[test]
    fn version_id_encodes_php() {
        assert_eq!(version_id(PhpVersion::new(8, 4)), 80_400);
    }

    #[test]
    fn valid_version_rejects_injection() {
        assert!(valid_version("2.10.1"));
        assert!(!valid_version("../evil"));
        assert!(!valid_version("v2.0"));
    }

    #[test]
    fn choose_version_honours_min_php() {
        let v: Versions = serde_json::from_str(versions_json()).unwrap();
        assert_eq!(choose_version(&v, Some(80_400)).as_deref(), Some("2.10.1"));
        assert_eq!(choose_version(&v, Some(70_000)).as_deref(), Some("2.2.22"));
        assert_eq!(choose_version(&v, None).as_deref(), Some("2.10.1"));
    }
}
