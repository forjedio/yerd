//! zrok installer - the open-source zrok v2 agent from the openziti/zrok
//! GitHub releases.
//!
//! zrok v2 renamed its binary to `zrok2` (and its environment dir to
//! `~/.zrok2`) so v1 and v2 can coexist; Yerd installs `zrok2` at
//! `{data}/tools/zrok/zrok2` and exposes it as `zrok2` through the yerd
//! multi-call shim, which adds the outbound proxy environment.
//!
//! The version comes from the release list, not `releases/latest`: zrok still
//! publishes v1 maintenance releases interleaved with v2, so the newest release
//! is not necessarily the newest v2. The highest stable tag with major
//! [`SUPPORTED_MAJOR`] wins; a future major is a deliberate code change.
//!
//! Integrity is **fail-closed** against the per-asset GitHub `digest`
//! (`sha256:<hex>`), as for pnpm: the install is refused when it is missing.

use serde::Deserialize;
use yerd_php::{current_os_arch, Arch, Downloader, Os};
use yerd_platform::PlatformDirs;

use super::{
    extract_file_from_tgz, note, stage_and_swap, valid_tag, verify_sha256, write_executable,
    ProgressTx, Tool, ToolError,
};

/// The release list (newest first); one page is ample to reach the latest v2.
const RELEASES_API: &str = "https://api.github.com/repos/openziti/zrok/releases?per_page=30";

/// Base of the release asset download URLs.
const RELEASE_BASE: &str = "https://github.com/openziti/zrok/releases/download";

/// File name of the binary in the archive and in the tool dir.
const BINARY: &str = "zrok2";

/// The only zrok major Yerd installs (the one whose binary is `zrok2`).
const SUPPORTED_MAJOR: u64 = 2;

#[derive(Debug, Deserialize)]
struct Release {
    tag_name: String,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    prerelease: bool,
    #[serde(default)]
    assets: Vec<Asset>,
}

#[derive(Debug, Deserialize)]
struct Asset {
    name: String,
    #[serde(default)]
    digest: Option<String>,
}

/// zrok's `<os>_<arch>` token in artifact names.
fn host_platform(os: Os, arch: Arch) -> &'static str {
    match (os, arch) {
        (Os::Macos, Arch::X86_64) => "darwin_amd64",
        (Os::Macos, Arch::Aarch64) => "darwin_arm64",
        (Os::Linux, Arch::X86_64) => "linux_amd64",
        (Os::Linux, Arch::Aarch64) => "linux_arm64",
    }
}

/// `zrok_<version>_<platform>.tar.gz` for a `v`-prefixed `tag`.
fn asset_name(tag: &str, platform: &str) -> Option<String> {
    let version = tag.strip_prefix('v')?;
    Some(format!("zrok_{version}_{platform}.tar.gz"))
}

/// The highest stable, non-draft release whose tag is a safe `v2.x.y`.
fn pick_latest(releases: &[Release]) -> Option<&Release> {
    releases
        .iter()
        .filter(|r| !r.draft && !r.prerelease && valid_tag(&r.tag_name))
        .filter_map(|r| {
            let version = yerd_update::parse_tag(&r.tag_name)?;
            (version.pre.is_empty() && version.major == SUPPORTED_MAJOR).then_some((version, r))
        })
        .max_by(|a, b| a.0.cmp(&b.0))
        .map(|(_, r)| r)
}

/// The lowercase SHA-256 hex GitHub publishes for the asset named exactly
/// `asset`, from its `sha256:<hex>` digest. `None` if the asset or a
/// well-formed SHA-256 digest is missing.
fn asset_sha256(release: &Release, asset: &str) -> Option<String> {
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

/// Fetch and parse the release list.
async fn fetch_releases(dl: &dyn Downloader) -> Result<Vec<Release>, ToolError> {
    let body = dl
        .download(RELEASES_API)
        .await
        .map_err(|e| ToolError::Download(format!("zrok releases: {e}")))?;
    serde_json::from_slice(&body)
        .map_err(|e| ToolError::Download(format!("zrok releases parse: {e}")))
}

fn no_v2() -> ToolError {
    ToolError::Download("zrok: no v2 release found".to_owned())
}

/// The latest zrok v2 release tag (`v2.0.11`), for the update poll.
pub(crate) async fn latest(dl: &dyn Downloader) -> Result<String, ToolError> {
    let releases = fetch_releases(dl).await?;
    pick_latest(&releases)
        .map(|r| r.tag_name.clone())
        .ok_or_else(no_v2)
}

/// Install the latest zrok v2 for the host into `{data}/tools/zrok/`.
pub async fn install(
    dirs: &PlatformDirs,
    dl: &dyn Downloader,
    progress: Option<&ProgressTx>,
) -> Result<(), ToolError> {
    let (os, arch) = current_os_arch().map_err(|_| ToolError::UnsupportedHost("zrok"))?;
    let platform = host_platform(os, arch);

    note(progress, "Fetching zrok release info…");
    let releases = fetch_releases(dl).await?;
    let release = pick_latest(&releases).ok_or_else(no_v2)?;
    let tag = release.tag_name.as_str();
    let asset = asset_name(tag, platform).ok_or_else(no_v2)?;
    let want_sha = asset_sha256(release, &asset).ok_or_else(|| {
        ToolError::Download(format!("zrok: no sha256 digest published for {asset}"))
    })?;

    note(progress, format!("Downloading {asset}…"));
    let url = format!("{RELEASE_BASE}/{tag}/{asset}");
    let bytes = dl
        .download(&url)
        .await
        .map_err(|e| ToolError::Download(format!("{asset}: {e}")))?;
    verify_and_place(dirs, tag, &asset, &bytes, &want_sha)?;
    note(progress, format!("Installed zrok {tag}"));
    tracing::info!(version = %tag, "installed zrok");
    Ok(())
}

/// Verify the downloaded tarball and stage its `zrok2` binary. Synchronous for
/// the reason `pnpm::verify_and_place` documents.
fn verify_and_place(
    dirs: &PlatformDirs,
    tag: &str,
    asset: &str,
    bytes: &[u8],
    want_sha: &str,
) -> Result<(), ToolError> {
    verify_sha256(bytes, want_sha, asset)?;
    let binary = extract_file_from_tgz(bytes, BINARY, asset)?;
    stage_and_swap(dirs, Tool::Zrok, tag, |staging| {
        write_executable(staging, BINARY, &binary)
    })
}

/// A release list shaped like GitHub's: a draft, a v3, a prerelease, a v1
/// maintenance release published after v2.0.11, a hostile tag, then v2.0.11
/// (with `linux_armv7` and `checksums.sha256.txt` decoys) and v2.0.10. Shared
/// with the `tools` and `tool_updates` tests.
#[cfg(test)]
pub(crate) const RELEASES_FIXTURE: &str = r#"[
  {
    "tag_name": "v2.0.13",
    "draft": true,
    "prerelease": false,
    "assets": []
  },
  {
    "tag_name": "v3.0.0",
    "draft": false,
    "prerelease": false,
    "assets": [
      {
        "name": "zrok_3.0.0_linux_amd64.tar.gz",
        "digest": "sha256:0000000000000000000000000000000000000000000000000000000000000000"
      }
    ]
  },
  {
    "tag_name": "v2.0.12-rc1",
    "draft": false,
    "prerelease": true,
    "assets": []
  },
  {
    "tag_name": "v1.1.13",
    "draft": false,
    "prerelease": false,
    "assets": [
      {
        "name": "zrok_1.1.13_linux_amd64.tar.gz",
        "digest": "sha256:9999999999999999999999999999999999999999999999999999999999999999"
      }
    ]
  },
  {
    "tag_name": "v2.0.99/../x",
    "draft": false,
    "prerelease": false,
    "assets": []
  },
  {
    "tag_name": "v2.0.11",
    "draft": false,
    "prerelease": false,
    "assets": [
      {
        "name": "checksums.sha256.txt",
        "digest": "sha256:eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee"
      },
      {
        "name": "zrok_2.0.11_darwin_amd64.tar.gz",
        "digest": "sha256:1111111111111111111111111111111111111111111111111111111111111111"
      },
      {
        "name": "zrok_2.0.11_darwin_arm64.tar.gz",
        "digest": "sha256:2222222222222222222222222222222222222222222222222222222222222222"
      },
      {
        "name": "zrok_2.0.11_linux_amd64.tar.gz",
        "digest": "sha256:3333333333333333333333333333333333333333333333333333333333333333"
      },
      {
        "name": "zrok_2.0.11_linux_arm64.tar.gz",
        "digest": "sha256:4444444444444444444444444444444444444444444444444444444444444444"
      },
      {
        "name": "zrok_2.0.11_linux_armv7.tar.gz",
        "digest": "sha256:5555555555555555555555555555555555555555555555555555555555555555"
      }
    ]
  },
  {
    "tag_name": "v2.0.10",
    "draft": false,
    "prerelease": false,
    "assets": [
      {
        "name": "zrok_2.0.10_linux_amd64.tar.gz",
        "digest": "sha256:6666666666666666666666666666666666666666666666666666666666666666"
      }
    ]
  }
]"#;

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::panic, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use crate::test_support::{dirs_in, UrlDl};

    fn host() -> &'static str {
        let (os, arch) = current_os_arch().unwrap();
        host_platform(os, arch)
    }

    fn fixture() -> Vec<Release> {
        serde_json::from_str(RELEASES_FIXTURE).unwrap()
    }

    fn tgz(members: &[(&str, &[u8])]) -> Vec<u8> {
        let mut b = tar::Builder::new(Vec::new());
        for (name, body) in members {
            let mut h = tar::Header::new_gnu();
            h.set_path(name).unwrap();
            h.set_mode(0o755);
            h.set_size(body.len() as u64);
            h.set_entry_type(tar::EntryType::Regular);
            h.set_cksum();
            b.append(&h, *body).unwrap();
        }
        let mut enc = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        std::io::Write::write_all(&mut enc, &b.into_inner().unwrap()).unwrap();
        enc.finish().unwrap()
    }

    fn zrok_tarball() -> Vec<u8> {
        tgz(&[
            ("CHANGELOG.md", b"changes"),
            ("LICENSE", b"license"),
            ("README.md", b"readme"),
            ("zrok2", b"#!/bin/sh\necho zrok2\n"),
        ])
    }

    fn served(digest: Option<&str>) -> UrlDl {
        let asset = asset_name("v2.0.11", host()).unwrap();
        let json = serde_json::json!([
            { "tag_name": "v1.1.13", "assets": [] },
            {
                "tag_name": "v2.0.11",
                "assets": [
                    { "name": "checksums.sha256.txt", "digest": format!("sha256:{}", "0".repeat(64)) },
                    { "name": asset, "digest": digest },
                ],
            },
        ]);
        let mut dl = UrlDl::default().with(RELEASES_API, &json.to_string());
        dl.0.insert(format!("{RELEASE_BASE}/v2.0.11/{asset}"), zrok_tarball());
        dl
    }

    #[test]
    fn host_platform_tokens_are_zroks() {
        let cases = [
            (Os::Macos, Arch::X86_64, "darwin_amd64"),
            (Os::Macos, Arch::Aarch64, "darwin_arm64"),
            (Os::Linux, Arch::X86_64, "linux_amd64"),
            (Os::Linux, Arch::Aarch64, "linux_arm64"),
        ];
        for (os, arch, want) in cases {
            assert_eq!(host_platform(os, arch), want, "{os:?} {arch:?}");
        }
    }

    #[test]
    fn asset_name_cases() {
        let cases: &[(&str, &str, Option<&str>)] = &[
            (
                "v2.0.11",
                "darwin_arm64",
                Some("zrok_2.0.11_darwin_arm64.tar.gz"),
            ),
            (
                "v2.0.11",
                "linux_amd64",
                Some("zrok_2.0.11_linux_amd64.tar.gz"),
            ),
            ("2.0.11", "linux_amd64", None),
        ];
        for (tag, platform, want) in cases {
            assert_eq!(
                asset_name(tag, platform).as_deref(),
                *want,
                "{tag} {platform}"
            );
        }
    }

    #[test]
    fn pick_latest_takes_the_highest_stable_v2() {
        let releases = fixture();
        assert_eq!(pick_latest(&releases).unwrap().tag_name, "v2.0.11");

        let v1_only: Vec<Release> =
            serde_json::from_str(r#"[{"tag_name":"v1.1.13"},{"tag_name":"v1.1.12"}]"#).unwrap();
        assert!(pick_latest(&v1_only).is_none());
        assert!(pick_latest(&[]).is_none());
    }

    #[test]
    fn asset_sha256_picks_exact_asset_and_validates_digest() {
        let releases = fixture();
        let release = pick_latest(&releases).unwrap();
        let cases = [
            ("zrok_2.0.11_linux_amd64.tar.gz", Some("3".repeat(64))),
            ("zrok_2.0.11_darwin_arm64.tar.gz", Some("2".repeat(64))),
            ("zrok_2.0.11_linux_arm.tar.gz", None),
            ("checksums.sha256", None),
        ];
        for (asset, want) in cases {
            assert_eq!(asset_sha256(release, asset), want, "{asset}");
        }

        let good = "AB".repeat(32);
        let accepted = format!("sha256:{good}");
        let sha512 = format!("sha512:{good}");
        let non_hex = format!("sha256:{}", "z".repeat(64));
        let digests = [
            (Some(accepted.as_str()), Some(good.to_ascii_lowercase())),
            (None, None),
            (Some(sha512.as_str()), None),
            (Some("sha256:abc"), None),
            (Some(non_hex.as_str()), None),
        ];
        for (digest, want) in digests {
            let release = Release {
                tag_name: "v2.0.11".to_owned(),
                draft: false,
                prerelease: false,
                assets: vec![Asset {
                    name: "zrok_2.0.11_linux_amd64.tar.gz".to_owned(),
                    digest: digest.map(str::to_owned),
                }],
            };
            assert_eq!(
                asset_sha256(&release, "zrok_2.0.11_linux_amd64.tar.gz"),
                want,
                "{digest:?}"
            );
        }
    }

    #[tokio::test]
    async fn latest_reads_the_highest_v2_tag() {
        let dl = UrlDl::default().with(RELEASES_API, RELEASES_FIXTURE);
        assert_eq!(latest(&dl).await.unwrap(), "v2.0.11");
        assert!(latest(&UrlDl::default()).await.is_err());
        let v1_only = UrlDl::default().with(RELEASES_API, r#"[{"tag_name":"v1.1.13"}]"#);
        assert!(matches!(
            latest(&v1_only).await,
            Err(ToolError::Download(_))
        ));
    }

    #[tokio::test]
    async fn install_places_zrok2_and_marker() {
        let tmp = tempfile::tempdir().unwrap();
        let dirs = dirs_in(tmp.path());
        let digest = format!("sha256:{}", crate::ext_install::sha256_hex(&zrok_tarball()));
        install(&dirs, &served(Some(&digest)), None).await.unwrap();

        let path = super::super::tool_dir(&dirs, Tool::Zrok).join(BINARY);
        assert_eq!(std::fs::read(&path).unwrap(), b"#!/bin/sh\necho zrok2\n");
        assert_eq!(
            super::super::installed_version(&dirs, Tool::Zrok, None).as_deref(),
            Some("v2.0.11")
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o755);
        }
    }

    #[tokio::test]
    async fn install_rejects_wrong_digest() {
        let tmp = tempfile::tempdir().unwrap();
        let dirs = dirs_in(tmp.path());
        let digest = format!("sha256:{}", "f".repeat(64));
        let err = install(&dirs, &served(Some(&digest)), None)
            .await
            .unwrap_err();
        assert!(matches!(err, ToolError::Sha256Mismatch(_)), "{err:?}");
        assert!(!super::super::tool_dir(&dirs, Tool::Zrok).exists());
    }

    #[tokio::test]
    async fn install_requires_a_published_digest() {
        let tmp = tempfile::tempdir().unwrap();
        let dirs = dirs_in(tmp.path());
        let err = install(&dirs, &served(None), None).await.unwrap_err();
        assert!(matches!(err, ToolError::Download(_)), "{err:?}");
        assert!(!super::super::tool_dir(&dirs, Tool::Zrok).exists());
    }
}
