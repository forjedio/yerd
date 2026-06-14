//! Downloads the `yerd-dump` PHP extension (`.so`) per installed PHP version.
//!
//! Artifacts come from the `yerd-php-ext` GitHub releases, described by a
//! `manifest.json` listing each file's `php`/`os`/`arch`/`sha256`. yerd resolves
//! the file matching the host triple for each installed PHP minor, verifies the
//! SHA-256, and places it at `{data}/php-ext/php-<ver>/yerd-dump.so` (a sibling
//! of the PHP installs, so a PHP patch update — which wipes `{data}/php/php-<ver>`
//! — never removes the extension).
//!
//! Everything here is best-effort: a download/verify failure logs and leaves the
//! site running with no dumps.

use std::path::{Path, PathBuf};

use serde::Deserialize;
use sha2::{Digest, Sha256};

use yerd_core::PhpVersion;
use yerd_php::Downloader;
use yerd_platform::PlatformDirs;

/// Where per-version extension artifacts are downloaded to fetch the manifest
/// and the `.so` files. The `latest` channel auto-picks up new releases (each
/// asset is still SHA-256-verified against the manifest).
const RELEASE_BASE: &str = "https://github.com/forjedio/yerd-php-ext/releases/latest/download";

/// One entry in the release `manifest.json`.
#[derive(Debug, Deserialize)]
struct ManifestFile {
    name: String,
    php: String,
    os: String,
    arch: String,
    sha256: String,
}

#[derive(Debug, Deserialize)]
struct Manifest {
    files: Vec<ManifestFile>,
}

/// The host OS/arch as the manifest names them (`macos`/`linux`,
/// `aarch64`/`x86_64`) — `std::env::consts` already uses these spellings.
fn host_os_arch() -> (&'static str, &'static str) {
    (std::env::consts::OS, std::env::consts::ARCH)
}

/// Absolute path of the extension `.so` for `v` (present or not).
#[must_use]
pub fn so_path(dirs: &PlatformDirs, v: PhpVersion) -> PathBuf {
    dirs.data
        .join("php-ext")
        .join(format!("php-{v}"))
        .join("yerd-dump.so")
}

/// Installed PHP minors discovered from `{data}/php`.
#[must_use]
pub fn installed_versions(dirs: &PlatformDirs) -> Vec<PhpVersion> {
    yerd_php::discover_bundled(dirs)
        .map(|v| v.into_iter().map(|(ver, _)| ver).collect())
        .unwrap_or_default()
}

/// Ensure the extension is present and current for every installed PHP version
/// that has a published artifact for the host triple. Best-effort.
pub async fn ensure_for_installed(dirs: &PlatformDirs, dl: &dyn Downloader) {
    let Some(manifest) = fetch_manifest(dl).await else {
        return;
    };
    let (os, arch) = host_os_arch();
    for v in installed_versions(dirs) {
        let minor = v.to_string();
        let Some(file) = manifest
            .files
            .iter()
            .find(|f| f.php == minor && f.os == os && f.arch == arch)
        else {
            tracing::info!(php = %minor, os, arch, "no yerd-dump extension published for this triple");
            continue;
        };
        let dest = so_path(dirs, v);
        if existing_matches(&dest, &file.sha256) {
            continue; // already current
        }
        match download_and_place(dl, &file.name, &file.sha256, &dest).await {
            Ok(()) => tracing::info!(php = %minor, "installed yerd-dump extension"),
            Err(e) => {
                tracing::warn!(php = %minor, error = %e, "failed to install yerd-dump extension");
            }
        }
    }
}

async fn fetch_manifest(dl: &dyn Downloader) -> Option<Manifest> {
    let url = format!("{RELEASE_BASE}/manifest.json");
    match dl.download(&url).await {
        Ok(bytes) => match serde_json::from_slice::<Manifest>(&bytes) {
            Ok(m) => Some(m),
            Err(e) => {
                tracing::warn!(error = %e, "yerd-dump manifest parse failed");
                None
            }
        },
        Err(e) => {
            tracing::warn!(error = %e, "yerd-dump manifest download failed");
            None
        }
    }
}

/// True if `path` already holds bytes whose SHA-256 hex equals `want`.
fn existing_matches(path: &Path, want: &str) -> bool {
    match std::fs::read(path) {
        Ok(bytes) => sha256_hex(&bytes).eq_ignore_ascii_case(want),
        Err(_) => false,
    }
}

async fn download_and_place(
    dl: &dyn Downloader,
    name: &str,
    want_sha: &str,
    dest: &Path,
) -> std::io::Result<()> {
    let url = format!("{RELEASE_BASE}/{name}");
    let bytes = dl
        .download(&url)
        .await
        .map_err(|e| std::io::Error::other(format!("download {name}: {e}")))?;
    let got = sha256_hex(&bytes);
    if !got.eq_ignore_ascii_case(want_sha) {
        return Err(std::io::Error::other(format!(
            "sha256 mismatch for {name}: got {got}, want {want_sha}"
        )));
    }
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)?;
    }
    // Atomic place: write a unique temp sibling (pid + sequence, so overlapping
    // installs of the same version don't share a temp path) then rename over.
    let seq = TMP_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let tmp = dest.with_extension(format!("so.{}.{}.tmp", std::process::id(), seq));
    std::fs::write(&tmp, &bytes)?;
    std::fs::rename(&tmp, dest)?;
    Ok(())
}

/// Monotonic counter for unique temp filenames (combined with the pid).
static TMP_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hex::encode(hasher.finalize())
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod tests {
    use super::*;

    fn dirs_in(tmp: &Path) -> PlatformDirs {
        PlatformDirs {
            config: tmp.join("c"),
            data: tmp.join("d"),
            state: tmp.join("s"),
            cache: tmp.join("ca"),
            runtime: tmp.join("r"),
        }
    }

    #[test]
    fn so_path_is_sibling_of_php_installs() {
        let tmp = tempfile::tempdir().unwrap();
        let dirs = dirs_in(tmp.path());
        let p = so_path(&dirs, PhpVersion::new(8, 5));
        assert!(p.ends_with("php-ext/php-8.5/yerd-dump.so"));
        // Crucially NOT under {data}/php/php-8.5 (which is wiped on PHP update).
        assert!(!p.starts_with(dirs.data.join("php").join("php-8.5")));
    }

    #[test]
    fn sha256_hex_matches_known_vector() {
        // SHA-256 of the empty input.
        assert_eq!(
            sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[test]
    fn existing_matches_detects_correct_and_wrong_hash() {
        let tmp = tempfile::tempdir().unwrap();
        let f = tmp.path().join("x.so");
        std::fs::write(&f, b"hello").unwrap();
        let good = sha256_hex(b"hello");
        assert!(existing_matches(&f, &good));
        assert!(!existing_matches(&f, "deadbeef"));
        assert!(!existing_matches(&tmp.path().join("missing.so"), &good));
    }
}
