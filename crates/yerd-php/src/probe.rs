//! The extension load-probe: a one-shot PHP run whose output the pure
//! [`interpret_probe`] classifies.
//!
//! The existing [`crate::traits::ProcessSpawner`] returns a supervised child
//! handle (`wait`/`kill`) and cannot capture one-shot output, so this seam adds a
//! minimal [`CommandRunner`] that returns the finished command's exit status and
//! stderr. The daemon injects the real, tokio-backed runner; tests inject a fake
//! that returns canned output, keeping [`probe_extension`] testable without
//! spawning PHP.

use std::io;
use std::path::Path;
use std::process::{Command, Stdio};

use async_trait::async_trait;

use crate::pure::ext_probe::{interpret_probe, ExtLoadError};

/// The captured result of a one-shot command: whether it exited successfully and
/// both of its output streams (UTF-8 lossy). Both are captured because PHP routes
/// a failed extension load to **stdout** when `display_errors` sends errors there
/// (the default under `-n`), not stderr - so the probe must inspect both.
/// Deliberately small and owned so the trait stays runtime-free at its boundary,
/// mirroring [`crate::traits::ProcessSpawner`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProbeOutput {
    /// The process exited with a success status.
    pub status_ok: bool,
    /// The process's stdout, decoded lossily.
    pub stdout: String,
    /// The process's stderr, decoded lossily.
    pub stderr: String,
}

/// Runs a command to completion and returns its [`ProbeOutput`].
///
/// `cmd` is a `std::process::Command` so the trait stays runtime-free; the
/// production impl converts to `tokio::process::Command` internally (same
/// pattern as [`crate::traits::ProcessSpawner`]).
#[async_trait]
pub trait CommandRunner: Send + Sync + 'static {
    /// Run `cmd`, wait for it, and capture its status + stderr.
    async fn run(&self, cmd: Command) -> Result<ProbeOutput, io::Error>;
}

/// Production [`CommandRunner`] backed by `tokio::process`.
#[derive(Debug, Clone, Copy, Default)]
pub struct TokioCommandRunner;

#[async_trait]
impl CommandRunner for TokioCommandRunner {
    async fn run(&self, cmd: Command) -> Result<ProbeOutput, io::Error> {
        let out = tokio::process::Command::from(cmd).output().await?;
        Ok(ProbeOutput {
            status_ok: out.status.success(),
            stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        })
    }
}

/// Load-probe `ext_path` against `php_bin`: run
/// `php -n -d display_errors=stderr -d [zend_]extension=<path> -m` and classify
/// the result. `-n` ignores every ini so the probe tests the `.so` against that
/// PHP build in isolation (a caveat: an extension that depends on *another*
/// shared extension being loaded first can fail here yet load fine in the real
/// pool). `-d display_errors=stderr` forces PHP's load-failure warnings onto the
/// captured stderr rather than stdout; both streams are inspected regardless.
/// Returns `Ok(())` when the extension loads cleanly.
///
/// # Errors
/// [`ExtLoadError::SpawnFailed`] if the probe process could not be run; otherwise
/// the classification from [`interpret_probe`].
pub async fn probe_extension(
    runner: &dyn CommandRunner,
    php_bin: &Path,
    ext_path: &Path,
    zend: bool,
) -> Result<(), ExtLoadError> {
    let directive = if zend { "zend_extension" } else { "extension" };
    let mut cmd = Command::new(php_bin);
    cmd.arg("-n")
        .arg("-d")
        .arg("display_errors=stderr")
        .arg("-d")
        .arg(format!("{directive}={}", ext_path.display()))
        .arg("-m")
        .stdin(Stdio::null());
    let out = runner
        .run(cmd)
        .await
        .map_err(|_| ExtLoadError::SpawnFailed)?;
    let diagnostics = format!("{}{}", out.stdout, out.stderr);
    interpret_probe(out.status_ok, &diagnostics)
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod tests {
    use super::*;

    struct FakeRunner {
        status_ok: bool,
        stdout: String,
        stderr: String,
    }

    #[async_trait]
    impl CommandRunner for FakeRunner {
        async fn run(&self, _cmd: Command) -> Result<ProbeOutput, io::Error> {
            Ok(ProbeOutput {
                status_ok: self.status_ok,
                stdout: self.stdout.clone(),
                stderr: self.stderr.clone(),
            })
        }
    }

    struct FailingRunner;

    #[async_trait]
    impl CommandRunner for FailingRunner {
        async fn run(&self, _cmd: Command) -> Result<ProbeOutput, io::Error> {
            Err(io::Error::from(io::ErrorKind::NotFound))
        }
    }

    #[tokio::test]
    async fn clean_probe_accepts() {
        let runner = FakeRunner {
            status_ok: true,
            stdout: "[PHP Modules]\nscrypt\nstandard\n".to_owned(),
            stderr: String::new(),
        };
        probe_extension(&runner, Path::new("/php"), Path::new("/a/scrypt.so"), false)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn unable_to_load_on_stdout_rejects() {
        // Real PHP writes this to stdout (display_errors), not stderr - the probe
        // must inspect both streams or it green-lights a broken .so.
        let runner = FakeRunner {
            status_ok: true,
            stdout: "PHP Warning:  PHP Startup: Unable to load dynamic library 'scrypt.so'"
                .to_owned(),
            stderr: String::new(),
        };
        let e = probe_extension(&runner, Path::new("/php"), Path::new("/a/scrypt.so"), false)
            .await
            .unwrap_err();
        assert_eq!(e, ExtLoadError::NotLoadable);
    }

    #[tokio::test]
    async fn abi_mismatch_on_stdout_rejects() {
        let runner = FakeRunner {
            status_ok: true,
            stdout: "PHP Warning:  PHP Startup: scrypt: Unable to initialize module\n\
                 Module compiled with module API=20210902\nPHP    module API=20230831"
                .to_owned(),
            stderr: String::new(),
        };
        let e = probe_extension(&runner, Path::new("/php"), Path::new("/a/scrypt.so"), false)
            .await
            .unwrap_err();
        assert_eq!(e, ExtLoadError::AbiMismatch);
    }

    #[tokio::test]
    async fn zend_flag_hint_surfaces() {
        let runner = FakeRunner {
            status_ok: true,
            stdout: String::new(),
            stderr: "doesn't appear to be a valid Zend extension".to_owned(),
        };
        let e = probe_extension(&runner, Path::new("/php"), Path::new("/a/scrypt.so"), true)
            .await
            .unwrap_err();
        assert_eq!(e, ExtLoadError::NotZend);
    }

    #[tokio::test]
    async fn spawn_failure_maps_to_spawn_failed() {
        let e = probe_extension(
            &FailingRunner,
            Path::new("/php"),
            Path::new("/a/x.so"),
            false,
        )
        .await
        .unwrap_err();
        assert_eq!(e, ExtLoadError::SpawnFailed);
    }
}
