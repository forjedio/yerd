//! PHP version install: download prebuilt static builds and unpack them into
//! yerd's data dir.
//!
//! The `reqwest`-backed [`Downloader`] lives here (a binary) so `yerd-php`
//! stays dependency-light. Version resolution + tar-member safety are pure
//! helpers from `yerd_php::release`; this module is the I/O edge: fetch +
//! **verify** the signed `php.json` manifest → resolve → fetch tarballs →
//! **SHA-256-verify** and safe-extract the single binary → atomic install.
//! Integrity is anchored by the manifest's minisign signature (verified here)
//! and each tarball's published SHA-256.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use async_trait::async_trait;

use yerd_core::PhpVersion;
use yerd_php::{
    current_os_arch, is_safe_member, listing_sig_url, listing_url, Artifact, BinaryKind,
    DownloadError, Downloader, PhpError,
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

/// Download the signed `php.json` manifest and its detached minisign signature,
/// verify the signature against `public_key`, and return the trusted JSON body.
///
/// This is the single choke point through which every PHP listing read passes
/// (install, update poll, available-versions). The signature is **prehashed**
/// (`minisign -H`); `verify_minisign` rejects legacy signatures. Mirrors
/// `self_update::stage_update`, which likewise threads the public key so tests
/// can sign a fixture manifest with their own key. Returns the body via
/// `from_utf8` (not lossily), so the parsed manifest is byte-for-byte what the
/// signature covered - invalid UTF-8 is rejected as `ListingParse`.
pub(crate) async fn fetch_verified_listing(
    dl: &dyn Downloader,
    public_key: &str,
    channel: yerd_php::Channel,
) -> Result<String, PhpError> {
    let body = dl.download(&listing_url(channel)).await?;
    let sig = dl.download(&listing_sig_url(channel)).await?;
    let sig = String::from_utf8_lossy(&sig);
    yerd_update::verify_minisign(public_key, &sig, &body).map_err(|e| {
        tracing::warn!(error = %e, "php listing signature verification failed");
        PhpError::ListingUntrusted
    })?;
    String::from_utf8(body).map_err(|e| PhpError::ListingParse {
        detail: format!("listing body is not valid UTF-8: {e}"),
    })
}

/// Install `version` (major.minor) into `dirs.data/php/php-<minor>/`.
///
/// Fetches + verifies the signed `php.json` manifest, resolves the single build
/// for this platform, downloads the CLI and FPM tarballs, **verifies each
/// tarball's SHA-256** against the manifest, safely extracts the single binary
/// from each, and atomically swaps the result into place. Idempotent:
/// reinstalling replaces the dir. `public_key` is the minisign key the manifest
/// is verified against (prod passes [`yerd_update::PHP_LISTING_PUBLIC_KEY`]).
///
/// When `progress` is set, coarse phase + byte-count updates are streamed to it
/// (the GUI's streamed install polls these); pass `None` for the silent path.
pub async fn install(
    version: PhpVersion,
    dirs: &PlatformDirs,
    dl: &dyn Downloader,
    public_key: &str,
    progress: Option<&ProgressTx>,
) -> Result<(), PhpError> {
    let (os, arch) = current_os_arch()?;
    let channel = yerd_php::Channel::of(version);
    note(progress, format!("Resolving PHP {version}…"));
    let listing = fetch_verified_listing(dl, public_key, channel).await?;
    let artifact = yerd_php::resolve_from_listing(&listing, version, os, arch, channel)?;
    tracing::info!(%version, patch = %artifact.full_version, revision = artifact.revision, "resolved PHP build; downloading");
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

/// Write the CLI `php.ini` files the version-aware `php`/`php<ver>` shims point
/// `PHPRC` at, rendered from the effective PHP settings. Two layers are written,
/// each atomically (temp + rename) so a crash mid-write can't leave PHP reading a
/// truncated ini:
///
/// - the base `{data}/php-cli.ini` (global settings only, no extensions and no
///   per-version entries), kept for the shell-profile `PHPRC` export and any
///   non-shim `php`;
/// - one `{data}/php-cli-<minor>.ini` per **installed** version, rendering that
///   version's *effective* settings (global merged with its sparse overrides)
///   and its registered extensions. Writing per *installed* version (not per
///   config key) is load-bearing: a version whose last extension/override was
///   just removed still gets its file rewritten back to base-only, rather than
///   left stale.
///
/// Always rewrites (idempotent).
pub fn write_cli_ini(
    dirs: &PlatformDirs,
    settings: &std::collections::BTreeMap<String, String>,
    ca_bundle: Option<&Path>,
    extensions: &std::collections::BTreeMap<PhpVersion, Vec<yerd_config::ExtEntry>>,
    version_settings: &std::collections::BTreeMap<
        PhpVersion,
        std::collections::BTreeMap<String, String>,
    >,
) -> std::io::Result<()> {
    std::fs::create_dir_all(&dirs.data)?;
    let base = decorate_cli_ini(
        &yerd_core::php_settings::render_cli_ini(settings),
        ca_bundle,
    );
    yerd_php::io::atomic_write::write(&dirs.data.join("php-cli.ini"), base.as_bytes())?;

    let no_overrides = std::collections::BTreeMap::new();
    let installed =
        yerd_php::discover_bundled(dirs).map_err(|e| std::io::Error::other(e.to_string()))?;
    for (v, _) in installed {
        let refs: Vec<(&str, bool)> = extensions
            .get(&v)
            .map(|es| {
                es.iter()
                    .filter(|e| Path::new(&e.path).is_file())
                    .map(|e| (e.path.as_str(), e.zend))
                    .collect()
            })
            .unwrap_or_default();
        let effective = yerd_core::php_settings::merge_effective(
            settings,
            version_settings.get(&v).unwrap_or(&no_overrides),
        );
        let body = yerd_core::php_settings::render_cli_ini_with_ext(&effective, &refs);
        let contents = decorate_cli_ini(&body, ca_bundle);
        let name = format!("php-cli-{}.{}.ini", v.major, v.minor);
        yerd_php::io::atomic_write::write(&dirs.data.join(name), contents.as_bytes())?;
    }
    Ok(())
}

/// Prepend the generated-file header and append the managed CA `openssl.cafile` /
/// `curl.cainfo` lines to a rendered CLI ini body.
fn decorate_cli_ini(body: &str, ca_bundle: Option<&Path>) -> String {
    let mut contents = format!(
        "; Generated by Yerd — CLI PHP defaults (PHPRC target). Manage via the GUI or `yerd`.\n{body}"
    );
    if let Some(path) = ca_bundle {
        use std::fmt::Write as _;
        if let Some(p) = yerd_core::php_settings::sanitize_ca_bundle_path(path) {
            let _ = write!(contents, "openssl.cafile = {p}\ncurl.cainfo = {p}\n");
        }
    }
    contents
}

/// Filename of the installed-patch marker inside a per-version dir. Kept
/// byte-identical to older yerd (bare patch string) so a rolled-back client
/// still parses it; the revision lives in a sibling [`REVISION_MARKER`].
const VERSION_MARKER: &str = ".yerd-version";

/// Filename of the installed-revision marker inside a per-version dir (the `-N`
/// suffix). Absent for pre-cutover installs, which read as revision 0.
const REVISION_MARKER: &str = ".yerd-revision";

async fn stage(
    artifact: &Artifact,
    dl: &dyn Downloader,
    staging: &Path,
    progress: Option<&ProgressTx>,
) -> Result<(), PhpError> {
    fetch_and_extract(
        dl,
        &artifact.cli_url,
        &artifact.cli_sha256,
        BinaryKind::Cli,
        staging,
        progress,
        "CLI",
    )
    .await?;
    fetch_and_extract(
        dl,
        &artifact.fpm_url,
        &artifact.fpm_sha256,
        BinaryKind::Fpm,
        staging,
        progress,
        "FPM",
    )
    .await?;
    fs_ctx(std::fs::create_dir_all(staging), staging)?;
    let marker = staging.join(VERSION_MARKER);
    fs_ctx(std::fs::write(&marker, &artifact.full_version), &marker)?;
    let rev_marker = staging.join(REVISION_MARKER);
    fs_ctx(
        std::fs::write(&rev_marker, artifact.revision.to_string()),
        &rev_marker,
    )?;
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

/// The installed build revision of `minor` (reads the `.yerd-revision` marker).
/// A missing/unparseable marker reads as `0` - a legacy install predating the
/// c-ares cutover, which every `revision >= 1` manifest build then supersedes.
#[must_use]
pub fn installed_revision(dirs: &PlatformDirs, minor: PhpVersion) -> u32 {
    let marker = dirs
        .data
        .join("php")
        .join(format!("php-{}.{}", minor.major, minor.minor))
        .join(REVISION_MARKER);
    std::fs::read_to_string(marker)
        .ok()
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or(0)
}

/// Download one PHP binary tarball, verify its SHA-256 against the manifest, and
/// extract its single member into `staging`.
///
/// Streams with byte-progress when `progress` is attached, otherwise takes the
/// silent path; a download error converts to `PhpError` via `#[from]`. The
/// SHA-256 check happens on both paths (this is the single fetch site) - bytes
/// that don't match `expected_sha256` are never extracted.
async fn fetch_and_extract(
    dl: &dyn Downloader,
    url: &str,
    expected_sha256: &str,
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
    if !yerd_update::sha256_hex(&bytes).eq_ignore_ascii_case(expected_sha256) {
        tracing::warn!(%url, "PHP tarball sha256 mismatch");
        return Err(PhpError::ShaMismatch {
            file: url.to_owned(),
        });
    }
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

/// The `PHPRC` target for a version's CLI: its per-version ini
/// (`data/php-cli-<major>.<minor>.ini`) if present, else the base
/// `data/php-cli.ini` if present, else `None` (leave `PHPRC` unset). Mirrors
/// `bin/yerd/src/shim.rs::cli_phprc` - the two binaries can't share code across
/// the boundary, so the filename shape here is kept byte-for-byte in step with
/// what [`write_cli_ini`] emits. Pointing a wp-cli launch at this ini is what
/// gets the user's global CLI settings (`memory_limit`, ...) applied; without
/// it, wp-cli runs under PHP's compiled-in defaults.
#[must_use]
pub fn cli_phprc(dirs: &PlatformDirs, version: PhpVersion) -> Option<PathBuf> {
    let per_version = dirs
        .data
        .join(format!("php-cli-{}.{}.ini", version.major, version.minor));
    if per_version.is_file() {
        return Some(per_version);
    }
    let base = dirs.data.join("php-cli.ini");
    base.is_file().then_some(base)
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

/// Ensure the managed `php` shim points at the Yerd multi-call wrapper
/// (`yerd_bin`), created/replaced atomically. The wrapper resolves the default
/// version from `config.php.default` at run time, so this is version-independent.
/// Returns the shim dir for PATH hints.
#[cfg(unix)]
pub fn set_default_shim(dirs: &PlatformDirs, yerd_bin: &Path) -> Result<PathBuf, PhpError> {
    let bin = shim_dir(dirs);
    fs_ctx(std::fs::create_dir_all(&bin), &bin)?;
    place_symlink(&bin.join("php"), yerd_bin)?;
    Ok(bin)
}

#[cfg(not(unix))]
pub fn set_default_shim(dirs: &PlatformDirs, _yerd_bin: &Path) -> Result<PathBuf, PhpError> {
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

/// Reconcile the per-version CLI shims in `{data}/bin` against what's installed.
///
/// Every clean and cover shim is a Yerd multi-call wrapper (a symlink to
/// `yerd_bin`): the wrapper reads `argv[0]`, resolves the target PHP + minor, and
/// points `PHPRC` at that version's generated ini before `exec`ing PHP. So:
///
/// * for each installed `v`: `php<v>` → `yerd_bin`, `php<v>cover` → `yerd_bin`;
/// * `phpcover` → `yerd_bin`;
/// * `php` → `yerd_bin` when at least one version is installed (the wrapper
///   resolves the default from `config.php.default` at run time);
/// * prune managed `php<X.Y>`/`php<X.Y>cover` symlinks whose version is no longer
///   installed.
///
/// A **single** `discover_bundled` snapshot drives both create and prune. Callers
/// must serialize invocations (the daemon holds a dedicated mutex) so the
/// scan→prune can't race a concurrent install's create. Unix-only; no-op elsewhere.
#[cfg(unix)]
pub fn reconcile_shims(dirs: &PlatformDirs, yerd_bin: &Path) -> Result<(), PhpError> {
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
        place_symlink(&bin.join(versioned_shim_name(v, false)), yerd_bin)?;
        // No cover shim for legacy (< 8.2): pcov is never built for it, so
        // `php<ver>cover` could only error. The gate in `cover_shim` still
        // rejects it, but not creating the shim keeps `{data}/bin` honest.
        if !v.is_legacy() {
            place_symlink(&bin.join(versioned_shim_name(v, true)), yerd_bin)?;
        }
    }
    place_symlink(&bin.join("phpcover"), yerd_bin)?;

    if !installed.is_empty() {
        let php = bin.join("php");
        if std::fs::read_link(&php).ok().as_deref() != Some(yerd_bin) {
            place_symlink(&php, yerd_bin)?;
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
        // Keep a shim only if its version is installed - except a legacy cover
        // shim (`php7.4cover`), which must be pruned even when 7.4 is installed,
        // since pcov is never built for legacy.
        let stale_legacy_cover = name.ends_with("cover") && v.is_legacy();
        if installed.contains(&v) && !stale_legacy_cover {
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
pub fn reconcile_shims(_dirs: &PlatformDirs, _yerd_bin: &Path) -> Result<(), PhpError> {
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

        let exts = std::collections::BTreeMap::new();
        let no_vs = std::collections::BTreeMap::new();
        write_cli_ini(&dirs, &settings, None, &exts, &no_vs).unwrap();
        let without = std::fs::read_to_string(dirs.data.join("php-cli.ini")).unwrap();
        assert!(!without.contains("openssl.cafile"));
        assert!(!without.contains("curl.cainfo"));

        let bundle = dirs.data.join("cacert.pem");
        write_cli_ini(&dirs, &settings, Some(&bundle), &exts, &no_vs).unwrap();
        let with = std::fs::read_to_string(dirs.data.join("php-cli.ini")).unwrap();
        assert!(with.contains(&format!("openssl.cafile = {}\n", bundle.display())));
        assert!(with.contains(&format!("curl.cainfo = {}\n", bundle.display())));

        for bad in ["/d/ca\ncert.pem", "/d/ca;cert.pem", "/d/ca#cert.pem"] {
            write_cli_ini(&dirs, &settings, Some(Path::new(bad)), &exts, &no_vs).unwrap();
            let injected = std::fs::read_to_string(dirs.data.join("php-cli.ini")).unwrap();
            assert!(
                !injected.contains("openssl.cafile"),
                "not skipped for {bad:?}"
            );
            assert!(!injected.contains("curl.cainfo"), "not skipped for {bad:?}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn write_cli_ini_scopes_overrides_to_the_version_file() {
        let tmp = tempfile::tempdir().unwrap();
        let dirs = dirs_in(tmp.path());
        let fpm_dir = dirs.data.join("php").join("php-8.3").join("sbin");
        std::fs::create_dir_all(&fpm_dir).unwrap();
        std::fs::write(fpm_dir.join("php-fpm"), b"#!/bin/sh\n").unwrap();

        let settings =
            std::collections::BTreeMap::from([("memory_limit".to_string(), "512M".to_string())]);
        let exts = std::collections::BTreeMap::new();
        let v83 = yerd_core::PhpVersion::new(8, 3);
        let version_settings = std::collections::BTreeMap::from([(
            v83,
            std::collections::BTreeMap::from([("memory_limit".to_string(), "1G".to_string())]),
        )]);
        write_cli_ini(&dirs, &settings, None, &exts, &version_settings).unwrap();

        let base = std::fs::read_to_string(dirs.data.join("php-cli.ini")).unwrap();
        assert!(base.contains("memory_limit = 512M\n"), "got: {base}");
        assert!(!base.contains("1G"), "override leaked into base: {base}");

        let per = std::fs::read_to_string(dirs.data.join("php-cli-8.3.ini")).unwrap();
        assert!(per.contains("memory_limit = 1G\n"), "got: {per}");
        assert!(!per.contains("512M"), "global shadowed override: {per}");
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

    /// URL-keyed fake: any `*.json.minisig` → signature; any `*.json` (stable
    /// `php.json` or legacy `php-legacy.json`) → the single `manifest`;
    /// `-cli-`/`-fpm-` → the respective tarball. Each test drives one channel, so
    /// serving the same manifest for whichever `.json` is requested is enough.
    struct FakeDownloader {
        manifest: String,
        minisig: String,
        cli: Vec<u8>,
        fpm: Vec<u8>,
    }

    #[async_trait]
    impl Downloader for FakeDownloader {
        async fn download(&self, url: &str) -> Result<Vec<u8>, DownloadError> {
            if url.ends_with(".minisig") {
                Ok(self.minisig.clone().into_bytes())
            } else if url.contains(".json") {
                Ok(self.manifest.clone().into_bytes())
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

    /// Build a one-build `php.json` for the host platform whose `cli`/`fpm`
    /// sha256 match the given tarball bytes, then sign it.
    fn signed_manifest_for(
        php: &str,
        minor: &str,
        revision: u32,
        cli: &[u8],
        fpm: &[u8],
    ) -> crate::test_support::SignedManifest {
        let (os, arch) = current_os_arch().unwrap();
        let manifest = format!(
            r#"{{ "schema": 1, "builds": [
                {{ "php": "{php}", "minor": "{minor}", "os": "{os}", "arch": "{arch}", "revision": {revision},
                   "cli": {{ "file": "php-{php}-{revision}-cli-{os}-{arch}.tar.gz", "sha256": "{cli_sha}", "size": {cli_len} }},
                   "fpm": {{ "file": "php-{php}-{revision}-fpm-{os}-{arch}.tar.gz", "sha256": "{fpm_sha}", "size": {fpm_len} }} }}
            ] }}"#,
            os = os.as_str(),
            arch = arch.as_str(),
            cli_sha = yerd_update::sha256_hex(cli),
            fpm_sha = yerd_update::sha256_hex(fpm),
            cli_len = cli.len(),
            fpm_len = fpm.len(),
        );
        crate::test_support::sign_manifest(&manifest)
    }

    const KEY: &str = yerd_update::PHP_LISTING_PUBLIC_KEY;

    #[tokio::test]
    async fn install_lays_down_both_binaries_executable() {
        let tmp = tempfile::tempdir().unwrap();
        let dirs = dirs_in(tmp.path());
        let cli = gzip_tar_single("php", b"CLI-BYTES", 0o755);
        let fpm = gzip_tar_single("php-fpm", b"FPM-BYTES", 0o755);
        let signed = signed_manifest_for("8.5.7", "8.5", 1, &cli, &fpm);
        let dl = FakeDownloader {
            manifest: signed.manifest.clone(),
            minisig: signed.minisig.clone(),
            cli,
            fpm,
        };

        install(PhpVersion::new(8, 5), &dirs, &dl, &signed.public_key, None)
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
            Some("8.5.7")
        );
        assert_eq!(installed_revision(&dirs, PhpVersion::new(8, 5)), 1);
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

    /// The manifest is signed over the correct shas, but the fake serves
    /// different tarball bytes, so the post-download sha check must abort.
    #[tokio::test]
    async fn install_rejects_sha_mismatch_and_writes_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        let dirs = dirs_in(tmp.path());
        let cli = gzip_tar_single("php", b"CLI-BYTES", 0o755);
        let fpm = gzip_tar_single("php-fpm", b"FPM-BYTES", 0o755);
        let signed = signed_manifest_for("8.5.7", "8.5", 1, &cli, &fpm);
        let dl = FakeDownloader {
            manifest: signed.manifest.clone(),
            minisig: signed.minisig.clone(),
            cli: gzip_tar_single("php", b"TAMPERED", 0o755),
            fpm,
        };
        let err = install(PhpVersion::new(8, 5), &dirs, &dl, &signed.public_key, None)
            .await
            .unwrap_err();
        assert!(matches!(err, PhpError::ShaMismatch { .. }), "got {err:?}");
        assert!(!dirs.data.join("php").join("php-8.5").exists());
    }

    /// A manifest signed by a throwaway key fails when verified against the
    /// production `PHP_LISTING_PUBLIC_KEY`, and nothing is installed.
    #[tokio::test]
    async fn install_rejects_untrusted_listing() {
        let tmp = tempfile::tempdir().unwrap();
        let dirs = dirs_in(tmp.path());
        let cli = gzip_tar_single("php", b"CLI-BYTES", 0o755);
        let fpm = gzip_tar_single("php-fpm", b"FPM-BYTES", 0o755);
        let signed = signed_manifest_for("8.5.7", "8.5", 1, &cli, &fpm);
        let dl = FakeDownloader {
            manifest: signed.manifest.clone(),
            minisig: signed.minisig.clone(),
            cli,
            fpm,
        };
        let err = install(PhpVersion::new(8, 5), &dirs, &dl, KEY, None)
            .await
            .unwrap_err();
        assert!(matches!(err, PhpError::ListingUntrusted), "got {err:?}");
        assert!(!dirs.data.join("php").join("php-8.5").exists());
    }

    /// A validly-signed manifest that simply lacks the requested minor (only 8.4
    /// present) resolves to `VersionUnavailable`, not a trust error.
    /// The fetch choke point rejects a manifest whose body was altered after
    /// signing: the original signature no longer covers the served bytes, so
    /// `fetch_verified_listing` returns `ListingUntrusted` before any resolve.
    #[tokio::test]
    async fn fetch_verified_listing_rejects_tampered_body() {
        let cli = gzip_tar_single("php", b"CLI-BYTES", 0o755);
        let fpm = gzip_tar_single("php-fpm", b"FPM-BYTES", 0o755);
        let signed = signed_manifest_for("8.5.7", "8.5", 1, &cli, &fpm);
        let dl = FakeDownloader {
            manifest: signed.manifest.replace("8.5.7", "8.5.9"),
            minisig: signed.minisig.clone(),
            cli,
            fpm,
        };
        let err = fetch_verified_listing(&dl, &signed.public_key, yerd_php::Channel::Stable)
            .await
            .unwrap_err();
        assert!(matches!(err, PhpError::ListingUntrusted), "got {err:?}");
    }

    /// A legacy (7.4) build installs from the `php-legacy.json` channel into
    /// `php-7.4/bin/php`, exercising the channel-aware `install` path.
    #[tokio::test]
    async fn install_legacy_version_lands_in_versioned_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let dirs = dirs_in(tmp.path());
        let cli = gzip_tar_single("php", b"LEGACY-CLI", 0o755);
        let fpm = gzip_tar_single("php-fpm", b"LEGACY-FPM", 0o755);
        let signed = signed_manifest_for("7.4.33", "7.4", 1, &cli, &fpm);
        let dl = FakeDownloader {
            manifest: signed.manifest.clone(),
            minisig: signed.minisig.clone(),
            cli,
            fpm,
        };
        install(PhpVersion::new(7, 4), &dirs, &dl, &signed.public_key, None)
            .await
            .unwrap();
        assert!(dirs
            .data
            .join("php")
            .join("php-7.4")
            .join("bin")
            .join("php")
            .exists());
    }

    #[tokio::test]
    async fn install_errors_when_version_not_published_and_writes_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        let dirs = dirs_in(tmp.path());
        let cli = gzip_tar_single("php", b"x", 0o755);
        let fpm = gzip_tar_single("php-fpm", b"y", 0o755);
        let signed = signed_manifest_for("8.4.21", "8.4", 1, &cli, &fpm);
        let dl = FakeDownloader {
            manifest: signed.manifest.clone(),
            minisig: signed.minisig.clone(),
            cli,
            fpm,
        };
        let err = install(PhpVersion::new(8, 5), &dirs, &dl, &signed.public_key, None)
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

    #[test]
    fn cli_phprc_prefers_per_version_then_base_then_none() {
        let tmp = tempfile::tempdir().unwrap();
        let dirs = PlatformDirs {
            config: tmp.path().join("c"),
            data: tmp.path().join("d"),
            state: tmp.path().join("s"),
            cache: tmp.path().join("ca"),
            runtime: tmp.path().join("r"),
        };
        std::fs::create_dir_all(&dirs.data).unwrap();
        let v = PhpVersion::new(8, 5);

        assert_eq!(cli_phprc(&dirs, v), None);

        let base = dirs.data.join("php-cli.ini");
        std::fs::write(&base, "; base\n").unwrap();
        assert_eq!(cli_phprc(&dirs, v), Some(base.clone()));

        let per_version = dirs.data.join("php-cli-8.5.ini");
        std::fs::write(&per_version, "; per-version\n").unwrap();
        assert_eq!(cli_phprc(&dirs, v), Some(per_version));
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

        reconcile_shims(&dirs, &yerd_bin).unwrap();

        assert!(bin.join("php8.4cover").exists());
        assert!(bin.join("phpcover").exists());
        assert_eq!(std::fs::read_link(bin.join("php8.4")).unwrap(), yerd_bin);
        assert_eq!(std::fs::read_link(bin.join("php")).unwrap(), yerd_bin);
        assert!(!bin.join("php8.2cover").exists());
        assert!(bin.join("keep.txt").exists());
    }

    #[cfg(unix)]
    #[test]
    fn reconcile_gives_legacy_a_version_shim_but_no_cover_shim() {
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
        mk(PhpVersion::new(7, 4));
        mk(PhpVersion::new(8, 4));

        let bin = shim_dir(&dirs);
        std::fs::create_dir_all(&bin).unwrap();
        // A stale legacy cover shim (from an older yerd) must be pruned even
        // though 7.4 is installed.
        std::os::unix::fs::symlink(&yerd_bin, bin.join("php7.4cover")).unwrap();

        reconcile_shims(&dirs, &yerd_bin).unwrap();

        assert!(
            bin.join("php7.4").exists(),
            "legacy version shim is created"
        );
        assert!(
            !bin.join("php7.4cover").exists(),
            "no cover shim for a legacy version, and a stale one is pruned"
        );
        assert!(bin.join("php8.4cover").exists(), "stable cover shim stays");
    }

    #[cfg(unix)]
    #[test]
    fn reconcile_points_php_at_wrapper_regardless_of_config_default() {
        let tmp = tempfile::tempdir().unwrap();
        let dirs = dirs_in(tmp.path());
        let yerd_bin = tmp.path().join("yerd");
        std::fs::write(&yerd_bin, b"#!fake").unwrap();
        let base = dirs.data.join("php").join("php-8.4");
        std::fs::create_dir_all(base.join("bin")).unwrap();
        std::fs::create_dir_all(base.join("sbin")).unwrap();
        std::fs::write(base.join("bin").join("php"), b"cli").unwrap();
        std::fs::write(base.join("sbin").join("php-fpm"), b"fpm").unwrap();

        reconcile_shims(&dirs, &yerd_bin).unwrap();

        assert_eq!(
            std::fs::read_link(shim_dir(&dirs).join("php")).unwrap(),
            yerd_bin
        );
        assert!(shim_dir(&dirs).join("php8.4cover").exists());
    }
}
