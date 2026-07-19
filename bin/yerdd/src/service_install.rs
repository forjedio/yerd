//! Service install: download a prebuilt service archive from yerd's own
//! distribution and unpack it into yerd's data dir.
//!
//! Mirrors `php_install` (the I/O edge; version resolution + tar-member safety
//! are pure helpers from `yerd-services` / `yerd-php`). Unlike PHP - a single
//! binary - a service archive is a small tree (`bin/`, `lib/`, …), so the whole
//! archive is unpacked (each member zip-slip-guarded) into a staging dir, then
//! atomically renamed into place. Integrity is TLS-only (no sha pinning), as for
//! PHP.

use std::path::Path;

use yerd_php::is_safe_member;
use yerd_supervise::Downloader;

use yerd_platform::PlatformDirs;
use yerd_services::version::{self, VERSION_MARKER};
use yerd_services::{
    current_os_arch, listing_url, resolve_from_listing, DatadirScope, ServiceError, ServiceVersion,
};

/// Install `service_id` at `version` into `data/services/<id>/<version>/`.
///
/// Resolves the artifact from yerd's services listing, downloads the `.tar.gz`,
/// safely unpacks it into a staging dir, verifies the server binary is present,
/// then atomically swaps it into place and records the version marker.
/// Idempotent: reinstalling replaces the dir.
///
/// `server_binary` is the expected `bin/<name>` of the type's server; a
/// versioned type that reaches install always has one, so `None` is rejected.
pub async fn install(
    service_id: &str,
    server_binary: Option<&str>,
    version: &ServiceVersion,
    dirs: &PlatformDirs,
    dl: &dyn Downloader,
) -> Result<(), ServiceError> {
    let server_binary = server_binary.ok_or_else(|| ServiceError::Unsupported {
        service: service_id.to_owned(),
        detail: "type has no server binary".into(),
    })?;

    let (os, arch) = current_os_arch()?;
    let listing = dl.download(&listing_url()).await?;
    let listing = String::from_utf8_lossy(&listing);
    let artifact = resolve_from_listing(&listing, service_id, version, os, arch)?;

    let svc_root = version::service_root(dirs, service_id);
    fs_ctx(std::fs::create_dir_all(&svc_root), &svc_root)?;
    let staging = svc_root.join(format!(".staging-{version}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&staging);

    if let Err(e) = stage(server_binary, &artifact.url, dl, &staging).await {
        let _ = std::fs::remove_dir_all(&staging);
        return Err(e);
    }

    let final_dir = version::install_dir(dirs, service_id, version);
    if final_dir.exists() {
        if let Err(e) = std::fs::remove_dir_all(&final_dir) {
            let _ = std::fs::remove_dir_all(&staging);
            return Err(fs_err(&final_dir, &e));
        }
    }
    fs_ctx(std::fs::rename(&staging, &final_dir), &final_dir)
}

/// Remove an installed version's files. With `purge`, also delete the engine's
/// stored data (destructive). For exact-version-scoped services this removes
/// the target store plus orphaned `data-*` stores, while preserving stores
/// owned by other versions whose expected server binary is still installed.
/// Returns the selected version's retained datadir path when data was kept.
///
/// `datadir_scope` selects the engine's compatibility layout.
pub fn uninstall(
    service_id: &str,
    server_binary: Option<&str>,
    datadir_scope: DatadirScope,
    version: &ServiceVersion,
    dirs: &PlatformDirs,
    purge: bool,
) -> Result<Option<std::path::PathBuf>, ServiceError> {
    let dir = version::install_dir(dirs, service_id, version);
    if dir.exists() {
        fs_ctx(std::fs::remove_dir_all(&dir), &dir)?;
    }
    let datadir = version::datadir(dirs, service_id, datadir_scope, version);
    if purge {
        if matches!(datadir_scope, DatadirScope::Version) {
            purge_version_datadirs(dirs, service_id, server_binary, version)?;
        } else if datadir.exists() {
            fs_ctx(std::fs::remove_dir_all(&datadir), &datadir)?;
        }
        Ok(None)
    } else {
        Ok(datadir.exists().then_some(datadir))
    }
}

/// Delete only exact-version datadirs under this service root. Install dirs,
/// staging dirs, notices, and unrelated entries are deliberately untouched.
fn purge_version_datadirs(
    dirs: &PlatformDirs,
    service_id: &str,
    server_binary: Option<&str>,
    target_version: &ServiceVersion,
) -> Result<(), ServiceError> {
    let root = version::service_root(dirs, service_id);
    let entries = match std::fs::read_dir(&root) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(fs_err(&root, &e)),
    };
    for entry in entries {
        let entry = entry.map_err(|e| fs_err(&root, &e))?;
        let name = entry.file_name();
        let Some(suffix) = name.to_str().and_then(|name| name.strip_prefix("data-")) else {
            continue;
        };
        if !entry
            .file_type()
            .map_err(|e| fs_err(&entry.path(), &e))?
            .is_dir()
        {
            continue;
        }
        let owned_by_other_install = suffix != target_version.as_str()
            && server_binary.is_some_and(|binary| {
                suffix
                    .parse::<ServiceVersion>()
                    .is_ok_and(|installed_version| {
                        version::server_path(dirs, service_id, binary, &installed_version).is_file()
                    })
            });
        if !owned_by_other_install {
            let path = entry.path();
            fs_ctx(std::fs::remove_dir_all(&path), &path)?;
        }
    }
    Ok(())
}

async fn stage(
    server_binary: &str,
    url: &str,
    dl: &dyn Downloader,
    staging: &Path,
) -> Result<(), ServiceError> {
    let bytes = dl.download(url).await?;
    fs_ctx(std::fs::create_dir_all(staging), staging)?;
    extract_all(&bytes, staging, url)?;

    let server = staging.join("bin").join(server_binary);
    if !server.is_file() {
        return Err(ServiceError::Extract {
            what: url.to_owned(),
            detail: format!("archive missing bin/{server_binary}"),
        });
    }
    let marker = staging.join(VERSION_MARKER);
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

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn dirs_in(root: &Path) -> PlatformDirs {
        PlatformDirs {
            config: root.join("config"),
            data: root.join("data"),
            state: root.join("state"),
            cache: root.join("cache"),
            runtime: root.join("runtime"),
        }
    }

    #[test]
    fn version_scoped_purge_removes_all_retained_datadirs_only() {
        let tmp = tempfile::tempdir().unwrap();
        let dirs = dirs_in(tmp.path());
        let root = version::service_root(&dirs, "meilisearch");
        for name in ["data-1.10", "data-1.11", "data-1.12", "1.12", ".staging-x"] {
            std::fs::create_dir_all(root.join(name)).unwrap();
        }
        std::fs::write(root.join("data-not-a-directory"), b"keep").unwrap();

        uninstall(
            "meilisearch",
            Some("meilisearch"),
            DatadirScope::Version,
            &"1.12".parse().unwrap(),
            &dirs,
            true,
        )
        .unwrap();

        for name in ["data-1.10", "data-1.11", "data-1.12", "1.12"] {
            assert!(!root.join(name).exists(), "{name} should be removed");
        }
        assert!(root.join(".staging-x").is_dir());
        assert!(root.join("data-not-a-directory").is_file());
    }

    #[test]
    fn version_scoped_uninstall_without_purge_retains_every_datadir() {
        let tmp = tempfile::tempdir().unwrap();
        let dirs = dirs_in(tmp.path());
        let root = version::service_root(&dirs, "meilisearch");
        for name in ["data-1.11", "data-1.12", "1.12"] {
            std::fs::create_dir_all(root.join(name)).unwrap();
        }

        let retained = uninstall(
            "meilisearch",
            Some("meilisearch"),
            DatadirScope::Version,
            &"1.12".parse().unwrap(),
            &dirs,
            false,
        )
        .unwrap();

        assert_eq!(retained, Some(root.join("data-1.12")));
        assert!(root.join("data-1.11").is_dir());
        assert!(root.join("data-1.12").is_dir());
    }

    #[test]
    fn version_scoped_purge_preserves_datadir_owned_by_another_install() {
        let tmp = tempfile::tempdir().unwrap();
        let dirs = dirs_in(tmp.path());
        let root = version::service_root(&dirs, "meilisearch");
        for version in ["1.11", "1.12"] {
            std::fs::create_dir_all(root.join(version).join("bin")).unwrap();
            std::fs::write(root.join(version).join("bin/meilisearch"), b"binary").unwrap();
            std::fs::create_dir_all(root.join(format!("data-{version}"))).unwrap();
        }
        std::fs::create_dir_all(root.join("data-1.10")).unwrap();

        uninstall(
            "meilisearch",
            Some("meilisearch"),
            DatadirScope::Version,
            &"1.11".parse().unwrap(),
            &dirs,
            true,
        )
        .unwrap();

        assert!(!root.join("1.11").exists());
        assert!(!root.join("data-1.11").exists());
        assert!(
            !root.join("data-1.10").exists(),
            "orphan should be reclaimed"
        );
        assert!(root.join("1.12/bin/meilisearch").is_file());
        assert!(
            root.join("data-1.12").is_dir(),
            "live version data must survive"
        );
    }
}
