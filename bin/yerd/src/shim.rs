//! Shared helpers for the multi-call shims (`phpcover`/`php<ver>cover` and
//! `composer`). When the `yerd` binary is invoked under one of those symlinked
//! names, it resolves the right managed PHP and `exec`s it; these helpers do the
//! version resolution against `{data}/php` and the `php` default shim.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use yerd_platform::PlatformDirs;

/// `{data}/php/php-<minor>/bin/php`.
#[must_use]
pub(crate) fn cli_binary(dirs: &PlatformDirs, minor: &str) -> PathBuf {
    dirs.data
        .join("php")
        .join(format!("php-{minor}"))
        .join("bin")
        .join("php")
}

/// The default PHP `(binary, "major.minor")` via the `php` shim's target, if the
/// shim is present and the derived binary exists.
#[must_use]
pub(crate) fn default_from_shim(dirs: &PlatformDirs) -> Option<(PathBuf, String)> {
    let target = std::fs::read_link(dirs.data.join("bin").join("php")).ok()?;
    let minor = minor_from_php_path(&target)?;
    let php = cli_binary(dirs, &minor);
    php.is_file().then_some((php, minor))
}

/// Extract `"major.minor"` from a `…/php/php-<major>.<minor>/…` path.
#[must_use]
pub(crate) fn minor_from_php_path(p: &Path) -> Option<String> {
    p.components().find_map(|c| {
        let rest = c.as_os_str().to_str()?.strip_prefix("php-")?;
        let (maj, min) = rest.split_once('.')?;
        let ok = !maj.is_empty()
            && !min.is_empty()
            && maj.bytes().all(|b| b.is_ascii_digit())
            && min.bytes().all(|b| b.is_ascii_digit());
        ok.then(|| rest.to_owned())
    })
}

/// Highest installed minor under `{data}/php` whose CLI binary exists.
#[must_use]
pub(crate) fn highest_installed(dirs: &PlatformDirs) -> Option<(PathBuf, String)> {
    let root = dirs.data.join("php");
    let mut best: Option<(u8, u8)> = None;
    for entry in std::fs::read_dir(&root).ok()?.flatten() {
        let fname = entry.file_name();
        let Some(name) = fname.to_str() else { continue };
        let Some(rest) = name.strip_prefix("php-") else {
            continue;
        };
        let Some((maj, min)) = rest.split_once('.') else {
            continue;
        };
        let (Ok(maj), Ok(min)) = (maj.parse::<u8>(), min.parse::<u8>()) else {
            continue;
        };
        if !cli_binary(dirs, rest).is_file() {
            continue;
        }
        if best.map_or(true, |b| (maj, min) > b) {
            best = Some((maj, min));
        }
    }
    let (maj, min) = best?;
    let minor = format!("{maj}.{min}");
    Some((cli_binary(dirs, &minor), minor))
}

/// Resolve the default PHP `(binary, "major.minor")`: the `php` shim's target if
/// set, else the highest installed version. `None` when no PHP is installed.
#[must_use]
pub(crate) fn resolve_default_php(dirs: &PlatformDirs) -> Option<(PathBuf, String)> {
    default_from_shim(dirs).or_else(|| highest_installed(dirs))
}

/// Print `yerd: <msg>` to stderr and return a failure exit code.
pub(crate) fn fail(msg: String) -> ExitCode {
    eprintln!("yerd: {msg}");
    ExitCode::FAILURE
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn minor_from_php_path_extracts_dotted_minor() {
        assert_eq!(
            minor_from_php_path(Path::new("/d/php/php-8.4/bin/php")).as_deref(),
            Some("8.4")
        );
        assert_eq!(minor_from_php_path(Path::new("/d/bin/php")), None);
    }
}
