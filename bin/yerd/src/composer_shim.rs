//! `composer` multi-call shim.
//!
//! `{data}/bin/composer` is a symlink to *this* `yerd` binary. When invoked under
//! that name (detected from `argv[0]` before clap), yerd runs the bundled
//! `composer.phar` under the default managed PHP - `php composer.phar <args…>` -
//! then `exec`s, so Composer sees a normal `php` process. Unix-only.
//!
//! Like the `php` shim, it points `PHPRC` at the default version's CLI ini
//! (`php-cli-<minor>.ini`, else the base `php-cli.ini`), overriding the base ini
//! the shell profile exports. Without that, extensions registered for the
//! default version are missing from Composer's platform check and from the PHP
//! processes it spawns.

use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Command, ExitCode};

use yerd_platform::{ActivePaths, Paths};

use crate::shim::{cli_phprc, fail, resolve_default_php};

/// If `argv[0]` is `composer`, run the bundled phar under the default PHP and
/// return its exit code (on success `exec` replaces the process and never
/// returns); otherwise `None`, so `main` falls through to the next shim / CLI.
#[must_use]
pub fn dispatch() -> Option<ExitCode> {
    let arg0 = std::env::args_os().next()?;
    let name = Path::new(&arg0).file_name()?.to_str()?;
    if name != "composer" {
        return None;
    }
    Some(run())
}

fn run() -> ExitCode {
    let dirs = match ActivePaths::new().resolve() {
        Ok(d) => d,
        Err(e) => return fail(format!("cannot resolve yerd directories: {e}")),
    };

    let Some((php_bin, minor)) = resolve_default_php(&dirs) else {
        return fail(crate::shim::no_default_php_message(&dirs));
    };

    let phar = crate::shim::composer_phar(&dirs);
    if !phar.is_file() {
        return fail(crate::shim::composer_missing_message());
    }

    let mut cmd = Command::new(&php_bin);
    cmd.arg(&phar).args(std::env::args_os().skip(1));
    if let Some(phprc) = cli_phprc(&dirs, &minor) {
        cmd.env("PHPRC", phprc);
    }
    let err = cmd.exec();
    if err.kind() == std::io::ErrorKind::NotFound {
        return fail(format!(
            "PHP binary not found at {} ({err}) — reinstall with `yerd install php`",
            php_bin.display()
        ));
    }
    fail(format!("failed to exec {}: {err}", php_bin.display()))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn dispatch_ignores_non_composer_argv0() {
        assert_eq!(Path::new("/x/composer").file_name().unwrap(), "composer");
        assert_ne!(Path::new("/x/composer2").file_name().unwrap(), "composer");
    }
}
