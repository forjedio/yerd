//! Downloads PHP extension `.so`s (`yerd-dump`, `pcov`, `xdebug`) per installed
//! PHP version.
//!
//! All come from the same `yerd-php-ext` GitHub release, each described by its
//! own manifest (`manifest.json` for dump, `pcov-manifest.json` for pcov,
//! `xdebug-manifest.json` for Xdebug) listing
//! each file's `php`/`os`/`arch`/`sha256`. yerd resolves
//! the file matching the host triple for each installed PHP minor, verifies the
//! SHA-256, and places it at `{data}/php-ext/php-<ver>/yerd-dump.so` (a sibling
//! of the PHP installs, so a PHP patch update - which wipes `{data}/php/php-<ver>`
//! - never removes the extension).
//!
//! Everything here is best-effort: a download/verify failure logs and leaves the
//! site running with no dumps.

use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};
use std::time::Duration;

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

const XDEBUG_SPEC: ExtSpec = ExtSpec {
    manifest_name: "xdebug-manifest.json",
    so_name: "xdebug.so",
    label: "xdebug",
};

/// Why a bundled Xdebug could not be made available for a version.
#[derive(Debug, thiserror::Error)]
pub enum XdebugFetchError {
    /// The release manifest could not be downloaded or parsed.
    #[error("the Xdebug release manifest is unavailable: {0}")]
    ManifestUnavailable(String),
    /// The manifest has no build for this PHP version and host triple.
    #[error("no Xdebug build is published for PHP {version} on {os}/{arch}")]
    NotPublished {
        /// The PHP minor asked for.
        version: PhpVersion,
        /// The host OS, as the manifest spells it.
        os: String,
        /// The host architecture, as the manifest spells it.
        arch: String,
    },
    /// The download or the write of the staged file failed.
    #[error("{0}")]
    Download(String),
    /// The downloaded bytes did not match the manifest's SHA-256.
    #[error("sha256 mismatch for {name}: got {got}, want {want}")]
    ChecksumMismatch {
        /// The release asset name.
        name: String,
        /// The SHA-256 of the downloaded bytes.
        got: String,
        /// The SHA-256 the manifest lists.
        want: String,
    },
}

/// A downloaded, SHA-verified `xdebug.so` held at a temp sibling of its final
/// path until it has been load-probed. [`StagedSo::publish`] renames it into
/// place; dropping it unpublished (a failed probe, a timeout, a cancelled
/// request) deletes the temp file, so the last good `.so` is never touched.
#[derive(Debug)]
pub struct StagedSo {
    tmp: PathBuf,
    dest: PathBuf,
    published: bool,
}

impl StagedSo {
    /// The temp file to load-probe.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.tmp
    }

    /// Atomically rename the staged file over its final path.
    ///
    /// # Errors
    /// The rename's I/O error; the temp file is then removed on drop.
    pub fn publish(mut self) -> std::io::Result<()> {
        std::fs::rename(&self.tmp, &self.dest)?;
        self.published = true;
        Ok(())
    }
}

impl Drop for StagedSo {
    fn drop(&mut self) {
        if !self.published {
            let _ = std::fs::remove_file(&self.tmp);
        }
    }
}

/// The outcome of [`stage_xdebug`].
#[derive(Debug)]
pub enum XdebugStage {
    /// The `.so` on disk already matches the manifest; nothing was downloaded.
    Current,
    /// New bytes were downloaded and staged, waiting for a probe and publish.
    Staged(StagedSo),
}

/// The host OS/arch as the manifest names them (`macos`/`linux`,
/// `aarch64`/`x86_64`) - `std::env::consts` already uses these spellings.
pub(crate) fn host_os_arch() -> (&'static str, &'static str) {
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

/// Absolute path of the bundled `xdebug.so` for `v` (present or not). Sibling
/// of the dump and pcov `.so`s.
#[must_use]
pub fn xdebug_so_path(dirs: &PlatformDirs, v: PhpVersion) -> PathBuf {
    so_path_named(dirs, v, XDEBUG_SPEC.so_name)
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
///
/// Returns the versions whose `.so` was replaced, so the caller can restart
/// just those pools: a running FPM keeps the old extension mapped until then.
pub async fn ensure_for_installed(dirs: &PlatformDirs, dl: &dyn Downloader) -> Vec<PhpVersion> {
    let placed = Mutex::new(Vec::new());
    ensure_for_installed_spec(dirs, dl, &DUMP_SPEC, &placed).await;
    placed.into_inner().unwrap_or_else(PoisonError::into_inner)
}

/// What [`ensure_for_installed_within`] placed before it finished or ran out
/// of time.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct EnsureOutcome {
    /// Versions whose `.so` was replaced, including those placed before a
    /// timeout cut the fetch short.
    pub replaced: Vec<PhpVersion>,
    /// Whether `limit` elapsed before every version was checked.
    pub timed_out: bool,
}

/// [`ensure_for_installed`] bounded by `limit`. Each placement is recorded as
/// it happens, so a timeout still reports the versions already replaced: their
/// running pools keep the old `.so` mapped and a later check would see a
/// matching hash and never restart them.
pub async fn ensure_for_installed_within(
    dirs: &PlatformDirs,
    dl: &dyn Downloader,
    limit: Duration,
) -> EnsureOutcome {
    let placed = Mutex::new(Vec::new());
    let ensure = ensure_for_installed_spec(dirs, dl, &DUMP_SPEC, &placed);
    let timed_out = tokio::time::timeout(limit, ensure).await.is_err();
    EnsureOutcome {
        replaced: placed.into_inner().unwrap_or_else(PoisonError::into_inner),
        timed_out,
    }
}

/// Ensure the `pcov` extension is present for every installed PHP version.
///
/// Used by the CLI cover shims (`phpcover`/`php<ver>cover`); ungated, unlike the
/// dump fetch. Warm/offline starts skip the network entirely: if every installed
/// version already has a `pcov.so`, return without touching GitHub. (The "present"
/// check is a proxy for "current" - a stale `.so` won't refresh on a pure restart,
/// which is fine: pcov is ABI-stable per PHP minor and any *missing* `.so` still
/// forces a full manifest fetch + re-verify.)
pub async fn ensure_pcov_for_installed(dirs: &PlatformDirs, dl: &dyn Downloader) {
    let versions: Vec<PhpVersion> = installed_versions(dirs)
        .into_iter()
        .filter(|v| !v.is_legacy())
        .collect();
    if versions.is_empty() || versions.iter().all(|v| pcov_so_path(dirs, *v).is_file()) {
        return;
    }
    ensure_for_installed_spec(dirs, dl, &PCOV_SPEC, &Mutex::new(Vec::new())).await;
}

/// Shared fetch loop: resolve the host triple in `spec`'s manifest for each
/// installed PHP minor, sha-verify, and atomically place the `.so`. Best-effort.
/// Legacy minors (< 8.2) are skipped: `forjedio/yerd-php-ext` builds neither
/// `pcov` nor `yerd-dump` for EOL PHP, so a fetch would only 404-log per poll.
/// Each version whose `.so` was replaced is pushed to `placed` as soon as it
/// lands, so the record survives the future being cancelled.
async fn ensure_for_installed_spec(
    dirs: &PlatformDirs,
    dl: &dyn Downloader,
    spec: &ExtSpec,
    placed: &Mutex<Vec<PhpVersion>>,
) {
    let Some(manifest) = fetch_manifest(dl, spec.manifest_name, spec.label).await else {
        return;
    };
    let (os, arch) = host_os_arch();
    for v in installed_versions(dirs)
        .into_iter()
        .filter(|v| !v.is_legacy())
    {
        match place_from_manifest(dirs, dl, &manifest, spec, v, os, arch).await {
            Ok(true) => placed
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(v),
            Ok(false) => {}
            Err(PlaceError::NotPublished) => {
                tracing::info!(php = %v, os, arch, ext = spec.label, "no extension published for this triple");
            }
            Err(PlaceError::Download(e)) => {
                tracing::warn!(php = %v, ext = spec.label, error = %e, "failed to install PHP extension");
            }
        }
    }
}

/// Why [`place_from_manifest`] could not make a version's `.so` current.
#[derive(Debug, thiserror::Error)]
enum PlaceError {
    /// The manifest has no entry for the version and host triple.
    #[error("not published for this triple")]
    NotPublished,
    /// The download, its SHA-256 check or the write to disk failed.
    #[error("{0}")]
    Download(String),
}

/// Make `spec`'s `.so` for `v` match its `(v, os, arch)` manifest entry,
/// downloading only when the file on disk is missing or has a different
/// SHA-256. Returns whether the file was replaced.
async fn place_from_manifest(
    dirs: &PlatformDirs,
    dl: &dyn Downloader,
    manifest: &Manifest,
    spec: &ExtSpec,
    v: PhpVersion,
    os: &str,
    arch: &str,
) -> Result<bool, PlaceError> {
    let minor = v.to_string();
    let file = manifest
        .files
        .iter()
        .find(|f| f.php == minor && f.os == os && f.arch == arch)
        .ok_or(PlaceError::NotPublished)?;
    let dest = so_path_named(dirs, v, spec.so_name);
    if existing_matches(&dest, &file.sha256) {
        return Ok(false);
    }
    download_and_place(dl, &file.name, &file.sha256, &dest)
        .await
        .map_err(|e| PlaceError::Download(e.to_string()))?;
    tracing::info!(php = %minor, ext = spec.label, "installed PHP extension");
    Ok(true)
}

/// Stage the bundled `xdebug.so` for `v`: fetch the Xdebug manifest, find the
/// `(v, os, arch)` entry, and, unless the file on disk already matches its
/// SHA-256, download and verify it into a [`StagedSo`] next to the final path.
/// The final `.so` is never written here. The triple is injected so tests
/// don't depend on the CI host.
///
/// # Errors
/// [`XdebugFetchError`] when the manifest is unavailable, has no build for the
/// triple, or the download or checksum fails.
pub async fn stage_xdebug(
    dirs: &PlatformDirs,
    dl: &dyn Downloader,
    v: PhpVersion,
    os: &str,
    arch: &str,
) -> Result<XdebugStage, XdebugFetchError> {
    let manifest = fetch_manifest_result(dl, XDEBUG_SPEC.manifest_name)
        .await
        .map_err(XdebugFetchError::ManifestUnavailable)?;
    stage_from_manifest(dirs, dl, &manifest, v, os, arch).await
}

/// Startup refresh staging for `versions` (installed versions with Xdebug
/// enabled): one manifest fetch, then a [`StagedSo`] for each version whose
/// `.so` is missing or no longer matches. No network when `versions` is
/// empty. Failures only log.
pub async fn stage_xdebug_for(
    dirs: &PlatformDirs,
    dl: &dyn Downloader,
    versions: &[PhpVersion],
    os: &str,
    arch: &str,
) -> Vec<(PhpVersion, StagedSo)> {
    if versions.is_empty() {
        return Vec::new();
    }
    let manifest = match fetch_manifest_result(dl, XDEBUG_SPEC.manifest_name).await {
        Ok(m) => m,
        Err(e) => {
            tracing::warn!(error = %e, ext = XDEBUG_SPEC.label, "manifest unavailable");
            return Vec::new();
        }
    };
    let mut staged = Vec::new();
    for v in versions {
        match stage_from_manifest(dirs, dl, &manifest, *v, os, arch).await {
            Ok(XdebugStage::Staged(so)) => staged.push((*v, so)),
            Ok(XdebugStage::Current) => {}
            Err(e) => {
                tracing::warn!(php = %v, ext = XDEBUG_SPEC.label, error = %e, "failed to refresh PHP extension");
            }
        }
    }
    staged
}

async fn stage_from_manifest(
    dirs: &PlatformDirs,
    dl: &dyn Downloader,
    manifest: &Manifest,
    v: PhpVersion,
    os: &str,
    arch: &str,
) -> Result<XdebugStage, XdebugFetchError> {
    let minor = v.to_string();
    let file = manifest
        .files
        .iter()
        .find(|f| f.php == minor && f.os == os && f.arch == arch)
        .ok_or_else(|| XdebugFetchError::NotPublished {
            version: v,
            os: os.to_owned(),
            arch: arch.to_owned(),
        })?;
    let dest = xdebug_so_path(dirs, v);
    if existing_matches(&dest, &file.sha256) {
        return Ok(XdebugStage::Current);
    }
    let url = format!("{RELEASE_BASE}/{}", file.name);
    let bytes = dl
        .download(&url)
        .await
        .map_err(|e| XdebugFetchError::Download(format!("download {}: {e}", file.name)))?;
    let got = sha256_hex(&bytes);
    if !got.eq_ignore_ascii_case(&file.sha256) {
        return Err(XdebugFetchError::ChecksumMismatch {
            name: file.name.clone(),
            got,
            want: file.sha256.clone(),
        });
    }
    let tmp = write_temp_sibling(&dest, &bytes)
        .map_err(|e| XdebugFetchError::Download(format!("write {}: {e}", dest.display())))?;
    Ok(XdebugStage::Staged(StagedSo {
        tmp,
        dest,
        published: false,
    }))
}

async fn fetch_manifest(dl: &dyn Downloader, manifest_name: &str, label: &str) -> Option<Manifest> {
    match fetch_manifest_result(dl, manifest_name).await {
        Ok(m) => Some(m),
        Err(e) => {
            tracing::warn!(error = %e, ext = label, "manifest unavailable");
            None
        }
    }
}

async fn fetch_manifest_result(
    dl: &dyn Downloader,
    manifest_name: &str,
) -> Result<Manifest, String> {
    let url = format!("{RELEASE_BASE}/{manifest_name}");
    let bytes = dl
        .download(&url)
        .await
        .map_err(|e| format!("download failed: {e}"))?;
    serde_json::from_slice::<Manifest>(&bytes).map_err(|e| format!("parse failed: {e}"))
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
    let tmp = write_temp_sibling(dest, &bytes)?;
    if let Err(e) = std::fs::rename(&tmp, dest) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    Ok(())
}

/// Write `bytes` to a unique temp sibling of `dest` (pid + sequence, so
/// overlapping installs of the same version don't share a temp path) and
/// return its path, creating the parent directory first.
fn write_temp_sibling(dest: &Path, bytes: &[u8]) -> std::io::Result<PathBuf> {
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let seq = TMP_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let tmp = dest.with_extension(format!("so.{}.{}.tmp", std::process::id(), seq));
    if let Err(e) = std::fs::write(&tmp, bytes) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    Ok(tmp)
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

    #[test]
    fn host_os_arch_uses_std_env_consts() {
        let (os, arch) = host_os_arch();
        assert_eq!(os, std::env::consts::OS);
        assert_eq!(arch, std::env::consts::ARCH);
    }

    #[test]
    fn so_path_named_layout() {
        let tmp = tempfile::tempdir().unwrap();
        let dirs = dirs_in(tmp.path());
        let p = so_path_named(&dirs, PhpVersion::new(8, 3), "custom.so");
        assert!(p.ends_with("php-ext/php-8.3/custom.so"));
    }

    /// A URL-keyed fake downloader: returns the queued bytes for any URL that
    /// ends with one of the registered suffixes, else a transport error. Counts
    /// `download` invocations so tests can assert the network is never touched.
    struct FakeDownloader {
        routes: std::collections::HashMap<String, Vec<u8>>,
        calls: std::sync::atomic::AtomicUsize,
    }

    impl FakeDownloader {
        fn new(routes: std::collections::HashMap<String, Vec<u8>>) -> Self {
            Self {
                routes,
                calls: std::sync::atomic::AtomicUsize::new(0),
            }
        }

        fn call_count(&self) -> usize {
            self.calls.load(std::sync::atomic::Ordering::Relaxed)
        }
    }

    #[async_trait::async_trait]
    impl Downloader for FakeDownloader {
        async fn download(&self, url: &str) -> Result<Vec<u8>, yerd_php::DownloadError> {
            self.calls
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            for (suffix, bytes) in &self.routes {
                if url.ends_with(suffix) {
                    return Ok(bytes.clone());
                }
            }
            Err(yerd_php::DownloadError::Transport {
                url: url.to_owned(),
                reason: "no route".into(),
            })
        }
    }

    #[tokio::test]
    async fn fetch_manifest_parses_valid_json() {
        let body = serde_json::json!({
            "files": [
                {"name":"yerd-dump-php8.4-macos-aarch64.so","php":"8.4","os":"macos","arch":"aarch64","sha256":"ab"}
            ]
        })
        .to_string();
        let dl = FakeDownloader::new(
            [("manifest.json".to_owned(), body.into_bytes())]
                .into_iter()
                .collect(),
        );
        let m = fetch_manifest(&dl, "manifest.json", "yerd-dump")
            .await
            .expect("manifest parses");
        assert_eq!(m.files.len(), 1);
        assert_eq!(m.files[0].php, "8.4");
    }

    #[tokio::test]
    async fn fetch_manifest_none_on_bad_json_or_transport() {
        let dl = FakeDownloader::new(
            [("manifest.json".to_owned(), b"not json".to_vec())]
                .into_iter()
                .collect(),
        );
        assert!(fetch_manifest(&dl, "manifest.json", "yerd-dump")
            .await
            .is_none());
        let empty = FakeDownloader::new(std::collections::HashMap::new());
        assert!(fetch_manifest(&empty, "manifest.json", "yerd-dump")
            .await
            .is_none());
    }

    #[tokio::test]
    async fn ensure_pcov_skips_network_when_no_php_installed() {
        let tmp = tempfile::tempdir().unwrap();
        let dirs = dirs_in(tmp.path());
        let dl = FakeDownloader::new(std::collections::HashMap::new());
        ensure_pcov_for_installed(&dirs, &dl).await;
        assert_eq!(
            dl.call_count(),
            0,
            "no PHP installed: must not hit the network"
        );
    }

    fn fake_install(dirs: &PlatformDirs, v: PhpVersion) {
        let base = dirs.data.join("php").join(format!("php-{v}")).join("sbin");
        std::fs::create_dir_all(&base).unwrap();
        std::fs::write(base.join("php-fpm"), b"x").unwrap();
    }

    #[tokio::test]
    async fn ensure_pcov_skips_network_when_only_missing_is_legacy() {
        let tmp = tempfile::tempdir().unwrap();
        let dirs = dirs_in(tmp.path());
        fake_install(&dirs, PhpVersion::new(8, 4));
        fake_install(&dirs, PhpVersion::new(7, 4));
        let pcov = pcov_so_path(&dirs, PhpVersion::new(8, 4));
        std::fs::create_dir_all(pcov.parent().unwrap()).unwrap();
        std::fs::write(&pcov, b"so").unwrap();
        let dl = FakeDownloader::new(std::collections::HashMap::new());
        ensure_pcov_for_installed(&dirs, &dl).await;
        assert_eq!(
            dl.call_count(),
            0,
            "the only version without pcov is legacy (7.4), which is skipped — no fetch"
        );
    }

    fn xdebug_manifest(entries: &[(&str, &str, &str, &str, &str)]) -> Vec<u8> {
        let files: Vec<_> = entries
            .iter()
            .map(|(name, php, os, arch, sha)| {
                serde_json::json!({"name": name, "php": php, "os": os, "arch": arch, "sha256": sha})
            })
            .collect();
        serde_json::json!({ "files": files })
            .to_string()
            .into_bytes()
    }

    fn xdebug_routes(manifest: Vec<u8>, so: &[u8]) -> std::collections::HashMap<String, Vec<u8>> {
        [
            ("/xdebug-manifest.json".to_owned(), manifest),
            ("/xdebug-8.4-macos-aarch64.so".to_owned(), so.to_vec()),
        ]
        .into_iter()
        .collect()
    }

    #[test]
    fn xdebug_so_path_layout() {
        let tmp = tempfile::tempdir().unwrap();
        let dirs = dirs_in(tmp.path());
        let p = xdebug_so_path(&dirs, PhpVersion::new(8, 4));
        assert!(p.ends_with("php-ext/php-8.4/xdebug.so"));
        assert_eq!(
            p.parent(),
            pcov_so_path(&dirs, PhpVersion::new(8, 4)).parent()
        );
    }

    fn staged(stage: XdebugStage) -> StagedSo {
        match stage {
            XdebugStage::Staged(so) => so,
            XdebugStage::Current => panic!("expected a staged file"),
        }
    }

    fn so_bytes_manifest() -> Vec<u8> {
        xdebug_manifest(&[(
            "xdebug-8.4-macos-aarch64.so",
            "8.4",
            "macos",
            "aarch64",
            &sha256_hex(b"so-bytes"),
        )])
    }

    #[tokio::test]
    async fn stage_xdebug_manifest_missing_is_manifest_unavailable() {
        let tmp = tempfile::tempdir().unwrap();
        let dirs = dirs_in(tmp.path());
        let dl = FakeDownloader::new(std::collections::HashMap::new());
        let err = stage_xdebug(&dirs, &dl, PhpVersion::new(8, 4), "macos", "aarch64")
            .await
            .unwrap_err();
        assert!(
            matches!(err, XdebugFetchError::ManifestUnavailable(_)),
            "{err:?}"
        );
    }

    #[tokio::test]
    async fn stage_xdebug_missing_triple_is_not_published() {
        let tmp = tempfile::tempdir().unwrap();
        let dirs = dirs_in(tmp.path());
        let manifest =
            xdebug_manifest(&[("xdebug-8.4-linux-x86_64.so", "8.4", "linux", "x86_64", "ab")]);
        let dl = FakeDownloader::new(xdebug_routes(manifest, b"so"));
        let err = stage_xdebug(&dirs, &dl, PhpVersion::new(8, 4), "macos", "aarch64")
            .await
            .unwrap_err();
        assert!(
            matches!(err, XdebugFetchError::NotPublished { .. }),
            "{err:?}"
        );
        assert!(!xdebug_so_path(&dirs, PhpVersion::new(8, 4)).exists());
    }

    #[tokio::test]
    async fn stage_xdebug_bad_sha_is_a_checksum_error_and_writes_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        let dirs = dirs_in(tmp.path());
        let manifest = xdebug_manifest(&[(
            "xdebug-8.4-macos-aarch64.so",
            "8.4",
            "macos",
            "aarch64",
            "deadbeef",
        )]);
        let dl = FakeDownloader::new(xdebug_routes(manifest, b"so-bytes"));
        let err = stage_xdebug(&dirs, &dl, PhpVersion::new(8, 4), "macos", "aarch64")
            .await
            .unwrap_err();
        match err {
            XdebugFetchError::ChecksumMismatch { name, got, want } => {
                assert_eq!(name, "xdebug-8.4-macos-aarch64.so");
                assert_eq!(got, sha256_hex(b"so-bytes"));
                assert_eq!(want, "deadbeef");
            }
            other => panic!("expected a checksum mismatch, got {other:?}"),
        }
        assert!(!dirs.data.join("php-ext").join("php-8.4").exists());
    }

    #[tokio::test]
    async fn staged_xdebug_publishes_only_on_request_and_cleans_up_on_drop() {
        let tmp = tempfile::tempdir().unwrap();
        let dirs = dirs_in(tmp.path());
        let v = PhpVersion::new(8, 4);
        let dl = FakeDownloader::new(xdebug_routes(so_bytes_manifest(), b"so-bytes"));
        let path = xdebug_so_path(&dirs, v);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, b"last-good").unwrap();

        let dropped = staged(
            stage_xdebug(&dirs, &dl, v, "macos", "aarch64")
                .await
                .unwrap(),
        );
        let tmp_path = dropped.path().to_path_buf();
        assert_eq!(std::fs::read(&tmp_path).unwrap(), b"so-bytes");
        assert_eq!(std::fs::read(&path).unwrap(), b"last-good");
        drop(dropped);
        assert!(!tmp_path.exists());
        assert_eq!(std::fs::read(&path).unwrap(), b"last-good");

        let kept = staged(
            stage_xdebug(&dirs, &dl, v, "macos", "aarch64")
                .await
                .unwrap(),
        );
        let tmp_path = kept.path().to_path_buf();
        kept.publish().unwrap();
        assert!(!tmp_path.exists());
        assert_eq!(std::fs::read(&path).unwrap(), b"so-bytes");

        let calls = dl.call_count();
        let again = stage_xdebug(&dirs, &dl, v, "macos", "aarch64")
            .await
            .unwrap();
        assert!(matches!(again, XdebugStage::Current), "{again:?}");
        assert_eq!(
            dl.call_count(),
            calls + 1,
            "only the manifest is fetched again"
        );
    }

    #[tokio::test]
    async fn stage_xdebug_for_no_versions_makes_no_calls() {
        let tmp = tempfile::tempdir().unwrap();
        let dirs = dirs_in(tmp.path());
        let dl = FakeDownloader::new(std::collections::HashMap::new());
        let staged = stage_xdebug_for(&dirs, &dl, &[], "macos", "aarch64").await;
        assert!(staged.is_empty());
        assert_eq!(dl.call_count(), 0);
    }

    #[tokio::test]
    async fn stage_xdebug_for_stages_only_stale_versions() {
        let tmp = tempfile::tempdir().unwrap();
        let dirs = dirs_in(tmp.path());
        let v84 = PhpVersion::new(8, 4);
        let v83 = PhpVersion::new(8, 3);
        let dl = FakeDownloader::new(xdebug_routes(so_bytes_manifest(), b"so-bytes"));
        let stale = xdebug_so_path(&dirs, v84);
        std::fs::create_dir_all(stale.parent().unwrap()).unwrap();
        std::fs::write(&stale, b"stale").unwrap();

        let got = stage_xdebug_for(&dirs, &dl, &[v83, v84], "macos", "aarch64").await;
        assert_eq!(got.iter().map(|(v, _)| *v).collect::<Vec<_>>(), vec![v84]);
        assert_eq!(std::fs::read(&stale).unwrap(), b"stale");
        for (_, so) in got {
            so.publish().unwrap();
        }
        assert_eq!(std::fs::read(&stale).unwrap(), b"so-bytes");

        let again = stage_xdebug_for(&dirs, &dl, &[v84], "macos", "aarch64").await;
        assert!(again.is_empty());
    }

    #[tokio::test]
    async fn stage_xdebug_for_manifest_failure_stages_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        let dirs = dirs_in(tmp.path());
        let dl = FakeDownloader::new(std::collections::HashMap::new());
        let staged =
            stage_xdebug_for(&dirs, &dl, &[PhpVersion::new(8, 4)], "macos", "aarch64").await;
        assert!(staged.is_empty());
        assert_eq!(dl.call_count(), 1);
    }

    fn dump_routes_for_host(so: &[u8]) -> std::collections::HashMap<String, Vec<u8>> {
        let (os, arch) = host_os_arch();
        let name = format!("yerd-dump-php8.4-{os}-{arch}.so");
        let manifest = serde_json::json!({
            "files": [
                {"name": name, "php": "8.4", "os": os, "arch": arch, "sha256": sha256_hex(so)}
            ]
        })
        .to_string();
        [
            ("/manifest.json".to_owned(), manifest.into_bytes()),
            (format!("/{name}"), so.to_vec()),
        ]
        .into_iter()
        .collect()
    }

    #[tokio::test]
    async fn ensure_for_installed_returns_versions_whose_so_changed() {
        let tmp = tempfile::tempdir().unwrap();
        let dirs = dirs_in(tmp.path());
        let v = PhpVersion::new(8, 4);
        fake_install(&dirs, v);
        fake_install(&dirs, PhpVersion::new(7, 4));
        let dest = so_path(&dirs, v);
        std::fs::create_dir_all(dest.parent().unwrap()).unwrap();
        std::fs::write(&dest, b"old build").unwrap();

        let dl = FakeDownloader::new(dump_routes_for_host(b"new build"));
        let replaced = ensure_for_installed(&dirs, &dl).await;

        assert_eq!(replaced, vec![v]);
        assert_eq!(std::fs::read(&dest).unwrap(), b"new build");
    }

    #[tokio::test]
    async fn ensure_for_installed_returns_nothing_when_sha_matches() {
        let tmp = tempfile::tempdir().unwrap();
        let dirs = dirs_in(tmp.path());
        let v = PhpVersion::new(8, 4);
        fake_install(&dirs, v);
        let dest = so_path(&dirs, v);
        std::fs::create_dir_all(dest.parent().unwrap()).unwrap();
        std::fs::write(&dest, b"same build").unwrap();

        let dl = FakeDownloader::new(dump_routes_for_host(b"same build"));
        let replaced = ensure_for_installed(&dirs, &dl).await;

        assert!(replaced.is_empty());
        assert_eq!(dl.call_count(), 1);
    }

    struct HangingDownloader {
        inner: FakeDownloader,
        hang_suffix: String,
    }

    #[async_trait::async_trait]
    impl Downloader for HangingDownloader {
        async fn download(&self, url: &str) -> Result<Vec<u8>, yerd_php::DownloadError> {
            if url.ends_with(&self.hang_suffix) {
                std::future::pending::<()>().await;
            }
            self.inner.download(url).await
        }
    }

    #[tokio::test]
    async fn ensure_within_keeps_versions_placed_before_the_timeout() {
        let tmp = tempfile::tempdir().unwrap();
        let dirs = dirs_in(tmp.path());
        let (os, arch) = host_os_arch();
        let v83 = PhpVersion::new(8, 3);
        let v84 = PhpVersion::new(8, 4);
        fake_install(&dirs, v83);
        fake_install(&dirs, v84);
        let so = b"new build";
        let name83 = format!("yerd-dump-php8.3-{os}-{arch}.so");
        let name84 = format!("yerd-dump-php8.4-{os}-{arch}.so");
        let manifest = serde_json::json!({
            "files": [
                {"name": name83, "php": "8.3", "os": os, "arch": arch, "sha256": sha256_hex(so)},
                {"name": name84, "php": "8.4", "os": os, "arch": arch, "sha256": sha256_hex(so)}
            ]
        })
        .to_string();
        let routes = [
            ("/manifest.json".to_owned(), manifest.into_bytes()),
            (format!("/{name83}"), so.to_vec()),
        ]
        .into_iter()
        .collect();
        let dl = HangingDownloader {
            inner: FakeDownloader::new(routes),
            hang_suffix: format!("/{name84}"),
        };

        let outcome = ensure_for_installed_within(&dirs, &dl, Duration::from_millis(200)).await;

        assert_eq!(
            outcome,
            EnsureOutcome {
                replaced: vec![v83],
                timed_out: true,
            }
        );
        assert_eq!(std::fs::read(so_path(&dirs, v83)).unwrap(), so);
        assert!(!so_path(&dirs, v84).exists());
    }

    #[tokio::test]
    async fn ensure_within_reports_no_timeout_when_unchanged() {
        let tmp = tempfile::tempdir().unwrap();
        let dirs = dirs_in(tmp.path());
        let v = PhpVersion::new(8, 4);
        fake_install(&dirs, v);
        let dest = so_path(&dirs, v);
        std::fs::create_dir_all(dest.parent().unwrap()).unwrap();
        std::fs::write(&dest, b"same build").unwrap();

        let dl = FakeDownloader::new(dump_routes_for_host(b"same build"));
        let outcome = ensure_for_installed_within(&dirs, &dl, Duration::from_secs(5)).await;

        assert_eq!(outcome, EnsureOutcome::default());
    }

    #[tokio::test]
    async fn ensure_for_installed_is_noop_without_versions() {
        let tmp = tempfile::tempdir().unwrap();
        let dirs = dirs_in(tmp.path());
        let body = serde_json::json!({ "files": [] }).to_string();
        let dl = FakeDownloader::new(
            [("manifest.json".to_owned(), body.into_bytes())]
                .into_iter()
                .collect(),
        );
        assert!(ensure_for_installed(&dirs, &dl).await.is_empty());
        assert!(!dirs.data.join("php-ext").exists());
        assert_eq!(
            dl.call_count(),
            1,
            "fetches the manifest once, then places no .so because no PHP is installed"
        );
    }
}
