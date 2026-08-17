//! pcov "cover" CLI shims (`phpcover`, `php<major>.<minor>cover`).
//!
//! These names are symlinks in `{data}/bin` pointing at *this* `yerd` binary.
//! When `yerd` is invoked under such a name (detected from `argv[0]` before clap),
//! it resolves the matching PHP CLI binary plus that version's `pcov.so`, points
//! `PHPRC` at a pcov-augmented copy of Yerd's CLI ini, and `exec`s PHP with
//! coverage enabled - leaving the clean `php`/`php<ver>` shims untouched.
//! `PHPRC` (rather than `-d` flags) is what it is: those flags are process-local,
//! but this env var is inherited by any PHP process the exec'd one spawns in
//! turn (e.g. `artisan test`'s child PHPUnit/Pest/paratest run), so coverage
//! stays enabled across that hop too. The shims are symlinks on Unix and `.cmd`
//! wrappers on Windows; [`PCOV_SO_NAME`] is cfg-split to match the host's
//! extension suffix.

use std::ffi::OsString;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

use yerd_core::{php_settings, PhpVersion};
use yerd_platform::PlatformDirs;

use crate::shim::{
    cli_binary, default_php_or_message, dirs_or_fail, exec_php, fail, parse_version_affix,
    ShimVersion,
};

/// On-disk pcov extension filename for the host: `pcov.dll` on Windows, `pcov.so`
/// elsewhere. Mirrors `bin/yerdd`'s `ext_install::PCOV_SPEC.so_name` cfg split
/// (the two binaries can't share code across the boundary, so this is kept
/// byte-in-step, exactly as `cli_phprc` is).
#[cfg(windows)]
const PCOV_SO_NAME: &str = "pcov.dll";
#[cfg(not(windows))]
const PCOV_SO_NAME: &str = "pcov.so";

/// If the shim name is a cover-alias name, run that PHP with pcov enabled and
/// return its exit code (on success `exec` replaces the process and never
/// returns); otherwise `None`, so dispatch falls through to the next shim.
#[must_use]
pub(crate) fn dispatch(name: &str, forward: &[OsString]) -> Option<ExitCode> {
    let spec = parse_cover_name(name)?;
    Some(run(&spec, forward))
}

/// Front door for `yerd coverage <args…>`: run the default PHP version with pcov
/// enabled, forwarding `args` to PHP (same effect as the `phpcover` shim). On
/// success `exec` replaces the process and never returns.
#[must_use]
pub fn run_coverage(args: &[OsString]) -> ExitCode {
    run(&ShimVersion::Default, args)
}

/// Parse a cover-alias basename. Matches `phpcover` and `php<MAJOR>.<MINOR>cover`
/// exactly; returns `None` for `php`, `php<ver>`, and anything else (so a normal
/// `yerd` invocation, or a clean versioned shim, is never intercepted).
fn parse_cover_name(name: &str) -> Option<ShimVersion> {
    parse_version_affix(name, "php", "cover")
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
    let ext_dir = dirs.data.join("php-ext").join(format!("php-{minor}"));
    let pcov = ext_dir.join(PCOV_SO_NAME);
    if !pcov.is_file() {
        return fail(format!(
            "pcov not installed for PHP {minor} — reinstall PHP or wait for the background fetch"
        ));
    }

    let base = match crate::shim::cli_phprc(&dirs, &minor) {
        Some(ini) => match std::fs::read_to_string(&ini) {
            Ok(s) => s,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(e) => return fail(format!("cannot read Yerd's CLI php.ini: {e}")),
        },
        None => String::new(),
    };
    let Some(cover_ini) = php_settings::render_cover_ini(&base, &pcov) else {
        return fail(format!(
            "cannot enable pcov: {} isn't safe to use as an ini value (no control characters, `;`, or `#`, and it must be valid UTF-8) - move Yerd's data directory to a path without those",
            pcov.display()
        ));
    };
    let cover_ini_path = ext_dir.join("cover.ini");
    if let Err(e) = atomic_write(&cover_ini_path, cover_ini.as_bytes()) {
        return fail(format!("cannot write {}: {e}", cover_ini_path.display()));
    }

    let mut cmd = Command::new(&php_bin);
    cmd.env("PHPRC", &cover_ini_path).args(forward);
    exec_php(cmd, &php_bin, Some(&minor))
}

/// Write `bytes` to `path` atomically (tempfile in the same directory +
/// rename). `bin/yerd` doesn't otherwise depend on `yerd-php` (the FPM/
/// site-pool crate), so this ~15-line helper is duplicated here rather than
/// pulling in that whole crate for it - the same trade `yerd-php`'s own
/// `io::atomic_write` already made against `yerd-config`'s equivalent.
fn atomic_write(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let parent = path.parent().ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::InvalidInput, "path has no parent")
    })?;
    let mut tmp = tempfile::NamedTempFile::new_in(parent)?;
    tmp.write_all(bytes)?;
    tmp.flush()?;
    tmp.persist(path).map_err(|e| e.error)?;
    Ok(())
}

/// Resolve `(php_binary, "major.minor")` for the spec.
fn resolve_target(dirs: &PlatformDirs, spec: &ShimVersion) -> Result<(PathBuf, String), String> {
    match spec {
        ShimVersion::Version(maj, min) => {
            if PhpVersion::new(*maj, *min).is_legacy() {
                return Err(format!(
                    "code coverage is not available for PHP {maj}.{min}: pcov is not built for \
                     out-of-support legacy versions (< 8.2). Use a supported PHP version."
                ));
            }
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
#[allow(clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn pcov_so_name_is_dll_on_windows_so_elsewhere() {
        #[cfg(windows)]
        assert_eq!(PCOV_SO_NAME, "pcov.dll");
        #[cfg(not(windows))]
        assert_eq!(PCOV_SO_NAME, "pcov.so");
    }

    #[test]
    fn parses_default_and_versioned_cover_names() {
        assert!(matches!(
            parse_cover_name("phpcover"),
            Some(ShimVersion::Default)
        ));
        assert!(matches!(
            parse_cover_name("php8.4cover"),
            Some(ShimVersion::Version(8, 4))
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

    /// With no 7.4 installed, the legacy gate must still fire first and produce
    /// the pcov message rather than "not installed".
    #[test]
    fn resolve_target_rejects_legacy_before_checking_install() {
        let tmp = tempfile::tempdir().unwrap();
        let dirs = PlatformDirs {
            config: tmp.path().join("c"),
            data: tmp.path().join("d"),
            state: tmp.path().join("s"),
            cache: tmp.path().join("ca"),
            runtime: tmp.path().join("r"),
        };
        match resolve_target(&dirs, &ShimVersion::Version(7, 4)) {
            Err(msg) => {
                assert!(msg.contains("pcov"), "got {msg}");
                assert!(msg.contains("legacy"), "got {msg}");
            }
            Ok(_) => panic!("expected legacy rejection"),
        }
    }
}
