//! Node.js installer - verified side-by-side releases in `{data}/tools/node-versions/`
//! and expose `node`/`npm`/`npx`.
//!
//! Node's `.tar.gz` bundles `node` plus npm/npx (relative symlinks into
//! `lib/node_modules/npm`). Integrity uses the per-release `SHASUMS256.txt`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicU64;

use serde::Deserialize;
use yerd_core::node::{IndexRelease, MajorLine};

use yerd_php::{current_os_arch, is_safe_member, Arch, Downloader, Os};
use yerd_platform::PlatformDirs;

use yerd_platform::node::InstalledNode;

use super::{note, sha_for_asset, stage_at, verify_sha256, ProgressTx, ToolError};

const DIST_INDEX: &str = "https://nodejs.org/dist/index.json";
const DIST_BASE: &str = "https://nodejs.org/dist";
/// Release lines with their end-of-life dates. Optional: without it the
/// release list simply has no EOL badges.
const RELEASE_SCHEDULE: &str =
    "https://raw.githubusercontent.com/nodejs/Release/main/schedule.json";

/// One entry of the Node dist `index.json`.
#[derive(Debug, Deserialize)]
pub(crate) struct Release {
    version: String,
    /// `false` for non-LTS, or the LTS codename string (e.g. `"Krypton"`).
    lts: serde_json::Value,
    /// Published build keys, e.g. `linux-x64` or `osx-arm64-tar`.
    #[serde(default)]
    files: Vec<String>,
}

impl Release {
    fn codename(&self) -> Option<&str> {
        self.lts.as_str()
    }

    fn installable(&self, host_key: &str) -> bool {
        let major = yerd_core::node::numeric_selector(&self.version)
            .ok()
            .and_then(|parts| parts.first().copied());
        let files: Vec<&str> = self.files.iter().map(String::as_str).collect();
        major.is_some_and(|m| yerd_core::node::installable(&files, host_key, m))
    }
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

/// The `index.json` `files` key of the host's tarball build.
fn host_index_key() -> Option<&'static str> {
    let (os, arch) = current_os_arch().ok()?;
    Some(match (os, arch) {
        (Os::Macos, Arch::Aarch64) => "osx-arm64-tar",
        (Os::Macos, Arch::X86_64) => "osx-x64-tar",
        (Os::Linux, Arch::X86_64) => "linux-x64",
        (Os::Linux, Arch::Aarch64) => "linux-arm64",
    })
}

/// Parse the dist `index.json`.
pub(crate) fn parse_index(bytes: &[u8]) -> Result<Vec<Release>, ToolError> {
    serde_json::from_slice(bytes).map_err(|e| ToolError::Download(format!("node index.json: {e}")))
}

async fn fetch_index(dl: &dyn Downloader) -> Result<Vec<Release>, ToolError> {
    let index = dl
        .download(DIST_INDEX)
        .await
        .map_err(|e| ToolError::Download(format!("node index.json: {e}")))?;
    parse_index(&index)
}

/// Install the latest LTS and retain the previous releases.
pub async fn install(dirs: &PlatformDirs, dl: &dyn Downloader) -> Result<(), ToolError> {
    let version = install_version(dirs, dl, None, None).await?;
    let root = dirs.data.join("tools/node");
    std::fs::create_dir_all(&root).map_err(|e| ToolError::Io(e.to_string()))?;
    std::fs::write(root.join(".default"), version).map_err(|e| ToolError::Io(e.to_string()))
}

/// Install an optional numeric selector, resolving it against Node's upstream
/// index. Only releases with a host build at or above the listed major floor
/// are considered.
pub async fn install_version(
    dirs: &PlatformDirs,
    dl: &dyn Downloader,
    selector: Option<&str>,
    progress: Option<&ProgressTx>,
) -> Result<String, ToolError> {
    install_release(dirs, dl, selector, progress)
        .await
        .map(|installed| installed.exact)
}

/// A finished install plus what the index it resolved against says is newest.
pub(crate) struct NodeInstall {
    /// The exact version installed, without a leading `v`.
    pub exact: String,
    /// Major → newest installable patch, for `DaemonState::node_updates`.
    pub latest: BTreeMap<u32, String>,
}

/// [`install_version`], also returning the index's newest patch per major so
/// the daemon can refresh its update cache without a second fetch.
pub(crate) async fn install_release(
    dirs: &PlatformDirs,
    dl: &dyn Downloader,
    selector: Option<&str>,
    progress: Option<&ProgressTx>,
) -> Result<NodeInstall, ToolError> {
    if let Some(selector) = selector {
        yerd_core::node::validate_selector(selector).map_err(ToolError::Download)?;
    }
    let plat = host_platform().ok_or(ToolError::UnsupportedHost("Node.js"))?;
    let key = host_index_key().ok_or(ToolError::UnsupportedHost("Node.js"))?;
    note(
        progress,
        format!("Resolving Node {}", selector.unwrap_or("lts/*")),
    );
    let releases: Vec<Release> = fetch_index(dl)
        .await?
        .into_iter()
        .filter(|r| r.installable(key))
        .collect();
    let latest = latest_by_major(&lines_of(&releases, &[], None, ""));
    let exact = yerd_core::node::select_release(
        selector.unwrap_or("lts/*"),
        releases.iter().map(|r| IndexRelease {
            version: &r.version,
            lts: r.codename(),
        }),
    )
    .map_err(|_| {
        ToolError::Download(format!(
            "no upstream Node release matches {}",
            selector.unwrap_or("latest LTS")
        ))
    })?;
    let version = format!("v{exact}");
    let codename = releases
        .iter()
        .find(|r| r.version == version)
        .and_then(Release::codename)
        .map(str::to_owned);
    let asset = format!("node-{version}-{plat}.tar.gz");
    let tarball_url = format!("{DIST_BASE}/{version}/{asset}");
    let sums_url = format!("{DIST_BASE}/{version}/SHASUMS256.txt");

    let sums = dl
        .download(&sums_url)
        .await
        .map_err(|e| ToolError::Download(format!("node SHASUMS256.txt: {e}")))?;
    let want_sha = sha_for_asset(&String::from_utf8_lossy(&sums), &asset)
        .ok_or_else(|| ToolError::Download(format!("node: {asset} not in SHASUMS256.txt")))?;

    note(progress, format!("Downloading {asset}"));
    let bytes = match progress {
        Some(tx) => {
            let last = AtomicU64::new(u64::MAX);
            let cb = |done: u64, total: Option<u64>| {
                crate::php_install::emit_byte_progress(tx, &asset, done, total, &last);
            };
            dl.download_with_progress(&tarball_url, &cb).await
        }
        None => dl.download(&tarball_url).await,
    }
    .map_err(|e| ToolError::Download(format!("{asset}: {e}")))?;
    note(progress, "Verifying SHA-256");
    verify_sha256(&bytes, &want_sha, &asset)?;

    note(progress, "Unpacking");
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
            if let Some(codename) = &codename {
                std::fs::write(staging.join(".lts"), codename)
                    .map_err(|e| ToolError::Io(e.to_string()))?;
            }
            Ok(())
        },
    )?;
    tracing::info!(version = %version, "installed Node.js");
    note(progress, format!("Installed Node {exact}"));
    Ok(NodeInstall { exact, latest })
}

/// The installable published releases grouped by major, newest first. The
/// release index is required; the EOL schedule is optional and its failure
/// only leaves every `eol` unknown.
pub(crate) async fn available(
    dl: &dyn Downloader,
    installed: &[&str],
    today: &str,
) -> Result<Vec<MajorLine>, ToolError> {
    let key = host_index_key().ok_or(ToolError::UnsupportedHost("Node.js"))?;
    let (releases, schedule) = tokio::join!(fetch_index(dl), dl.download(RELEASE_SCHEDULE));
    let releases = installable_only(releases?, key);
    let schedule = match schedule {
        Ok(bytes) => parse_schedule(&bytes),
        Err(e) => {
            tracing::warn!(error = %e, "Node release schedule unavailable");
            None
        }
    };
    Ok(lines_of(&releases, installed, schedule.as_ref(), today))
}

/// The newest installable patch of every published major, for update checks.
/// Only the release index is fetched; EOL status is not needed here.
pub(crate) async fn latest_patches(
    dl: &dyn Downloader,
) -> Result<BTreeMap<u32, String>, ToolError> {
    let key = host_index_key().ok_or(ToolError::UnsupportedHost("Node.js"))?;
    let releases = installable_only(fetch_index(dl).await?, key);
    Ok(latest_by_major(&lines_of(&releases, &[], None, "")))
}

fn installable_only(releases: Vec<Release>, key: &str) -> Vec<Release> {
    releases
        .into_iter()
        .filter(|r| r.installable(key))
        .collect()
}

/// Group already-filtered releases into major lines; the one place the
/// listing, the update check and the installer agree on what "newest" means.
fn lines_of(
    releases: &[Release],
    installed: &[&str],
    schedule: Option<&BTreeMap<u32, String>>,
    today: &str,
) -> Vec<MajorLine> {
    yerd_core::node::major_lines(
        releases.iter().map(|r| IndexRelease {
            version: &r.version,
            lts: r.codename(),
        }),
        installed,
        schedule,
        today,
    )
}

/// Major → newest patch, as cached in `DaemonState::node_updates`.
pub(crate) fn latest_by_major(lines: &[MajorLine]) -> BTreeMap<u32, String> {
    lines.iter().map(|l| (l.major, l.latest.clone())).collect()
}

fn parse_schedule(bytes: &[u8]) -> Option<BTreeMap<u32, String>> {
    let raw: BTreeMap<String, serde_json::Value> = serde_json::from_slice(bytes).ok()?;
    Some(
        raw.into_iter()
            .filter_map(|(key, line)| {
                let major = yerd_core::node::schedule_major(&key)?;
                let end = line.get("end")?.as_str()?.to_owned();
                Some((major, end))
            })
            .collect(),
    )
}

fn normalised(version: &str) -> &str {
    let version = version.trim();
    version.strip_prefix('v').unwrap_or(version)
}

fn names(path: &Path, version: &str) -> bool {
    std::fs::read_to_string(path).is_ok_and(|v| normalised(&v) == version)
}

/// Remove one release. A legacy `{data}/tools/node` install of the same
/// version goes too, or discovery would find it again. The legacy `.default`
/// pointer is removed when it names the release, or always when `last` is set
/// because no release remains for it to point at.
pub(crate) fn uninstall_version(
    dirs: &PlatformDirs,
    installed: &InstalledNode,
    last: bool,
) -> Result<(), ToolError> {
    let io = |path: &Path, e: std::io::Error| ToolError::Io(format!("{}: {e}", path.display()));
    let legacy = dirs.data.join("tools/node");
    std::fs::remove_dir_all(&installed.root).map_err(|e| io(&installed.root, e))?;
    if installed.root != legacy && names(&legacy.join(".version"), &installed.version) {
        std::fs::remove_dir_all(&legacy).map_err(|e| io(&legacy, e))?;
    }
    let pointer = legacy.join(".default");
    if pointer.exists() && (last || names(&pointer, &installed.version)) {
        std::fs::remove_file(&pointer).map_err(|e| io(&pointer, e))?;
    }
    Ok(())
}

/// `(name_in_bin, target)` links for an installed Node: `node`/`npm`/`npx` →
/// the dist `bin/`. Empty if the install root can't be resolved.
#[cfg(unix)]
pub(crate) fn shim_links(dirs: &PlatformDirs, configured: Option<&str>) -> Vec<(String, PathBuf)> {
    let Some(version) = yerd_platform::node::default_selector(dirs, configured) else {
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
        schedule: Option<&'static str>,
        requests: std::sync::Mutex<Vec<String>>,
    }

    fn fake(corrupt: bool) -> FakeDownloader {
        FakeDownloader {
            corrupt,
            schedule: None,
            requests: std::sync::Mutex::new(Vec::new()),
        }
    }

    fn index_body() -> String {
        let key = host_index_key().unwrap();
        format!(
            r#"[
                {{"version":"v26.0.0","lts":false,"files":["some-other-host"]}},
                {{"version":"v25.1.0","lts":false,"files":["{key}"]}},
                {{"version":"v24.2.0","lts":"Krypton","files":["{key}"]}},
                {{"version":"v22.9.0","lts":"Jod","files":["{key}"]}},
                {{"version":"v8.17.0","lts":"Carbon","files":["{key}"]}}
            ]"#
        )
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
                return Ok(index_body().into_bytes());
            }
            if url == RELEASE_SCHEDULE {
                return match self.schedule {
                    Some(body) => Ok(body.as_bytes().to_vec()),
                    None => Err(yerd_php::DownloadError::Transport {
                        url: url.to_owned(),
                        reason: "offline".into(),
                    }),
                };
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
        let dl = fake(false);
        assert_eq!(
            install_version(&dirs, &dl, Some("v22"), None)
                .await
                .unwrap(),
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
            install_version(&dirs, &dl, Some("node"), None)
                .await
                .unwrap(),
            "25.1.0"
        );
        let bad = fake(true);
        assert!(install_version(&dirs, &bad, Some("22"), None)
            .await
            .is_err());
        let old = yerd_platform::node::resolve(&dirs, "22").unwrap();
        assert_eq!(std::fs::read(old.bin.join("node")).unwrap(), b"fake");
        let invalid = fake(false);
        assert!(install_version(&dirs, &invalid, Some("../24"), None)
            .await
            .is_err());
        assert!(invalid.requests.lock().unwrap().is_empty());
        assert!(install_version(&dirs, &dl, Some("23"), None).await.is_err());
        super::super::uninstall(&dirs, super::super::Tool::Node).unwrap();
        assert!(yerd_platform::node::installed(&dirs).is_empty());
    }
    #[test]
    fn host_platform_known() {
        assert!(host_platform().is_some());
    }

    #[tokio::test]
    async fn filter_excludes_foreign_hosts_and_old_majors_and_records_codename() {
        let tmp = tempfile::tempdir().unwrap();
        let dirs = dirs(tmp.path());
        let dl = fake(false);
        assert!(install_version(&dirs, &dl, Some("26"), None).await.is_err());
        assert!(install_version(&dirs, &dl, Some("8"), None).await.is_err());
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        assert_eq!(
            install_version(&dirs, &dl, Some("22"), Some(&tx))
                .await
                .unwrap(),
            "22.9.0"
        );
        drop(tx);
        let mut lines = Vec::new();
        while let Some(line) = rx.recv().await {
            lines.push(line);
        }
        assert_eq!(lines.first().map(String::as_str), Some("Resolving Node 22"));
        assert!(lines.iter().any(|l| l == "Verifying SHA-256"));
        assert_eq!(
            lines.last().map(String::as_str),
            Some("Installed Node 22.9.0")
        );
        let installed = yerd_platform::node::resolve(&dirs, "22").unwrap();
        assert_eq!(installed.codename.as_deref(), Some("Jod"));
        install_version(&dirs, &dl, Some("25"), None).await.unwrap();
        let current = yerd_platform::node::resolve(&dirs, "25").unwrap();
        assert!(!current.lts);
        assert_eq!(current.codename, None);
    }

    #[tokio::test]
    async fn codename_selectors_install_the_newest_release_of_that_line() {
        let tmp = tempfile::tempdir().unwrap();
        let dirs = dirs(tmp.path());
        let dl = fake(false);
        for (selector, expected) in [("lts/jod", "22.9.0"), ("lts/KRYPTON", "24.2.0")] {
            assert_eq!(
                install_version(&dirs, &dl, Some(selector), None)
                    .await
                    .unwrap(),
                expected
            );
        }
        assert_eq!(
            yerd_platform::node::resolve(&dirs, "lts/krypton")
                .unwrap()
                .version,
            "24.2.0"
        );
        for selector in ["lts/carbon", "lts/hydrogen", "lts/"] {
            assert!(install_version(&dirs, &dl, Some(selector), None)
                .await
                .is_err());
        }
    }

    #[tokio::test]
    async fn available_lists_installable_majors_with_optional_eol() {
        let mut dl = fake(false);
        let lines = available(&dl, &["22.1.0"], "2026-11-17").await.unwrap();
        assert_eq!(
            lines
                .iter()
                .map(|l| (l.major, l.latest.as_str(), l.eol, l.update_available))
                .collect::<Vec<_>>(),
            [
                (25, "25.1.0", None, false),
                (24, "24.2.0", None, false),
                (22, "22.9.0", None, true),
            ]
        );
        dl.schedule = Some(
            r#"{"v0.12":{"end":"2016-12-31"},"v22":{"start":"2024-04-24","end":"2027-04-30"},"v24":{"end":"2026-01-01"}}"#,
        );
        let lines = available(&dl, &[], "2026-11-17").await.unwrap();
        assert_eq!(
            lines.iter().map(|l| l.eol).collect::<Vec<_>>(),
            [None, Some(true), Some(false)]
        );
        dl.schedule = Some("not json");
        let lines = available(&dl, &[], "2026-11-17").await.unwrap();
        assert!(lines.iter().all(|l| l.eol.is_none()));
    }

    #[tokio::test]
    async fn install_release_and_latest_patches_agree_with_the_listing() {
        let tmp = tempfile::tempdir().unwrap();
        let dirs = dirs(tmp.path());
        let dl = fake(false);
        let listed = latest_by_major(&available(&dl, &[], "2026-11-17").await.unwrap());
        let installed = install_release(&dirs, &dl, Some("22"), None).await.unwrap();
        assert_eq!(installed.exact, "22.9.0");
        assert_eq!(installed.latest, listed);
        assert_eq!(latest_patches(&dl).await.unwrap(), listed);
    }

    fn release_at(root: &Path, version: &str, marker: &str) -> InstalledNode {
        let bin = root.join(format!("node-v{version}-test/bin"));
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::write(root.join(".version"), marker).unwrap();
        for tool in ["node", "npm", "npx"] {
            std::fs::write(bin.join(tool), "fake").unwrap();
        }
        InstalledNode {
            version: version.to_owned(),
            bin,
            lts: false,
            root: root.to_path_buf(),
            codename: None,
        }
    }

    #[test]
    fn uninstall_version_removes_release_legacy_copies_and_pointers() {
        let tmp = tempfile::tempdir().unwrap();
        let dirs = dirs(tmp.path());
        let legacy = dirs.data.join("tools/node");
        let side = |v: &str| yerd_platform::node::versions_dir(&dirs).join(v);
        for (target, legacy_marker, pointer, last, legacy_gone, pointer_gone) in [
            ("22.9.0", None, Some("22.9.0"), false, false, true),
            ("22.9.0", None, Some("24.1.0"), false, false, false),
            ("22.9.0", Some("v22.9.0\n"), None, false, true, true),
            ("22.9.0", Some("20.1.0"), Some("24.1.0"), true, false, true),
        ] {
            let _ = std::fs::remove_dir_all(&dirs.data);
            let release = release_at(&side(target), target, target);
            if let Some(marker) = legacy_marker {
                release_at(&legacy, "x", marker);
            }
            if let Some(pointer) = pointer {
                std::fs::create_dir_all(&legacy).unwrap();
                std::fs::write(legacy.join(".default"), pointer).unwrap();
            }
            uninstall_version(&dirs, &release, last).unwrap();
            assert!(!side(target).exists(), "{target} {legacy_marker:?}");
            if legacy_marker.is_some() {
                assert_eq!(!legacy.join(".version").exists(), legacy_gone);
            }
            if pointer.is_some() {
                assert_eq!(!legacy.join(".default").exists(), pointer_gone);
            }
        }
    }

    #[test]
    fn uninstall_version_removes_a_legacy_only_release() {
        let tmp = tempfile::tempdir().unwrap();
        let dirs = dirs(tmp.path());
        let legacy = dirs.data.join("tools/node");
        release_at(&legacy, "20.1.0", "v20.1.0\n");
        std::fs::write(legacy.join(".default"), "20.1.0").unwrap();
        let found = yerd_platform::node::resolve(&dirs, "20").unwrap();
        assert_eq!(found.root, legacy);
        uninstall_version(&dirs, &found, true).unwrap();
        assert!(yerd_platform::node::installed(&dirs).is_empty());
        assert!(yerd_platform::node::legacy_default(&dirs).is_none());
    }
}
