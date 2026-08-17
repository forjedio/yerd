//! Node.js installer - fetch the latest **LTS** build into `{data}/tools/node/`
//! and expose `node`/`npm`/`npx`.
//!
//! Node's Unix `.tar.gz` bundles `node` plus npm/npx (relative symlinks into
//! `lib/node_modules/npm`); its Windows `.zip` bundles `node.exe` plus
//! `npm.cmd`/`npx.cmd` at the archive root. Integrity uses the per-release
//! `SHASUMS256.txt`, which lists the platform-specific asset (zip on Windows).

use std::path::Path;
#[cfg(unix)]
use std::path::PathBuf;

use serde::Deserialize;

use yerd_php::{current_os_arch, is_safe_member, Arch, Downloader, Os};
use yerd_platform::PlatformDirs;

#[cfg(any(unix, windows))]
use super::{extract_root_dir, tool_dir};
use super::{sha_for_asset, stage_and_swap, verify_sha256, Tool, ToolError};

const DIST_INDEX: &str = "https://nodejs.org/dist/index.json";
const DIST_BASE: &str = "https://nodejs.org/dist";

/// Node's release-archive extension for the host: Windows ships only a `.zip`,
/// Unix a `.tar.gz`. Chosen at compile time (the binary only runs on its target
/// OS), matching the host-gated extraction path below.
#[cfg(windows)]
const ASSET_EXT: &str = "zip";
#[cfg(not(windows))]
const ASSET_EXT: &str = "tar.gz";

/// One entry of the Node dist `index.json`.
#[derive(Debug, Deserialize)]
struct Release {
    version: String,
    /// `false` for non-LTS, or the LTS codename string (e.g. `"Krypton"`).
    lts: serde_json::Value,
}

/// The platform token Node uses in artifact names for `(os, arch)`, e.g.
/// `darwin-arm64` or `win-x64`. Node publishes a build for every OS/arch Yerd
/// targets (both `win-x64` and `win-arm64`), so this is total.
fn platform_token(os: Os, arch: Arch) -> &'static str {
    match (os, arch) {
        (Os::Macos, Arch::Aarch64) => "darwin-arm64",
        (Os::Macos, Arch::X86_64) => "darwin-x64",
        (Os::Linux, Arch::X86_64) => "linux-x64",
        (Os::Linux, Arch::Aarch64) => "linux-arm64",
        (Os::Windows, Arch::X86_64) => "win-x64",
        (Os::Windows, Arch::Aarch64) => "win-arm64",
    }
}

/// The platform token for the running host. `None` if the host OS/arch can't be
/// resolved.
fn host_platform() -> Option<&'static str> {
    let (os, arch) = current_os_arch().ok()?;
    Some(platform_token(os, arch))
}

/// Latest LTS version (`v24.17.0`) from a dist `index.json` body. The index is
/// newest-first, so the first entry with a string `lts` is the latest LTS.
fn latest_lts(index_json: &[u8]) -> Option<String> {
    let releases: Vec<Release> = serde_json::from_slice(index_json).ok()?;
    releases
        .into_iter()
        .find(|r| r.lts.as_str().is_some())
        .map(|r| r.version)
}

/// Install the latest Node LTS for the host into `{data}/tools/node/`.
pub async fn install(dirs: &PlatformDirs, dl: &dyn Downloader) -> Result<(), ToolError> {
    let plat = host_platform().ok_or(ToolError::UnsupportedHost("Node.js"))?;
    let index = dl
        .download(DIST_INDEX)
        .await
        .map_err(|e| ToolError::Download(format!("node index.json: {e}")))?;
    let version = latest_lts(&index)
        .ok_or_else(|| ToolError::Download("node: no LTS release found".to_owned()))?;

    let asset = format!("node-{version}-{plat}.{ASSET_EXT}");
    let tarball_url = format!("{DIST_BASE}/{version}/{asset}");
    let sums_url = format!("{DIST_BASE}/{version}/SHASUMS256.txt");

    let sums = dl
        .download(&sums_url)
        .await
        .map_err(|e| ToolError::Download(format!("node SHASUMS256.txt: {e}")))?;
    let want_sha = sha_for_asset(&String::from_utf8_lossy(&sums), &asset)
        .ok_or_else(|| ToolError::Download(format!("node: {asset} not in SHASUMS256.txt")))?;

    let bytes = dl
        .download(&tarball_url)
        .await
        .map_err(|e| ToolError::Download(format!("{asset}: {e}")))?;
    verify_sha256(&bytes, &want_sha, &asset)?;

    stage_and_swap(dirs, Tool::Node, &version, |staging| {
        unpack(&bytes, staging, &asset)
    })?;
    tracing::info!(version = %version, "installed Node.js");
    Ok(())
}

/// Unpack Node's release archive into `dest`: a `.tar.gz` on Unix, a `.zip` on
/// Windows. Both validate member names against traversal; the sha256 check above
/// is the integrity boundary.
fn unpack(bytes: &[u8], dest: &Path, label: &str) -> Result<(), ToolError> {
    #[cfg(windows)]
    {
        unpack_zip(bytes, dest, label)
    }
    #[cfg(not(windows))]
    {
        unpack_tar_gz(bytes, dest, label)
    }
}

/// `(name_in_bin, target)` links for an installed Node: `node`/`npm`/`npx` →
/// the dist `bin/`. Empty if the install root can't be resolved.
#[cfg(unix)]
pub(crate) fn shim_links(dirs: &PlatformDirs) -> Vec<(String, PathBuf)> {
    let Ok(root) = extract_root_dir(&tool_dir(dirs, Tool::Node)) else {
        return Vec::new();
    };
    let bin = root.join("bin");
    ["node", "npm", "npx"]
        .into_iter()
        .map(|n| (n.to_owned(), bin.join(n)))
        .collect()
}

/// Forwarding shims for an installed Node on Windows: `node`→`node.exe`,
/// `npm`→`npm.cmd`, `npx`→`npx.cmd`, all at the extracted dist root (Node's
/// Windows zip places them there, not under a `bin/` subdir). Empty if the
/// install root can't be resolved.
#[cfg(windows)]
pub(crate) fn shim_links(dirs: &PlatformDirs) -> Vec<super::ForwardShim> {
    let Ok(root) = extract_root_dir(&tool_dir(dirs, Tool::Node)) else {
        return Vec::new();
    };
    vec![
        super::ForwardShim {
            name: "node".to_owned(),
            target: root.join("node.exe"),
            prefix_args: &[],
        },
        super::ForwardShim {
            name: "npm".to_owned(),
            target: root.join("npm.cmd"),
            prefix_args: &[],
        },
        super::ForwardShim {
            name: "npx".to_owned(),
            target: root.join("npx.cmd"),
            prefix_args: &[],
        },
    ]
}

/// Safely unpack a Node `.tar.gz` full tree into `dest`, preserving permissions
/// and the internal npm/npx symlinks. Member *names* are validated against
/// traversal; the sha256 verification above is the integrity boundary.
#[cfg(not(windows))]
fn unpack_tar_gz(gz_bytes: &[u8], dest: &Path, label: &str) -> Result<(), ToolError> {
    let decoder = flate2::read::GzDecoder::new(gz_bytes);
    let mut archive = tar::Archive::new(decoder);
    archive.set_preserve_permissions(true);
    let entries = archive
        .entries()
        .map_err(|e| ToolError::Unpack(format!("{label}: {e}")))?;
    for entry in entries {
        let mut entry = entry.map_err(|e| ToolError::Unpack(format!("{label}: {e}")))?;
        let path = entry
            .path()
            .map_err(|e| ToolError::Unpack(format!("{label}: {e}")))?
            .into_owned();
        let name = path.to_string_lossy().into_owned();
        if !is_safe_member(&name) {
            return Err(ToolError::Unpack(format!("unsafe archive member {name:?}")));
        }
        let out = dest.join(&path);
        if let Some(parent) = out.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| ToolError::Unpack(format!("{}: {e}", parent.display())))?;
        }
        entry
            .unpack(&out)
            .map_err(|e| ToolError::Unpack(format!("{name}: {e}")))?;
    }
    Ok(())
}

/// Safely unpack Node's Windows `.zip` full tree into `dest` (the zip wraps the
/// payload in one `node-v<ver>-win-<arch>` dir holding `node.exe`, `npm`/`npm.cmd`,
/// `npx`/`npx.cmd`, and `node_modules\npm\...`). Member names are validated via
/// the shared [`is_safe_member`] zip-slip guard; the sha256 check above is the
/// integrity boundary. No permission bits: Windows has no executable mode.
#[cfg(windows)]
fn unpack_zip(zip_bytes: &[u8], dest: &Path, label: &str) -> Result<(), ToolError> {
    use std::io::{Cursor, Read as _};

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
    }
    Ok(())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn latest_lts_picks_first_string_lts() {
        let json = br#"[
            {"version":"v26.3.1","lts":false},
            {"version":"v24.17.0","lts":"Krypton"},
            {"version":"v22.9.0","lts":"Jod"}
        ]"#;
        assert_eq!(latest_lts(json).as_deref(), Some("v24.17.0"));
    }

    #[test]
    fn latest_lts_none_when_no_lts() {
        let json = br#"[{"version":"v26.0.0","lts":false}]"#;
        assert_eq!(latest_lts(json), None);
    }

    #[test]
    fn host_platform_known() {
        assert!(host_platform().is_some());
    }

    #[test]
    fn platform_token_covers_every_host() {
        let cases = [
            (Os::Macos, Arch::Aarch64, "darwin-arm64"),
            (Os::Macos, Arch::X86_64, "darwin-x64"),
            (Os::Linux, Arch::X86_64, "linux-x64"),
            (Os::Linux, Arch::Aarch64, "linux-arm64"),
            (Os::Windows, Arch::X86_64, "win-x64"),
            (Os::Windows, Arch::Aarch64, "win-arm64"),
        ];
        for (os, arch, want) in cases {
            assert_eq!(platform_token(os, arch), want, "{os:?}/{arch:?}");
        }
    }

    /// The asset name is host-gated: a `.zip` on Windows, a `.tar.gz` elsewhere.
    #[test]
    fn asset_name_uses_host_extension() {
        let plat = platform_token(Os::Windows, Arch::X86_64);
        let win_asset = format!("node-v24.17.0-{plat}.zip");
        assert_eq!(win_asset, "node-v24.17.0-win-x64.zip");

        let plat = platform_token(Os::Linux, Arch::X86_64);
        let nix_asset = format!("node-v24.17.0-{plat}.tar.gz");
        assert_eq!(nix_asset, "node-v24.17.0-linux-x64.tar.gz");

        #[cfg(windows)]
        assert_eq!(ASSET_EXT, "zip");
        #[cfg(not(windows))]
        assert_eq!(ASSET_EXT, "tar.gz");
    }

    #[cfg(windows)]
    #[test]
    fn unpack_zip_lays_out_windows_node_tree() {
        use std::io::Cursor;
        let mut buf = Vec::new();
        {
            let mut w = zip::ZipWriter::new(Cursor::new(&mut buf));
            let opts: zip::write::FileOptions<()> = zip::write::FileOptions::default();
            w.start_file("node-v24.17.0-win-x64/node.exe", opts)
                .unwrap();
            std::io::Write::write_all(&mut w, b"MZ-fake").unwrap();
            w.start_file("node-v24.17.0-win-x64/npm.cmd", opts).unwrap();
            std::io::Write::write_all(&mut w, b"@echo npm").unwrap();
            w.add_directory("node-v24.17.0-win-x64/node_modules/npm", opts)
                .unwrap();
            w.finish().unwrap();
        }
        let tmp = tempfile::tempdir().unwrap();
        unpack_zip(&buf, tmp.path(), "node.zip").unwrap();
        let root = tmp.path().join("node-v24.17.0-win-x64");
        assert_eq!(std::fs::read(root.join("node.exe")).unwrap(), b"MZ-fake");
        assert_eq!(std::fs::read(root.join("npm.cmd")).unwrap(), b"@echo npm");
        assert!(root.join("node_modules").join("npm").is_dir());
    }
}
