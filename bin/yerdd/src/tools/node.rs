//! Node.js installer - verified side-by-side releases in `{data}/tools/node-versions/`
//! and expose `node`/`npm`/`npx`.
//!
//! Node's `.tar.gz` bundles `node` plus npm/npx (relative symlinks into
//! `lib/node_modules/npm`). Integrity uses the per-release `SHASUMS256.txt`.

use std::path::{Path, PathBuf};

use serde::Deserialize;

use yerd_php::{current_os_arch, is_safe_member, Arch, Downloader, Os};
use yerd_platform::PlatformDirs;

use super::{sha_for_asset, stage_at, verify_sha256, ToolError};

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

/// Install the latest LTS and retain the previous releases.
pub async fn install(dirs: &PlatformDirs, dl: &dyn Downloader) -> Result<(), ToolError> {
    let version = install_version(dirs, dl, None).await?;
    let root = dirs.data.join("tools/node");
    std::fs::create_dir_all(&root).map_err(|e| ToolError::Io(e.to_string()))?;
    std::fs::write(root.join(".default"), version).map_err(|e| ToolError::Io(e.to_string()))
}

/// Install an optional numeric selector, resolving it against Node's upstream index.
pub async fn install_version(
    dirs: &PlatformDirs,
    dl: &dyn Downloader,
    selector: Option<&str>,
) -> Result<String, ToolError> {
    if let Some(selector) = selector {
        if !matches!(selector, "node" | "lts/*") {
            yerd_core::node::numeric_selector(selector).map_err(ToolError::Download)?;
        }
    }
    let plat = host_platform().ok_or(ToolError::UnsupportedHost("Node.js"))?;
    let index = dl
        .download(DIST_INDEX)
        .await
        .map_err(|e| ToolError::Download(format!("node index.json: {e}")))?;
    let releases: Vec<Release> = serde_json::from_slice(&index)
        .map_err(|e| ToolError::Download(format!("node index.json: {e}")))?;
    let exact = yerd_core::node::select_release(
        selector.unwrap_or("lts/*"),
        releases
            .iter()
            .map(|r| (r.version.as_str(), r.lts.as_str().is_some())),
    )
    .map_err(|_| {
        ToolError::Download(format!(
            "no upstream Node release matches {}",
            selector.unwrap_or("latest LTS")
        ))
    })?;
    let version = format!("v{exact}");
    let lts = releases
        .iter()
        .any(|r| r.version == version && r.lts.as_str().is_some());
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

    stage_at(
        &yerd_platform::node::versions_dir(dirs).join(&exact),
        &exact,
        |staging| {
            unpack_tar_gz(&bytes, staging, &asset)?;
            let bin = super::extract_root_dir(staging)?.join("bin");
            if ["node", "npm", "npx"]
                .iter()
                .any(|name| !bin.join(name).is_file())
            {
                return Err(ToolError::Unpack(
                    "Node archive is missing node/npm/npx".to_owned(),
                ));
            }
            if lts {
                std::fs::write(staging.join(".lts"), "")
                    .map_err(|e| ToolError::Io(e.to_string()))?;
            }
            Ok(())
        },
    )?;
    tracing::info!(version = %version, "installed Node.js");
    Ok(exact)
}

/// `(name_in_bin, target)` links for an installed Node: `node`/`npm`/`npx` →
/// the dist `bin/`. Empty if the install root can't be resolved.
#[cfg(unix)]
pub(crate) fn shim_links(dirs: &PlatformDirs) -> Vec<(String, PathBuf)> {
    let configured = yerd_config::Config::load(&dirs.config.join("yerd.toml"))
        .ok()
        .and_then(|c| c.node.default);
    let Some(version) = configured.or_else(|| yerd_platform::node::legacy_default(dirs)) else {
        return Vec::new();
    };
    let Ok(selected) = yerd_platform::node::resolve(dirs, &version) else {
        return Vec::new();
    };
    let bin = selected.bin;
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

    struct FakeDownloader {
        corrupt: bool,
        requests: std::sync::Mutex<Vec<String>>,
    }

    fn archive(version: &str) -> Vec<u8> {
        let encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        let mut tar = tar::Builder::new(encoder);
        let root = format!("node-v{version}-{}", host_platform().unwrap());
        for name in ["node", "npm", "npx"] {
            let mut header = tar::Header::new_gnu();
            header.set_mode(0o755);
            header.set_size(4);
            header.set_cksum();
            tar.append_data(&mut header, format!("{root}/bin/{name}"), &b"fake"[..])
                .unwrap();
        }
        tar.into_inner().unwrap().finish().unwrap()
    }

    #[async_trait::async_trait]
    impl Downloader for FakeDownloader {
        async fn download(&self, url: &str) -> Result<Vec<u8>, yerd_php::DownloadError> {
            self.requests.lock().unwrap().push(url.to_owned());
            if url == DIST_INDEX {
                return Ok(br#"[
                {"version":"v25.1.0","lts":false},
                {"version":"v24.2.0","lts":"Krypton"},
                {"version":"v22.9.0","lts":"Jod"}
            ]"#
                .to_vec());
            }
            let version = if url.contains("v22.9.0") {
                "22.9.0"
            } else if url.contains("v25.1.0") {
                "25.1.0"
            } else {
                "24.2.0"
            };
            let bytes = archive(version);
            if url.ends_with("SHASUMS256.txt") {
                let asset = format!("node-v{version}-{}.tar.gz", host_platform().unwrap());
                return Ok(
                    format!("{}  {asset}\n", crate::ext_install::sha256_hex(&bytes)).into_bytes(),
                );
            }
            Ok(if self.corrupt {
                b"corrupt".to_vec()
            } else {
                bytes
            })
        }
    }

    fn dirs(tmp: &Path) -> PlatformDirs {
        PlatformDirs {
            config: tmp.join("c"),
            data: tmp.join("d"),
            state: tmp.join("s"),
            cache: tmp.join("ca"),
            runtime: tmp.join("r"),
        }
    }

    #[tokio::test]
    async fn installs_side_by_side_verifies_integrity_and_preserves_prior_release() {
        let tmp = tempfile::tempdir().unwrap();
        let dirs = dirs(tmp.path());
        let dl = FakeDownloader {
            corrupt: false,
            requests: std::sync::Mutex::new(Vec::new()),
        };
        assert_eq!(
            install_version(&dirs, &dl, Some("v22")).await.unwrap(),
            "22.9.0"
        );
        install(&dirs, &dl).await.unwrap();
        let versions = yerd_platform::node::installed(&dirs);
        assert_eq!(
            versions
                .iter()
                .map(|v| v.version.as_str())
                .collect::<Vec<_>>(),
            ["22.9.0", "24.2.0"]
        );
        assert_eq!(
            yerd_platform::node::legacy_default(&dirs).as_deref(),
            Some("24.2.0")
        );
        assert!(versions.iter().all(|v| v.lts));
        assert_eq!(
            install_version(&dirs, &dl, Some("node")).await.unwrap(),
            "25.1.0"
        );
        let bad = FakeDownloader {
            corrupt: true,
            requests: std::sync::Mutex::new(Vec::new()),
        };
        assert!(install_version(&dirs, &bad, Some("22")).await.is_err());
        let old = yerd_platform::node::resolve(&dirs, "22").unwrap();
        assert_eq!(std::fs::read(old.bin.join("node")).unwrap(), b"fake");
        let invalid = FakeDownloader {
            corrupt: false,
            requests: std::sync::Mutex::new(Vec::new()),
        };
        assert!(install_version(&dirs, &invalid, Some("../24"))
            .await
            .is_err());
        assert!(invalid.requests.lock().unwrap().is_empty());
        assert!(install_version(&dirs, &dl, Some("23")).await.is_err());
        super::super::uninstall(&dirs, super::super::Tool::Node).unwrap();
        assert!(yerd_platform::node::installed(&dirs).is_empty());
    }
    #[test]
    fn host_platform_known() {
        assert!(host_platform().is_some());
    }
}
