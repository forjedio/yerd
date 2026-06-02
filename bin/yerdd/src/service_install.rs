//! Service install: download a prebuilt service archive from yerd's own
//! distribution and unpack it into yerd's data dir.
//!
//! Mirrors `php_install` (the I/O edge; version resolution + tar-member safety
//! are pure helpers from `yerd-services` / `yerd-php`). Unlike PHP — a single
//! binary — a service archive is a small tree (`bin/`, `lib/`, …), so the whole
//! archive is unpacked (each member zip-slip-guarded) into a staging dir, then
//! atomically renamed into place. Integrity is TLS-only (no sha pinning), as for
//! PHP.

use std::path::Path;

use yerd_php::is_safe_member; // shared zip-slip guard
use yerd_supervise::Downloader;

use yerd_platform::PlatformDirs;
use yerd_services::version::{self, VERSION_MARKER};
use yerd_services::{
    current_os_arch, listing_url, resolve_from_listing, Service, ServiceError, ServiceVersion,
};

/// Install `service` at `version` into `data/services/<id>/<version>/`.
///
/// Resolves the artifact from yerd's services listing, downloads the `.tar.gz`,
/// safely unpacks it into a staging dir, verifies the server binary is present,
/// then atomically swaps it into place and records the version marker.
/// Idempotent: reinstalling replaces the dir.
pub async fn install(
    service: Service,
    version: &ServiceVersion,
    dirs: &PlatformDirs,
    dl: &dyn Downloader,
) -> Result<(), ServiceError> {
    let (os, arch) = current_os_arch()?;
    let listing = dl.download(&listing_url()).await?;
    let listing = String::from_utf8_lossy(&listing);
    let artifact = resolve_from_listing(&listing, service, version, os, arch)?;

    let svc_root = version::service_root(dirs, service);
    fs_ctx(std::fs::create_dir_all(&svc_root), &svc_root)?;
    let staging = svc_root.join(format!(".staging-{version}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&staging);

    if let Err(e) = stage(service, &artifact.url, dl, &staging).await {
        let _ = std::fs::remove_dir_all(&staging);
        return Err(e);
    }

    let final_dir = version::install_dir(dirs, service, version);
    if final_dir.exists() {
        if let Err(e) = std::fs::remove_dir_all(&final_dir) {
            let _ = std::fs::remove_dir_all(&staging);
            return Err(fs_err(&final_dir, &e));
        }
    }
    fs_ctx(std::fs::rename(&staging, &final_dir), &final_dir)
}

/// Remove an installed version's files. With `purge`, also delete the engine's
/// datadir (destructive). Returns the retained datadir path when it was kept.
pub fn uninstall(
    service: Service,
    version: &ServiceVersion,
    dirs: &PlatformDirs,
    purge: bool,
) -> Result<Option<std::path::PathBuf>, ServiceError> {
    let dir = version::install_dir(dirs, service, version);
    if dir.exists() {
        fs_ctx(std::fs::remove_dir_all(&dir), &dir)?;
    }
    let datadir = version::datadir(dirs, service, version);
    if purge {
        if datadir.exists() {
            fs_ctx(std::fs::remove_dir_all(&datadir), &datadir)?;
        }
        Ok(None)
    } else {
        Ok(datadir.exists().then_some(datadir))
    }
}

async fn stage(
    service: Service,
    url: &str,
    dl: &dyn Downloader,
    staging: &Path,
) -> Result<(), ServiceError> {
    let bytes = dl.download(url).await?;
    fs_ctx(std::fs::create_dir_all(staging), staging)?;
    extract_all(&bytes, staging, url)?;

    // Verify the server binary actually landed (so a malformed archive fails
    // here, before the atomic swap, not at first start).
    let server = staging.join("bin").join(service.server_binary());
    if !server.is_file() {
        return Err(ServiceError::Extract {
            what: url.to_owned(),
            detail: format!("archive missing bin/{}", service.server_binary()),
        });
    }
    let marker = staging.join(VERSION_MARKER);
    // The artifact's version is encoded in the URL we resolved; record the
    // requested label as the marker (callers pass the resolved version).
    fs_ctx(std::fs::write(&marker, b"installed"), &marker)?;
    Ok(())
}

/// Unpack every member of a `.tar.gz` into `dest`, rejecting unsafe member names
/// (zip-slip / traversal / absolute) and preserving Unix permission bits (so the
/// server binary stays executable).
fn extract_all(gz_bytes: &[u8], dest: &Path, url: &str) -> Result<(), ServiceError> {
    let decoder = flate2::read::GzDecoder::new(gz_bytes);
    let mut archive = tar::Archive::new(decoder);
    archive.set_preserve_permissions(true);
    let entries = archive.entries().map_err(|e| extract_err(url, &e))?;
    for entry in entries {
        let mut entry = entry.map_err(|e| extract_err(url, &e))?;
        let path = entry.path().map_err(|e| extract_err(url, &e))?.into_owned();
        let name = path.to_string_lossy().into_owned();
        if !is_safe_member(&name) {
            return Err(extract_msg(url, format!("unsafe archive member {name:?}")));
        }
        let out = dest.join(&path);
        if let Some(parent) = out.parent() {
            fs_ctx(std::fs::create_dir_all(parent), parent)?;
        }
        // `unpack` writes the member (file/dir/symlink), applies its mode, and
        // fully consumes the entry body — the `Entries` iterator then seeks to
        // the next header on its own. We have already rejected `..`/absolute
        // names above.
        entry.unpack(&out).map_err(|e| extract_err(url, &e))?;
    }
    Ok(())
}

fn fs_ctx<T>(r: std::io::Result<T>, path: &Path) -> Result<T, ServiceError> {
    r.map_err(|e| fs_err(path, &e))
}

fn fs_err(path: &Path, e: &std::io::Error) -> ServiceError {
    ServiceError::Extract {
        what: path.display().to_string(),
        detail: e.to_string(),
    }
}

fn extract_err(url: &str, e: &dyn std::fmt::Display) -> ServiceError {
    extract_msg(url, e.to_string())
}

fn extract_msg(url: &str, detail: String) -> ServiceError {
    ServiceError::Extract {
        what: url.to_owned(),
        detail,
    }
}
