//! pcov "cover" CLI shims (`phpcover`, `php<major>.<minor>cover`).
//!
//! These names are symlinks in `{data}/bin` pointing at *this* `yerd` binary.
//! When `yerd` is invoked under such a name (detected from `argv[0]` before clap),
//! it resolves the matching PHP CLI binary plus that version's `pcov.so` and
//! `exec`s PHP with coverage enabled — leaving the clean `php`/`php<ver>` shims
//! untouched. Unix-only: cover shims are never created on other platforms.

use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

use yerd_platform::{ActivePaths, Paths, PlatformDirs};

/// Which PHP a cover alias targets.
enum CoverSpec {
    /// `phpcover` — the default version (resolved at run time).
    Default,
    /// `php<major>.<minor>cover` — an explicit version.
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
    let rest = rest.strip_suffix("cover")?; // must end with `cover` to be a cover alias
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

    // `exec` only returns on failure. argv[0] defaults to the real php path
    // (correct: PHP reads PHP_BINARY from /proc/self/exe and $_SERVER['argv'][0]
    // becomes the real php, not the cover name). pcov is a normal extension, so
    // `-d extension=` (absolute path) is the right directive.
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
        CoverSpec::Default => default_from_shim(dirs)
            .or_else(|| highest_installed(dirs))
            .ok_or_else(|| "no PHP installed — run `yerd install php <version>`".to_owned()),
    }
}

/// `{data}/php/php-<minor>/bin/php`.
fn cli_binary(dirs: &PlatformDirs, minor: &str) -> PathBuf {
    dirs.data
        .join("php")
        .join(format!("php-{minor}"))
        .join("bin")
        .join("php")
}

/// Default via the `php` shim's target (what `php` itself resolves to), if present
/// and the derived binary exists.
fn default_from_shim(dirs: &PlatformDirs) -> Option<(PathBuf, String)> {
    let target = std::fs::read_link(dirs.data.join("bin").join("php")).ok()?;
    let minor = minor_from_php_path(&target)?;
    let php = cli_binary(dirs, &minor);
    php.is_file().then_some((php, minor))
}

/// Extract `"major.minor"` from a `…/php/php-<major>.<minor>/…` path.
fn minor_from_php_path(p: &Path) -> Option<String> {
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
fn highest_installed(dirs: &PlatformDirs) -> Option<(PathBuf, String)> {
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

fn fail(msg: String) -> ExitCode {
    eprintln!("yerd: {msg}");
    ExitCode::FAILURE
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
        // Clean versioned + default names and foreign binaries must fall through.
        assert!(parse_cover_name("php").is_none());
        assert!(parse_cover_name("php8.4").is_none());
        assert!(parse_cover_name("phpunit").is_none());
        assert!(parse_cover_name("php8.cover").is_none());
        assert!(parse_cover_name("phpx.4cover").is_none());
    }

    #[test]
    fn minor_from_php_path_extracts_dotted_minor() {
        assert_eq!(
            minor_from_php_path(Path::new("/d/php/php-8.4/bin/php")).as_deref(),
            Some("8.4")
        );
        assert_eq!(minor_from_php_path(Path::new("/d/bin/php")), None);
    }
}
