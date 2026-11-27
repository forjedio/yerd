//! Which `cloudflared` binary tunnels use, and the `{data}/tunnel` dir.
//!
//! The managed copy is a regular tool (`tools::cloudflared`, installed under
//! `{data}/tools/cloudflared/`); a `cloudflared` already on the user's `PATH`
//! that passes the minimum-version gate is used when no managed copy exists.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use async_trait::async_trait;
use yerd_platform::PlatformDirs;

use crate::tools::{self, Tool};

/// `{data}/tunnel`.
pub(crate) fn tunnel_dir(dirs: &PlatformDirs) -> PathBuf {
    dirs.data.join("tunnel")
}

/// Whether the managed `cloudflared` binary is installed.
#[must_use]
pub fn is_installed(dirs: &PlatformDirs) -> bool {
    tools::cloudflared::binary_path(dirs).is_file()
}

/// Where the `cloudflared` binary Yerd is using came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloudflaredSource {
    /// Downloaded and verified by Yerd into `{data}/tools/cloudflared`.
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
#[async_trait]
pub trait PathSearch: Send + Sync + 'static {
    /// Find an executable named `cloudflared` on the host, or `None`.
    async fn find_cloudflared(&self) -> Option<PathBuf>;
}

/// The real `PATH` search.
///
/// `yerdd` is normally launched by a `launchd`/`SMAppService` `LaunchAgent`
/// (macOS) or a `systemd --user` unit (Linux), neither of which sets an
/// `Environment`/`EnvironmentVariables` override, so the daemon's own
/// inherited `PATH` is a stripped service-manager default that omits Homebrew
/// or other user-level install directories - it only looks complete when
/// developing under `cargo run` from an interactive shell. So this checks the
/// daemon's own `PATH` first (cheap, catches that dev/test case and anything
/// already on the restricted default `PATH`), then falls back to
/// [`crate::tools::external::resolve_user_path`] - the same
/// interactive-login-shell resolution `tools::external` already uses to find
/// Homebrew/fnm/global-Composer installs for the exact same reason.
pub struct RealPathSearch;

#[async_trait]
impl PathSearch for RealPathSearch {
    async fn find_cloudflared(&self) -> Option<PathBuf> {
        if let Some(path) = std::env::var_os("PATH") {
            if let Some(found) = find_in_paths(&path) {
                return Some(found);
            }
        }
        let user_dirs = crate::tools::external::resolve_user_path().await?;
        find_in_dirs(user_dirs)
    }
}

/// Search `dirs` in order for an executable file named `cloudflared`; first
/// match wins.
fn find_in_dirs(dirs: impl IntoIterator<Item = PathBuf>) -> Option<PathBuf> {
    dirs.into_iter().find_map(|dir| {
        let candidate = dir.join("cloudflared");
        is_executable(&candidate).then_some(candidate)
    })
}

/// The `PATH`-search logic, taking the `PATH` value directly so it's testable
/// without mutating the process environment.
fn find_in_paths(path_var: &std::ffi::OsStr) -> Option<PathBuf> {
    find_in_dirs(std::env::split_paths(path_var))
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

/// Well past when all of `--no-autoupdate`/`--origincert`/`--config`/`--url`/
/// `--credentials-file`/`--overwrite-dns`, the `TUNNEL_METRICS` env var, and the
/// `/ready` metrics endpoint were stable in `cloudflared`, so any release at or
/// above this floor is assumed to support everything Yerd depends on.
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
            binary: tools::cloudflared::binary_path(dirs),
            source: CloudflaredSource::Managed,
            version: tools::installed_version(dirs, Tool::Cloudflared, None),
        });
    }

    let candidate = path_search.find_cloudflared().await?;
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

    #[cfg(unix)]
    fn make_executable(path: &Path) -> std::io::Result<()> {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))
    }

    #[cfg(not(unix))]
    fn make_executable(_path: &Path) -> std::io::Result<()> {
        Ok(())
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
        std::fs::write(second.join("cloudflared"), b"#!/bin/sh\n").unwrap();
        make_executable(&second.join("cloudflared")).unwrap();
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

    #[async_trait]
    impl PathSearch for PanicsIfCalled {
        async fn find_cloudflared(&self) -> Option<PathBuf> {
            panic!("managed binary should short-circuit before any PATH search runs");
        }
    }

    /// A fixed `PATH` candidate, standing in for `RealPathSearch` without
    /// touching the process's real `PATH` (parallel test execution would make
    /// that flaky).
    struct FakePathSearch(Option<PathBuf>);

    #[async_trait]
    impl PathSearch for FakePathSearch {
        async fn find_cloudflared(&self) -> Option<PathBuf> {
            self.0.clone()
        }
    }

    #[tokio::test]
    async fn resolve_prefers_managed_over_system_without_probing() {
        let tmp = tempfile::tempdir().unwrap();
        let dirs = dirs_in(tmp.path());
        tools::cloudflared::install_binary_for_test(&dirs, "2026.6.1", b"#!/bin/sh\n");

        let resolved = resolve(&dirs, &PanicsIfCalled, &PanicsIfCalled)
            .await
            .unwrap();
        assert_eq!(resolved.source, CloudflaredSource::Managed);
        assert_eq!(resolved.binary, tools::cloudflared::binary_path(&dirs));
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
        make_executable(&candidate).unwrap();

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
        make_executable(&candidate).unwrap();

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
        make_executable(&candidate).unwrap();

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
        make_executable(&candidate).unwrap();

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
