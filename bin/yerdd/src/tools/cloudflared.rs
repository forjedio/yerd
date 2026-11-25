//! `cloudflared` installer (the Cloudflare Tunnel connector Share uses).
//!
//! Fetches the official Apache-2.0 static binary from Cloudflare's GitHub
//! releases on demand and installs it as `{data}/tools/cloudflared/cloudflared`
//! with the usual `.version` marker and atomic dir swap. Which binary a tunnel
//! actually spawns (this managed copy or one on `PATH`) is decided by
//! `tunnel::install`.
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

use super::{installed_version, note, stage_and_swap, tool_dir, ProgressTx, Tool, ToolError};

/// Latest-release metadata endpoint for the `cloudflared` repo.
const LATEST_RELEASE_API: &str =
    "https://api.github.com/repos/cloudflare/cloudflared/releases/latest";

/// File name of the binary inside the tool dir.
const BINARY: &str = "cloudflared";

/// Version recorded for a migrated install whose legacy marker was missing.
const UNKNOWN_VERSION: &str = "installed";

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
    let authority = rest.split(['/', '?', '#', '\\']).next().unwrap_or(rest);
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

impl From<CloudflaredInstallError> for ToolError {
    fn from(e: CloudflaredInstallError) -> Self {
        match e {
            CloudflaredInstallError::UnsupportedHost => ToolError::UnsupportedHost("cloudflared"),
            CloudflaredInstallError::Download(msg) => ToolError::Download(msg),
            CloudflaredInstallError::Metadata(msg) => {
                ToolError::Download(format!("release metadata error: {msg}"))
            }
            CloudflaredInstallError::Sha256Mismatch(msg) => ToolError::Sha256Mismatch(msg),
            CloudflaredInstallError::MissingDigest(_)
            | CloudflaredInstallError::UntrustedHost(_) => ToolError::Sha256Mismatch(e.to_string()),
            CloudflaredInstallError::Unpack(msg) => ToolError::Unpack(msg),
            CloudflaredInstallError::Io(msg) => ToolError::Io(msg),
        }
    }
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

/// `{data}/tools/cloudflared/cloudflared`, the managed binary.
pub(crate) fn binary_path(dirs: &PlatformDirs) -> PathBuf {
    tool_dir(dirs, Tool::Cloudflared).join(BINARY)
}

/// `{data}/tunnel/bin/cloudflared`, where older daemons installed the binary.
fn legacy_binary_path(dirs: &PlatformDirs) -> PathBuf {
    legacy_bin_dir(dirs).join(BINARY)
}

/// `{data}/tunnel/bin`.
fn legacy_bin_dir(dirs: &PlatformDirs) -> PathBuf {
    crate::tunnel::install::tunnel_dir(dirs).join("bin")
}

/// `{data}/tunnel/.cloudflared-version`, the legacy install's version marker.
fn legacy_marker_path(dirs: &PlatformDirs) -> PathBuf {
    crate::tunnel::install::tunnel_dir(dirs).join(".cloudflared-version")
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

/// The latest published `cloudflared` release tag, for the notify-only update
/// poll. Installs nothing.
pub(crate) async fn latest(dl: &dyn Downloader) -> Result<String, CloudflaredInstallError> {
    let meta = dl
        .download(LATEST_RELEASE_API)
        .await
        .map_err(|e| CloudflaredInstallError::Download(format!("release metadata: {e}")))?;
    let release: Release = serde_json::from_slice(&meta)
        .map_err(|e| CloudflaredInstallError::Metadata(format!("parse release json: {e}")))?;
    Ok(release.tag_name)
}

/// Download + install the latest `cloudflared` for the host into
/// `{data}/tools/cloudflared/`. Idempotent (replaces the tool dir via staging +
/// atomic swap, like every other tool).
pub async fn install(
    dirs: &PlatformDirs,
    dl: &dyn Downloader,
    progress: Option<&ProgressTx>,
) -> Result<(), ToolError> {
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
        return Err(
            CloudflaredInstallError::UntrustedHost(asset.browser_download_url.clone()).into(),
        );
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

    stage_and_swap(dirs, Tool::Cloudflared, &release.tag_name, |staging| {
        write_binary(staging, &binary)
    })?;
    note(
        progress,
        format!("Installed cloudflared {}", release.tag_name),
    );
    tracing::info!(version = %release.tag_name, "installed cloudflared");
    Ok(())
}

/// Write `bytes` as the executable `cloudflared` inside a staging dir.
fn write_binary(staging: &Path, bytes: &[u8]) -> Result<(), ToolError> {
    let dest = staging.join(BINARY);
    std::fs::write(&dest, bytes).map_err(|e| ToolError::Io(format!("{}: {e}", dest.display())))?;
    set_executable(&dest)?;
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

/// Move a `cloudflared` installed by an older daemon at
/// `{data}/tunnel/bin/cloudflared` into `{data}/tools/cloudflared/`, so an
/// upgrade never sees an existing install as missing. `Ok(true)` when a copy was
/// migrated, `Ok(false)` when there was nothing to do (no legacy binary, or a
/// usable managed install already exists, in which case the legacy files are
/// dropped). A managed dir with a version marker but no binary is not usable, so
/// the legacy copy replaces it.
///
/// Must run once at startup before the IPC server opens and before named
/// tunnels resume, the tool shims are reconciled, or the update poll runs: then
/// no tunnel is running and nothing has cached a resolution of the old path.
///
/// It copies rather than renames so a failed swap never loses the only copy; the
/// legacy binary and marker are removed only after the swap succeeds, and
/// `{data}/tunnel/bin` only when it is then empty. Nothing else in
/// `{data}/tunnel` (login cert, credentials, configs) is touched.
///
/// A legacy install without a version marker migrates as `installed`, which is
/// not comparable, so that copy never shows an update notice. Uninstall then
/// Install in Tooling gives a versioned install.
///
/// # Errors
///
/// Staging or swapping the new tool dir failed; the legacy files are left in
/// place for the next start.
pub fn migrate_legacy(dirs: &PlatformDirs) -> Result<bool, ToolError> {
    let legacy = legacy_binary_path(dirs);
    if !legacy.is_file() {
        return Ok(false);
    }
    if installed_version(dirs, Tool::Cloudflared, None).is_some() && binary_path(dirs).is_file() {
        remove_legacy(dirs);
        return Ok(false);
    }
    let version = std::fs::read_to_string(legacy_marker_path(dirs))
        .ok()
        .map(|v| v.trim().to_owned())
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| UNKNOWN_VERSION.to_owned());
    stage_and_swap(dirs, Tool::Cloudflared, &version, |staging| {
        let dest = staging.join(BINARY);
        std::fs::copy(&legacy, &dest)
            .map_err(|e| ToolError::Io(format!("{}: {e}", dest.display())))?;
        set_executable(&dest)?;
        Ok(())
    })?;
    remove_legacy(dirs);
    Ok(true)
}

/// Best-effort removal of the legacy binary, its marker, and an empty
/// `{data}/tunnel/bin`.
fn remove_legacy(dirs: &PlatformDirs) {
    let _ = std::fs::remove_file(legacy_binary_path(dirs));
    let _ = std::fs::remove_file(legacy_marker_path(dirs));
    let _ = std::fs::remove_dir(legacy_bin_dir(dirs));
}

/// Run [`migrate_legacy`] on the blocking pool and log the outcome. Awaited by
/// `run_until_shutdown` before the IPC server is spawned (see the ordering
/// requirement on [`migrate_legacy`]); a failure is logged, never fatal.
pub(crate) async fn migrate_cloudflared_now(dirs: &PlatformDirs) {
    let dirs = dirs.clone();
    match tokio::task::spawn_blocking(move || migrate_legacy(&dirs)).await {
        Ok(Ok(true)) => tracing::info!("moved cloudflared into the managed tools dir"),
        Ok(Ok(false)) => {}
        Ok(Err(e)) => {
            tracing::warn!(error = %e, "could not migrate cloudflared, will retry next start");
        }
        Err(e) => tracing::warn!(error = %e, "cloudflared migration task failed"),
    }
}

/// Test-only helper placing a managed `cloudflared` the same way [`install`]
/// does, for sibling test modules (tunnel cache, supervise, named, updates).
#[cfg(test)]
#[allow(clippy::unwrap_used)]
pub(crate) fn install_binary_for_test(dirs: &PlatformDirs, version: &str, bytes: &[u8]) {
    stage_and_swap(dirs, Tool::Cloudflared, version, |staging| {
        write_binary(staging, bytes)
    })
    .unwrap();
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::panic, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use crate::test_support::{dirs_in, UrlDl};

    fn tgz_with_cloudflared(body: &[u8]) -> Vec<u8> {
        let mut tar_buf = Vec::new();
        {
            let mut builder = tar::Builder::new(&mut tar_buf);
            let mut header = tar::Header::new_gnu();
            header.set_path("cloudflared").unwrap();
            header.set_size(body.len() as u64);
            header.set_mode(0o755);
            header.set_entry_type(tar::EntryType::Regular);
            header.set_cksum();
            builder.append(&header, body).unwrap();
            builder.finish().unwrap();
        }
        let mut gz = Vec::new();
        {
            use std::io::Write as _;
            let mut enc = flate2::write::GzEncoder::new(&mut gz, flate2::Compression::default());
            enc.write_all(&tar_buf).unwrap();
            enc.finish().unwrap();
        }
        gz
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
        assert!(!host_is_trusted("https://evil.example#@github.com/x"));
        assert!(!host_is_trusted("https://evil.example?@github.com/x"));
        assert!(!host_is_trusted("https://evil.example\\@github.com/x"));
    }

    #[test]
    fn tool_error_messages_carry_one_prefix() {
        let cases = [
            (
                CloudflaredInstallError::Download("release metadata: offline".to_owned()),
                "download failed: release metadata: offline",
            ),
            (
                CloudflaredInstallError::Metadata("parse release json: eof".to_owned()),
                "download failed: release metadata error: parse release json: eof",
            ),
            (
                CloudflaredInstallError::Sha256Mismatch("x: got a, want b".to_owned()),
                "integrity check failed: x: got a, want b",
            ),
            (
                CloudflaredInstallError::UntrustedHost("https://evil.example.com".to_owned()),
                "integrity check failed: refusing to download from untrusted host: https://evil.example.com",
            ),
        ];
        for (err, want) in cases {
            assert_eq!(ToolError::from(err).to_string(), want);
        }
    }

    #[test]
    fn extract_pulls_cloudflared_from_tgz() {
        let gz = tgz_with_cloudflared(b"ELF-ish cloudflared bytes");
        let out = extract_cloudflared_from_tgz(&gz).unwrap();
        assert_eq!(out, b"ELF-ish cloudflared bytes");
    }

    #[tokio::test]
    async fn latest_reads_release_tag_name() {
        let dl = UrlDl::default().with(
            LATEST_RELEASE_API,
            r#"{"tag_name":"2025.11.1","assets":[]}"#,
        );
        assert_eq!(latest(&dl).await.unwrap(), "2025.11.1");
        let offline = UrlDl::default();
        assert!(latest(&offline).await.is_err());
    }

    #[tokio::test]
    async fn install_places_binary_and_marker() {
        let tmp = tempfile::tempdir().unwrap();
        let dirs = dirs_in(tmp.path());
        let (os, arch) = current_os_arch().unwrap();
        let (asset, is_tgz) = host_asset(os, arch);
        let body = b"#!/bin/sh\necho cloudflared\n".to_vec();
        let served = if is_tgz {
            tgz_with_cloudflared(&body)
        } else {
            body.clone()
        };
        let url = format!(
            "https://github.com/cloudflare/cloudflared/releases/download/2025.11.1/{asset}"
        );
        let json = serde_json::json!({
            "tag_name": "2025.11.1",
            "assets": [{
                "name": asset,
                "browser_download_url": url,
                "digest": format!("sha256:{}", yerd_update::sha256_hex(&served)),
            }],
        });
        let mut dl = UrlDl::default().with(LATEST_RELEASE_API, &json.to_string());
        dl.0.insert(url, served);

        install(&dirs, &dl, None).await.unwrap();

        assert_eq!(std::fs::read(binary_path(&dirs)).unwrap(), body);
        assert_eq!(
            installed_version(&dirs, Tool::Cloudflared, None).as_deref(),
            Some("2025.11.1")
        );
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

    fn seed_legacy(dirs: &PlatformDirs, marker: Option<&str>) {
        std::fs::create_dir_all(legacy_bin_dir(dirs)).unwrap();
        std::fs::write(legacy_binary_path(dirs), b"#!/bin/sh\nlegacy\n").unwrap();
        if let Some(v) = marker {
            std::fs::write(legacy_marker_path(dirs), v).unwrap();
        }
    }

    fn seed_tunnel_state(dirs: &PlatformDirs) -> (PathBuf, PathBuf) {
        let tunnel = crate::tunnel::install::tunnel_dir(dirs);
        let cert = tunnel.join(".cloudflared").join("cert.pem");
        let quick = tunnel.join("quick.yml");
        std::fs::create_dir_all(cert.parent().unwrap()).unwrap();
        std::fs::write(&cert, b"cert").unwrap();
        std::fs::write(&quick, b"quick").unwrap();
        (cert, quick)
    }

    enum Legacy {
        Absent,
        Unmarked,
        Marked(&'static str),
    }

    #[test]
    fn migrate_legacy_cases() {
        struct Case {
            name: &'static str,
            legacy: Legacy,
            managed: Option<&'static str>,
            broken_managed: bool,
            want_migrated: bool,
            want_version: Option<&'static str>,
            want_binary: &'static [u8],
        }
        let cases = [
            Case {
                name: "legacy with marker",
                legacy: Legacy::Marked("2025.4.0\n"),
                managed: None,
                broken_managed: false,
                want_migrated: true,
                want_version: Some("2025.4.0"),
                want_binary: b"#!/bin/sh\nlegacy\n",
            },
            Case {
                name: "legacy without marker",
                legacy: Legacy::Unmarked,
                managed: None,
                broken_managed: false,
                want_migrated: true,
                want_version: Some(UNKNOWN_VERSION),
                want_binary: b"#!/bin/sh\nlegacy\n",
            },
            Case {
                name: "legacy and managed",
                legacy: Legacy::Marked("2025.4.0"),
                managed: Some("2025.11.1"),
                broken_managed: false,
                want_migrated: false,
                want_version: Some("2025.11.1"),
                want_binary: b"managed",
            },
            Case {
                name: "legacy and a marker-only managed dir",
                legacy: Legacy::Marked("2025.4.0"),
                managed: Some("2025.11.1"),
                broken_managed: true,
                want_migrated: true,
                want_version: Some("2025.4.0"),
                want_binary: b"#!/bin/sh\nlegacy\n",
            },
            Case {
                name: "nothing",
                legacy: Legacy::Absent,
                managed: None,
                broken_managed: false,
                want_migrated: false,
                want_version: None,
                want_binary: b"",
            },
        ];
        for case in cases {
            let tmp = tempfile::tempdir().unwrap();
            let dirs = dirs_in(tmp.path());
            let (cert, quick) = seed_tunnel_state(&dirs);
            match case.legacy {
                Legacy::Absent => {}
                Legacy::Unmarked => seed_legacy(&dirs, None),
                Legacy::Marked(v) => seed_legacy(&dirs, Some(v)),
            }
            if let Some(v) = case.managed {
                install_binary_for_test(&dirs, v, b"managed");
            }
            if case.broken_managed {
                std::fs::remove_file(binary_path(&dirs)).unwrap();
            }

            assert_eq!(
                migrate_legacy(&dirs).unwrap(),
                case.want_migrated,
                "{}",
                case.name
            );
            assert_eq!(
                installed_version(&dirs, Tool::Cloudflared, None).as_deref(),
                case.want_version,
                "{}",
                case.name
            );
            if case.want_version.is_some() {
                assert_eq!(
                    std::fs::read(binary_path(&dirs)).unwrap(),
                    case.want_binary,
                    "{}",
                    case.name
                );
            }
            assert!(!legacy_binary_path(&dirs).exists(), "{}", case.name);
            assert!(!legacy_marker_path(&dirs).exists(), "{}", case.name);
            assert!(!legacy_bin_dir(&dirs).exists(), "{}", case.name);
            assert_eq!(std::fs::read(&cert).unwrap(), b"cert", "{}", case.name);
            assert_eq!(std::fs::read(&quick).unwrap(), b"quick", "{}", case.name);
        }
    }
}
