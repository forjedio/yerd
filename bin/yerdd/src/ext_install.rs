//! Downloads PHP extension `.so`s (`yerd-dump`, `pcov`) per installed PHP version.
//!
//! Both come from the same `yerd-php-ext` GitHub release, each described by its
//! own manifest (`manifest.json` for dump, `pcov-manifest.json` for pcov) listing
//! each file's `php`/`os`/`arch`/`sha256`. yerd resolves
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

/// One downloadable extension: which manifest names it in the (shared) release
/// and what its `.so` is called on disk. Lets the same fetch loop serve both
/// `yerd-dump` and `pcov` from the one `yerd-php-ext` release.
struct ExtSpec {
    /// Manifest filename within the release (e.g. `manifest.json`).
    manifest_name: &'static str,
    /// On-disk `.so` filename under `{data}/php-ext/php-<ver>/`.
    so_name: &'static str,
    /// Human label for log lines.
    label: &'static str,
}

const DUMP_SPEC: ExtSpec = ExtSpec {
    manifest_name: "manifest.json",
    so_name: "yerd-dump.so",
    label: "yerd-dump",
};

const PCOV_SPEC: ExtSpec = ExtSpec {
    manifest_name: "pcov-manifest.json",
    so_name: "pcov.so",
    label: "pcov",
};

/// The host OS/arch as the manifest names them (`macos`/`linux`,
/// `aarch64`/`x86_64`) — `std::env::consts` already uses these spellings.
fn host_os_arch() -> (&'static str, &'static str) {
    (std::env::consts::OS, std::env::consts::ARCH)
}

/// Absolute path of a named extension `.so` for `v` (present or not).
fn so_path_named(dirs: &PlatformDirs, v: PhpVersion, so_name: &str) -> PathBuf {
    dirs.data
        .join("php-ext")
        .join(format!("php-{v}"))
        .join(so_name)
}

/// Absolute path of the `yerd-dump` `.so` for `v` (present or not).
#[must_use]
pub fn so_path(dirs: &PlatformDirs, v: PhpVersion) -> PathBuf {
    so_path_named(dirs, v, DUMP_SPEC.so_name)
}

/// Absolute path of the `pcov` `.so` for `v` (present or not). Sibling of the
/// dump `.so`, so a PHP patch update (which wipes `{data}/php/php-<ver>`) leaves
/// it intact.
#[must_use]
pub fn pcov_so_path(dirs: &PlatformDirs, v: PhpVersion) -> PathBuf {
    so_path_named(dirs, v, PCOV_SPEC.so_name)
}

/// Installed PHP minors discovered from `{data}/php`.
#[must_use]
pub fn installed_versions(dirs: &PlatformDirs) -> Vec<PhpVersion> {
    yerd_php::discover_bundled(dirs)
        .map(|v| v.into_iter().map(|(ver, _)| ver).collect())
        .unwrap_or_default()
}

/// Ensure the `yerd-dump` extension is present and current for every installed
/// PHP version that has a published artifact for the host triple. Best-effort.
pub async fn ensure_for_installed(dirs: &PlatformDirs, dl: &dyn Downloader) {
    ensure_for_installed_spec(dirs, dl, &DUMP_SPEC).await;
}

/// Ensure the `pcov` extension is present for every installed PHP version.
///
/// Used by the CLI cover shims (`phpcover`/`php<ver>cover`); ungated, unlike the
/// dump fetch. Warm/offline starts skip the network entirely: if every installed
/// version already has a `pcov.so`, return without touching GitHub. (The "present"
/// check is a proxy for "current" — a stale `.so` won't refresh on a pure restart,
/// which is fine: pcov is ABI-stable per PHP minor and any *missing* `.so` still
/// forces a full manifest fetch + re-verify.)
pub async fn ensure_pcov_for_installed(dirs: &PlatformDirs, dl: &dyn Downloader) {
    let versions = installed_versions(dirs);
    // Nothing to fetch when no PHP is installed, or when every version already
    // has its `.so` — skip the manifest GET entirely.
    if versions.is_empty() || versions.iter().all(|v| pcov_so_path(dirs, *v).is_file()) {
        return;
    }
    ensure_for_installed_spec(dirs, dl, &PCOV_SPEC).await;
}

/// Shared fetch loop: resolve the host triple in `spec`'s manifest for each
/// installed PHP minor, sha-verify, and atomically place the `.so`. Best-effort.
async fn ensure_for_installed_spec(dirs: &PlatformDirs, dl: &dyn Downloader, spec: &ExtSpec) {
    let Some(manifest) = fetch_manifest(dl, spec.manifest_name, spec.label).await else {
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
            tracing::info!(php = %minor, os, arch, ext = spec.label, "no extension published for this triple");
            continue;
        };
        let dest = so_path_named(dirs, v, spec.so_name);
        if existing_matches(&dest, &file.sha256) {
            continue; // already current
        }
        match download_and_place(dl, &file.name, &file.sha256, &dest).await {
            Ok(()) => tracing::info!(php = %minor, ext = spec.label, "installed PHP extension"),
            Err(e) => {
                tracing::warn!(php = %minor, ext = spec.label, error = %e, "failed to install PHP extension");
            }
        }
    }
}

async fn fetch_manifest(dl: &dyn Downloader, manifest_name: &str, label: &str) -> Option<Manifest> {
    let url = format!("{RELEASE_BASE}/{manifest_name}");
    match dl.download(&url).await {
        Ok(bytes) => match serde_json::from_slice::<Manifest>(&bytes) {
            Ok(m) => Some(m),
            Err(e) => {
                tracing::warn!(error = %e, ext = label, "manifest parse failed");
                None
            }
        },
        Err(e) => {
            tracing::warn!(error = %e, ext = label, "manifest download failed");
            None
        }
    }
}

/// True if `path` already holds bytes whose SHA-256 hex equals `want`.
pub(crate) fn existing_matches(path: &Path, want: &str) -> bool {
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

pub(crate) fn sha256_hex(bytes: &[u8]) -> String {
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
    fn pcov_so_path_is_sibling_of_dump_so() {
        let tmp = tempfile::tempdir().unwrap();
        let dirs = dirs_in(tmp.path());
        let p = pcov_so_path(&dirs, PhpVersion::new(8, 4));
        assert!(p.ends_with("php-ext/php-8.4/pcov.so"));
        // Same dir as yerd-dump.so (shared php-ext/php-<ver>/), distinct file.
        assert_eq!(p.parent(), so_path(&dirs, PhpVersion::new(8, 4)).parent());
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
