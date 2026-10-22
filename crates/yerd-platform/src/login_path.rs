//! Is Yerd's `{data}/bin` on the `PATH` the user's shell actually builds?
//!
//! The daemon's own environment can't answer that (a GUI-launched daemon has a
//! minimal `PATH`), and scanning rc files for Yerd's managed block misses PATH
//! set up by hand or by a dotfiles manager. So the primary check asks a login
//! shell for its `PATH`. The probe is injected behind [`LoginShellProbe`] so the
//! decision logic is tested with fakes; [`RealLoginShell`] is the thin Unix edge.
//! Argv building and parsing are pure ([`crate::pure::shell_profile`]).

use std::path::Path;

use crate::pure::shell_profile::{self, Shell};

/// Runs a shell and returns what it printed.
pub trait LoginShellProbe {
    /// Run `shell_bin` with `args` and return its stdout, or `None` if it
    /// couldn't be started, failed, or timed out.
    fn run(&self, shell_bin: &Path, args: &[String]) -> Option<String>;
}

/// Whether Yerd's `bin_dir` still needs wiring onto the user's `PATH`.
///
/// When `shell` is known and the probe yields a `PATH`, that is authoritative.
/// Otherwise falls back to scanning `rc_contents` (the user's rc files) for
/// Yerd's managed block or a spelling of `bin_dir`.
#[must_use]
pub fn bin_dir_needs_setup(
    probe: &impl LoginShellProbe,
    shell: Option<(&Path, Shell)>,
    bin_dir: &Path,
    home: &Path,
    rc_contents: &[String],
) -> bool {
    let probed = shell.and_then(|(bin, sh)| {
        probe
            .run(bin, &shell_profile::login_path_args(sh))
            .and_then(|out| shell_profile::extract_login_path(&out))
    });
    match probed {
        Some(path) => !shell_profile::path_has_dir(&path, bin_dir),
        None => !rc_contents.iter().any(|c| {
            shell_profile::contains_block(c) || shell_profile::mentions_dir(c, bin_dir, home)
        }),
    }
}

#[cfg(unix)]
pub use real::RealLoginShell;

#[cfg(unix)]
mod real {
    use std::io::Read;
    use std::path::Path;
    use std::process::{Command, Stdio};
    use std::sync::mpsc;
    use std::time::Duration;

    use super::LoginShellProbe;

    /// Spawns the user's shell with a hard timeout, so a slow or interactive rc
    /// file can't wedge the daemon.
    pub struct RealLoginShell {
        timeout: Duration,
    }

    impl RealLoginShell {
        /// A probe that kills the shell after `timeout`.
        #[must_use]
        pub fn new(timeout: Duration) -> Self {
            Self { timeout }
        }
    }

    impl LoginShellProbe for RealLoginShell {
        fn run(&self, shell_bin: &Path, args: &[String]) -> Option<String> {
            let mut child = Command::new(shell_bin)
                .args(args)
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .spawn()
                .ok()?;
            let mut stdout = child.stdout.take()?;
            let (tx, rx) = mpsc::channel();
            std::thread::spawn(move || {
                let mut buf = Vec::new();
                let _ = stdout.read_to_end(&mut buf);
                let _ = tx.send(buf);
            });
            if let Ok(buf) = rx.recv_timeout(self.timeout) {
                let status = child.wait().ok()?;
                status
                    .success()
                    .then(|| String::from_utf8_lossy(&buf).into_owned())
            } else {
                let _ = child.kill();
                let _ = child.wait();
                None
            }
        }
    }

    #[cfg(test)]
    #[allow(clippy::unwrap_used)]
    mod tests {
        use super::*;

        fn args(script: &str) -> Vec<String> {
            vec!["-c".to_owned(), script.to_owned()]
        }

        #[test]
        fn returns_stdout_of_a_successful_shell() {
            let probe = RealLoginShell::new(Duration::from_secs(5));
            let out = probe.run(Path::new("/bin/sh"), &args("printf hello"));
            assert_eq!(out.as_deref(), Some("hello"));
        }

        #[test]
        fn nonzero_exit_is_none() {
            let probe = RealLoginShell::new(Duration::from_secs(5));
            assert_eq!(probe.run(Path::new("/bin/sh"), &args("exit 3")), None);
        }

        #[test]
        fn missing_binary_is_none() {
            let probe = RealLoginShell::new(Duration::from_secs(5));
            assert_eq!(probe.run(Path::new("/nonexistent/sh"), &args("true")), None);
        }

        #[test]
        fn slow_shell_times_out() {
            let probe = RealLoginShell::new(Duration::from_millis(100));
            let started = std::time::Instant::now();
            assert_eq!(probe.run(Path::new("/bin/sh"), &args("sleep 5")), None);
            assert!(started.elapsed() < Duration::from_secs(3));
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    struct FakeProbe(Option<String>);

    impl LoginShellProbe for FakeProbe {
        fn run(&self, _shell_bin: &Path, _args: &[String]) -> Option<String> {
            self.0.clone()
        }
    }

    fn bin() -> PathBuf {
        PathBuf::from("/Users/x/Library/Application Support/io.yerd.Yerd/bin")
    }

    fn home() -> PathBuf {
        PathBuf::from("/Users/x")
    }

    fn zsh() -> (&'static Path, Shell) {
        (Path::new("/bin/zsh"), Shell::Zsh)
    }

    fn probe_out(path: &str) -> FakeProbe {
        FakeProbe(Some(format!(
            "noise__YERD_PATH_BEGIN__{path}__YERD_PATH_END__"
        )))
    }

    #[test]
    fn probed_path_with_bin_dir_needs_no_setup() {
        let path = format!("/usr/bin:{}", bin().display());
        assert!(!bin_dir_needs_setup(
            &probe_out(&path),
            Some(zsh()),
            &bin(),
            &home(),
            &[]
        ));
    }

    #[test]
    fn probed_path_without_bin_dir_needs_setup_even_if_rc_has_block() {
        let rc = vec!["# >>> yerd PATH >>>\n# <<< yerd PATH <<<\n".to_owned()];
        assert!(bin_dir_needs_setup(
            &probe_out("/usr/bin:/bin"),
            Some(zsh()),
            &bin(),
            &home(),
            &rc
        ));
    }

    #[test]
    fn failed_probe_falls_back_to_rc_scan() {
        let none = FakeProbe(None);
        let block = vec!["# >>> yerd PATH >>>\nx\n# <<< yerd PATH <<<\n".to_owned()];
        let literal = vec![format!("export PATH=\"{}:$PATH\"", bin().display())];
        let unrelated = vec!["export PATH=\"/usr/bin\"".to_owned()];
        assert!(!bin_dir_needs_setup(
            &none,
            Some(zsh()),
            &bin(),
            &home(),
            &block
        ));
        assert!(!bin_dir_needs_setup(
            &none,
            Some(zsh()),
            &bin(),
            &home(),
            &literal
        ));
        assert!(bin_dir_needs_setup(
            &none,
            Some(zsh()),
            &bin(),
            &home(),
            &unrelated
        ));
        assert!(bin_dir_needs_setup(
            &none,
            Some(zsh()),
            &bin(),
            &home(),
            &[]
        ));
    }

    #[test]
    fn unparseable_probe_output_falls_back() {
        let garbled = FakeProbe(Some("/usr/bin:/bin".to_owned()));
        let literal = vec![format!("PATH={}", bin().display())];
        assert!(!bin_dir_needs_setup(
            &garbled,
            Some(zsh()),
            &bin(),
            &home(),
            &literal
        ));
    }

    #[test]
    fn unknown_shell_skips_the_probe() {
        let path = format!("{}", bin().display());
        assert!(bin_dir_needs_setup(
            &probe_out(&path),
            None,
            &bin(),
            &home(),
            &[]
        ));
    }
}
