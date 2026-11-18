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
use std::time::Duration;

use async_trait::async_trait;

use crate::pure::ext_probe::{ffi_listed, interpret_probe, require_zend_module, ExtLoadError};

/// Upper bound on one `php -n -m` FFI probe. A cold first run (macOS
/// Gatekeeper) takes about a second; anything past this is treated as unknown.
const FFI_PROBE_TIMEOUT: Duration = Duration::from_secs(10);

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
    /// Run `cmd`, wait for it, and capture its status, stdout, and stderr.
    async fn run(&self, cmd: Command) -> Result<ProbeOutput, io::Error>;
}

/// Production [`CommandRunner`] backed by `tokio::process`. The child is
/// killed if the returned future is dropped early (e.g. by a timeout).
#[derive(Debug, Clone, Copy, Default)]
pub struct TokioCommandRunner;

#[async_trait]
impl CommandRunner for TokioCommandRunner {
    async fn run(&self, cmd: Command) -> Result<ProbeOutput, io::Error> {
        let mut cmd = tokio::process::Command::from(cmd);
        cmd.kill_on_drop(true);
        let out = cmd.output().await?;
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
    let out = run_probe(runner, php_bin, ext_path, zend).await?;
    let diagnostics = format!("{}{}", out.stdout, out.stderr);
    interpret_probe(out.status_ok, &diagnostics)
}

/// Load-probe a Zend extension positively: the same run as
/// [`probe_extension`] with `zend_extension=`, which must also list `module`
/// (for example `Xdebug`) under `[Zend Modules]`. A probe that merely shows no
/// failure marker is not enough.
///
/// # Errors
/// [`ExtLoadError::SpawnFailed`] if the probe could not run; otherwise the
/// classification from [`require_zend_module`].
pub async fn probe_zend_module(
    runner: &dyn CommandRunner,
    php_bin: &Path,
    ext_path: &Path,
    module: &str,
) -> Result<(), ExtLoadError> {
    let out = run_probe(runner, php_bin, ext_path, true).await?;
    require_zend_module(out.status_ok, &out.stdout, &out.stderr, module)
}

async fn run_probe(
    runner: &dyn CommandRunner,
    php_bin: &Path,
    ext_path: &Path,
    zend: bool,
) -> Result<ProbeOutput, ExtLoadError> {
    let directive = if zend { "zend_extension" } else { "extension" };
    let mut cmd = Command::new(php_bin);
    cmd.arg("-n")
        .arg("-d")
        .arg("display_errors=stderr")
        .arg("-d")
        .arg(format!("{directive}={}", ext_path.display()))
        .arg("-m")
        .stdin(Stdio::null());
    runner.run(cmd).await.map_err(|_| ExtLoadError::SpawnFailed)
}

/// Whether the PHP build at `php_bin` includes the FFI extension, by running
/// `php -n -m` (Yerd's builds are static, so `-n` still lists every module).
/// `None` when the probe could not answer: the process failed to spawn, exited
/// unsuccessfully, or ran past [`FFI_PROBE_TIMEOUT`]. Callers must not cache a
/// `None`.
pub async fn probe_ffi(runner: &dyn CommandRunner, php_bin: &Path) -> Option<bool> {
    let mut cmd = Command::new(php_bin);
    cmd.arg("-n").arg("-m").stdin(Stdio::null());
    let out = tokio::time::timeout(FFI_PROBE_TIMEOUT, runner.run(cmd))
        .await
        .ok()?
        .ok()?;
    out.status_ok.then(|| ffi_listed(&out.stdout))
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

    /// Real PHP writes the load failure to stdout (via `display_errors`), not
    /// stderr, so the probe must inspect both streams or it green-lights a
    /// broken `.so`.
    #[tokio::test]
    async fn unable_to_load_on_stdout_rejects() {
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

    #[tokio::test]
    async fn probe_zend_module_requires_the_module_listed() {
        let listed = FakeRunner {
            status_ok: true,
            stdout: "[PHP Modules]\nCore\n\n[Zend Modules]\nXdebug\n".to_owned(),
            stderr: String::new(),
        };
        let silent = FakeRunner {
            status_ok: true,
            stdout: "[PHP Modules]\nCore\n\n[Zend Modules]\n".to_owned(),
            stderr: String::new(),
        };
        let php = Path::new("/php");
        let so = Path::new("/a/xdebug.so");
        probe_zend_module(&listed, php, so, "Xdebug").await.unwrap();
        assert_eq!(
            probe_zend_module(&silent, php, so, "Xdebug").await,
            Err(ExtLoadError::NotListed)
        );
        assert_eq!(
            probe_zend_module(&FailingRunner, php, so, "Xdebug").await,
            Err(ExtLoadError::SpawnFailed)
        );
    }

    #[tokio::test]
    async fn probe_ffi_reports_presence_absence_and_failures() {
        let with = FakeRunner {
            status_ok: true,
            stdout: "[PHP Modules]\nCore\nFFI\n".to_owned(),
            stderr: String::new(),
        };
        let without = FakeRunner {
            status_ok: true,
            stdout: "[PHP Modules]\nCore\n".to_owned(),
            stderr: String::new(),
        };
        let failed_exit = FakeRunner {
            status_ok: false,
            stdout: "[PHP Modules]\nFFI\n".to_owned(),
            stderr: String::new(),
        };
        let php = Path::new("/php");
        assert_eq!(probe_ffi(&with, php).await, Some(true));
        assert_eq!(probe_ffi(&without, php).await, Some(false));
        assert_eq!(probe_ffi(&failed_exit, php).await, None);
        assert_eq!(probe_ffi(&FailingRunner, php).await, None);
    }
}
