//! Clean CLI shims (`php`, `php<major>.<minor>`).
//!
//! These names are symlinks in `{data}/bin` pointing at *this* `yerd` binary
//! (like the `phpcover` shims). When `yerd` is invoked under such a name, it
//! resolves the matching PHP CLI binary, points `PHPRC` at that version's
//! generated ini (`{data}/php-cli-<minor>.ini`, which carries the user's global
//! settings **and** that version's registered extensions), and `exec`s PHP.
//! Pointing `PHPRC` per version is what lets a custom extension load in the CLI,
//! and `PHPRC` (rather than `-d`) is inherited by any child PHP the exec'd one
//! spawns. On Unix these shims are symlinks read from `argv[0]`; on Windows they
//! are `.cmd` wrappers that re-invoke `yerd.exe __shim <name>`.

use std::ffi::OsString;
use std::process::ExitCode;

use yerd_platform::PlatformDirs;

use crate::shim::{
    cli_binary, cli_phprc, default_php_or_message, dirs_or_fail, exec_php, fail,
    parse_version_affix, ShimVersion,
};

/// If the shim name is a clean CLI shim name (`php` / `php<M>.<N>`), run that PHP
/// with the version's `PHPRC` set and return its exit code (on success `exec`
/// replaces the process and never returns); otherwise `None`, so dispatch falls
/// through to the next shim. Runs *after* the cover-shim dispatch so
/// `php<ver>cover` is never routed here.
#[must_use]
pub(crate) fn dispatch(name: &str, forward: &[OsString]) -> Option<ExitCode> {
    let spec = parse_cli_name(name)?;
    Some(run(&spec, forward))
}

/// Parse a clean CLI shim basename. Matches `php` and `php<MAJOR>.<MINOR>`
/// exactly, and **rejects a trailing `cover`** so `php<ver>cover` can never be
/// misrouted here even if dispatch order changed. Returns `None` for `yerd`,
/// `composer`, and anything else.
fn parse_cli_name(name: &str) -> Option<ShimVersion> {
    if name.ends_with("cover") {
        return None;
    }
    parse_version_affix(name, "php", "")
}

fn run(spec: &ShimVersion, forward: &[OsString]) -> ExitCode {
    let dirs = match dirs_or_fail() {
        Ok(d) => d,
        Err(code) => return code,
    };
    let (php_bin, minor) = match resolve_target(&dirs, spec) {
        Ok(t) => t,
        Err(msg) => return fail(msg),
    };

    let mut cmd = std::process::Command::new(&php_bin);
    if let Some(phprc) = cli_phprc(&dirs, &minor) {
        cmd.env("PHPRC", phprc);
    }
    cmd.args(forward);
    exec_php(cmd, &php_bin, Some(&minor))
}

/// Resolve `(php_binary, "major.minor")` for the spec.
fn resolve_target(
    dirs: &PlatformDirs,
    spec: &ShimVersion,
) -> Result<(std::path::PathBuf, String), String> {
    match spec {
        ShimVersion::Version(maj, min) => {
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
        ShimVersion::Default => default_php_or_message(dirs),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_plain_and_versioned_php_names() {
        assert!(matches!(parse_cli_name("php"), Some(ShimVersion::Default)));
        assert!(matches!(
            parse_cli_name("php8.5"),
            Some(ShimVersion::Version(8, 5))
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
