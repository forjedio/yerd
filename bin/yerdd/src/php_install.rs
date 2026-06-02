//! PHP version install: download prebuilt static builds and unpack them into
//! yerd's data dir.
//!
//! The `reqwest`-backed [`Downloader`] lives here (a binary) so `yerd-php`
//! stays dependency-light. Version resolution + tar-member safety are pure
//! helpers from `yerd_php::release`; this module is the I/O edge: fetch the
//! listing → resolve → fetch tarballs → safe-extract the single binary →
//! atomic install. Integrity is TLS-only (no sha pinning — per user decision).

use std::io::Read;
use std::path::{Path, PathBuf};

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

impl ReqwestDownloader {
    /// Construct a fresh client.
    #[must_use]
    pub fn new() -> Self {
        Self {
            client: reqwest::Client::new(),
        }
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
        let transport = |e: reqwest::Error| DownloadError::Transport {
            url: url.to_owned(),
            reason: e.to_string(),
        };
        let resp = self
            .client
            .get(url)
            .send()
            .await
            .map_err(transport)?
            .error_for_status()
            .map_err(transport)?;
        let bytes = resp.bytes().await.map_err(transport)?;
        Ok(bytes.to_vec())
    }
}

/// Install `version` (major.minor) into `dirs.data/php/php-<minor>/`.
///
/// Resolves the latest patch from the distribution's live listing, downloads
/// the CLI and FPM tarballs, safely extracts the single binary from each, and
/// atomically swaps the result into place. Idempotent: reinstalling replaces
/// the dir. **Integrity is TLS-only** — the distribution publishes no checksum
/// sidecars and yerd does not pin hashes (deliberate; see `yerd_php::release`).
pub async fn install(
    version: PhpVersion,
    dirs: &PlatformDirs,
    dl: &dyn Downloader,
) -> Result<(), PhpError> {
    let (os, arch) = current_os_arch()?;
    let listing = dl.download(&yerd_php::listing_url()).await?;
    let listing = String::from_utf8_lossy(&listing);
    let artifact = yerd_php::resolve_from_listing(&listing, version, os, arch)?;

    let php_root = dirs.data.join("php");
    fs_ctx(std::fs::create_dir_all(&php_root), &php_root)?;

    let staging = php_root.join(format!(
        ".staging-{}-{}",
        artifact.install_dir_name,
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&staging); // clear any stale staging

    // Stage both binaries; on any failure, clean up and propagate.
    if let Err(e) = stage(&artifact, dl, &staging).await {
        let _ = std::fs::remove_dir_all(&staging);
        return Err(e);
    }

    let final_dir = php_root.join(&artifact.install_dir_name);
    if final_dir.exists() {
        if let Err(e) = std::fs::remove_dir_all(&final_dir) {
            let _ = std::fs::remove_dir_all(&staging);
            return Err(fs_err(&final_dir, &e));
        }
    }
    fs_ctx(std::fs::rename(&staging, &final_dir), &final_dir)
}

/// Filename of the installed-patch marker inside a per-version dir.
const VERSION_MARKER: &str = ".yerd-version";

async fn stage(artifact: &Artifact, dl: &dyn Downloader, staging: &Path) -> Result<(), PhpError> {
    fetch_and_extract(dl, &artifact.cli_url, BinaryKind::Cli, staging).await?;
    fetch_and_extract(dl, &artifact.fpm_url, BinaryKind::Fpm, staging).await?;
    // Record the exact patch *in the staging dir* so it lands atomically with
    // the binaries on rename (update-checks read it back).
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

async fn fetch_and_extract(
    dl: &dyn Downloader,
    url: &str,
    kind: BinaryKind,
    staging: &Path,
) -> Result<(), PhpError> {
    let bytes = dl.download(url).await?; // -> PhpError::Download via #[from]
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
/// names, and duplicates — closes zip-slip and link-target escapes.
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

/// Point the managed `php` shim at `version`'s CLI binary (unix symlink),
/// created/replaced atomically. Returns the shim dir for PATH hints.
#[cfg(unix)]
pub fn set_default_shim(dirs: &PlatformDirs, version: PhpVersion) -> Result<PathBuf, PhpError> {
    let bin = shim_dir(dirs);
    fs_ctx(std::fs::create_dir_all(&bin), &bin)?;
    let link = bin.join("php");
    let tmp = bin.join(format!(".php.tmp-{}", std::process::id()));
    let _ = std::fs::remove_file(&tmp);
    fs_ctx(
        std::os::unix::fs::symlink(cli_binary_path(dirs, version), &tmp),
        &tmp,
    )?;
    // rename is atomic and replaces any existing shim.
    fs_ctx(std::fs::rename(&tmp, &link), &link)?;
    Ok(bin)
}

#[cfg(not(unix))]
pub fn set_default_shim(dirs: &PlatformDirs, _version: PhpVersion) -> Result<PathBuf, PhpError> {
    Ok(shim_dir(dirs))
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

        install(PhpVersion::new(8, 5), &dirs, &dl).await.unwrap();

        let base = dirs.data.join("php").join("php-8.5");
        assert_eq!(
            std::fs::read(base.join("bin").join("php")).unwrap(),
            b"CLI-BYTES"
        );
        assert_eq!(
            std::fs::read(base.join("sbin").join("php-fpm")).unwrap(),
            b"FPM-BYTES"
        );
        // The version marker records the resolved patch (latest in the listing).
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
        // Listing has only 8.4, not the requested 8.5.
        let listing = format!("php-8.4.21-cli-{}-{}.tar.gz", os.as_str(), arch.as_str());
        let dl = FakeDownloader {
            listing,
            cli: vec![],
            fpm: vec![],
        };
        let err = install(PhpVersion::new(8, 5), &dirs, &dl)
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
}
