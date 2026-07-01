//! `cloudflared` binary installer (the Cloudflare Tunnel prerequisite).
//!
//! Fetches the official Apache-2.0 static binary from Cloudflare's GitHub
//! releases on demand and installs it under `{data}/tunnel/bin/cloudflared`.
//! Deliberately NOT part of `tools::Tool`: `cloudflared` is daemon-internal (no
//! user-`PATH` shim) and its install layout differs (a single binary, not a
//! Yerd-distribution tarball), so it gets its own module + atomic swap rather
//! than reusing the `Tool`-keyed `stage_and_swap`.
//!
//! Integrity is **fail-closed**. `cloudflared` publishes no `SHASUMS` sidecar, so
//! the download is verified against, in order of preference: (1) a compiled-in
//! pinned `(version, asset) → sha256` entry in [`PINNED_SHA256`] when one exists,
//! which is authoritative; otherwise (2) the per-asset `digest` (`sha256:…`) the
//! GitHub Releases API reports. If neither is available the install is refused
//! rather than trusting TLS alone. The asset URL must also resolve to a GitHub
//! host (see [`host_is_trusted`]) so a tampered metadata response cannot redirect
//! the fetch to an attacker-controlled origin.

use std::path::{Path, PathBuf};

use serde::Deserialize;
use yerd_php::{current_os_arch, Arch, Downloader, Os};
use yerd_platform::PlatformDirs;

use super::ProgressTx;

/// Latest-release metadata endpoint for the `cloudflared` repo.
const LATEST_RELEASE_API: &str =
    "https://api.github.com/repos/cloudflare/cloudflared/releases/latest";

/// Authoritative `(release tag, asset name, lowercase sha256-hex)` pins.
///
/// When the resolved release+asset matches an entry here, that hash is the
/// primary integrity source and a downloaded binary MUST match it. Empty by
/// default: a maintainer pins a known-good release by appending its tag, asset
/// filename, and `sha256sum` here, after which that release installs
/// reproducibly even if GitHub's per-asset digest is absent or changes. Releases
/// not listed fall back to the GitHub per-asset digest (still fail-closed).
const PINNED_SHA256: &[(&str, &str, &str)] = &[];

/// The pinned sha256 for a `(tag, asset)`, if one is compiled in.
fn pinned_sha256(tag: &str, asset: &str) -> Option<&'static str> {
    PINNED_SHA256
        .iter()
        .find(|(t, a, _)| *t == tag && *a == asset)
        .map(|(_, _, sha)| *sha)
}

/// Whether `url` is an `https` URL whose host is GitHub's (`github.com` or a
/// `*.githubusercontent.com` asset host). The asset URL comes from the release
/// JSON, so this stops a tampered response from redirecting the fetch elsewhere.
fn host_is_trusted(url: &str) -> bool {
    let Some(rest) = url.strip_prefix("https://") else {
        return false;
    };
    let authority = rest.split('/').next().unwrap_or(rest);
    let hostport = authority.rsplit('@').next().unwrap_or(authority);
    let host = hostport.split(':').next().unwrap_or(hostport);
    host.eq_ignore_ascii_case("github.com")
        || host.eq_ignore_ascii_case("githubusercontent.com")
        || host
            .to_ascii_lowercase()
            .ends_with(".githubusercontent.com")
}

/// Failure modes of a `cloudflared` install.
#[derive(Debug, thiserror::Error)]
pub enum CloudflaredInstallError {
    /// No prebuilt `cloudflared` is published for this OS/arch.
    #[error("cloudflared is not available for this platform")]
    UnsupportedHost,
    /// Network / HTTP failure fetching the release metadata or binary.
    #[error("download failed: {0}")]
    Download(String),
    /// The release JSON could not be parsed, or the expected asset was absent.
    #[error("release metadata error: {0}")]
    Metadata(String),
    /// The downloaded artifact's SHA-256 did not match the published digest.
    #[error("integrity check failed: {0}")]
    Sha256Mismatch(String),
    /// No pinned hash and no published digest were available to verify against,
    /// so the install was refused (integrity is fail-closed).
    #[error("refusing to install unverified cloudflared: {0}")]
    MissingDigest(String),
    /// The asset download URL did not resolve to a trusted GitHub host.
    #[error("refusing to download from untrusted host: {0}")]
    UntrustedHost(String),
    /// Unpacking the macOS `.tgz` failed or its layout was unexpected.
    #[error("unpack failed: {0}")]
    Unpack(String),
    /// A filesystem operation failed.
    #[error("{0}")]
    Io(String),
}

/// One asset in a GitHub release.
#[derive(Debug, Deserialize)]
struct Asset {
    name: String,
    browser_download_url: String,
    /// `"sha256:<hex>"` when GitHub has computed it; absent on older releases.
    #[serde(default)]
    digest: Option<String>,
}

/// The subset of a GitHub release we read.
#[derive(Debug, Deserialize)]
struct Release {
    tag_name: String,
    assets: Vec<Asset>,
}

/// `{data}/tunnel`.
pub(crate) fn tunnel_dir(dirs: &PlatformDirs) -> PathBuf {
    dirs.data.join("tunnel")
}

/// `{data}/tunnel/bin/cloudflared`.
pub(crate) fn binary_path(dirs: &PlatformDirs) -> PathBuf {
    tunnel_dir(dirs).join("bin").join("cloudflared")
}

/// `{data}/tunnel/.cloudflared-version`.
fn version_marker(dirs: &PlatformDirs) -> PathBuf {
    tunnel_dir(dirs).join(".cloudflared-version")
}

/// Whether the `cloudflared` binary is installed.
#[must_use]
pub fn is_installed(dirs: &PlatformDirs) -> bool {
    binary_path(dirs).is_file()
}

/// The installed `cloudflared` version from its marker, or `None`.
#[must_use]
pub fn installed_version(dirs: &PlatformDirs) -> Option<String> {
    let v = std::fs::read_to_string(version_marker(dirs)).ok()?;
    let v = v.trim().to_owned();
    (!v.is_empty()).then_some(v)
}

/// The `(asset_filename, is_tgz)` for the host.
///
/// macOS ships a `.tgz` wrapping a single `cloudflared`; Linux ships a bare
/// ungzipped executable. Cloudflare uses `amd64`/`arm64` arch tokens (not the
/// `x86_64`/`aarch64` that `yerd-php`'s `Arch::as_str` renders), so the mapping
/// is explicit here.
fn host_asset(os: Os, arch: Arch) -> (String, bool) {
    let token = match arch {
        Arch::X86_64 => "amd64",
        Arch::Aarch64 => "arm64",
    };
    match os {
        Os::Macos => (format!("cloudflared-darwin-{token}.tgz"), true),
        Os::Linux => (format!("cloudflared-linux-{token}"), false),
    }
}

/// Emit one progress line if a sink is attached.
fn note(progress: Option<&ProgressTx>, msg: impl Into<String>) {
    if let Some(tx) = progress {
        let _ = tx.send(msg.into());
    }
}

/// Download + install the latest `cloudflared` for the host. Idempotent
/// (replaces the binary in place via a staging file + atomic rename).
pub async fn install(
    dirs: &PlatformDirs,
    dl: &dyn Downloader,
    progress: Option<&ProgressTx>,
) -> Result<(), CloudflaredInstallError> {
    let (os, arch) = current_os_arch().map_err(|_| CloudflaredInstallError::UnsupportedHost)?;
    let (asset_name, is_tgz) = host_asset(os, arch);

    note(progress, "Fetching cloudflared release info…");
    let meta = dl
        .download(LATEST_RELEASE_API)
        .await
        .map_err(|e| CloudflaredInstallError::Download(format!("release metadata: {e}")))?;
    let release: Release = serde_json::from_slice(&meta)
        .map_err(|e| CloudflaredInstallError::Metadata(format!("parse release json: {e}")))?;
    let asset = release
        .assets
        .iter()
        .find(|a| a.name == asset_name)
        .ok_or_else(|| {
            CloudflaredInstallError::Metadata(format!("no asset {asset_name} in latest release"))
        })?;

    if !host_is_trusted(&asset.browser_download_url) {
        return Err(CloudflaredInstallError::UntrustedHost(
            asset.browser_download_url.clone(),
        ));
    }

    note(progress, format!("Downloading {asset_name}…"));
    let bytes = dl
        .download(&asset.browser_download_url)
        .await
        .map_err(|e| CloudflaredInstallError::Download(format!("{asset_name}: {e}")))?;

    verify_integrity(
        &bytes,
        pinned_sha256(&release.tag_name, &asset_name),
        asset.digest.as_deref(),
        &asset_name,
    )?;

    let binary = if is_tgz {
        note(progress, "Extracting…");
        extract_cloudflared_from_tgz(&bytes)?
    } else {
        bytes
    };

    install_binary(dirs, &release.tag_name, &binary)?;
    note(
        progress,
        format!("Installed cloudflared {}", release.tag_name),
    );
    tracing::info!(version = %release.tag_name, "installed cloudflared");
    Ok(())
}

/// Verify `bytes`, fail-closed. A compiled-in `pinned` hash is authoritative
/// when present (and the GitHub `digest`, if any, is cross-checked against it);
/// otherwise the GitHub `digest` is required and must match. With neither, the
/// install is refused rather than trusting TLS alone.
fn verify_integrity(
    bytes: &[u8],
    pinned: Option<&str>,
    digest: Option<&str>,
    label: &str,
) -> Result<(), CloudflaredInstallError> {
    let got = yerd_update::sha256_hex(bytes);
    let github = digest.and_then(|d| d.strip_prefix("sha256:"));
    if let Some(want) = pinned {
        if !got.eq_ignore_ascii_case(want) {
            return Err(CloudflaredInstallError::Sha256Mismatch(format!(
                "{label}: got {got}, want pinned {want}"
            )));
        }
        if let Some(gh) = github {
            if !got.eq_ignore_ascii_case(gh) {
                return Err(CloudflaredInstallError::Sha256Mismatch(format!(
                    "{label}: pinned hash and GitHub digest disagree (github {gh})"
                )));
            }
        }
        return Ok(());
    }
    let Some(want) = github else {
        return Err(CloudflaredInstallError::MissingDigest(format!(
            "{label}: no pinned hash and no published digest"
        )));
    };
    if got.eq_ignore_ascii_case(want) {
        Ok(())
    } else {
        Err(CloudflaredInstallError::Sha256Mismatch(format!(
            "{label}: got {got}, want {want}"
        )))
    }
}

/// Pull the single `cloudflared` executable out of a macOS `.tgz`.
fn extract_cloudflared_from_tgz(gz_bytes: &[u8]) -> Result<Vec<u8>, CloudflaredInstallError> {
    use std::io::Read as _;
    let decoder = flate2::read::GzDecoder::new(gz_bytes);
    let mut archive = tar::Archive::new(decoder);
    let entries = archive
        .entries()
        .map_err(|e| CloudflaredInstallError::Unpack(e.to_string()))?;
    for entry in entries {
        let mut entry = entry.map_err(|e| CloudflaredInstallError::Unpack(e.to_string()))?;
        let path = entry
            .path()
            .map_err(|e| CloudflaredInstallError::Unpack(e.to_string()))?
            .into_owned();
        let is_cloudflared = path.file_name().is_some_and(|n| n == "cloudflared");
        if is_cloudflared && entry.header().entry_type().is_file() {
            let mut buf = Vec::new();
            entry
                .read_to_end(&mut buf)
                .map_err(|e| CloudflaredInstallError::Unpack(e.to_string()))?;
            return Ok(buf);
        }
    }
    Err(CloudflaredInstallError::Unpack(
        "no cloudflared executable in archive".to_owned(),
    ))
}

/// Write `bytes` to `{data}/tunnel/bin/cloudflared` via a staging file + atomic
/// rename, make it executable, and record the version.
fn install_binary(
    dirs: &PlatformDirs,
    version: &str,
    bytes: &[u8],
) -> Result<(), CloudflaredInstallError> {
    let tunnel_root = tunnel_dir(dirs);
    crate::secure_fs::create_private_dir(&tunnel_root)
        .map_err(|e| CloudflaredInstallError::Io(format!("{}: {e}", tunnel_root.display())))?;
    let bin_dir = tunnel_root.join("bin");
    std::fs::create_dir_all(&bin_dir)
        .map_err(|e| CloudflaredInstallError::Io(format!("{}: {e}", bin_dir.display())))?;
    let final_path = binary_path(dirs);
    let staging = bin_dir.join(format!(".cloudflared.staging-{}", std::process::id()));
    let _ = std::fs::remove_file(&staging);
    std::fs::write(&staging, bytes)
        .map_err(|e| CloudflaredInstallError::Io(format!("{}: {e}", staging.display())))?;
    set_executable(&staging)?;
    std::fs::rename(&staging, &final_path).map_err(|e| {
        let _ = std::fs::remove_file(&staging);
        CloudflaredInstallError::Io(format!("{}: {e}", final_path.display()))
    })?;
    let marker = version_marker(dirs);
    std::fs::write(&marker, version)
        .map_err(|e| CloudflaredInstallError::Io(format!("{}: {e}", marker.display())))?;
    Ok(())
}

/// Mark a file `rwxr-xr-x` on Unix; a no-op elsewhere.
#[cfg(unix)]
fn set_executable(path: &Path) -> Result<(), CloudflaredInstallError> {
    use std::os::unix::fs::PermissionsExt as _;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))
        .map_err(|e| CloudflaredInstallError::Io(format!("chmod {}: {e}", path.display())))
}

#[cfg(not(unix))]
fn set_executable(_path: &Path) -> Result<(), CloudflaredInstallError> {
    Ok(())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::panic, clippy::indexing_slicing)]
mod tests {
    use super::*;

    fn dirs_in(tmp: &std::path::Path) -> PlatformDirs {
        PlatformDirs {
            config: tmp.join("c"),
            data: tmp.join("d"),
            state: tmp.join("s"),
            cache: tmp.join("ca"),
            runtime: tmp.join("r"),
        }
    }

    #[test]
    fn host_asset_tokens_are_cloudflares_not_phps() {
        assert_eq!(
            host_asset(Os::Linux, Arch::X86_64),
            ("cloudflared-linux-amd64".to_owned(), false)
        );
        assert_eq!(
            host_asset(Os::Linux, Arch::Aarch64),
            ("cloudflared-linux-arm64".to_owned(), false)
        );
        assert_eq!(
            host_asset(Os::Macos, Arch::Aarch64),
            ("cloudflared-darwin-arm64.tgz".to_owned(), true)
        );
        assert_eq!(
            host_asset(Os::Macos, Arch::X86_64),
            ("cloudflared-darwin-amd64.tgz".to_owned(), true)
        );
    }

    #[test]
    fn verify_integrity_is_fail_closed() {
        let bytes = b"hello cloudflared";
        let hex = yerd_update::sha256_hex(bytes);
        let good = format!("sha256:{hex}");
        assert!(verify_integrity(bytes, None, Some(&good), "x").is_ok());
        assert!(verify_integrity(bytes, None, Some("sha256:deadbeef"), "x").is_err());
        assert!(matches!(
            verify_integrity(bytes, None, None, "x"),
            Err(CloudflaredInstallError::MissingDigest(_))
        ));
        assert!(verify_integrity(bytes, Some(&hex), None, "x").is_ok());
        assert!(verify_integrity(bytes, Some("deadbeef"), Some(&good), "x").is_err());
        assert!(verify_integrity(bytes, Some(&hex), Some("sha256:deadbeef"), "x").is_err());
    }

    #[test]
    fn host_allowlist_rejects_non_github() {
        assert!(host_is_trusted(
            "https://github.com/cloudflare/cloudflared/releases/download/x/cloudflared-linux-amd64"
        ));
        assert!(host_is_trusted(
            "https://release-assets.githubusercontent.com/github-production-release/x"
        ));
        assert!(host_is_trusted("https://objects.githubusercontent.com/x"));
        assert!(!host_is_trusted("https://evil.example.com/cloudflared"));
        assert!(!host_is_trusted("http://github.com/x"));
        assert!(!host_is_trusted("https://github.com.evil.example.com/x"));
        assert!(!host_is_trusted("https://evil.example.com/?u=github.com"));
    }

    #[test]
    fn install_binary_writes_executable_and_marker() {
        let tmp = tempfile::tempdir().unwrap();
        let dirs = dirs_in(tmp.path());
        assert!(!is_installed(&dirs));
        install_binary(&dirs, "2026.6.1", b"#!/bin/sh\n").unwrap();
        assert!(is_installed(&dirs));
        assert_eq!(installed_version(&dirs).as_deref(), Some("2026.6.1"));
        // Reinstall replaces in place.
        install_binary(&dirs, "2026.7.0", b"#!/bin/sh\nv2\n").unwrap();
        assert_eq!(installed_version(&dirs).as_deref(), Some("2026.7.0"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = std::fs::metadata(binary_path(&dirs))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o111, 0o111, "binary should be executable");
        }
    }

    #[test]
    fn extract_pulls_cloudflared_from_tgz() {
        // Build a tiny .tgz containing a `cloudflared` file.
        let mut tar_buf = Vec::new();
        {
            let mut builder = tar::Builder::new(&mut tar_buf);
            let body = b"ELF-ish cloudflared bytes";
            let mut header = tar::Header::new_gnu();
            header.set_path("cloudflared").unwrap();
            header.set_size(body.len() as u64);
            header.set_mode(0o755);
            header.set_entry_type(tar::EntryType::Regular);
            header.set_cksum();
            builder.append(&header, &body[..]).unwrap();
            builder.finish().unwrap();
        }
        let mut gz = Vec::new();
        {
            use std::io::Write as _;
            let mut enc = flate2::write::GzEncoder::new(&mut gz, flate2::Compression::default());
            enc.write_all(&tar_buf).unwrap();
            enc.finish().unwrap();
        }
        let out = extract_cloudflared_from_tgz(&gz).unwrap();
        assert_eq!(out, b"ELF-ish cloudflared bytes");
    }
}
