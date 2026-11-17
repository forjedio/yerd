//! `laravel` multi-call shim.
//!
//! `{data}/bin/laravel` is a symlink to *this* `yerd` binary. When invoked under
//! that name (detected from `argv[0]` before clap), yerd execs the managed
//! Laravel installer under the default managed PHP -
//! `php …/tools/laravel/bin/laravel <args…>`. Unix-only. The daemon's
//! own site-creation handler does **not** use this shim (it pins a specific PHP
//! per job); this is purely for terminal use of `laravel new`.
//!
//! Like the `php` shim, it points `PHPRC` at the default version's CLI ini
//! (`php-cli-<minor>.ini`, else the base `php-cli.ini`), so extensions registered
//! for the default version are visible to the installer and its child processes.

use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Command, ExitCode};

use yerd_platform::{ActivePaths, Paths};

use crate::shim::{cli_phprc, fail, resolve_default_php};

/// If `argv[0]` is `laravel`, exec the installer under the default PHP and return
/// its exit code (on success `exec` replaces the process and never returns);
/// otherwise `None`, so `main` falls through to the next shim / CLI.
#[must_use]
pub fn dispatch() -> Option<ExitCode> {
    let arg0 = std::env::args_os().next()?;
    let name = Path::new(&arg0).file_name()?.to_str()?;
    if name != "laravel" {
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

    let installer = dirs
        .data
        .join("tools")
        .join("laravel")
        .join("bin")
        .join("laravel");
    if !installer.is_file() {
        return fail(
            "the Laravel installer is not installed — install it from the Tooling page \
             (or run `yerd install tool laravel`)"
                .to_owned(),
        );
    }

    let mut cmd = Command::new(&php_bin);
    cmd.arg(&installer).args(std::env::args_os().skip(1));
    if let Some(phprc) = cli_phprc(&dirs, &minor) {
        cmd.env("PHPRC", phprc);
    }
    crate::shim::apply_network_env(&mut cmd, &dirs);
    let err = cmd.exec();
    if err.kind() == std::io::ErrorKind::NotFound {
        return fail(format!(
            "PHP binary not found at {} ({err}) — reinstall with `yerd install php`",
            php_bin.display()
        ));
    }
    fail(format!("failed to exec {}: {err}", php_bin.display()))
}
