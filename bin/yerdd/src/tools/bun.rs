//! Bun installer — fetch the latest release zip into `{data}/tools/bun/` and
//! expose `bun`/`bunx`.
//!
//! Bun ships a single self-contained binary in a `.zip` (one per platform).
//! Integrity uses the per-release `SHASUMS256.txt`. The version is resolved from
//! the GitHub "latest release" API (`tag_name`, e.g. `bun-v1.3.14`).

use std::io::{Cursor, Read as _};
use std::path::{Path, PathBuf};

use serde::Deserialize;

use yerd_php::{current_os_arch, is_safe_member, Arch, Downloader, Os};
use yerd_platform::PlatformDirs;

use super::{
    extract_root_dir, sha_for_asset, stage_and_swap, tool_dir, verify_sha256, Tool, ToolError,
};

const LATEST_API: &str = "https://api.github.com/repos/oven-sh/bun/releases/latest";
const RELEASE_BASE: &str = "https://github.com/oven-sh/bun/releases/download";

#[derive(Debug, Deserialize)]
struct LatestRelease {
    tag_name: String,
}

/// The platform token Bun uses in artifact names for the host, e.g.
/// `darwin-aarch64`. `None` if Bun publishes no build for this OS/arch.
fn host_platform() -> Option<&'static str> {
    let (os, arch) = current_os_arch().ok()?;
    Some(match (os, arch) {
        (Os::Macos, Arch::Aarch64) => "darwin-aarch64",
        (Os::Macos, Arch::X86_64) => "darwin-x64",
        (Os::Linux, Arch::Aarch64) => "linux-aarch64",
        (Os::Linux, Arch::X86_64) => "linux-x64",
    })
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

    // Plain (non-baseline) asset; baseline/musl/profile are decoys in SHASUMS.
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
            let mode = entry.unix_mode().unwrap_or(0o644);
            let _ = std::fs::set_permissions(&out, std::fs::Permissions::from_mode(mode));
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
    fn unpack_zip_extracts_executable_binary() {
        // Build a tiny zip containing `bun-darwin-aarch64/bun` with mode 0755.
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
