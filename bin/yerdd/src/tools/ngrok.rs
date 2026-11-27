//! ngrok installer - the ngrok v3 agent, a single static binary.
//!
//! ngrok's download pages link unversioned `-stable-` archives with no
//! checksum, so the source is ngrok's official versioned download archive page
//! ([`ARCHIVE_URL`], newest release first, the same page Homebrew's cask
//! livecheck reads). Each release block lists a
//! `https://bin.ngrok.com/a/<id>/ngrok-v3-<ver>-<os>-<arch>.tar.gz` link
//! followed by its SHA-256. Only the first (newest) block is read.
//!
//! Integrity is **fail-closed**: the download is verified against the SHA-256
//! published beside its link on that page, and the install is refused when the
//! version, link, or hash cannot be found. The link comes from fetched metadata,
//! so it must also be `https` on exactly [`DOWNLOAD_HOST`]. This is the same
//! trust model as a GitHub release `digest`: the hash is served by the
//! publisher's metadata endpoint over TLS, the bytes by a separate CDN host.
//!
//! The binary lands at `{data}/tools/ngrok/ngrok` and is exposed as `ngrok`
//! through the yerd multi-call shim, which adds the outbound proxy environment.

use yerd_php::{current_os_arch, Arch, Downloader, Os};
use yerd_platform::PlatformDirs;

use super::{
    extract_file_from_tgz, note, stage_and_swap, verify_sha256, write_executable, ProgressTx, Tool,
    ToolError,
};

/// ngrok's official versioned download archive (HTML, newest release first).
const ARCHIVE_URL: &str = "https://ngrok.com/download/archive/ngrok/ngrok-v3/stable/ngrok_archive";

/// The only host an archive link may download from.
const DOWNLOAD_HOST: &str = "bin.ngrok.com";

/// File name of the binary in the archive and in the tool dir.
const BINARY: &str = "ngrok";

/// Marks the start of a release block (and its version) on the archive page.
const RELEASE_HEADING: &str = "<h2>Version ";

/// The newest release's `.tar.gz` for one platform, as published on the
/// archive page.
#[derive(Debug, Clone, PartialEq, Eq)]
struct NgrokRelease {
    version: String,
    url: String,
    sha256: String,
}

/// ngrok's `<os>-<arch>` token in artifact names. ngrok uses `amd64`/`arm64`,
/// not the `x86_64`/`aarch64` that `Arch::as_str` renders.
fn host_platform(os: Os, arch: Arch) -> &'static str {
    match (os, arch) {
        (Os::Macos, Arch::X86_64) => "darwin-amd64",
        (Os::Macos, Arch::Aarch64) => "darwin-arm64",
        (Os::Linux, Arch::X86_64) => "linux-amd64",
        (Os::Linux, Arch::Aarch64) => "linux-arm64",
    }
}

/// A plain `N.N.N` version (ASCII digits and two dots, nothing else), safe to
/// interpolate into an asset name and store as the version marker.
fn is_plain_version(v: &str) -> bool {
    let parts: Vec<&str> = v.split('.').collect();
    parts.len() == 3
        && parts
            .iter()
            .all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()))
}

/// The version in the first `<h2>Version X.Y.Z</h2>` heading, when it is a
/// plain `N.N.N`.
fn parse_latest_version(html: &str) -> Option<String> {
    let start = html.find(RELEASE_HEADING)? + RELEASE_HEADING.len();
    let rest = html.get(start..)?;
    let version = rest.get(..rest.find('<')?)?.trim();
    is_plain_version(version).then(|| version.to_owned())
}

/// The first release block: from the first release heading to the next `<h2>`
/// (or the end of the page).
fn first_release_block(html: &str) -> Option<&str> {
    let start = html.find(RELEASE_HEADING)?;
    let block = html.get(start..)?;
    let body = block.get(RELEASE_HEADING.len()..)?;
    let end = body
        .find("<h2>")
        .map_or(block.len(), |i| i + RELEASE_HEADING.len());
    block.get(..end)
}

/// The value of the first `value="…"` attribute in `text`, when it is a 64-char
/// hex SHA-256, lowercased.
fn first_sha256(text: &str) -> Option<String> {
    let start = text.find("value=\"")? + "value=\"".len();
    let rest = text.get(start..)?;
    let hex = rest.get(..rest.find('"')?)?;
    (hex.len() == 64 && hex.bytes().all(|b| b.is_ascii_hexdigit()))
        .then(|| hex.to_ascii_lowercase())
}

/// The newest release's `.tar.gz` link and SHA-256 for `platform`, read only
/// from the first release block. The SHA is the first `value="…"` after the
/// link and before the next `href="`. `None` when the version, the link, or a
/// well-formed SHA is missing.
fn parse_release(html: &str, platform: &str) -> Option<NgrokRelease> {
    let block = first_release_block(html)?;
    let version = parse_latest_version(block)?;
    let suffix = format!("/ngrok-v3-{version}-{platform}.tar.gz");
    let mut rest = block;
    while let Some(i) = rest.find("href=\"") {
        let after = rest.get(i + "href=\"".len()..)?;
        let url = after.get(..after.find('"')?)?;
        let tail = after.get(url.len()..)?;
        if url.ends_with(&suffix) {
            let scope = tail.get(..tail.find("href=\"").unwrap_or(tail.len()))?;
            return first_sha256(scope).map(|sha256| NgrokRelease {
                version,
                url: url.to_owned(),
                sha256,
            });
        }
        rest = tail;
    }
    None
}

/// Whether `url` is `https` on exactly [`DOWNLOAD_HOST`]. The authority ends at
/// the first `/`, `?`, `#` or `\`, and userinfo and port are stripped, so a
/// crafted link cannot smuggle the trusted name past a different real host.
fn host_is_ngrok(url: &str) -> bool {
    let Some(rest) = url.strip_prefix("https://") else {
        return false;
    };
    let authority = rest.split(['/', '?', '#', '\\']).next().unwrap_or(rest);
    let hostport = authority.rsplit('@').next().unwrap_or(authority);
    let host = hostport.split(':').next().unwrap_or(hostport);
    host.eq_ignore_ascii_case(DOWNLOAD_HOST)
}

/// Fetch the archive page as text.
async fn fetch_archive(dl: &dyn Downloader) -> Result<String, ToolError> {
    let bytes = dl
        .download(ARCHIVE_URL)
        .await
        .map_err(|e| ToolError::Download(format!("ngrok archive page: {e}")))?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

/// The latest ngrok version (`3.39.13`), for the update poll.
pub(crate) async fn latest(dl: &dyn Downloader) -> Result<String, ToolError> {
    let html = fetch_archive(dl).await?;
    parse_latest_version(&html)
        .ok_or_else(|| ToolError::Download("ngrok: no version on archive page".to_owned()))
}

/// Install the latest ngrok for the host into `{data}/tools/ngrok/`.
pub async fn install(
    dirs: &PlatformDirs,
    dl: &dyn Downloader,
    progress: Option<&ProgressTx>,
) -> Result<(), ToolError> {
    let (os, arch) = current_os_arch().map_err(|_| ToolError::UnsupportedHost("ngrok"))?;
    let platform = host_platform(os, arch);

    note(progress, "Fetching ngrok release info…");
    let html = fetch_archive(dl).await?;
    let release = parse_release(&html, platform).ok_or_else(|| {
        ToolError::Sha256Mismatch(format!(
            "refusing to install unverified ngrok: no SHA256 for the {platform} .tar.gz on the archive page"
        ))
    })?;
    if !host_is_ngrok(&release.url) {
        return Err(ToolError::Sha256Mismatch(format!(
            "refusing to download from untrusted host: {}",
            release.url
        )));
    }

    let asset = format!("ngrok-v3-{}-{platform}.tar.gz", release.version);
    note(progress, format!("Downloading {asset}…"));
    let bytes = dl
        .download(&release.url)
        .await
        .map_err(|e| ToolError::Download(format!("{asset}: {e}")))?;
    verify_and_place(dirs, &release, &asset, &bytes)?;
    note(progress, format!("Installed ngrok {}", release.version));
    tracing::info!(version = %release.version, "installed ngrok");
    Ok(())
}

/// Verify the downloaded tarball and stage its `ngrok` binary. Synchronous for
/// the reason `pnpm::verify_and_place` documents.
fn verify_and_place(
    dirs: &PlatformDirs,
    release: &NgrokRelease,
    asset: &str,
    bytes: &[u8],
) -> Result<(), ToolError> {
    verify_sha256(bytes, &release.sha256, asset)?;
    let binary = extract_file_from_tgz(bytes, BINARY, asset)?;
    stage_and_swap(dirs, Tool::Ngrok, &release.version, |staging| {
        write_executable(staging, BINARY, &binary)
    })
}

/// A trimmed copy of the real archive page: two release blocks (3.39.13, then
/// 3.39.12), each with `.zip` and `.tar.gz` links for every supported platform
/// and a distinct SHA per link. Shared with the `tools` and `tool_updates` tests.
#[cfg(test)]
pub(crate) const ARCHIVE_FIXTURE: &str = r#"<!DOCTYPE html>
<html><head><title>ngrok - Download Archive</title></head><body>
<h1>ngrok-v3 stable</h1>
<p>Powered by Equinox.io</p>
<div class="release"><h2>Version 3.39.13</h2>
<ul>
<li><a href="https://bin.ngrok.com/a/b2Xq/ngrok-v3-3.39.13-darwin-amd64.zip">ngrok-v3-3.39.13-darwin-amd64.zip</a> <input type="text" readonly value="1414141414141414141414141414141414141414141414141414141414141414"> SHA256</li>
<li><a href="https://bin.ngrok.com/a/b2Xq/ngrok-v3-3.39.13-darwin-amd64.tar.gz">ngrok-v3-3.39.13-darwin-amd64.tar.gz</a> <input type="text" readonly value="2525252525252525252525252525252525252525252525252525252525252525"> SHA256</li>
<li><a href="https://bin.ngrok.com/a/b2Xq/ngrok-v3-3.39.13-darwin-arm64.zip">ngrok-v3-3.39.13-darwin-arm64.zip</a> <input type="text" readonly value="3636363636363636363636363636363636363636363636363636363636363636"> SHA256</li>
<li><a href="https://bin.ngrok.com/a/b2Xq/ngrok-v3-3.39.13-darwin-arm64.tar.gz">ngrok-v3-3.39.13-darwin-arm64.tar.gz</a> <input type="text" readonly value="4747474747474747474747474747474747474747474747474747474747474747"> SHA256</li>
<li><a href="https://bin.ngrok.com/a/b2Xq/ngrok-v3-3.39.13-linux-amd64.zip">ngrok-v3-3.39.13-linux-amd64.zip</a> <input type="text" readonly value="5858585858585858585858585858585858585858585858585858585858585858"> SHA256</li>
<li><a href="https://bin.ngrok.com/a/b2Xq/ngrok-v3-3.39.13-linux-amd64.tar.gz">ngrok-v3-3.39.13-linux-amd64.tar.gz</a> <input type="text" readonly value="6969696969696969696969696969696969696969696969696969696969696969"> SHA256</li>
<li><a href="https://bin.ngrok.com/a/b2Xq/ngrok-v3-3.39.13-linux-arm64.zip">ngrok-v3-3.39.13-linux-arm64.zip</a> <input type="text" readonly value="7a7a7a7a7a7a7a7a7a7a7a7a7a7a7a7a7a7a7a7a7a7a7a7a7a7a7a7a7a7a7a7a"> SHA256</li>
<li><a href="https://bin.ngrok.com/a/b2Xq/ngrok-v3-3.39.13-linux-arm64.tar.gz">ngrok-v3-3.39.13-linux-arm64.tar.gz</a> <input type="text" readonly value="8b8b8b8b8b8b8b8b8b8b8b8b8b8b8b8b8b8b8b8b8b8b8b8b8b8b8b8b8b8b8b8b"> SHA256</li>
</ul></div>
<div class="release"><h2>Version 3.39.12</h2>
<ul>
<li><a href="https://bin.ngrok.com/a/a9Lm/ngrok-v3-3.39.12-darwin-amd64.zip">ngrok-v3-3.39.12-darwin-amd64.zip</a> <input type="text" readonly value="9c9c9c9c9c9c9c9c9c9c9c9c9c9c9c9c9c9c9c9c9c9c9c9c9c9c9c9c9c9c9c9c"> SHA256</li>
<li><a href="https://bin.ngrok.com/a/a9Lm/ngrok-v3-3.39.12-darwin-amd64.tar.gz">ngrok-v3-3.39.12-darwin-amd64.tar.gz</a> <input type="text" readonly value="adadadadadadadadadadadadadadadadadadadadadadadadadadadadadadadad"> SHA256</li>
<li><a href="https://bin.ngrok.com/a/a9Lm/ngrok-v3-3.39.12-darwin-arm64.zip">ngrok-v3-3.39.12-darwin-arm64.zip</a> <input type="text" readonly value="bebebebebebebebebebebebebebebebebebebebebebebebebebebebebebebebe"> SHA256</li>
<li><a href="https://bin.ngrok.com/a/a9Lm/ngrok-v3-3.39.12-darwin-arm64.tar.gz">ngrok-v3-3.39.12-darwin-arm64.tar.gz</a> <input type="text" readonly value="cfcfcfcfcfcfcfcfcfcfcfcfcfcfcfcfcfcfcfcfcfcfcfcfcfcfcfcfcfcfcfcf"> SHA256</li>
<li><a href="https://bin.ngrok.com/a/a9Lm/ngrok-v3-3.39.12-linux-amd64.zip">ngrok-v3-3.39.12-linux-amd64.zip</a> <input type="text" readonly value="d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0"> SHA256</li>
<li><a href="https://bin.ngrok.com/a/a9Lm/ngrok-v3-3.39.12-linux-amd64.tar.gz">ngrok-v3-3.39.12-linux-amd64.tar.gz</a> <input type="text" readonly value="e1e1e1e1e1e1e1e1e1e1e1e1e1e1e1e1e1e1e1e1e1e1e1e1e1e1e1e1e1e1e1e1"> SHA256</li>
<li><a href="https://bin.ngrok.com/a/a9Lm/ngrok-v3-3.39.12-linux-arm64.zip">ngrok-v3-3.39.12-linux-arm64.zip</a> <input type="text" readonly value="f2f2f2f2f2f2f2f2f2f2f2f2f2f2f2f2f2f2f2f2f2f2f2f2f2f2f2f2f2f2f2f2"> SHA256</li>
<li><a href="https://bin.ngrok.com/a/a9Lm/ngrok-v3-3.39.12-linux-arm64.tar.gz">ngrok-v3-3.39.12-linux-arm64.tar.gz</a> <input type="text" readonly value="0303030303030303030303030303030303030303030303030303030303030303"> SHA256</li>
</ul></div>
</body></html>
"#;

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::panic, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use crate::test_support::{dirs_in, UrlDl};

    /// The fixture's 3.39.13 `.tar.gz` SHA per platform.
    const TGZ_SHA: [(&str, &str); 4] = [
        (
            "darwin-amd64",
            "2525252525252525252525252525252525252525252525252525252525252525",
        ),
        (
            "darwin-arm64",
            "4747474747474747474747474747474747474747474747474747474747474747",
        ),
        (
            "linux-amd64",
            "6969696969696969696969696969696969696969696969696969696969696969",
        ),
        (
            "linux-arm64",
            "8b8b8b8b8b8b8b8b8b8b8b8b8b8b8b8b8b8b8b8b8b8b8b8b8b8b8b8b8b8b8b8b",
        ),
    ];

    fn fixture_sha(platform: &str) -> &'static str {
        TGZ_SHA.iter().find(|(p, _)| *p == platform).unwrap().1
    }

    fn host() -> &'static str {
        let (os, arch) = current_os_arch().unwrap();
        host_platform(os, arch)
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

    fn host_url() -> String {
        format!(
            "https://bin.ngrok.com/a/b2Xq/ngrok-v3-3.39.13-{}.tar.gz",
            host()
        )
    }

    fn served(page: &str, url: &str, tarball: Vec<u8>) -> UrlDl {
        let mut dl = UrlDl::default().with(ARCHIVE_URL, page);
        dl.0.insert(url.to_owned(), tarball);
        dl
    }

    #[test]
    fn host_platform_tokens_are_ngroks() {
        let cases = [
            (Os::Macos, Arch::X86_64, "darwin-amd64"),
            (Os::Macos, Arch::Aarch64, "darwin-arm64"),
            (Os::Linux, Arch::X86_64, "linux-amd64"),
            (Os::Linux, Arch::Aarch64, "linux-arm64"),
        ];
        for (os, arch, want) in cases {
            assert_eq!(host_platform(os, arch), want, "{os:?} {arch:?}");
        }
    }

    #[test]
    fn parse_latest_version_cases() {
        let cases: &[(&str, Option<&str>)] = &[
            (ARCHIVE_FIXTURE, Some("3.39.13")),
            ("<h2>Version  3.39.13 </h2>", Some("3.39.13")),
            ("<h2>Version 3.39.13-beta</h2>", None),
            ("<h2>Version 1/../2</h2>", None),
            ("<h2>Version 3.39</h2>", None),
            ("<h2>Version 3.39.13", None),
            ("<h2>Release 3.39.13</h2>", None),
            ("", None),
        ];
        for (html, want) in cases {
            assert_eq!(parse_latest_version(html).as_deref(), *want, "{html:?}");
        }
    }

    #[test]
    fn parse_release_picks_newest_tar_gz_per_platform() {
        for (platform, sha) in TGZ_SHA {
            let got = parse_release(ARCHIVE_FIXTURE, platform).unwrap();
            assert_eq!(
                got,
                NgrokRelease {
                    version: "3.39.13".to_owned(),
                    url: format!("https://bin.ngrok.com/a/b2Xq/ngrok-v3-3.39.13-{platform}.tar.gz"),
                    sha256: sha.to_owned(),
                },
                "{platform}"
            );
        }
    }

    #[test]
    fn parse_release_rejects_bad_or_missing_entries() {
        let sha = fixture_sha("linux-amd64");
        let upper = ARCHIVE_FIXTURE.replace(sha, &sha.to_ascii_uppercase());
        assert_eq!(
            parse_release(&upper, "linux-amd64").unwrap().sha256,
            sha.to_owned()
        );

        let non_hex = ARCHIVE_FIXTURE.replace(sha, &"z".repeat(64));
        let short = ARCHIVE_FIXTURE.replace(sha, "abc");
        let no_link = ARCHIVE_FIXTURE.replace("ngrok-v3-3.39.13-linux-amd64.tar.gz", "gone");
        let line = ARCHIVE_FIXTURE
            .lines()
            .find(|l| l.contains("ngrok-v3-3.39.13-linux-amd64.tar.gz"))
            .unwrap();
        let older_only = ARCHIVE_FIXTURE.replace(&format!("{line}\n"), "").replace(
            "<h2>Version 3.39.12</h2>",
            &format!("<h2>Version 3.39.12</h2>\n{line}"),
        );
        let cases = [
            ("non-hex sha", non_hex.as_str()),
            ("short sha", short.as_str()),
            ("missing link", no_link.as_str()),
            ("only an older block has it", older_only.as_str()),
            ("empty page", ""),
        ];
        for (name, html) in cases {
            assert_eq!(parse_release(html, "linux-amd64"), None, "{name}");
        }
        assert_eq!(parse_release(ARCHIVE_FIXTURE, "windows-amd64"), None);
    }

    #[test]
    fn host_is_ngrok_cases() {
        let cases = [
            (
                "https://bin.ngrok.com/a/x/ngrok-v3-3.39.13-linux-amd64.tar.gz",
                true,
            ),
            ("https://BIN.NGROK.COM:443/a/x/ngrok.tar.gz", true),
            ("http://bin.ngrok.com/a/x/ngrok.tar.gz", false),
            ("https://bin.ngrok.com.evil.example/a/x", false),
            ("https://evil.example/?u=bin.ngrok.com", false),
            ("https://user@evil.example/a/x", false),
            ("https://evil.example#@bin.ngrok.com/a/x", false),
            ("https://evil.example\\@bin.ngrok.com/a/x", false),
            ("https://ngrok.com/a/x", false),
            ("bin.ngrok.com/a/x", false),
        ];
        for (url, want) in cases {
            assert_eq!(host_is_ngrok(url), want, "{url}");
        }
    }

    #[tokio::test]
    async fn latest_reads_the_first_version_heading() {
        let dl = UrlDl::default().with(ARCHIVE_URL, ARCHIVE_FIXTURE);
        assert_eq!(latest(&dl).await.unwrap(), "3.39.13");
        assert!(latest(&UrlDl::default()).await.is_err());
        let empty = UrlDl::default().with(ARCHIVE_URL, "<html></html>");
        assert!(matches!(latest(&empty).await, Err(ToolError::Download(_))));
    }

    #[tokio::test]
    async fn install_places_binary_and_marker() {
        let tmp = tempfile::tempdir().unwrap();
        let dirs = dirs_in(tmp.path());
        let body = b"#!/bin/sh\necho ngrok\n";
        let tarball = tgz(&[("LICENSE", b"license"), ("ngrok", body)]);
        let page = ARCHIVE_FIXTURE.replace(
            fixture_sha(host()),
            &crate::ext_install::sha256_hex(&tarball),
        );

        install(&dirs, &served(&page, &host_url(), tarball), None)
            .await
            .unwrap();

        let path = super::super::tool_dir(&dirs, Tool::Ngrok).join(BINARY);
        assert_eq!(std::fs::read(&path).unwrap(), body);
        assert_eq!(
            super::super::installed_version(&dirs, Tool::Ngrok, None).as_deref(),
            Some("3.39.13")
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o755);
        }
    }

    #[tokio::test]
    async fn install_rejects_wrong_sha() {
        let tmp = tempfile::tempdir().unwrap();
        let dirs = dirs_in(tmp.path());
        let tarball = tgz(&[("ngrok", b"bin")]);
        let err = install(&dirs, &served(ARCHIVE_FIXTURE, &host_url(), tarball), None)
            .await
            .unwrap_err();
        assert!(matches!(err, ToolError::Sha256Mismatch(_)), "{err:?}");
        assert!(!super::super::tool_dir(&dirs, Tool::Ngrok).exists());
    }

    #[tokio::test]
    async fn install_rejects_untrusted_host() {
        let tmp = tempfile::tempdir().unwrap();
        let dirs = dirs_in(tmp.path());
        let tarball = tgz(&[("ngrok", b"bin")]);
        let evil = host_url().replace("bin.ngrok.com", "evil.example");
        let page = ARCHIVE_FIXTURE.replace(&host_url(), &evil).replace(
            fixture_sha(host()),
            &crate::ext_install::sha256_hex(&tarball),
        );
        let err = install(&dirs, &served(&page, &evil, tarball), None)
            .await
            .unwrap_err();
        assert!(
            matches!(&err, ToolError::Sha256Mismatch(m) if m.contains("untrusted host")),
            "{err:?}"
        );
        assert!(!super::super::tool_dir(&dirs, Tool::Ngrok).exists());
    }
}
