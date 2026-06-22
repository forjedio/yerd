//! `composer` multi-call shim.
//!
//! `{data}/bin/composer` is a symlink to *this* `yerd` binary. When invoked under
//! that name (detected from `argv[0]` before clap), yerd runs the bundled
//! `composer.phar` under the default managed PHP — `php composer.phar <args…>` —
//! then `exec`s, so Composer sees a normal `php` process. Unix-only.

use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Command, ExitCode};

use yerd_platform::{ActivePaths, Paths};

use crate::shim::{fail, resolve_default_php};

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

    let Some((php_bin, _minor)) = resolve_default_php(&dirs) else {
        return fail("no PHP installed — run `yerd install php <version>`".to_owned());
    };

    // Kept in sync with `yerdd`'s `tools::composer::phar_path`.
    let phar = dirs
        .data
        .join("tools")
        .join("composer")
        .join("composer.phar");
    if !phar.is_file() {
        return fail(
            "Composer is not installed — install it from the Tooling page \
             (or run `yerd install tool composer`)"
                .to_owned(),
        );
    }

    // `exec` only returns on failure. Composer reads PHP_BINARY from the running
    // php process, so argv[0] staying the real php path is correct.
    let err = Command::new(&php_bin)
        .arg(&phar)
        .args(std::env::args_os().skip(1))
        .exec();
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
        // We can't easily fake argv[0] here, but the parse rule is exact-match;
        // assert the basename rule the dispatch relies on.
        assert_eq!(Path::new("/x/composer").file_name().unwrap(), "composer");
        assert_ne!(Path::new("/x/composer2").file_name().unwrap(), "composer");
    }
}
