//! pcov "cover" CLI shims (`phpcover`, `php<major>.<minor>cover`).
//!
//! These names are symlinks in `{data}/bin` pointing at *this* `yerd` binary.
//! When `yerd` is invoked under such a name (detected from `argv[0]` before clap),
//! it resolves the matching PHP CLI binary plus that version's `pcov.so` and
//! `exec`s PHP with coverage enabled - leaving the clean `php`/`php<ver>` shims
//! untouched. Unix-only: cover shims are never created on other platforms.

use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

use yerd_platform::{ActivePaths, Paths, PlatformDirs};

use crate::shim::{cli_binary, fail, resolve_default_php};

/// Which PHP a cover alias targets.
enum CoverSpec {
    /// `phpcover` - the default version (resolved at run time).
    Default,
    /// `php<major>.<minor>cover` - an explicit version.
    Version(u8, u8),
}

/// If `argv[0]` is a cover-alias name, run that PHP with pcov enabled and return
/// its exit code (on success `exec` replaces the process and never returns);
/// otherwise `None`, so `main` falls through to the normal CLI.
#[must_use]
pub fn dispatch() -> Option<ExitCode> {
    let arg0 = std::env::args_os().next()?;
    let name = Path::new(&arg0).file_name()?.to_str()?;
    let spec = parse_cover_name(name)?;
    Some(run(&spec))
}

/// Parse a cover-alias basename. Matches `phpcover` and `php<MAJOR>.<MINOR>cover`
/// exactly; returns `None` for `php`, `php<ver>`, and anything else (so a normal
/// `yerd` invocation, or a clean versioned shim, is never intercepted).
fn parse_cover_name(name: &str) -> Option<CoverSpec> {
    let rest = name.strip_prefix("php")?;
    let rest = rest.strip_suffix("cover")?;
    if rest.is_empty() {
        return Some(CoverSpec::Default);
    }
    let (maj, min) = rest.split_once('.')?;
    if maj.is_empty() || min.is_empty() {
        return None;
    }
    let major: u8 = maj.parse().ok()?;
    let minor: u8 = min.parse().ok()?;
    Some(CoverSpec::Version(major, minor))
}

fn run(spec: &CoverSpec) -> ExitCode {
    let dirs = match ActivePaths::new().resolve() {
        Ok(d) => d,
        Err(e) => return fail(format!("cannot resolve yerd directories: {e}")),
    };
    let (php_bin, minor) = match resolve_target(&dirs, spec) {
        Ok(t) => t,
        Err(msg) => return fail(msg),
    };
    let pcov = dirs
        .data
        .join("php-ext")
        .join(format!("php-{minor}"))
        .join("pcov.so");
    if !pcov.is_file() {
        return fail(format!(
            "pcov not installed for PHP {minor} — reinstall PHP or wait for the background fetch"
        ));
    }

    let err = Command::new(&php_bin)
        .arg("-d")
        .arg(format!("extension={}", pcov.display()))
        .arg("-d")
        .arg("pcov.enabled=1")
        .args(std::env::args_os().skip(1))
        .exec();
    if err.kind() == std::io::ErrorKind::NotFound {
        return fail(format!(
            "PHP binary not found at {} ({err}) — reinstall with `yerd install php {minor}`",
            php_bin.display()
        ));
    }
    fail(format!("failed to exec {}: {err}", php_bin.display()))
}

/// Resolve `(php_binary, "major.minor")` for the spec.
fn resolve_target(dirs: &PlatformDirs, spec: &CoverSpec) -> Result<(PathBuf, String), String> {
    match spec {
        CoverSpec::Version(maj, min) => {
            let minor = format!("{maj}.{min}");
            let php = cli_binary(dirs, &minor);
            if php.is_file() {
                Ok((php, minor))
            } else {
                Err(format!(
                    "PHP {minor} is not installed — run `yerd install php {minor}`"
                ))
            }
        }
        CoverSpec::Default => resolve_default_php(dirs)
            .ok_or_else(|| "no PHP installed — run `yerd install php <version>`".to_owned()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_default_and_versioned_cover_names() {
        assert!(matches!(
            parse_cover_name("phpcover"),
            Some(CoverSpec::Default)
        ));
        assert!(matches!(
            parse_cover_name("php8.4cover"),
            Some(CoverSpec::Version(8, 4))
        ));
    }

    #[test]
    fn ignores_non_cover_names() {
        assert!(parse_cover_name("php").is_none());
        assert!(parse_cover_name("php8.4").is_none());
        assert!(parse_cover_name("phpunit").is_none());
        assert!(parse_cover_name("php8.cover").is_none());
        assert!(parse_cover_name("phpx.4cover").is_none());
    }
}
