//! Node.js installer — fetch the latest **LTS** tarball into `{data}/tools/node/`
//! and expose `node`/`npm`/`npx`.
//!
//! Node's `.tar.gz` bundles `node` plus npm/npx (relative symlinks into
//! `lib/node_modules/npm`). Integrity uses the per-release `SHASUMS256.txt`.

use std::path::{Path, PathBuf};

use serde::Deserialize;

use yerd_php::{current_os_arch, is_safe_member, Arch, Downloader, Os};
use yerd_platform::PlatformDirs;

use super::{
    extract_root_dir, sha_for_asset, stage_and_swap, tool_dir, verify_sha256, Tool, ToolError,
};

const DIST_INDEX: &str = "https://nodejs.org/dist/index.json";
const DIST_BASE: &str = "https://nodejs.org/dist";

/// One entry of the Node dist `index.json`.
#[derive(Debug, Deserialize)]
struct Release {
    version: String,
    /// `false` for non-LTS, or the LTS codename string (e.g. `"Krypton"`).
    lts: serde_json::Value,
}

/// The platform token Node uses in artifact names for the host, e.g.
/// `darwin-arm64`. `None` if Node publishes no build for this OS/arch.
fn host_platform() -> Option<&'static str> {
    let (os, arch) = current_os_arch().ok()?;
    Some(match (os, arch) {
        (Os::Macos, Arch::Aarch64) => "darwin-arm64",
        (Os::Macos, Arch::X86_64) => "darwin-x64",
        (Os::Linux, Arch::X86_64) => "linux-x64",
        (Os::Linux, Arch::Aarch64) => "linux-arm64",
    })
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

    let asset = format!("node-{version}-{plat}.tar.gz");
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
        unpack_tar_gz(&bytes, staging, &asset)
    })?;
    tracing::info!(version = %version, "installed Node.js");
    Ok(())
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

/// Safely unpack a Node `.tar.gz` full tree into `dest`, preserving permissions
/// and the internal npm/npx symlinks. Member *names* are validated against
/// traversal; the sha256 verification above is the integrity boundary.
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
        // The dev/test hosts are macOS or Linux on x86_64/aarch64 — all mapped.
        assert!(host_platform().is_some());
    }
}
