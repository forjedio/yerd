//! PHP version install: download prebuilt static builds and unpack them into
//! yerd's data dir.
//!
//! The `reqwest`-backed [`Downloader`] lives here (a binary) so `yerd-php`
//! stays dependency-light. Version resolution + tar-member safety are pure
//! helpers from `yerd_php::release`; this module is the I/O edge: fetch the
//! listing → resolve → fetch tarballs → safe-extract the single binary →
//! atomic install. Integrity is TLS-only (no sha pinning - per user decision).

use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use async_trait::async_trait;

use yerd_core::PhpVersion;
use yerd_php::{
    current_os_arch, is_safe_member, Artifact, BinaryKind, DownloadError, Downloader, PhpError,
};
use yerd_platform::PlatformDirs;

/// `reqwest`-backed downloader (rustls, no OpenSSL; follows redirects).
pub struct ReqwestDownloader {
    client: reqwest::Client,
}

/// Emit a coarse byte-progress line at most once per [`PROGRESS_STEP`] (plus the
/// first call and completion), so a 40 MB+ download streams "X / Y MB" updates
/// into the job log without flooding it. `last` tracks the last *emitted* byte
/// count (`u64::MAX` = nothing emitted yet).
const PROGRESS_STEP: u64 = 4 * 1024 * 1024;

/// Byte counts are formatted as integer tenths-of-a-MB to avoid a float cast and
/// its precision lint.
fn emit_byte_progress(
    tx: &ProgressTx,
    label: &str,
    done: u64,
    total: Option<u64>,
    last: &AtomicU64,
) {
    let prev = last.load(Ordering::Relaxed);
    let first = prev == u64::MAX;
    let complete = total.is_some_and(|t| done >= t);
    if !first && !complete && done < prev.wrapping_add(PROGRESS_STEP) {
        return;
    }
    last.store(done, Ordering::Relaxed);
    let mb = |b: u64| {
        let tenths = b.saturating_mul(10) / (1024 * 1024);
        format!("{}.{} MB", tenths / 10, tenths % 10)
    };
    let line = match total {
        Some(t) => format!("Downloading {label}: {} / {}", mb(done), mb(t)),
        None => format!("Downloading {label}: {}", mb(done)),
    };
    let _ = tx.send(line);
}

impl ReqwestDownloader {
    /// Construct a fresh client. Sets a `User-Agent` (some hosts - notably the
    /// GitHub API used for Bun releases - reject requests without one); falls
    /// back to the default client if the builder fails.
    ///
    /// Bounds the two ways a download can wedge indefinitely: a `connect_timeout`
    /// for a connection that never establishes, and a `read_timeout` (idle/stall
    /// timeout between reads) for a body that stops mid-stream. reqwest's default
    /// is *unbounded*: the cause of a PHP install "spinning" for minutes until
    /// the kernel gives up. Deliberately no hard overall `.timeout()`, so a
    /// slow-but-progressing large download isn't killed.
    #[must_use]
    pub fn new() -> Self {
        let client = reqwest::Client::builder()
            .user_agent(concat!("yerd/", env!("CARGO_PKG_VERSION")))
            .connect_timeout(Duration::from_secs(30))
            .read_timeout(Duration::from_secs(60))
            .build()
            .unwrap_or_default();
        Self { client }
    }
}

impl Default for ReqwestDownloader {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Downloader for ReqwestDownloader {
    async fn download(&self, url: &str) -> Result<Vec<u8>, DownloadError> {
        self.download_with_progress(url, &|_, _| {}).await
    }

    /// Streams the body chunk-by-chunk so progress can be reported and a
    /// mid-stream stall trips `read_timeout` rather than buffering forever. The
    /// `Content-Length` capacity hint is clamped (the header is server-controlled;
    /// a bogus huge value would otherwise abort the daemon on a failed
    /// allocation), and the `Vec` still grows past the cap as needed.
    async fn download_with_progress(
        &self,
        url: &str,
        progress: &(dyn Fn(u64, Option<u64>) + Send + Sync),
    ) -> Result<Vec<u8>, DownloadError> {
        let transport = |e: reqwest::Error| DownloadError::Transport {
            url: url.to_owned(),
            reason: e.to_string(),
        };
        let mut resp = self
            .client
            .get(url)
            .send()
            .await
            .map_err(transport)?
            .error_for_status()
            .map_err(transport)?;
        let total = resp.content_length();
        let cap = total.map_or(0, |n| n.min(64 * 1024 * 1024) as usize);
        let mut buf = Vec::with_capacity(cap);
        progress(0, total);
        while let Some(chunk) = resp.chunk().await.map_err(transport)? {
            buf.extend_from_slice(&chunk);
            progress(buf.len() as u64, total);
        }
        Ok(buf)
    }
}

/// Sink for human-readable progress lines streamed into a job log (the streamed
/// install). `tokio`'s `UnboundedSender<String>`, same concrete type the tool
/// installer uses, so a job handler can hand its sender to either.
pub type ProgressTx = tokio::sync::mpsc::UnboundedSender<String>;

/// Emit one progress line if a sink is attached.
fn note(progress: Option<&ProgressTx>, msg: impl Into<String>) {
    if let Some(tx) = progress {
        let _ = tx.send(msg.into());
    }
}

/// Install `version` (major.minor) into `dirs.data/php/php-<minor>/`.
///
/// Resolves the latest patch from the distribution's live listing, downloads
/// the CLI and FPM tarballs, safely extracts the single binary from each, and
/// atomically swaps the result into place. Idempotent: reinstalling replaces
/// the dir. **Integrity is TLS-only** - the distribution publishes no checksum
/// sidecars and yerd does not pin hashes (deliberate; see `yerd_php::release`).
///
/// When `progress` is set, coarse phase + byte-count updates are streamed to it
/// (the GUI's streamed install polls these); pass `None` for the silent path.
pub async fn install(
    version: PhpVersion,
    dirs: &PlatformDirs,
    dl: &dyn Downloader,
    progress: Option<&ProgressTx>,
) -> Result<(), PhpError> {
    let (os, arch) = current_os_arch()?;
    note(progress, format!("Resolving latest PHP {version}…"));
    let listing = dl.download(&yerd_php::listing_url(os)).await?;
    let listing = String::from_utf8_lossy(&listing);
    let artifact = yerd_php::resolve_from_listing(&listing, version, os, arch)?;
    tracing::info!(%version, patch = %artifact.full_version, "resolved PHP build; downloading");
    note(
        progress,
        format!("Found PHP {} — downloading…", artifact.full_version),
    );

    let php_root = dirs.data.join("php");
    fs_ctx(std::fs::create_dir_all(&php_root), &php_root)?;

    let staging = php_root.join(format!(
        ".staging-{}-{}",
        artifact.install_dir_name,
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&staging);

    if let Err(e) = stage(&artifact, dl, &staging, progress).await {
        let _ = std::fs::remove_dir_all(&staging);
        return Err(e);
    }

    note(progress, "Finalising install…");
    let final_dir = php_root.join(&artifact.install_dir_name);
    if final_dir.exists() {
        if let Err(e) = std::fs::remove_dir_all(&final_dir) {
            let _ = std::fs::remove_dir_all(&staging);
            return Err(fs_err(&final_dir, &e));
        }
    }
    fs_ctx(std::fs::rename(&staging, &final_dir), &final_dir)
}

/// Write the CLI `php.ini` (`{data}/php-cli.ini`) that the `yerd path` rc-block
/// points `PHPRC` at, rendered from the effective PHP settings. Only the
/// CLI-relevant directives are emitted (see `yerd_core::php_settings`). The data
/// dir is created if missing. Written atomically (temp + rename, matching the FPM
/// pool conf) so a crash mid-write can't leave PHP reading a truncated ini.
/// Always rewrites (idempotent).
pub fn write_cli_ini(
    dirs: &PlatformDirs,
    settings: &std::collections::BTreeMap<String, String>,
    ca_bundle: Option<&Path>,
) -> std::io::Result<()> {
    std::fs::create_dir_all(&dirs.data)?;
    let body = yerd_core::php_settings::render_cli_ini(settings);
    let mut contents = format!(
        "; Generated by Yerd — CLI PHP defaults (PHPRC target). Manage via the GUI or `yerd`.\n{body}"
    );
    if let Some(path) = ca_bundle {
        use std::fmt::Write as _;
        let p = path.display().to_string();
        if !p.chars().any(char::is_control) {
            let _ = write!(contents, "openssl.cafile = {p}\ncurl.cainfo = {p}\n");
        }
    }
    yerd_php::io::atomic_write::write(&dirs.data.join("php-cli.ini"), contents.as_bytes())
}

/// Filename of the installed-patch marker inside a per-version dir.
const VERSION_MARKER: &str = ".yerd-version";

async fn stage(
    artifact: &Artifact,
    dl: &dyn Downloader,
    staging: &Path,
    progress: Option<&ProgressTx>,
) -> Result<(), PhpError> {
    fetch_and_extract(
        dl,
        &artifact.cli_url,
        BinaryKind::Cli,
        staging,
        progress,
        "CLI",
    )
    .await?;
    fetch_and_extract(
        dl,
        &artifact.fpm_url,
        BinaryKind::Fpm,
        staging,
        progress,
        "FPM",
    )
    .await?;
    fs_ctx(std::fs::create_dir_all(staging), staging)?;
    let marker = staging.join(VERSION_MARKER);
    fs_ctx(std::fs::write(&marker, &artifact.full_version), &marker)?;
    Ok(())
}

/// The installed full patch version of `minor` (reads the `.yerd-version`
/// marker), or `None` if not installed / unmarked.
#[must_use]
pub fn installed_patch(dirs: &PlatformDirs, minor: PhpVersion) -> Option<String> {
    let marker = dirs
        .data
        .join("php")
        .join(format!("php-{}.{}", minor.major, minor.minor))
        .join(VERSION_MARKER);
    let v = std::fs::read_to_string(marker).ok()?;
    let v = v.trim().to_owned();
    if v.is_empty() {
        None
    } else {
        Some(v)
    }
}

/// Download one PHP binary tarball and extract its single member into `staging`.
///
/// Streams with byte-progress when `progress` is attached, otherwise takes the
/// silent path; a download error converts to `PhpError` via `#[from]`.
async fn fetch_and_extract(
    dl: &dyn Downloader,
    url: &str,
    kind: BinaryKind,
    staging: &Path,
    progress: Option<&ProgressTx>,
    label: &str,
) -> Result<(), PhpError> {
    tracing::info!(%url, "downloading PHP binary");
    let bytes = match progress {
        Some(tx) => {
            let last = AtomicU64::new(u64::MAX);
            let cb = |done: u64, total: Option<u64>| {
                emit_byte_progress(tx, label, done, total, &last);
            };
            dl.download_with_progress(url, &cb).await?
        }
        None => dl.download(url).await?,
    };
    let binary = extract_member(&bytes, kind, url)?;

    let mut target = staging.to_path_buf();
    for seg in kind.install_segments() {
        target.push(seg);
    }
    if let Some(parent) = target.parent() {
        fs_ctx(std::fs::create_dir_all(parent), parent)?;
    }
    fs_ctx(std::fs::write(&target, binary), &target)?;
    make_executable(&target)?;
    Ok(())
}

/// Extract the single expected `Regular`-file member from a `.tar.gz`,
/// rejecting traversal, non-regular entries (symlink/hardlink/dir), unexpected
/// names, and duplicates - closes zip-slip and link-target escapes.
fn extract_member(gz_bytes: &[u8], kind: BinaryKind, url: &str) -> Result<Vec<u8>, PhpError> {
    let want = kind.archive_member();
    let decoder = flate2::read::GzDecoder::new(gz_bytes);
    let mut archive = tar::Archive::new(decoder);
    let entries = archive.entries().map_err(|e| extract_err(url, &e))?;

    let mut found: Option<Vec<u8>> = None;
    for entry in entries {
        let mut entry = entry.map_err(|e| extract_err(url, &e))?;
        let path = entry.path().map_err(|e| extract_err(url, &e))?;
        let name = path.to_string_lossy().into_owned();
        if !is_safe_member(&name) {
            return Err(extract_msg(url, format!("unsafe archive member {name:?}")));
        }
        if !entry.header().entry_type().is_file() {
            return Err(extract_msg(
                url,
                format!("archive contains a non-regular entry {name:?}"),
            ));
        }
        if path.file_name().and_then(|s| s.to_str()) != Some(want) {
            return Err(extract_msg(
                url,
                format!("unexpected archive member {name:?}"),
            ));
        }
        if found.is_some() {
            return Err(extract_msg(url, format!("duplicate {want:?} in archive")));
        }
        let mut buf = Vec::new();
        entry
            .read_to_end(&mut buf)
            .map_err(|e| extract_err(url, &e))?;
        found = Some(buf);
    }
    found.ok_or_else(|| extract_msg(url, format!("{want:?} not found in archive")))
}

#[cfg(unix)]
fn make_executable(path: &Path) -> Result<(), PhpError> {
    use std::os::unix::fs::PermissionsExt;
    fs_ctx(
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)),
        path,
    )
}

#[cfg(not(unix))]
fn make_executable(_path: &Path) -> Result<(), PhpError> {
    Ok(())
}

fn fs_ctx<T>(r: std::io::Result<T>, path: &Path) -> Result<T, PhpError> {
    r.map_err(|e| fs_err(path, &e))
}

fn fs_err(path: &Path, e: &std::io::Error) -> PhpError {
    PhpError::Extract {
        what: path.display().to_string(),
        detail: e.to_string(),
    }
}

fn extract_err(url: &str, e: &dyn std::fmt::Display) -> PhpError {
    extract_msg(url, e.to_string())
}

fn extract_msg(url: &str, detail: String) -> PhpError {
    PhpError::Extract {
        what: url.to_owned(),
        detail,
    }
}

/// Absolute path to a version's CLI binary (`data/php/php-<minor>/bin/php`).
#[must_use]
pub fn cli_binary_path(dirs: &PlatformDirs, version: PhpVersion) -> PathBuf {
    let mut p = dirs
        .data
        .join("php")
        .join(format!("php-{}.{}", version.major, version.minor));
    for seg in BinaryKind::Cli.install_segments() {
        p.push(seg);
    }
    p
}

/// The directory users add to PATH for the managed `php` shim (`data/bin`).
#[must_use]
pub fn shim_dir(dirs: &PlatformDirs) -> PathBuf {
    dirs.data.join("bin")
}

/// Atomically create/replace the symlink `link` → `target` (temp + rename, so a
/// concurrent reader never sees a half-written link). The temp name embeds the
/// link's filename + pid + a process-global sequence, so two writers racing on
/// the same link name (e.g. an unsynchronized `set_default_shim` vs a reconcile,
/// both touching `php`) never share a temp path.
#[cfg(unix)]
pub(crate) fn place_symlink(link: &Path, target: &Path) -> Result<(), PhpError> {
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let parent = link
        .parent()
        .ok_or_else(|| fs_err(link, &std::io::Error::other("shim link has no parent")))?;
    let name = link
        .file_name()
        .and_then(|s| s.to_str())
        .ok_or_else(|| fs_err(link, &std::io::Error::other("shim link has no file name")))?;
    let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let tmp = parent.join(format!(".{name}.tmp-{}-{seq}", std::process::id()));
    let _ = std::fs::remove_file(&tmp);
    fs_ctx(std::os::unix::fs::symlink(target, &tmp), &tmp)?;
    fs_ctx(std::fs::rename(&tmp, link), link)?;
    Ok(())
}

/// Point the managed `php` shim at `version`'s CLI binary (unix symlink),
/// created/replaced atomically. Returns the shim dir for PATH hints.
#[cfg(unix)]
pub fn set_default_shim(dirs: &PlatformDirs, version: PhpVersion) -> Result<PathBuf, PhpError> {
    let bin = shim_dir(dirs);
    fs_ctx(std::fs::create_dir_all(&bin), &bin)?;
    place_symlink(&bin.join("php"), &cli_binary_path(dirs, version))?;
    Ok(bin)
}

#[cfg(not(unix))]
pub fn set_default_shim(dirs: &PlatformDirs, _version: PhpVersion) -> Result<PathBuf, PhpError> {
    Ok(shim_dir(dirs))
}

/// The shim filename for `v`: `php<major>.<minor>` (clean) or `…cover` (pcov).
/// Dotted form matches the `PhpVersion` parser.
#[cfg(unix)]
fn versioned_shim_name(v: PhpVersion, cover: bool) -> String {
    if cover {
        format!("php{}.{}cover", v.major, v.minor)
    } else {
        format!("php{}.{}", v.major, v.minor)
    }
}

/// Parse a yerd-managed shim filename back to its PHP version. Matches **exactly**
/// `php<MAJOR>.<MINOR>` or `php<MAJOR>.<MINOR>cover`; returns `None` for `php`,
/// `phpcover`, and any other name - so the pruner never touches foreign files.
#[cfg(unix)]
fn managed_shim_version(name: &str) -> Option<PhpVersion> {
    let rest = name.strip_prefix("php")?;
    let rest = rest.strip_suffix("cover").unwrap_or(rest);
    let (maj, min) = rest.split_once('.')?;
    if maj.is_empty() || min.is_empty() {
        return None;
    }
    let major: u8 = maj.parse().ok()?;
    let minor: u8 = min.parse().ok()?;
    if maj != major.to_string() || min != minor.to_string() {
        return None;
    }
    Some(PhpVersion::new(major, minor))
}

/// Reconcile the per-version CLI shims in `{data}/bin` against what's installed:
///
/// * for each installed `v`: `php<v>` → its CLI binary, `php<v>cover` → `yerd_bin`;
/// * `phpcover` → `yerd_bin` (resolves the default at run time);
/// * `php` → `default`'s CLI binary when `default` is installed and the link is
///   missing/stale (covers "installed but never `yerd use`d");
/// * prune managed `php<X.Y>`/`php<X.Y>cover` symlinks whose version is no longer
///   installed.
///
/// A **single** `discover_bundled` snapshot drives both create and prune. Callers
/// must serialize invocations (the daemon holds a dedicated mutex) so the
/// scan→prune can't race a concurrent install's create. Unix-only; no-op elsewhere.
#[cfg(unix)]
pub fn reconcile_shims(
    dirs: &PlatformDirs,
    yerd_bin: &Path,
    default: PhpVersion,
) -> Result<(), PhpError> {
    let bin = shim_dir(dirs);
    fs_ctx(std::fs::create_dir_all(&bin), &bin)?;

    let installed: Vec<PhpVersion> = yerd_php::discover_bundled(dirs)
        .map_err(|e| {
            fs_err(
                &dirs.data.join("php"),
                &std::io::Error::other(e.to_string()),
            )
        })?
        .into_iter()
        .map(|(v, _)| v)
        .collect();

    for &v in &installed {
        place_symlink(
            &bin.join(versioned_shim_name(v, false)),
            &cli_binary_path(dirs, v),
        )?;
        place_symlink(&bin.join(versioned_shim_name(v, true)), yerd_bin)?;
    }
    place_symlink(&bin.join("phpcover"), yerd_bin)?;

    if installed.contains(&default) {
        let want = cli_binary_path(dirs, default);
        let php = bin.join("php");
        if std::fs::read_link(&php).ok().as_deref() != Some(want.as_path()) {
            place_symlink(&php, &want)?;
        }
    }

    let entries = match std::fs::read_dir(&bin) {
        Ok(e) => e,
        Err(ref e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(fs_err(&bin, &e)),
    };
    for entry in entries.flatten() {
        let fname = entry.file_name();
        let Some(name) = fname.to_str() else { continue };
        if name == "php" || name == "phpcover" {
            continue;
        }
        let Some(v) = managed_shim_version(name) else {
            continue;
        };
        if installed.contains(&v) {
            continue;
        }
        let path = entry.path();
        let is_link = std::fs::symlink_metadata(&path).is_ok_and(|m| m.file_type().is_symlink());
        if is_link {
            let _ = std::fs::remove_file(&path);
        }
    }
    Ok(())
}

#[cfg(not(unix))]
pub fn reconcile_shims(
    _dirs: &PlatformDirs,
    _yerd_bin: &Path,
    _default: PhpVersion,
) -> Result<(), PhpError> {
    Ok(())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::panic, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use std::io::Write as _;

    fn gzip_tar_single(name: &str, body: &[u8], mode: u32) -> Vec<u8> {
        let mut header = tar::Header::new_gnu();
        header.set_path(name).unwrap();
        header.set_size(body.len() as u64);
        header.set_mode(mode);
        header.set_entry_type(tar::EntryType::Regular);
        header.set_cksum();
        let mut tar_bytes = Vec::new();
        {
            let mut builder = tar::Builder::new(&mut tar_bytes);
            builder.append(&header, body).unwrap();
            builder.finish().unwrap();
        }
        let mut enc = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        enc.write_all(&tar_bytes).unwrap();
        enc.finish().unwrap()
    }

    fn gzip_tar_symlink(name: &str, target: &str) -> Vec<u8> {
        let mut header = tar::Header::new_gnu();
        header.set_entry_type(tar::EntryType::Symlink);
        header.set_size(0);
        header.set_mode(0o777);
        let mut tar_bytes = Vec::new();
        {
            let mut builder = tar::Builder::new(&mut tar_bytes);
            builder.append_link(&mut header, name, target).unwrap();
            builder.finish().unwrap();
        }
        let mut enc = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        enc.write_all(&tar_bytes).unwrap();
        enc.finish().unwrap()
    }

    #[test]
    fn write_cli_ini_appends_cert_lines_only_with_a_bundle() {
        let tmp = tempfile::tempdir().unwrap();
        let dirs = dirs_in(tmp.path());
        let settings = std::collections::BTreeMap::new();

        write_cli_ini(&dirs, &settings, None).unwrap();
        let without = std::fs::read_to_string(dirs.data.join("php-cli.ini")).unwrap();
        assert!(!without.contains("openssl.cafile"));
        assert!(!without.contains("curl.cainfo"));

        let bundle = dirs.data.join("cacert.pem");
        write_cli_ini(&dirs, &settings, Some(&bundle)).unwrap();
        let with = std::fs::read_to_string(dirs.data.join("php-cli.ini")).unwrap();
        assert!(with.contains(&format!("openssl.cafile = {}\n", bundle.display())));
        assert!(with.contains(&format!("curl.cainfo = {}\n", bundle.display())));

        write_cli_ini(&dirs, &settings, Some(Path::new("/d/ca\ncert.pem"))).unwrap();
        let injected = std::fs::read_to_string(dirs.data.join("php-cli.ini")).unwrap();
        assert!(
            !injected.contains("openssl.cafile"),
            "injection not skipped"
        );
        assert!(!injected.contains("curl.cainfo"), "injection not skipped");
    }

    #[test]
    fn extract_member_returns_the_single_binary() {
        let gz = gzip_tar_single("php", b"ELF-cli-bytes", 0o755);
        let out = extract_member(&gz, BinaryKind::Cli, "u").unwrap();
        assert_eq!(out, b"ELF-cli-bytes");
    }

    #[test]
    fn extract_member_rejects_wrong_name() {
        let gz = gzip_tar_single("evil", b"x", 0o755);
        assert!(extract_member(&gz, BinaryKind::Cli, "u").is_err());
    }

    #[test]
    fn extract_member_rejects_symlink_entry() {
        let gz = gzip_tar_symlink("php", "/home/user/.bashrc");
        let err = extract_member(&gz, BinaryKind::Cli, "u").unwrap_err();
        assert!(matches!(err, PhpError::Extract { .. }), "got {err:?}");
    }

    fn dirs_in(tmp: &Path) -> PlatformDirs {
        PlatformDirs {
            config: tmp.join("c"),
            data: tmp.join("d"),
            state: tmp.join("s"),
            cache: tmp.join("ca"),
            runtime: tmp.join("r"),
        }
    }

    /// URL-keyed fake: the directory URL (ends `/`) → listing; `-cli-`/`-fpm-`
    /// URLs → the respective tarball.
    struct FakeDownloader {
        listing: String,
        cli: Vec<u8>,
        fpm: Vec<u8>,
    }

    #[async_trait]
    impl Downloader for FakeDownloader {
        async fn download(&self, url: &str) -> Result<Vec<u8>, DownloadError> {
            if url.ends_with('/') {
                Ok(self.listing.clone().into_bytes())
            } else if url.contains("-cli-") {
                Ok(self.cli.clone())
            } else if url.contains("-fpm-") {
                Ok(self.fpm.clone())
            } else {
                Err(DownloadError::Transport {
                    url: url.to_owned(),
                    reason: "unexpected url".into(),
                })
            }
        }
    }

    #[tokio::test]
    async fn install_lays_down_both_binaries_executable() {
        let tmp = tempfile::tempdir().unwrap();
        let dirs = dirs_in(tmp.path());
        let (os, arch) = current_os_arch().unwrap();
        let listing = format!(
            "<a href=\"php-8.5.2-cli-{os}-{arch}.tar.gz\">x</a>\
             <a href=\"php-8.5.6-cli-{os}-{arch}.tar.gz\">y</a>\
             <a href=\"php-8.5.6-fpm-{os}-{arch}.tar.gz\">z</a>",
            os = os.as_str(),
            arch = arch.as_str()
        );
        let dl = FakeDownloader {
            listing,
            cli: gzip_tar_single("php", b"CLI-BYTES", 0o755),
            fpm: gzip_tar_single("php-fpm", b"FPM-BYTES", 0o755),
        };

        install(PhpVersion::new(8, 5), &dirs, &dl, None)
            .await
            .unwrap();

        let base = dirs.data.join("php").join("php-8.5");
        assert_eq!(
            std::fs::read(base.join("bin").join("php")).unwrap(),
            b"CLI-BYTES"
        );
        assert_eq!(
            std::fs::read(base.join("sbin").join("php-fpm")).unwrap(),
            b"FPM-BYTES"
        );
        assert_eq!(
            installed_patch(&dirs, PhpVersion::new(8, 5)).as_deref(),
            Some("8.5.6")
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(base.join("bin").join("php"))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o111, 0o111, "cli binary should be executable");
        }
    }

    #[tokio::test]
    async fn install_errors_when_version_not_published_and_writes_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        let dirs = dirs_in(tmp.path());
        let (os, arch) = current_os_arch().unwrap();
        let listing = format!("php-8.4.21-cli-{}-{}.tar.gz", os.as_str(), arch.as_str());
        let dl = FakeDownloader {
            listing,
            cli: vec![],
            fpm: vec![],
        };
        let err = install(PhpVersion::new(8, 5), &dirs, &dl, None)
            .await
            .unwrap_err();
        assert!(
            matches!(err, PhpError::VersionUnavailable { .. }),
            "got {err:?}"
        );
        assert!(!dirs.data.join("php").join("php-8.5").exists());
    }

    #[test]
    fn cli_binary_path_layout() {
        let dirs = PlatformDirs {
            config: "/c".into(),
            data: "/d".into(),
            state: "/s".into(),
            cache: "/ca".into(),
            runtime: "/r".into(),
        };
        assert_eq!(
            cli_binary_path(&dirs, PhpVersion::new(8, 5)),
            PathBuf::from("/d/php/php-8.5/bin/php")
        );
    }

    #[cfg(unix)]
    #[test]
    fn managed_shim_version_matches_only_canonical_names() {
        assert_eq!(managed_shim_version("php8.4"), Some(PhpVersion::new(8, 4)));
        assert_eq!(
            managed_shim_version("php8.4cover"),
            Some(PhpVersion::new(8, 4))
        );
        assert_eq!(managed_shim_version("php"), None);
        assert_eq!(managed_shim_version("phpcover"), None);
        assert_eq!(managed_shim_version("php8"), None);
        assert_eq!(managed_shim_version("php8.4.1"), None);
        assert_eq!(managed_shim_version("phpunit"), None);
        assert_eq!(managed_shim_version("php8.4covers"), None);
        assert_eq!(managed_shim_version("php08.04"), None);
        assert_eq!(managed_shim_version("php8.04"), None);
    }

    #[cfg(unix)]
    #[test]
    fn versioned_shim_name_is_dotted() {
        let v = PhpVersion::new(8, 4);
        assert_eq!(versioned_shim_name(v, false), "php8.4");
        assert_eq!(versioned_shim_name(v, true), "php8.4cover");
    }

    #[cfg(unix)]
    #[test]
    fn reconcile_creates_versioned_and_cover_shims_and_prunes_stale() {
        let tmp = tempfile::tempdir().unwrap();
        let dirs = dirs_in(tmp.path());
        let yerd_bin = tmp.path().join("yerd");
        std::fs::write(&yerd_bin, b"#!fake").unwrap();

        let mk = |v: PhpVersion| {
            let base = dirs
                .data
                .join("php")
                .join(format!("php-{}.{}", v.major, v.minor));
            std::fs::create_dir_all(base.join("bin")).unwrap();
            std::fs::create_dir_all(base.join("sbin")).unwrap();
            std::fs::write(base.join("bin").join("php"), b"cli").unwrap();
            std::fs::write(base.join("sbin").join("php-fpm"), b"fpm").unwrap();
        };
        mk(PhpVersion::new(8, 4));

        let bin = shim_dir(&dirs);
        std::fs::create_dir_all(&bin).unwrap();
        std::os::unix::fs::symlink(&yerd_bin, bin.join("php8.2cover")).unwrap();
        std::fs::write(bin.join("keep.txt"), b"user file").unwrap();

        reconcile_shims(&dirs, &yerd_bin, PhpVersion::new(8, 4)).unwrap();

        assert!(bin.join("php8.4").exists());
        assert!(bin.join("php8.4cover").exists());
        assert!(bin.join("phpcover").exists());
        assert_eq!(
            std::fs::read_link(bin.join("php")).unwrap(),
            cli_binary_path(&dirs, PhpVersion::new(8, 4))
        );
        assert!(!bin.join("php8.2cover").exists());
        assert!(bin.join("keep.txt").exists());
    }

    #[cfg(unix)]
    #[test]
    fn reconcile_leaves_php_alone_when_default_not_installed() {
        let tmp = tempfile::tempdir().unwrap();
        let dirs = dirs_in(tmp.path());
        let yerd_bin = tmp.path().join("yerd");
        std::fs::write(&yerd_bin, b"#!fake").unwrap();
        let base = dirs.data.join("php").join("php-8.4");
        std::fs::create_dir_all(base.join("bin")).unwrap();
        std::fs::create_dir_all(base.join("sbin")).unwrap();
        std::fs::write(base.join("bin").join("php"), b"cli").unwrap();
        std::fs::write(base.join("sbin").join("php-fpm"), b"fpm").unwrap();

        reconcile_shims(&dirs, &yerd_bin, PhpVersion::new(8, 3)).unwrap();

        assert!(!shim_dir(&dirs).join("php").exists());
        assert!(shim_dir(&dirs).join("php8.4cover").exists());
    }
}
