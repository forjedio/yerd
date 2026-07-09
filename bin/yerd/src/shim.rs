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
///
/// Only meaningful when `php` is a *direct symlink* to a version's binary (the
/// pre-wrapper layout). Once `php` is a Yerd multi-call wrapper, the target has
/// no `php-<ver>` component and this returns `None`, so [`resolve_default_php`]
/// falls through to the config default.
#[must_use]
pub(crate) fn default_from_shim(dirs: &PlatformDirs) -> Option<(PathBuf, String)> {
    let target = std::fs::read_link(dirs.data.join("bin").join("php")).ok()?;
    let minor = minor_from_php_path(&target)?;
    let php = cli_binary(dirs, &minor);
    php.is_file().then_some((php, minor))
}

/// The default PHP `(binary, "major.minor")` from the persisted config
/// (`{config}/yerd.toml`), if it parses and that version's CLI binary exists.
///
/// Every error (missing/corrupt/locked config, uninstalled default) is swallowed
/// to `None` so a `php` launch never fails on config trouble - the caller falls
/// back to the highest installed version.
#[must_use]
pub(crate) fn default_from_config(dirs: &PlatformDirs) -> Option<(PathBuf, String)> {
    let cfg = yerd_config::Config::load(&dirs.config.join("yerd.toml")).ok()?;
    let v = cfg.php.default;
    let minor = format!("{}.{}", v.major, v.minor);
    let php = cli_binary(dirs, &minor);
    php.is_file().then_some((php, minor))
}

/// Path to a version's generated CLI ini (`{data}/php-cli-<minor>.ini`).
#[must_use]
pub(crate) fn cli_ini_path(dirs: &PlatformDirs, minor: &str) -> PathBuf {
    dirs.data.join(format!("php-cli-{minor}.ini"))
}

/// The `PHPRC` target for a version's CLI: its per-version ini if present, else
/// the base `php-cli.ini` if present, else `None` (leave `PHPRC` unset).
#[must_use]
pub(crate) fn cli_phprc(dirs: &PlatformDirs, minor: &str) -> Option<PathBuf> {
    let per_version = cli_ini_path(dirs, minor);
    if per_version.is_file() {
        return Some(per_version);
    }
    let base = dirs.data.join("php-cli.ini");
    base.is_file().then_some(base)
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

/// Resolve the default PHP `(binary, "major.minor")`: the config default first
/// (authoritative), then a legacy direct-symlink `php` shim target, then the
/// highest installed version. `None` when no PHP is installed.
#[must_use]
pub(crate) fn resolve_default_php(dirs: &PlatformDirs) -> Option<(PathBuf, String)> {
    default_from_config(dirs)
        .or_else(|| default_from_shim(dirs))
        .or_else(|| highest_installed(dirs))
}

/// Print `yerd: <msg>` to stderr and return a failure exit code.
pub(crate) fn fail(msg: String) -> ExitCode {
    eprintln!("yerd: {msg}");
    ExitCode::FAILURE
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
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

    #[test]
    fn cli_phprc_prefers_per_version_then_base_then_none() {
        let tmp = tempfile::tempdir().unwrap();
        let dirs = PlatformDirs {
            config: tmp.path().join("c"),
            data: tmp.path().join("d"),
            state: tmp.path().join("s"),
            cache: tmp.path().join("ca"),
            runtime: tmp.path().join("r"),
        };
        std::fs::create_dir_all(&dirs.data).unwrap();

        assert_eq!(cli_phprc(&dirs, "8.4"), None);

        let base = dirs.data.join("php-cli.ini");
        std::fs::write(&base, "; base\n").unwrap();
        assert_eq!(cli_phprc(&dirs, "8.4"), Some(base.clone()));

        let per_version = dirs.data.join("php-cli-8.4.ini");
        std::fs::write(&per_version, "; per-version\n").unwrap();
        assert_eq!(cli_phprc(&dirs, "8.4"), Some(per_version));
    }
}
