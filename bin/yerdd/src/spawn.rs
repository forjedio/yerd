//! The daemon's constructor for un-supervised child processes.
//!
//! Supervised children (PHP pools, service engines, tunnels) are spawned through
//! `yerd-supervise`'s `TokioProcessSpawner`, which already hides their console.
//! This module is the equivalent seam for the daemon's *one-shot* helpers - the
//! PHP CLI runs behind site creation and the WordPress/Laravel tools, the
//! database dump/restore/query clients, `git --version` - so no call site has to
//! remember the Windows flag.

use std::ffi::OsStr;

/// A [`tokio::process::Command`] for `program` that will not flash a console
/// window.
///
/// On Windows every tool the daemon shells out to (`php.exe`, `mysql.exe`,
/// `mysqldump.exe`, `psql.exe`, `git.exe`, `cloudflared.exe`) is
/// console-subsystem, and `yerdd` runs without a console of its own, so an
/// unflagged spawn makes Windows allocate a fresh console *window* for the
/// child - a visible flash for every short-lived one-shot. `CREATE_NO_WINDOW`
/// suppresses it. On Unix this is a plain `Command::new`.
pub fn hidden_command<S: AsRef<OsStr>>(program: S) -> tokio::process::Command {
    #[cfg_attr(not(windows), allow(unused_mut))]
    let mut cmd = tokio::process::Command::new(program);
    #[cfg(windows)]
    cmd.creation_flags(yerd_platform::CREATE_NO_WINDOW);
    cmd
}
