//! Bundled-PHP and `mise` discovery.
//!
//! Both functions return `(PhpVersion, PathBuf)` tuples sorted by version.
//! Callers (typically `bin/yerdd` at startup) merge the two into the
//! manager's `binaries` map.

use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::time::Duration;

use yerd_core::PhpVersion;
use yerd_platform::PlatformDirs;

use crate::error::PhpError;

/// Filename of the FPM binary inside each per-version install dir.
#[cfg(unix)]
const FPM_BINARY_PATH: &[&str] = &["sbin", "php-fpm"];
#[cfg(not(unix))]
const FPM_BINARY_PATH: &[&str] = &["php-fpm.exe"];

/// Walk `dirs.data / "php"` looking for per-version FPM binaries.
///
/// Layout the caller is expected to ship (produced by `xtask` Phase 2):
///
/// ```text
/// {dirs.data}/php/php-8.3/sbin/php-fpm        (Unix)
/// {dirs.data}\php\php-8.3\php-fpm.exe         (Windows)
/// ```
///
/// Error policy:
/// - `read_dir` returning `ErrorKind::NotFound` → `Ok(vec![])` (empty
///   install — the daemon may still pick up `mise` versions).
/// - any other error → `Err(PhpError::DiscoveryIo)`.
///
/// Result is sorted by `PhpVersion`'s derived `Ord`.
pub fn discover_bundled(dirs: &PlatformDirs) -> Result<Vec<(PhpVersion, PathBuf)>, PhpError> {
    let root = dirs.data.join("php");
    let entries = match std::fs::read_dir(&root) {
        Ok(it) => it,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(source) => return Err(PhpError::DiscoveryIo { dir: root, source }),
    };

    let mut out: Vec<(PhpVersion, PathBuf)> = Vec::new();
    for entry in entries {
        let Ok(entry) = entry else {
            continue;
        };
        let Some(version) = parse_php_dirname(&entry.file_name().to_string_lossy()) else {
            continue;
        };
        let mut binary = entry.path();
        for segment in FPM_BINARY_PATH {
            binary.push(segment);
        }
        if !binary.exists() {
            continue;
        }
        out.push((version, binary));
    }
    out.sort_by_key(|(v, _)| *v);
    Ok(out)
}

/// Parse `"php-8.3"` (case-insensitive on the prefix) into `PhpVersion(8, 3)`.
fn parse_php_dirname(name: &str) -> Option<PhpVersion> {
    let rest = name
        .strip_prefix("php-")
        .or_else(|| name.strip_prefix("PHP-"))?;
    PhpVersion::from_str(rest).ok()
}

/// Best-effort live check via `mise ls --json --global php`.
///
/// Any failure (mise absent → `ENOENT`; non-zero exit; JSON parse failure;
/// timeout) returns `vec![]`. **Never errors.** Silent skip mirrors the
/// rest of the workspace's no-logging convention; the daemon may wrap
/// this call with its own logging.
///
/// Output sorted by `PhpVersion`.
pub async fn discover_mise() -> Vec<(PhpVersion, PathBuf)> {
    let output_fut = tokio::process::Command::new("mise")
        .args(["ls", "--json", "--global", "php"])
        .stderr(std::process::Stdio::null())
        .output();
    let output = match tokio::time::timeout(Duration::from_secs(1), output_fut).await {
        Ok(Ok(o)) if o.status.success() => o,
        _ => return Vec::new(),
    };

    let Ok(parsed) = serde_json::from_slice::<Vec<MiseEntry>>(&output.stdout) else {
        return Vec::new();
    };

    let mut versions: Vec<(PhpVersion, PathBuf)> = parsed
        .into_iter()
        .filter_map(|e| {
            let v = parse_mise_version(&e.version)?;
            let binary = compose_mise_binary(&e.install_path);
            if binary.exists() {
                Some((v, binary))
            } else {
                None
            }
        })
        .collect();
    versions.sort_by_key(|(v, _)| *v);
    versions
}

#[derive(serde::Deserialize)]
struct MiseEntry {
    version: String,
    install_path: String,
}

/// `"8.3.10"` → `PhpVersion(8, 3)`. Returns `None` for `"system"` etc.
fn parse_mise_version(s: &str) -> Option<PhpVersion> {
    let mut parts = s.split('.');
    let major: u8 = parts.next()?.parse().ok()?;
    let minor: u8 = parts.next()?.parse().ok()?;
    Some(PhpVersion::new(major, minor))
}

fn compose_mise_binary(install_path: &str) -> PathBuf {
    let p = Path::new(install_path);
    #[cfg(unix)]
    {
        p.join("bin").join("php-fpm")
    }
    #[cfg(not(unix))]
    {
        p.join("php-fpm.exe")
    }
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod tests {
    use super::*;

    #[test]
    fn parse_php_dirname_accepts_canonical_lowercase() {
        assert_eq!(parse_php_dirname("php-8.3"), Some(PhpVersion::new(8, 3)));
    }

    #[test]
    fn parse_php_dirname_accepts_uppercase_prefix() {
        assert_eq!(parse_php_dirname("PHP-7.4"), Some(PhpVersion::new(7, 4)));
    }

    #[test]
    fn parse_php_dirname_rejects_non_matching() {
        assert_eq!(parse_php_dirname("php8.3"), None);
        assert_eq!(parse_php_dirname("notphp"), None);
        assert_eq!(parse_php_dirname("php-system"), None);
    }

    #[test]
    fn parse_mise_version_accepts_three_part() {
        assert_eq!(parse_mise_version("8.3.10"), Some(PhpVersion::new(8, 3)));
        assert_eq!(parse_mise_version("8.3"), Some(PhpVersion::new(8, 3)));
    }

    #[test]
    fn parse_mise_version_rejects_system() {
        assert_eq!(parse_mise_version("system"), None);
    }

    fn make_dirs(tmp: &Path) -> PlatformDirs {
        PlatformDirs {
            config: tmp.join("cfg"),
            data: tmp.to_path_buf(),
            state: tmp.join("state"),
            cache: tmp.join("cache"),
            runtime: tmp.join("run"),
        }
    }

    #[cfg(unix)]
    #[test]
    fn discover_bundled_finds_versions_and_sorts() {
        let tmp = tempfile::tempdir().unwrap();
        let dirs = make_dirs(tmp.path());

        // php/php-8.3/sbin/php-fpm
        let v83 = dirs.data.join("php").join("php-8.3").join("sbin");
        std::fs::create_dir_all(&v83).unwrap();
        std::fs::write(v83.join("php-fpm"), b"#!/bin/sh\n").unwrap();

        // php/php-7.4/sbin/php-fpm (out-of-order to exercise sort)
        let v74 = dirs.data.join("php").join("php-7.4").join("sbin");
        std::fs::create_dir_all(&v74).unwrap();
        std::fs::write(v74.join("php-fpm"), b"#!/bin/sh\n").unwrap();

        // php/php-bogus — should be skipped (no PhpVersion parse).
        let bogus = dirs.data.join("php").join("php-bogus");
        std::fs::create_dir_all(bogus).unwrap();

        // php/php-9.0 — present dir but no binary; should be skipped.
        std::fs::create_dir_all(dirs.data.join("php").join("php-9.0")).unwrap();

        let out = discover_bundled(&dirs).unwrap();
        let versions: Vec<PhpVersion> = out.iter().map(|(v, _)| *v).collect();
        assert_eq!(versions, vec![PhpVersion::new(7, 4), PhpVersion::new(8, 3)]);
    }

    #[test]
    fn discover_bundled_missing_root_returns_empty_ok() {
        let tmp = tempfile::tempdir().unwrap();
        let dirs = make_dirs(tmp.path());
        // dirs.data exists (the tempdir), but dirs.data/php does not.
        let out = discover_bundled(&dirs).unwrap();
        assert!(out.is_empty());
    }
}
