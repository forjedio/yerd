//! `laravel` multi-call shim.
//!
//! `{data}/bin/laravel` is a symlink to *this* `yerd` binary. When invoked under
//! that name (`argv[0]` on Unix, the `__shim laravel` sentinel on Windows),
//! yerd execs the managed Laravel installer under the default managed PHP -
//! `php …/tools/laravel/bin/laravel <args…>`. The daemon's own site-creation
//! handler does **not** use this shim (it pins a specific PHP per job); this is
//! purely for terminal use of `laravel new`.

use std::ffi::OsString;
use std::process::{Command, ExitCode};

use yerd_platform::{ActivePaths, Paths};

use crate::shim::{fail, resolve_default_php, run_php};

/// If the shim name is `laravel`, exec the installer under the default PHP and
/// return its exit code; otherwise `None`, so `main` falls through to the next
/// shim / CLI.
#[must_use]
pub fn dispatch() -> Option<ExitCode> {
    let (name, forward) = crate::shim::shim_invocation()?;
    if name != "laravel" {
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
    cmd.arg(&installer).args(forward);
    match run_php(cmd) {
        Ok(code) => code,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => fail(format!(
            "PHP binary not found at {} ({err}) — reinstall with `yerd install php`",
            php_bin.display()
        )),
        Err(err) => fail(format!("failed to exec {}: {err}", php_bin.display())),
    }
}
