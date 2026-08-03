//! `composer` multi-call shim.
//!
//! `{data}/bin/composer` is a symlink to *this* `yerd` binary. When invoked under
//! that name (`argv[0]` on Unix, the `__shim composer` sentinel on Windows),
//! yerd runs the bundled `composer.phar` under the default managed PHP -
//! `php composer.phar <args…>` - so Composer sees a normal `php` process.

use std::ffi::OsString;
use std::process::{Command, ExitCode};

use yerd_platform::{ActivePaths, Paths};

use crate::shim::{fail, resolve_default_php, run_php};

/// If the shim name is `composer`, run the bundled phar under the default PHP
/// and return its exit code; otherwise `None`, so `main` falls through to the
/// next shim / CLI.
#[must_use]
pub fn dispatch() -> Option<ExitCode> {
    let (name, forward) = crate::shim::shim_invocation()?;
    if name != "composer" {
        return None;
    }
    Some(run(&forward))
}

fn run(forward: &[OsString]) -> ExitCode {
    let dirs = match ActivePaths::new().resolve() {
        Ok(d) => d,
        Err(e) => return fail(format!("cannot resolve yerd directories: {e}")),
    };

    let Some((php_bin, _minor)) = resolve_default_php(&dirs) else {
        return fail(crate::shim::no_default_php_message(&dirs));
    };

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

    let mut cmd = Command::new(&php_bin);
    cmd.arg(&phar).args(forward);
    match run_php(cmd) {
        Ok(code) => code,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => fail(format!(
            "PHP binary not found at {} ({err}) — reinstall with `yerd install php`",
            php_bin.display()
        )),
        Err(err) => fail(format!("failed to exec {}: {err}", php_bin.display())),
    }
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
