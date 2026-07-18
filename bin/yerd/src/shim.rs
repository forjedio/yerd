//! Shared helpers for the multi-call shims (`phpcover`/`php<ver>cover` and
//! `composer`). When the `yerd` binary is invoked under one of those symlinked
//! names, it resolves the right managed PHP and `exec`s it; these helpers do the
//! version resolution against `{data}/php` and the `php` default shim.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use yerd_core::PhpVersion;
use yerd_platform::PlatformDirs;

/// Whether a `"major.minor"` minor string names a legacy version (< 8.2). A
/// minor that doesn't parse is treated as non-legacy (it will fail elsewhere).
fn minor_is_legacy(minor: &str) -> bool {
    minor.parse::<PhpVersion>().is_ok_and(PhpVersion::is_legacy)
}

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
    if minor_is_legacy(&minor) {
        return None;
    }
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
    if v.is_legacy() {
        return None;
    }
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

/// Whether any PHP version is installed under `{data}/php` (legacy included) -
/// used to distinguish "nothing installed" from "only legacy installed" in the
/// no-default message.
#[must_use]
pub(crate) fn any_installed(dirs: &PlatformDirs) -> bool {
    let root = dirs.data.join("php");
    let Ok(entries) = std::fs::read_dir(&root) else {
        return false;
    };
    for entry in entries.flatten() {
        let fname = entry.file_name();
        let Some(name) = fname.to_str() else { continue };
        let Some(rest) = name.strip_prefix("php-") else {
            continue;
        };
        if cli_binary(dirs, rest).is_file() {
            return true;
        }
    }
    false
}

/// Message for when [`resolve_default_php`] returns `None`: distinguishes
/// "nothing installed" from "only legacy installed" so the guidance is accurate.
/// Shared by every shim that resolves the default PHP (`php`, `phpcover`,
/// `composer`, `laravel`, `wp`).
#[must_use]
pub(crate) fn no_default_php_message(dirs: &PlatformDirs) -> String {
    if any_installed(dirs) {
        "No supported default PHP version. Legacy versions (7.4 / 8.0 / 8.1) can't be the \
         default - install a supported version (e.g. `yerd install php 8.4`), or invoke a \
         legacy version explicitly, e.g. `php7.4`."
            .to_owned()
    } else {
        "no PHP installed - run `yerd install php <version>`".to_owned()
    }
}

/// Highest installed **non-legacy** minor under `{data}/php` whose CLI binary
/// exists. Legacy minors (< 8.2) are skipped so bare `php` never resolves to a
/// legacy interpreter.
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
        if PhpVersion::new(maj, min).is_legacy() {
            continue;
        }
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
/// highest installed version. Every fallback excludes legacy (< 8.2) versions,
/// so bare `php` never runs a legacy interpreter. `None` when no *supported* PHP
/// is installed (even if a legacy version is) - callers render
/// [`no_default_php_message`] to explain.
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

    fn dirs_at(tmp: &Path) -> PlatformDirs {
        PlatformDirs {
            config: tmp.join("c"),
            data: tmp.join("d"),
            state: tmp.join("s"),
            cache: tmp.join("ca"),
            runtime: tmp.join("r"),
        }
    }

    fn fake_cli(dirs: &PlatformDirs, minor: &str) {
        let base = dirs
            .data
            .join("php")
            .join(format!("php-{minor}"))
            .join("bin");
        std::fs::create_dir_all(&base).unwrap();
        std::fs::write(base.join("php"), b"x").unwrap();
    }

    #[test]
    fn highest_installed_skips_legacy_versions() {
        let tmp = tempfile::tempdir().unwrap();
        let dirs = dirs_at(tmp.path());
        fake_cli(&dirs, "8.0");
        fake_cli(&dirs, "8.2");
        assert_eq!(
            highest_installed(&dirs).map(|(_, m)| m),
            Some("8.2".to_owned()),
            "legacy 8.0 is skipped; 8.2 wins"
        );
    }

    #[test]
    fn highest_installed_none_when_only_legacy() {
        let tmp = tempfile::tempdir().unwrap();
        let dirs = dirs_at(tmp.path());
        fake_cli(&dirs, "7.4");
        assert_eq!(highest_installed(&dirs), None);
        assert!(any_installed(&dirs), "7.4 is still installed");
    }

    #[test]
    fn no_default_message_distinguishes_legacy_only_from_empty() {
        let tmp = tempfile::tempdir().unwrap();
        let dirs = dirs_at(tmp.path());
        std::fs::create_dir_all(dirs.data.join("php")).unwrap();
        assert!(no_default_php_message(&dirs).contains("no PHP installed"));
        fake_cli(&dirs, "7.4");
        let msg = no_default_php_message(&dirs);
        assert!(msg.contains("supported"));
        assert!(msg.contains("php7.4"));
    }

    #[test]
    fn minor_is_legacy_splits_at_floor() {
        assert!(minor_is_legacy("7.4"));
        assert!(minor_is_legacy("8.1"));
        assert!(!minor_is_legacy("8.2"));
        assert!(!minor_is_legacy("8.5"));
        assert!(!minor_is_legacy("garbage"));
    }

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
