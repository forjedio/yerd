//! pnpm installer - fetch the latest standalone release tarball into
//! `{data}/tools/pnpm/` and expose `pnpm`/`pnpx`.
//!
//! pnpm ships a native `pnpm` binary plus a bundled `dist/` tree in one
//! `pnpm-<os>-<arch>.tar.gz` per platform, with no wrapping directory. pnpm
//! publishes no checksum file, so integrity uses the per-asset `digest`
//! (`sha256:<hex>`) that the GitHub "latest release" API returns alongside
//! `tag_name`. The install fails closed if that digest is missing.
//!
//! `pnpx` is a hard link of `pnpm`, not a symlink: pnpm picks its `dlx` alias
//! from the file stem of `std::env::current_exe`, and on Linux that resolves
//! symlinks, so a `pnpx -> pnpm` symlink would run plain pnpm.

use std::path::{Path, PathBuf};

use serde::Deserialize;

use yerd_php::{current_os_arch, is_safe_member, Arch, Downloader, Os};
use yerd_platform::PlatformDirs;

use super::{
    extract_root_dir, stage_and_swap, tool_dir, valid_tag, verify_sha256, Tool, ToolError,
};

const LATEST_API: &str = "https://api.github.com/repos/pnpm/pnpm/releases/latest";
const RELEASE_BASE: &str = "https://github.com/pnpm/pnpm/releases/download";

#[derive(Debug, Deserialize)]
struct LatestRelease {
    tag_name: String,
    #[serde(default)]
    assets: Vec<ReleaseAsset>,
}

#[derive(Debug, Deserialize)]
struct ReleaseAsset {
    name: String,
    #[serde(default)]
    digest: Option<String>,
}

/// The platform token pnpm uses in artifact names for the host, e.g.
/// `darwin-arm64`. glibc builds only (the `-musl` assets are ignored, as for
/// Bun). `None` if pnpm publishes no build for this OS/arch.
fn host_platform() -> Option<&'static str> {
    let (os, arch) = current_os_arch().ok()?;
    Some(match (os, arch) {
        (Os::Macos, Arch::Aarch64) => "darwin-arm64",
        (Os::Macos, Arch::X86_64) => "darwin-x64",
        (Os::Linux, Arch::Aarch64) => "linux-arm64",
        (Os::Linux, Arch::X86_64) => "linux-x64",
    })
}

/// The lowercase SHA-256 hex GitHub publishes for the asset named exactly
/// `asset`, from its `sha256:<hex>` digest. `None` if the asset or a
/// well-formed SHA-256 digest is missing.
fn asset_sha256(release: &LatestRelease, asset: &str) -> Option<String> {
    let digest = release
        .assets
        .iter()
        .find(|a| a.name == asset)?
        .digest
        .as_deref()?;
    let hex = digest.strip_prefix("sha256:")?;
    (hex.len() == 64 && hex.bytes().all(|b| b.is_ascii_hexdigit()))
        .then(|| hex.to_ascii_lowercase())
}

/// The latest pnpm release tag (`v10.1.0`), for the update poll.
pub(crate) async fn latest(dl: &dyn Downloader) -> Result<String, ToolError> {
    let body = dl
        .download(LATEST_API)
        .await
        .map_err(|e| ToolError::Download(format!("pnpm latest release: {e}")))?;
    let release: LatestRelease = serde_json::from_slice(&body)
        .map_err(|e| ToolError::Download(format!("pnpm release parse: {e}")))?;
    if !valid_tag(&release.tag_name) {
        return Err(ToolError::Download(format!(
            "unexpected pnpm tag {:?}",
            release.tag_name
        )));
    }
    Ok(release.tag_name)
}

/// Install the latest pnpm release for the host into `{data}/tools/pnpm/`.
pub async fn install(dirs: &PlatformDirs, dl: &dyn Downloader) -> Result<(), ToolError> {
    let plat = host_platform().ok_or(ToolError::UnsupportedHost("pnpm"))?;
    let body = dl
        .download(LATEST_API)
        .await
        .map_err(|e| ToolError::Download(format!("pnpm latest release: {e}")))?;
    let release: LatestRelease = serde_json::from_slice(&body)
        .map_err(|e| ToolError::Download(format!("pnpm release parse: {e}")))?;
    let tag = release.tag_name.clone();
    if !valid_tag(&tag) {
        return Err(ToolError::Download(format!("unexpected pnpm tag {tag:?}")));
    }

    let asset = format!("pnpm-{plat}.tar.gz");
    let want_sha = asset_sha256(&release, &asset).ok_or_else(|| {
        ToolError::Download(format!("pnpm: no sha256 digest published for {asset}"))
    })?;

    let url = format!("{RELEASE_BASE}/{tag}/{asset}");
    let bytes = dl
        .download(&url)
        .await
        .map_err(|e| ToolError::Download(format!("{asset}: {e}")))?;
    verify_and_place(dirs, plat, &tag, &asset, &bytes, &want_sha)?;
    tracing::info!(version = %tag, "installed pnpm");
    Ok(())
}

/// Verify the downloaded tarball and stage it into `{data}/tools/pnpm/`.
/// Deliberately synchronous, like the other tool installers: a cancelled
/// install job drops the install future while the caller holds `tool_mutate`,
/// and a detached `spawn_blocking` worker would keep placing files after that
/// lock is released and the job reported cancelled.
fn verify_and_place(
    dirs: &PlatformDirs,
    plat: &str,
    tag: &str,
    asset: &str,
    bytes: &[u8],
    want_sha: &str,
) -> Result<(), ToolError> {
    verify_sha256(bytes, want_sha, asset)?;
    stage_and_swap(dirs, Tool::Pnpm, tag, |staging| {
        let root = staging.join(format!("pnpm-{plat}"));
        std::fs::create_dir_all(&root)
            .map_err(|e| ToolError::Io(format!("{}: {e}", root.display())))?;
        unpack_tar_gz(bytes, &root, asset)?;
        link_pnpx(&root, asset)
    })
}

/// `(name_in_bin, target)` links for an installed pnpm: `pnpm` and `pnpx` each
/// point at their own file in the install root (see the module doc for why
/// `pnpx` is a separate hard link). Empty if the install root can't be resolved.
#[cfg(unix)]
pub(crate) fn shim_links(dirs: &PlatformDirs) -> Vec<(String, PathBuf)> {
    let Ok(root) = extract_root_dir(&tool_dir(dirs, Tool::Pnpm)) else {
        return Vec::new();
    };
    vec![
        ("pnpm".to_owned(), root.join("pnpm")),
        ("pnpx".to_owned(), root.join("pnpx")),
    ]
}

/// Create `root/pnpx` as a hard link of `root/pnpm`, copying instead if the
/// filesystem refuses the link.
fn link_pnpx(root: &Path, label: &str) -> Result<(), ToolError> {
    let binary = root.join("pnpm");
    if !binary.is_file() {
        return Err(ToolError::Unpack(format!(
            "pnpm binary missing from {label}"
        )));
    }
    let alias = root.join("pnpx");
    if std::fs::hard_link(&binary, &alias).is_ok() {
        return Ok(());
    }
    std::fs::copy(&binary, &alias)
        .map(|_| ())
        .map_err(|e| ToolError::Io(format!("{}: {e}", alias.display())))
}

/// Safely unpack pnpm's `.tar.gz` into `dest`, preserving permissions. Uses
/// `Entry::unpack_in` rather than `Entry::unpack` (as Node's unpacker does)
/// because pnpm's tarball carries hard-link members, and only `unpack_in`
/// resolves their targets inside `dest` instead of the process working
/// directory. Member names are validated against traversal; the sha256 check
/// above is the integrity boundary.
fn unpack_tar_gz(gz_bytes: &[u8], dest: &Path, label: &str) -> Result<(), ToolError> {
    let decoder = flate2::read::GzDecoder::new(gz_bytes);
    let mut archive = tar::Archive::new(decoder);
    archive.set_preserve_permissions(true);
    let entries = archive
        .entries()
        .map_err(|e| ToolError::Unpack(format!("{label}: {e}")))?;
    for entry in entries {
        let mut entry = entry.map_err(|e| ToolError::Unpack(format!("{label}: {e}")))?;
        let name = entry
            .path()
            .map_err(|e| ToolError::Unpack(format!("{label}: {e}")))?
            .to_string_lossy()
            .into_owned();
        if !is_safe_member(&name) {
            return Err(ToolError::Unpack(format!("unsafe archive member {name:?}")));
        }
        let unpacked = entry
            .unpack_in(dest)
            .map_err(|e| ToolError::Unpack(format!("{name}: {e}")))?;
        if !unpacked {
            return Err(ToolError::Unpack(format!("unsafe archive member {name:?}")));
        }
    }
    Ok(())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::panic, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use crate::ext_install::sha256_hex;

    fn dirs_in(tmp: &Path) -> PlatformDirs {
        PlatformDirs {
            config: tmp.join("c"),
            data: tmp.join("d"),
            state: tmp.join("s"),
            cache: tmp.join("ca"),
            runtime: tmp.join("r"),
        }
    }

    fn release(assets: &[(&str, Option<&str>)]) -> LatestRelease {
        LatestRelease {
            tag_name: "v12.21.0".to_owned(),
            assets: assets
                .iter()
                .map(|(name, digest)| ReleaseAsset {
                    name: (*name).to_owned(),
                    digest: digest.map(str::to_owned),
                })
                .collect(),
        }
    }

    fn file_header(path: &str, mode: u32, len: usize) -> tar::Header {
        let mut h = tar::Header::new_gnu();
        h.set_path(path).unwrap();
        h.set_mode(mode);
        h.set_size(len as u64);
        h.set_entry_type(tar::EntryType::Regular);
        h.set_cksum();
        h
    }

    fn gzip(tar_bytes: &[u8]) -> Vec<u8> {
        let mut enc = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        std::io::Write::write_all(&mut enc, tar_bytes).unwrap();
        enc.finish().unwrap()
    }

    fn pnpm_tarball() -> Vec<u8> {
        let mut b = tar::Builder::new(Vec::new());
        let bin = b"#!fake-pnpm";
        b.append(&file_header("pnpm", 0o755, bin.len()), &bin[..])
            .unwrap();
        let a = b"license";
        b.append(&file_header("dist/a", 0o644, a.len()), &a[..])
            .unwrap();
        let mut link = tar::Header::new_gnu();
        link.set_path("dist/b").unwrap();
        link.set_link_name("dist/a").unwrap();
        link.set_entry_type(tar::EntryType::Link);
        link.set_mode(0o644);
        link.set_size(0);
        link.set_cksum();
        b.append(&link, std::io::empty()).unwrap();
        gzip(&b.into_inner().unwrap())
    }

    fn traversal_tarball() -> Vec<u8> {
        let mut h = tar::Header::new_old();
        let name = b"../evil";
        h.as_old_mut().name[..name.len()].copy_from_slice(name);
        h.set_mode(0o644);
        h.set_size(1);
        h.set_entry_type(tar::EntryType::Regular);
        h.set_cksum();
        let mut b = tar::Builder::new(Vec::new());
        b.append(&h, &b"x"[..]).unwrap();
        gzip(&b.into_inner().unwrap())
    }

    struct FakeDownloader(Vec<(String, Vec<u8>)>);

    #[async_trait::async_trait]
    impl Downloader for FakeDownloader {
        async fn download(&self, url: &str) -> Result<Vec<u8>, yerd_php::DownloadError> {
            self.0
                .iter()
                .find(|(suffix, _)| url.ends_with(suffix.as_str()))
                .map(|(_, bytes)| bytes.clone())
                .ok_or_else(|| yerd_php::DownloadError::Transport {
                    url: url.to_owned(),
                    reason: "no route".into(),
                })
        }
    }

    fn fake_release(digest: Option<&str>) -> FakeDownloader {
        let plat = host_platform().unwrap();
        let asset = format!("pnpm-{plat}.tar.gz");
        let json = serde_json::json!({
            "tag_name": "v12.21.0",
            "assets": [
                { "name": format!("pnpm-{plat}-musl.tar.gz"), "digest": format!("sha256:{}", "0".repeat(64)) },
                { "name": asset, "digest": digest },
            ],
        });
        FakeDownloader(vec![
            (
                "/releases/latest".to_owned(),
                serde_json::to_vec(&json).unwrap(),
            ),
            (format!("/v12.21.0/{asset}"), pnpm_tarball()),
        ])
    }

    #[test]
    fn asset_sha256_picks_exact_asset_and_validates_digest() {
        let good = "AB".repeat(32);
        let good_digest = format!("sha256:{good}");
        let decoy = format!("sha256:{}", "0".repeat(64));
        let r = release(&[
            ("pnpm-linux-x64-musl.tar.gz", Some(decoy.as_str())),
            ("pnpm-linux-x64.tar.gz", Some(good_digest.as_str())),
        ]);
        assert_eq!(
            asset_sha256(&r, "pnpm-linux-x64.tar.gz"),
            Some(good.to_ascii_lowercase())
        );
        assert_eq!(asset_sha256(&r, "pnpm-darwin-arm64.tar.gz"), None);

        let sha512 = format!("sha512:{good}");
        let short = "sha256:abc".to_owned();
        let non_hex = format!("sha256:{}", "z".repeat(64));
        let rejected = [
            None,
            Some(sha512.as_str()),
            Some(short.as_str()),
            Some(non_hex.as_str()),
        ];
        for digest in rejected {
            let r = release(&[("pnpm-linux-x64.tar.gz", digest)]);
            assert_eq!(
                asset_sha256(&r, "pnpm-linux-x64.tar.gz"),
                None,
                "{digest:?}"
            );
        }
    }

    #[test]
    fn host_platform_known() {
        assert!(host_platform().is_some());
    }

    #[test]
    fn unpack_tar_gz_resolves_hard_links_inside_dest() {
        let tmp = tempfile::tempdir().unwrap();
        unpack_tar_gz(&pnpm_tarball(), tmp.path(), "pnpm.tar.gz").unwrap();
        assert_eq!(
            std::fs::read(tmp.path().join("dist").join("b")).unwrap(),
            b"license"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(tmp.path().join("pnpm"))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o111, 0o111, "pnpm should be executable");
        }
    }

    #[test]
    fn unpack_tar_gz_rejects_traversal() {
        let tmp = tempfile::tempdir().unwrap();
        let dest = tmp.path().join("dest");
        std::fs::create_dir(&dest).unwrap();
        let err = unpack_tar_gz(&traversal_tarball(), &dest, "evil.tar.gz").unwrap_err();
        assert!(matches!(err, ToolError::Unpack(_)), "{err:?}");
        assert!(!tmp.path().join("evil").exists());
    }

    #[tokio::test]
    async fn install_places_pnpm_and_pnpx_and_shims() {
        let tmp = tempfile::tempdir().unwrap();
        let dirs = dirs_in(tmp.path());
        let digest = format!("sha256:{}", sha256_hex(&pnpm_tarball()));
        install(&dirs, &fake_release(Some(&digest))).await.unwrap();

        assert_eq!(
            super::super::installed_version(&dirs, Tool::Pnpm, None).as_deref(),
            Some("v12.21.0")
        );
        let root = tool_dir(&dirs, Tool::Pnpm).join(format!("pnpm-{}", host_platform().unwrap()));
        let pnpm = std::fs::read(root.join("pnpm")).unwrap();
        assert_eq!(pnpm, b"#!fake-pnpm");
        assert_eq!(std::fs::read(root.join("pnpx")).unwrap(), pnpm);

        #[cfg(unix)]
        {
            let yerd_bin = tmp.path().join("yerd");
            std::fs::write(&yerd_bin, b"#!fake").unwrap();
            let bin = super::super::bin_dir(&dirs);
            super::super::reconcile_tool_shims(&dirs, &yerd_bin, None).unwrap();
            assert_eq!(std::fs::read_link(bin.join("pnpm")).unwrap(), yerd_bin);
            assert_eq!(std::fs::read_link(bin.join("pnpx")).unwrap(), yerd_bin);

            super::super::uninstall(&dirs, Tool::Pnpm).unwrap();
            super::super::reconcile_tool_shims(&dirs, &yerd_bin, None).unwrap();
            assert!(std::fs::symlink_metadata(bin.join("pnpm")).is_err());
            assert!(std::fs::symlink_metadata(bin.join("pnpx")).is_err());
        }
    }

    #[tokio::test]
    async fn install_rejects_wrong_digest() {
        let tmp = tempfile::tempdir().unwrap();
        let dirs = dirs_in(tmp.path());
        let digest = format!("sha256:{}", "f".repeat(64));
        let err = install(&dirs, &fake_release(Some(&digest)))
            .await
            .unwrap_err();
        assert!(matches!(err, ToolError::Sha256Mismatch(_)), "{err:?}");
        assert!(!tool_dir(&dirs, Tool::Pnpm).exists());
    }

    #[tokio::test]
    async fn install_requires_a_published_digest() {
        let tmp = tempfile::tempdir().unwrap();
        let dirs = dirs_in(tmp.path());
        let err = install(&dirs, &fake_release(None)).await.unwrap_err();
        assert!(matches!(err, ToolError::Download(_)), "{err:?}");
        assert!(!tool_dir(&dirs, Tool::Pnpm).exists());
    }
}
