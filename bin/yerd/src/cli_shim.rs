//! Clean CLI shims (`php`, `php<major>.<minor>`).
//!
//! These names are symlinks in `{data}/bin` pointing at *this* `yerd` binary
//! (like the `phpcover` shims). When `yerd` is invoked under such a name, it
//! resolves the matching PHP CLI binary, points `PHPRC` at that version's
//! generated ini (`{data}/php-cli-<minor>.ini`, which carries the user's global
//! settings **and** that version's registered extensions), and `exec`s PHP.
//! Pointing `PHPRC` per version is what lets a custom extension load in the CLI,
//! and `PHPRC` (rather than `-d`) is inherited by any child PHP the exec'd one
//! spawns. Unix-only: these wrappers are never created on other platforms.

use std::os::unix::process::CommandExt as _;
use std::path::Path;
use std::process::ExitCode;

use yerd_platform::{ActivePaths, Paths, PlatformDirs};

use crate::shim::{cli_binary, cli_phprc, fail, resolve_default_php};

/// Which PHP a clean CLI shim targets.
enum CliSpec {
    /// `php` - the default version (resolved at run time).
    Default,
    /// `php<major>.<minor>` - an explicit version.
    Version(u8, u8),
}

/// If `argv[0]` is a clean CLI shim name (`php` / `php<M>.<N>`), run that PHP with
/// the version's `PHPRC` set and return its exit code (on success `exec` replaces
/// the process and never returns); otherwise `None`, so `main` falls through to
/// the normal CLI. Runs *after* the cover-shim dispatch so `php<ver>cover` is
/// never routed here.
#[must_use]
pub fn dispatch() -> Option<ExitCode> {
    let arg0 = std::env::args_os().next()?;
    let name = Path::new(&arg0).file_name()?.to_str()?;
    let spec = parse_cli_name(name)?;
    Some(run(&spec))
}

/// Parse a clean CLI shim basename. Matches `php` and `php<MAJOR>.<MINOR>`
/// exactly, and **rejects a trailing `cover`** so `php<ver>cover` can never be
/// misrouted here even if dispatch order changed. Returns `None` for `yerd`,
/// `composer`, and anything else.
fn parse_cli_name(name: &str) -> Option<CliSpec> {
    let rest = name.strip_prefix("php")?;
    if rest.ends_with("cover") {
        return None;
    }
    if rest.is_empty() {
        return Some(CliSpec::Default);
    }
    let (maj, min) = rest.split_once('.')?;
    if maj.is_empty() || min.is_empty() {
        return None;
    }
    let major: u8 = maj.parse().ok()?;
    let minor: u8 = min.parse().ok()?;
    Some(CliSpec::Version(major, minor))
}

fn run(spec: &CliSpec) -> ExitCode {
    let dirs = match ActivePaths::new().resolve() {
        Ok(d) => d,
        Err(e) => return fail(format!("cannot resolve yerd directories: {e}")),
    };
    let (php_bin, minor) = match resolve_target(&dirs, spec) {
        Ok(t) => t,
        Err(msg) => return fail(msg),
    };

    let mut cmd = std::process::Command::new(&php_bin);
    if let Some(phprc) = cli_phprc(&dirs, &minor) {
        cmd.env("PHPRC", phprc);
    }
    let err = cmd.args(std::env::args_os().skip(1)).exec();
    if err.kind() == std::io::ErrorKind::NotFound {
        return fail(format!(
            "PHP binary not found at {} ({err}) — reinstall with `yerd install php {minor}`",
            php_bin.display()
        ));
    }
    fail(format!("failed to exec {}: {err}", php_bin.display()))
}

/// Resolve `(php_binary, "major.minor")` for the spec.
fn resolve_target(
    dirs: &PlatformDirs,
    spec: &CliSpec,
) -> Result<(std::path::PathBuf, String), String> {
    match spec {
        CliSpec::Version(maj, min) => {
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
        CliSpec::Default => {
            resolve_default_php(dirs).ok_or_else(|| crate::shim::no_default_php_message(dirs))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_plain_and_versioned_php_names() {
        assert!(matches!(parse_cli_name("php"), Some(CliSpec::Default)));
        assert!(matches!(
            parse_cli_name("php8.5"),
            Some(CliSpec::Version(8, 5))
        ));
    }

    #[test]
    fn rejects_cover_yerd_and_malformed_names() {
        assert!(parse_cli_name("phpcover").is_none());
        assert!(parse_cli_name("php8.5cover").is_none());
        assert!(parse_cli_name("yerd").is_none());
        assert!(parse_cli_name("composer").is_none());
        assert!(parse_cli_name("php8").is_none());
        assert!(parse_cli_name("php8.").is_none());
        assert!(parse_cli_name("php.5").is_none());
    }
}
