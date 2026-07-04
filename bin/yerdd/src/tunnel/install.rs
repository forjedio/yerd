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
use std::process::Stdio;
use std::time::Duration;

use async_trait::async_trait;
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

/// Where the `cloudflared` binary Yerd is using came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloudflaredSource {
    /// Downloaded and verified by Yerd into `{data}/tunnel/bin/cloudflared`.
    Managed,
    /// A pre-existing binary found on `PATH` that passed the minimum-version
    /// check.
    System,
}

/// The `cloudflared` binary Yerd will use, and where it came from.
#[derive(Debug, Clone)]
pub struct Resolved {
    /// Path to the binary to spawn.
    pub binary: PathBuf,
    /// Where `binary` came from.
    pub source: CloudflaredSource,
    /// Its reported version, when known.
    pub version: Option<String>,
}

/// Bound on the `cloudflared --version` probe used to validate a `PATH`
/// candidate.
const VERSION_PROBE_TIMEOUT: Duration = Duration::from_secs(5);

/// Runs `<binary> --version` and returns its captured output, or `None` on
/// spawn failure/timeout/non-zero exit. Behind a trait purely so `resolve()`'s
/// parsing and version-gate logic is unit-testable without a real subprocess.
#[async_trait]
pub trait VersionProbe: Send + Sync + 'static {
    /// Probe `binary`'s reported `--version` output, or `None` if it couldn't
    /// be run to completion.
    async fn probe(&self, binary: &Path) -> Option<String>;
}

/// The real `cloudflared --version` probe.
pub struct RealVersionProbe;

#[async_trait]
impl VersionProbe for RealVersionProbe {
    async fn probe(&self, binary: &Path) -> Option<String> {
        let mut cmd = tokio::process::Command::new(binary);
        cmd.arg("--version").stdin(Stdio::null()).kill_on_drop(true);
        let out = tokio::time::timeout(VERSION_PROBE_TIMEOUT, cmd.output())
            .await
            .ok()?
            .ok()?;
        out.status
            .success()
            .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
    }
}

/// Locates a `cloudflared` candidate on the host. Behind a trait (like
/// [`VersionProbe`]) purely so `resolve()`'s System-adoption branch is
/// unit-testable end-to-end without mutating the process's real `PATH`
/// (parallel test execution would make that flaky).
pub trait PathSearch: Send + Sync + 'static {
    /// Find an executable named `cloudflared` on the host, or `None`.
    fn find_cloudflared(&self) -> Option<PathBuf>;
}

/// The real `PATH` search.
pub struct RealPathSearch;

impl PathSearch for RealPathSearch {
    fn find_cloudflared(&self) -> Option<PathBuf> {
        let path = std::env::var_os("PATH")?;
        find_in_paths(&path)
    }
}

/// The `PATH`-search logic, taking the `PATH` value directly so it's testable
/// without mutating the process environment.
fn find_in_paths(path_var: &std::ffi::OsStr) -> Option<PathBuf> {
    std::env::split_paths(path_var).find_map(|dir| {
        let candidate = dir.join("cloudflared");
        is_executable(&candidate).then_some(candidate)
    })
}

/// Whether `path` is a regular, executable file. `pub(crate)` so
/// `tunnel::resolved_cloudflared` can re-check a cached `System` binary is
/// still there without re-running the `--version` probe.
#[cfg(unix)]
pub(crate) fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt as _;
    std::fs::metadata(path).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

#[cfg(not(unix))]
pub(crate) fn is_executable(path: &Path) -> bool {
    path.is_file()
}

/// Well past when all of `--no-autoupdate`/`--origincert`/`--http-host-header`/
/// `--origin-server-name`/`--no-tls-verify`/`--credentials-file`/
/// `--overwrite-dns` were stable in `cloudflared`, so any release at or above
/// this floor is assumed to support every flag Yerd depends on.
const MIN_SYSTEM_VERSION: (u32, u32, u32) = (2023, 3, 0);

/// Parse the `YYYY.MM.N` version token out of `cloudflared --version` output
/// (e.g. `cloudflared version 2024.6.1 (built 2024-06-11-1622 UTC)`),
/// ignoring surrounding text. Returns `None` for unparseable/non-official
/// builds so the caller falls back to the managed download rather than
/// trusting a build it can't reason about.
fn parse_cloudflared_version(text: &str) -> Option<(String, (u32, u32, u32))> {
    text.split_whitespace().find_map(|tok| {
        let mut parts = tok.split('.');
        let year = parts.next()?.parse::<u32>().ok()?;
        let month = parts.next()?.parse::<u32>().ok()?;
        let patch = parts.next()?.parse::<u32>().ok()?;
        if parts.next().is_some() || !(1900..=9999).contains(&year) {
            return None;
        }
        Some((tok.to_owned(), (year, month, patch)))
    })
}

/// Resolve which `cloudflared` binary Yerd should use: the managed copy if one
/// is installed, otherwise a `PATH`-found binary that passes the
/// minimum-version gate, otherwise `None` (nothing usable is available).
///
/// Uncached and directly unit-testable (both the `PATH` search and the
/// version probe are injected); `resolved_cloudflared` in `tunnel::mod` is the
/// cache-aware entry point call sites use, backed by [`RealPathSearch`] and
/// [`RealVersionProbe`].
pub async fn resolve<P: VersionProbe, S: PathSearch>(
    dirs: &PlatformDirs,
    probe: &P,
    path_search: &S,
) -> Option<Resolved> {
    if is_installed(dirs) {
        return Some(Resolved {
            binary: binary_path(dirs),
            source: CloudflaredSource::Managed,
            version: installed_version(dirs),
        });
    }

    let candidate = path_search.find_cloudflared()?;
    let raw_version = probe.probe(&candidate).await?;
    let (version, parsed) = parse_cloudflared_version(&raw_version)?;
    if parsed < MIN_SYSTEM_VERSION {
        return None;
    }
    Some(Resolved {
        binary: candidate,
        source: CloudflaredSource::System,
        version: Some(version),
    })
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

/// Test-only helper exposing `install_binary` to sibling test modules (e.g.
/// `tunnel::mod`'s cache tests), which can't reach this module's own private
/// `#[cfg(test)] mod tests`.
#[cfg(test)]
#[allow(clippy::unwrap_used)]
pub(crate) fn install_binary_for_test(dirs: &PlatformDirs, version: &str, bytes: &[u8]) {
    install_binary(dirs, version, bytes).unwrap();
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

    struct FakeVersionProbe(Option<&'static str>);

    #[async_trait]
    impl VersionProbe for FakeVersionProbe {
        async fn probe(&self, _binary: &Path) -> Option<String> {
            self.0.map(str::to_owned)
        }
    }

    #[test]
    fn parse_cloudflared_version_extracts_token_ignoring_surrounding_text() {
        assert_eq!(
            parse_cloudflared_version("cloudflared version 2024.6.1 (built 2024-06-11-1622 UTC)"),
            Some(("2024.6.1".to_owned(), (2024, 6, 1)))
        );
        assert_eq!(
            parse_cloudflared_version("cloudflared version DEV (built dev)"),
            None
        );
        assert_eq!(parse_cloudflared_version(""), None);
        assert_eq!(
            parse_cloudflared_version("cloudflared version 2024.6.1.7"),
            None,
            "four dotted components should not parse as a date-style version"
        );
    }

    #[test]
    fn minimum_version_gate_is_inclusive_at_the_floor() {
        assert!(MIN_SYSTEM_VERSION <= (2023, 3, 0));
        assert!((2023, 3, 0) >= MIN_SYSTEM_VERSION);
        assert!((2023, 2, 99) < MIN_SYSTEM_VERSION);
        assert!((2026, 1, 0) >= MIN_SYSTEM_VERSION);
    }

    #[test]
    fn find_in_paths_returns_first_executable_match() {
        let tmp = tempfile::tempdir().unwrap();
        let first = tmp.path().join("first");
        let second = tmp.path().join("second");
        std::fs::create_dir_all(&first).unwrap();
        std::fs::create_dir_all(&second).unwrap();
        // Only `second` has a `cloudflared` on it.
        std::fs::write(second.join("cloudflared"), b"#!/bin/sh\n").unwrap();
        set_executable(&second.join("cloudflared")).unwrap();
        let path_var = std::env::join_paths([&first, &second]).unwrap();
        assert_eq!(find_in_paths(&path_var), Some(second.join("cloudflared")));
    }

    #[test]
    fn find_in_paths_ignores_non_executable_file() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("cloudflared"), b"not executable").unwrap();
        let path_var = std::env::join_paths([tmp.path()]).unwrap();
        assert_eq!(find_in_paths(&path_var), None);
    }

    struct PanicsIfCalled;
    #[async_trait]
    impl VersionProbe for PanicsIfCalled {
        async fn probe(&self, _binary: &Path) -> Option<String> {
            panic!("managed binary should short-circuit before any probe runs");
        }
    }

    impl PathSearch for PanicsIfCalled {
        fn find_cloudflared(&self) -> Option<PathBuf> {
            panic!("managed binary should short-circuit before any PATH search runs");
        }
    }

    /// A fixed `PATH` candidate, standing in for `RealPathSearch` without
    /// touching the process's real `PATH` (parallel test execution would make
    /// that flaky).
    struct FakePathSearch(Option<PathBuf>);

    impl PathSearch for FakePathSearch {
        fn find_cloudflared(&self) -> Option<PathBuf> {
            self.0.clone()
        }
    }

    #[tokio::test]
    async fn resolve_prefers_managed_over_system_without_probing() {
        let tmp = tempfile::tempdir().unwrap();
        let dirs = dirs_in(tmp.path());
        install_binary(&dirs, "2026.6.1", b"#!/bin/sh\n").unwrap();

        let resolved = resolve(&dirs, &PanicsIfCalled, &PanicsIfCalled)
            .await
            .unwrap();
        assert_eq!(resolved.source, CloudflaredSource::Managed);
        assert_eq!(resolved.binary, binary_path(&dirs));
        assert_eq!(resolved.version.as_deref(), Some("2026.6.1"));
    }

    #[tokio::test]
    async fn resolve_returns_none_with_no_managed_binary_and_no_path_candidate() {
        let tmp = tempfile::tempdir().unwrap();
        let dirs = dirs_in(tmp.path());
        let resolved = resolve(
            &dirs,
            &FakeVersionProbe(Some("cloudflared version 2024.6.1")),
            &FakePathSearch(None),
        )
        .await;
        assert!(resolved.is_none());
    }

    #[tokio::test]
    async fn resolve_adopts_a_compatible_system_binary() {
        let tmp = tempfile::tempdir().unwrap();
        let dirs = dirs_in(tmp.path());
        let candidate = tmp.path().join("cloudflared");
        std::fs::write(&candidate, b"#!/bin/sh\n").unwrap();
        set_executable(&candidate).unwrap();

        let resolved = resolve(
            &dirs,
            &FakeVersionProbe(Some(
                "cloudflared version 2024.6.1 (built 2024-06-11-1622 UTC)",
            )),
            &FakePathSearch(Some(candidate.clone())),
        )
        .await
        .unwrap();
        assert_eq!(resolved.source, CloudflaredSource::System);
        assert_eq!(resolved.binary, candidate);
        assert_eq!(resolved.version.as_deref(), Some("2024.6.1"));
    }

    #[tokio::test]
    async fn resolve_rejects_a_system_binary_below_the_version_floor() {
        let tmp = tempfile::tempdir().unwrap();
        let dirs = dirs_in(tmp.path());
        let candidate = tmp.path().join("cloudflared");
        std::fs::write(&candidate, b"#!/bin/sh\n").unwrap();
        set_executable(&candidate).unwrap();

        let resolved = resolve(
            &dirs,
            &FakeVersionProbe(Some("cloudflared version 2021.5.10 (built 2021-05-01 UTC)")),
            &FakePathSearch(Some(candidate)),
        )
        .await;
        assert!(resolved.is_none());
    }

    #[tokio::test]
    async fn resolve_rejects_a_system_binary_with_unparseable_version() {
        let tmp = tempfile::tempdir().unwrap();
        let dirs = dirs_in(tmp.path());
        let candidate = tmp.path().join("cloudflared");
        std::fs::write(&candidate, b"#!/bin/sh\n").unwrap();
        set_executable(&candidate).unwrap();

        let resolved = resolve(
            &dirs,
            &FakeVersionProbe(Some("cloudflared version DEV (built dev)")),
            &FakePathSearch(Some(candidate)),
        )
        .await;
        assert!(resolved.is_none());
    }

    #[tokio::test]
    async fn resolve_rejects_a_system_binary_when_the_probe_fails() {
        let tmp = tempfile::tempdir().unwrap();
        let dirs = dirs_in(tmp.path());
        let candidate = tmp.path().join("cloudflared");
        std::fs::write(&candidate, b"#!/bin/sh\n").unwrap();
        set_executable(&candidate).unwrap();

        // `None` stands in for a spawn failure, timeout, or non-zero exit -
        // `RealVersionProbe::probe` collapses all three to `None`.
        let resolved = resolve(
            &dirs,
            &FakeVersionProbe(None),
            &FakePathSearch(Some(candidate)),
        )
        .await;
        assert!(resolved.is_none());
    }

    #[test]
    fn version_gate_rejects_below_floor_and_unparseable_output() {
        let too_old = "cloudflared version 2021.5.10 (built 2021-05-01 UTC)";
        assert_eq!(
            parse_cloudflared_version(too_old).map(|(_, v)| v < MIN_SYSTEM_VERSION),
            Some(true)
        );
        assert_eq!(
            parse_cloudflared_version("cloudflared version DEV (built dev)"),
            None
        );
    }
}
