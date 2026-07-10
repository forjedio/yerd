//! End-to-end: prove that `PHPRC`, set by the cover launcher on the process it
//! `exec`s, is inherited by a subsequent process that one re-execs itself - the
//! actual mechanism that lets coverage survive `artisan test`'s child
//! PHPUnit/Pest/paratest hop. Covers both front doors that reach the same
//! cover-shim logic: the `php<ver>cover` argv[0] shim and the `yerd coverage`
//! subcommand. Spawns the real built `yerd` binary against a fully faked
//! `PlatformDirs` layout (a stub shell script standing in for the PHP
//! interpreter), rather than calling `cover_shim::dispatch()` in-process,
//! because it resolves `ActivePaths::new().resolve()` internally with no
//! dirs-injection seam.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

#[cfg(unix)]
mod tests {
    use std::fs;
    use std::os::unix::fs::{symlink, PermissionsExt};
    use std::process::Command;

    use yerd_platform::PlatformDirs;

    /// A `#!/bin/sh` stand-in for the PHP CLI binary: prints `$PHPRC`, then
    /// re-execs itself once with `--grandchild` appended (the actual hop under
    /// test - a plain re-exec inherits the parent's environment, same as
    /// Symfony `Process` spawning `PHPUnit` via `PHP_BINARY`), then exits.
    const STUB_PHP: &str = "#!/bin/sh\n\
        printf '%s\\n' \"$PHPRC\"\n\
        case \"$1\" in\n\
        --grandchild) exit 0 ;;\n\
        esac\n\
        exec \"$0\" --grandchild\n";

    /// Build a faked `PlatformDirs` layout under a fresh tempdir: a stub PHP 8.4
    /// CLI binary and a stub `pcov.so`. Returns `(tempdir, home, expected cover.ini)`;
    /// the tempdir is kept alive by the caller.
    fn faked_php_8_4_layout() -> (tempfile::TempDir, std::path::PathBuf, std::path::PathBuf) {
        let tmp = tempfile::tempdir().expect("tempdir");
        let home = tmp.path().join("home");
        fs::create_dir_all(&home).expect("mkdir home");
        let dirs = PlatformDirs::for_user(&home, 0);

        let php_bin_dir = dirs.data.join("php").join("php-8.4").join("bin");
        fs::create_dir_all(&php_bin_dir).expect("mkdir php bin");
        let php_bin = php_bin_dir.join("php");
        fs::write(&php_bin, STUB_PHP).expect("write stub php");
        fs::set_permissions(&php_bin, fs::Permissions::from_mode(0o755)).expect("chmod +x");

        let ext_dir = dirs.data.join("php-ext").join("php-8.4");
        fs::create_dir_all(&ext_dir).expect("mkdir php-ext");
        fs::write(ext_dir.join("pcov.so"), b"").expect("write stub pcov.so");

        let expected_phprc = ext_dir.join("cover.ini");
        (tmp, home, expected_phprc)
    }

    /// Invoke `program` with `args` under the faked home's XDG environment and
    /// return its captured output.
    fn run_in_home(
        program: &std::path::Path,
        args: &[&str],
        home: &std::path::Path,
    ) -> std::process::Output {
        Command::new(program)
            .args(args)
            .env_clear()
            .env("HOME", home)
            .env("XDG_DATA_HOME", home.join(".local").join("share"))
            .env("XDG_CONFIG_HOME", home.join(".config"))
            .env("XDG_STATE_HOME", home.join(".local").join("state"))
            .env("XDG_CACHE_HOME", home.join(".cache"))
            .output()
            .expect("run yerd")
    }

    /// Assert the stub PHP printed the expected `PHPRC` on both the top-level
    /// process and its re-exec'd grandchild, and that the cover ini was written.
    fn assert_phprc_hop(output: &std::process::Output, expected_phprc: &std::path::Path) {
        assert!(
            output.status.success(),
            "cover run failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let stdout = String::from_utf8_lossy(&output.stdout);
        let lines: Vec<&str> = stdout.lines().collect();
        let want = expected_phprc.to_str().expect("utf8 path");
        assert_eq!(
            lines,
            vec![want, want],
            "PHPRC must be identical across the re-exec hop (top-level process and its grandchild)"
        );
        assert!(expected_phprc.is_file(), "cover.ini must have been written");
    }

    #[test]
    fn phprc_survives_a_re_exec_grandchild_hop() {
        let (tmp, home, expected_phprc) = faked_php_8_4_layout();

        let cover_shim_bin = tmp.path().join("php8.4cover");
        symlink(env!("CARGO_BIN_EXE_yerd"), &cover_shim_bin).expect("symlink cover shim");

        let output = run_in_home(&cover_shim_bin, &[], &home);
        assert_phprc_hop(&output, &expected_phprc);
    }

    /// The `yerd coverage` subcommand front door reaches the same cover-shim
    /// logic as the `phpcover` argv[0] shim: invoked as the real `yerd` binary,
    /// it resolves the default PHP (8.4, the only installed version) and enables
    /// pcov via `PHPRC`, which survives the grandchild hop identically.
    #[test]
    fn coverage_subcommand_enables_pcov_like_phpcover() {
        let (_tmp, home, expected_phprc) = faked_php_8_4_layout();

        let output = run_in_home(
            std::path::Path::new(env!("CARGO_BIN_EXE_yerd")),
            &["coverage"],
            &home,
        );
        assert_phprc_hop(&output, &expected_phprc);
    }
}
