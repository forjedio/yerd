//! Bun installer - fetch the latest release zip into `{data}/tools/bun/` and
//! expose `bun`/`bunx`.
//!
//! Bun ships a single self-contained binary in a `.zip` (one per platform).
//! Integrity uses the per-release `SHASUMS256.txt`. The version is resolved from
//! the GitHub "latest release" API (`tag_name`, e.g. `bun-v1.3.14`).

use std::io::{Cursor, Read as _};
use std::path::Path;
#[cfg(unix)]
use std::path::PathBuf;

use serde::Deserialize;

use yerd_php::{current_os_arch, is_safe_member, Arch, Downloader, Os};
use yerd_platform::PlatformDirs;

#[cfg(any(unix, windows))]
use super::{extract_root_dir, tool_dir};
use super::{sha_for_asset, stage_and_swap, verify_sha256, Tool, ToolError};

const LATEST_API: &str = "https://api.github.com/repos/oven-sh/bun/releases/latest";
const RELEASE_BASE: &str = "https://github.com/oven-sh/bun/releases/download";

#[derive(Debug, Deserialize)]
struct LatestRelease {
    tag_name: String,
}

/// The platform token Bun uses in artifact names for `(os, arch)`, e.g.
/// `darwin-aarch64` or `windows-x64`. `None` if Bun publishes no build for that
/// pair - notably there is no `windows-aarch64` Bun build.
fn platform_token(os: Os, arch: Arch) -> Option<&'static str> {
    Some(match (os, arch) {
        (Os::Macos, Arch::Aarch64) => "darwin-aarch64",
        (Os::Macos, Arch::X86_64) => "darwin-x64",
        (Os::Linux, Arch::Aarch64) => "linux-aarch64",
        (Os::Linux, Arch::X86_64) => "linux-x64",
        (Os::Windows, Arch::X86_64) => "windows-x64",
        (Os::Windows, Arch::Aarch64) => return None,
    })
}

/// The platform token for the running host. `None` if Bun publishes no build
/// for this OS/arch.
fn host_platform() -> Option<&'static str> {
    let (os, arch) = current_os_arch().ok()?;
    platform_token(os, arch)
}

/// Display version from a `bun-v1.3.14` tag → `v1.3.14`.
fn display_version(tag: &str) -> &str {
    tag.strip_prefix("bun-").unwrap_or(tag)
}

/// Install the latest Bun release for the host into `{data}/tools/bun/`.
pub async fn install(dirs: &PlatformDirs, dl: &dyn Downloader) -> Result<(), ToolError> {
    let plat = host_platform().ok_or(ToolError::UnsupportedHost("Bun"))?;
    let body = dl
        .download(LATEST_API)
        .await
        .map_err(|e| ToolError::Download(format!("bun latest release: {e}")))?;
    let release: LatestRelease = serde_json::from_slice(&body)
        .map_err(|e| ToolError::Download(format!("bun release parse: {e}")))?;
    let tag = release.tag_name;
    if !tag.starts_with("bun-v") {
        return Err(ToolError::Download(format!("unexpected bun tag {tag:?}")));
    }

    let asset = format!("bun-{plat}.zip");
    let zip_url = format!("{RELEASE_BASE}/{tag}/{asset}");
    let sums_url = format!("{RELEASE_BASE}/{tag}/SHASUMS256.txt");

    let sums = dl
        .download(&sums_url)
        .await
        .map_err(|e| ToolError::Download(format!("bun SHASUMS256.txt: {e}")))?;
    let want_sha = sha_for_asset(&String::from_utf8_lossy(&sums), &asset)
        .ok_or_else(|| ToolError::Download(format!("bun: {asset} not in SHASUMS256.txt")))?;

    let bytes = dl
        .download(&zip_url)
        .await
        .map_err(|e| ToolError::Download(format!("{asset}: {e}")))?;
    verify_sha256(&bytes, &want_sha, &asset)?;

    let version = display_version(&tag).to_owned();
    stage_and_swap(dirs, Tool::Bun, &version, |staging| {
        unpack_zip(&bytes, staging, &asset)
    })?;
    tracing::info!(version = %version, "installed Bun");
    Ok(())
}

/// `(name_in_bin, target)` links for an installed Bun: `bun` → the binary;
/// `bunx` → the `{data}/bin/bun` shim (Bun dispatches on argv0). Empty if the
/// install root can't be resolved.
#[cfg(unix)]
pub(crate) fn shim_links(dirs: &PlatformDirs) -> Vec<(String, PathBuf)> {
    let Ok(root) = extract_root_dir(&tool_dir(dirs, Tool::Bun)) else {
        return Vec::new();
    };
    let bin = dirs.data.join("bin");
    vec![
        ("bun".to_owned(), root.join("bun")),
        ("bunx".to_owned(), bin.join("bun")),
    ]
}

/// Forwarding shims for an installed Bun on Windows: `bun`→`bun.exe`, and
/// `bunx`→`bun.exe x` (a `.cmd` cannot set argv0, so `bunx` is synthesized as
/// `bun x` rather than via Bun's argv0 dispatch as on Unix). Empty if the install
/// root can't be resolved.
#[cfg(windows)]
pub(crate) fn shim_links(dirs: &PlatformDirs) -> Vec<super::ForwardShim> {
    let Ok(root) = extract_root_dir(&tool_dir(dirs, Tool::Bun)) else {
        return Vec::new();
    };
    let bun = root.join("bun.exe");
    vec![
        super::ForwardShim {
            name: "bun".to_owned(),
            target: bun.clone(),
            prefix_args: &[],
        },
        super::ForwardShim {
            name: "bunx".to_owned(),
            target: bun,
            prefix_args: &["x"],
        },
    ]
}

/// Unzip Bun's archive into `dest`, preserving the executable bit on the `bun`
/// binary. Member names are validated against traversal; the sha256 check above
/// is the integrity boundary.
fn unpack_zip(zip_bytes: &[u8], dest: &Path, label: &str) -> Result<(), ToolError> {
    let mut archive = zip::ZipArchive::new(Cursor::new(zip_bytes))
        .map_err(|e| ToolError::Unpack(format!("{label}: {e}")))?;
    for i in 0..archive.len() {
        let mut entry = archive
            .by_index(i)
            .map_err(|e| ToolError::Unpack(format!("{label}: {e}")))?;
        let Some(rel) = entry.enclosed_name() else {
            return Err(ToolError::Unpack(format!(
                "unsafe archive member {:?}",
                entry.name()
            )));
        };
        let name = rel.to_string_lossy().into_owned();
        if !is_safe_member(&name) {
            return Err(ToolError::Unpack(format!("unsafe archive member {name:?}")));
        }
        let out = dest.join(&rel);
        if entry.is_dir() {
            std::fs::create_dir_all(&out)
                .map_err(|e| ToolError::Unpack(format!("{}: {e}", out.display())))?;
            continue;
        }
        if let Some(parent) = out.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| ToolError::Unpack(format!("{}: {e}", parent.display())))?;
        }
        let mut buf = Vec::with_capacity(usize::try_from(entry.size()).unwrap_or(0));
        entry
            .read_to_end(&mut buf)
            .map_err(|e| ToolError::Unpack(format!("{name}: {e}")))?;
        std::fs::write(&out, &buf)
            .map_err(|e| ToolError::Unpack(format!("{}: {e}", out.display())))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = match entry.unix_mode() {
                Some(mode) => mode,
                None if name.ends_with("/bun") || name == "bun" => 0o755,
                None => 0o644,
            };
            std::fs::set_permissions(&out, std::fs::Permissions::from_mode(mode))
                .map_err(|e| ToolError::Unpack(format!("{}: {e}", out.display())))?;
        }
    }
    Ok(())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn display_version_strips_prefix() {
        assert_eq!(display_version("bun-v1.3.14"), "v1.3.14");
        assert_eq!(display_version("weird"), "weird");
    }

    #[test]
    fn host_platform_known() {
        assert!(host_platform().is_some());
    }

    #[test]
    fn platform_token_covers_supported_hosts() {
        let cases = [
            (Os::Macos, Arch::Aarch64, "darwin-aarch64"),
            (Os::Macos, Arch::X86_64, "darwin-x64"),
            (Os::Linux, Arch::Aarch64, "linux-aarch64"),
            (Os::Linux, Arch::X86_64, "linux-x64"),
            (Os::Windows, Arch::X86_64, "windows-x64"),
        ];
        for (os, arch, want) in cases {
            assert_eq!(platform_token(os, arch), Some(want), "{os:?}/{arch:?}");
        }
    }

    /// Bun ships no `windows-aarch64` build, so that arm alone is `None`.
    #[test]
    fn platform_token_none_for_windows_arm() {
        assert_eq!(platform_token(Os::Windows, Arch::Aarch64), None);
    }

    /// Bun's asset is a `.zip` on every platform; the token is what varies.
    #[test]
    fn windows_asset_name_uses_zip() {
        let plat = platform_token(Os::Windows, Arch::X86_64).unwrap();
        assert_eq!(format!("bun-{plat}.zip"), "bun-windows-x64.zip");
    }

    #[test]
    fn unpack_zip_extracts_executable_binary() {
        let mut buf = Vec::new();
        {
            let mut w = zip::ZipWriter::new(Cursor::new(&mut buf));
            let opts: zip::write::FileOptions<()> =
                zip::write::FileOptions::default().unix_permissions(0o755);
            w.start_file("bun-darwin-aarch64/bun", opts).unwrap();
            std::io::Write::write_all(&mut w, b"#!fake-bun").unwrap();
            w.finish().unwrap();
        }
        let tmp = tempfile::tempdir().unwrap();
        unpack_zip(&buf, tmp.path(), "bun.zip").unwrap();
        let bin = tmp.path().join("bun-darwin-aarch64").join("bun");
        assert_eq!(std::fs::read(&bin).unwrap(), b"#!fake-bun");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&bin).unwrap().permissions().mode();
            assert_eq!(mode & 0o111, 0o111, "bun should be executable");
        }
    }
}
