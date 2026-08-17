//! `composer` multi-call shim.
//!
//! `{data}/bin/composer` is a symlink to *this* `yerd` binary. When invoked under
//! that name (`argv[0]` on Unix, the `__shim composer` sentinel on Windows),
//! yerd runs the bundled `composer.phar` under the default managed PHP -
//! `php composer.phar <args…>` - so Composer sees a normal `php` process.

use std::ffi::OsString;
use std::process::{Command, ExitCode};

use crate::shim::{default_php_or_message, dirs_or_fail, dispatch_named, exec_php, fail};

/// If the shim name is `composer`, run the bundled phar under the default PHP
/// and return its exit code; otherwise `None`, so dispatch falls through to the
/// next shim.
#[must_use]
pub(crate) fn dispatch(name: &str, forward: &[OsString]) -> Option<ExitCode> {
    dispatch_named("composer", name, forward, run)
}

fn run(forward: &[OsString]) -> ExitCode {
    let dirs = match dirs_or_fail() {
        Ok(d) => d,
        Err(code) => return code,
    };

    let (php_bin, _minor) = match default_php_or_message(&dirs) {
        Ok(t) => t,
        Err(msg) => return fail(msg),
    };

    let phar = crate::shim::composer_phar(&dirs);
    if !phar.is_file() {
        return fail(crate::shim::composer_missing_message());
    }

    let mut cmd = Command::new(&php_bin);
    cmd.arg(&phar).args(forward);
    exec_php(cmd, &php_bin, None)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::panic)]
mod tests {
    use std::path::Path;

    #[test]
    fn dispatch_ignores_non_composer_argv0() {
        assert_eq!(Path::new("/x/composer").file_name().unwrap(), "composer");
        assert_ne!(Path::new("/x/composer2").file_name().unwrap(), "composer");
    }
}
