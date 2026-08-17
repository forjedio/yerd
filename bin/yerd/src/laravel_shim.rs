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

use crate::shim::{
    default_php_or_message, dirs_or_fail, dispatch_named, exec_php, fail, managed_tool,
};

/// If the shim name is `laravel`, exec the installer under the default PHP and
/// return its exit code; otherwise `None`, so dispatch falls through to the next
/// shim.
#[must_use]
pub(crate) fn dispatch(name: &str, forward: &[OsString]) -> Option<ExitCode> {
    dispatch_named("laravel", name, forward, run)
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

    let installer = match managed_tool(
        &dirs,
        "the Laravel installer",
        "laravel",
        &["laravel", "bin", "laravel"],
    ) {
        Ok(p) => p,
        Err(msg) => return fail(msg),
    };

    let mut cmd = Command::new(&php_bin);
    cmd.arg(&installer).args(forward);
    exec_php(cmd, &php_bin, None)
}
