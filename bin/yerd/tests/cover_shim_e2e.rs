//! End-to-end: prove that `PHPRC`, set by the cover launcher on the process it
//! `exec`s, is inherited by a subsequent process that one re-execs itself - the
//! actual mechanism that lets coverage survive `artisan test`'s child
//! PHPUnit/Pest/paratest hop. Covers both front doors that reach the same
//! cover-shim logic: the `php<ver>cover` argv[0] shim and the `yerd coverage`
//! subcommand. Also covers the other hop, where a child resolves `php` from
//! `PATH` and re-enters the plain CLI shim: there the exported `YERD_COVER=1` is
//! what keeps coverage alive, with the child deriving the cover ini for its own
//! PHP version. Spawns the real built `yerd` binary against a fully faked
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

    /// A `#!/bin/sh` stand-in for the PHP CLI binary. On every process it prints
    /// `phprc=$PHPRC` and `yerd_cover=$YERD_COVER`; on the top-level pass (before
    /// the hop) it also prints `args=$*` so a test can assert the launcher
    /// forwarded the caller's arguments verbatim. It then re-execs itself once
    /// with `--grandchild` (the actual hop under test - a plain re-exec inherits
    /// the parent's environment, same as Symfony `Process` spawning `PHPUnit` via
    /// `PHP_BINARY`) and exits. The grandchild deliberately carries only
    /// `--grandchild`, so it exercises `PHPRC` inheritance, not arg forwarding.
    const STUB_PHP: &str = "#!/bin/sh\n\
        printf 'phprc=%s\\n' \"$PHPRC\"\n\
        printf 'yerd_cover=%s\\n' \"$YERD_COVER\"\n\
        case \"$1\" in\n\
        --grandchild) exit 0 ;;\n\
        esac\n\
        printf 'args=%s\\n' \"$*\"\n\
        exec \"$0\" --grandchild\n";

    /// A stub PHP that prints where its ini came from and exits, with no hop.
    /// Stands in for the child interpreter at the far end of a `PATH` hop.
    const STUB_PHP_PRINT_ONLY: &str = "#!/bin/sh\n\
        printf 'phprc=%s\\n' \"$PHPRC\"\n";

    /// A stub PHP that prints its own ini, then spawns `php8.4` resolved from
    /// `PATH` - the hop that re-enters Yerd's plain CLI shim rather than
    /// inheriting `PHPRC` from an absolute-interpreter spawn.
    const STUB_PHP_PATH_HOP: &str = "#!/bin/sh\n\
        printf 'phprc=%s\\n' \"$PHPRC\"\n\
        exec php8.4 --child\n";

    /// Install a stub PHP CLI binary for `minor`, plus that version's stub
    /// `pcov.so`. The binary lands at exactly `shim::cli_binary`'s path
    /// (`{data}/php/php-<minor>/bin/php`), which is the only location any shim
    /// ever execs - a stub written elsewhere would silently exercise nothing.
    fn install_stub_php(dirs: &PlatformDirs, minor: &str, body: &str) {
        let php_bin_dir = dirs
            .data
            .join("php")
            .join(format!("php-{minor}"))
            .join("bin");
        fs::create_dir_all(&php_bin_dir).expect("mkdir php bin");
        let php_bin = php_bin_dir.join("php");
        fs::write(&php_bin, body).expect("write stub php");
        fs::set_permissions(&php_bin, fs::Permissions::from_mode(0o755)).expect("chmod +x");

        let ext_dir = dirs.data.join("php-ext").join(format!("php-{minor}"));
        fs::create_dir_all(&ext_dir).expect("mkdir php-ext");
        fs::write(ext_dir.join("pcov.so"), b"").expect("write stub pcov.so");
    }

    /// Build a faked `PlatformDirs` layout under a fresh tempdir: a stub PHP 8.4
    /// CLI binary and a stub `pcov.so`. Returns `(tempdir, home, expected cover.ini)`;
    /// the tempdir is kept alive by the caller.
    ///
    /// 8.4 must stay the only installed version here: the default-resolution
    /// tests rely on it winning `highest_installed`.
    fn faked_php_8_4_layout() -> (tempfile::TempDir, std::path::PathBuf, std::path::PathBuf) {
        let tmp = tempfile::tempdir().expect("tempdir");
        let home = tmp.path().join("home");
        fs::create_dir_all(&home).expect("mkdir home");
        let dirs = PlatformDirs::for_user(&home, 0);

        install_stub_php(&dirs, "8.4", STUB_PHP);

        let expected_phprc = dirs.data.join("php-ext").join("php-8.4").join("cover.ini");
        (tmp, home, expected_phprc)
    }

    /// Invoke `program` with `args` under the faked home's XDG environment and
    /// return its captured output. The environment is otherwise cleared, so a
    /// test that needs `YERD_COVER` or a `PATH` passes it in `extra_env`.
    fn run_in_home_with_env(
        program: &std::path::Path,
        args: &[&str],
        home: &std::path::Path,
        extra_env: &[(&str, &str)],
    ) -> std::process::Output {
        let mut cmd = Command::new(program);
        cmd.args(args)
            .env_clear()
            .env("HOME", home)
            .env("XDG_DATA_HOME", home.join(".local").join("share"))
            .env("XDG_CONFIG_HOME", home.join(".config"))
            .env("XDG_STATE_HOME", home.join(".local").join("state"))
            .env("XDG_CACHE_HOME", home.join(".cache"));
        for (k, v) in extra_env {
            cmd.env(k, v);
        }
        cmd.output().expect("run yerd")
    }

    /// [`run_in_home_with_env`] with nothing beyond the faked home's XDG vars.
    fn run_in_home(
        program: &std::path::Path,
        args: &[&str],
        home: &std::path::Path,
    ) -> std::process::Output {
        run_in_home_with_env(program, args, home, &[])
    }

    /// Assert the stub PHP saw `PHPRC` pointing at the cover ini and
    /// `YERD_COVER=1` exported on both the top-level process and its re-exec'd
    /// grandchild (coverage surviving the hop), that the cover ini was written,
    /// and that `expected_args` reached the top-level PHP verbatim - i.e. the
    /// launcher forwarded the caller's args and leaked no shim or subcommand name
    /// into them.
    fn assert_cover_run(
        output: &std::process::Output,
        expected_phprc: &std::path::Path,
        expected_args: &str,
    ) {
        assert!(
            output.status.success(),
            "cover run failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let stdout = String::from_utf8_lossy(&output.stdout);
        let want_phprc = expected_phprc.to_str().expect("utf8 path");
        let phprc: Vec<&str> = stdout
            .lines()
            .filter_map(|l| l.strip_prefix("phprc="))
            .collect();
        assert_eq!(
            phprc,
            vec![want_phprc, want_phprc],
            "PHPRC must be identical across the re-exec hop (top-level process and its grandchild)"
        );
        let cover: Vec<&str> = stdout
            .lines()
            .filter_map(|l| l.strip_prefix("yerd_cover="))
            .collect();
        assert_eq!(
            cover,
            vec!["1", "1"],
            "the cover launcher must export YERD_COVER=1 into the PHP process tree"
        );
        let args: Vec<&str> = stdout
            .lines()
            .filter_map(|l| l.strip_prefix("args="))
            .collect();
        assert_eq!(
            args,
            vec![expected_args],
            "forwarded args must reach PHP verbatim with no shim/subcommand name leaked"
        );
        assert!(expected_phprc.is_file(), "cover.ini must have been written");
    }

    /// Build a faked layout whose ONLY installed PHP is legacy 7.4 (stub CLI at
    /// `php-7.4/bin/php`). Returns `(tempdir, home)`.
    fn faked_legacy_only_layout() -> (tempfile::TempDir, std::path::PathBuf) {
        let tmp = tempfile::tempdir().expect("tempdir");
        let home = tmp.path().join("home");
        fs::create_dir_all(&home).expect("mkdir home");
        let dirs = PlatformDirs::for_user(&home, 0);
        let php_bin_dir = dirs.data.join("php").join("php-7.4").join("bin");
        fs::create_dir_all(&php_bin_dir).expect("mkdir php bin");
        let php_bin = php_bin_dir.join("php");
        fs::write(&php_bin, STUB_PHP).expect("write stub php");
        fs::set_permissions(&php_bin, fs::Permissions::from_mode(0o755)).expect("chmod +x");
        (tmp, home)
    }

    #[test]
    fn php74cover_errors_on_legacy() {
        let (tmp, home) = faked_legacy_only_layout();
        let cover_shim_bin = tmp.path().join("php7.4cover");
        symlink(env!("CARGO_BIN_EXE_yerd"), &cover_shim_bin).expect("symlink cover shim");

        let output = run_in_home(&cover_shim_bin, &["artisan", "test"], &home);
        assert!(!output.status.success(), "php7.4cover must fail on legacy");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains("pcov") && stderr.contains("legacy"),
            "stderr should explain pcov/legacy, got: {stderr}"
        );
    }

    #[test]
    fn bare_php_errors_when_only_legacy_installed_but_versioned_shim_runs() {
        let (tmp, home) = faked_legacy_only_layout();

        let php_shim = tmp.path().join("php");
        symlink(env!("CARGO_BIN_EXE_yerd"), &php_shim).expect("symlink php shim");
        let bare = run_in_home(&php_shim, &["--version"], &home);
        assert!(
            !bare.status.success(),
            "bare php must not run a legacy interpreter"
        );
        let stderr = String::from_utf8_lossy(&bare.stderr);
        assert!(
            stderr.contains("supported") && stderr.contains("php7.4"),
            "stderr should steer to a supported version, got: {stderr}"
        );

        let php74_shim = tmp.path().join("php7.4");
        symlink(env!("CARGO_BIN_EXE_yerd"), &php74_shim).expect("symlink php7.4 shim");
        let versioned = run_in_home(&php74_shim, &["--version"], &home);
        assert!(
            versioned.status.success(),
            "php7.4 must still run the legacy interpreter: {}",
            String::from_utf8_lossy(&versioned.stderr)
        );
    }

    #[test]
    fn phprc_survives_a_re_exec_grandchild_hop() {
        let (tmp, home, expected_phprc) = faked_php_8_4_layout();

        let cover_shim_bin = tmp.path().join("php8.4cover");
        symlink(env!("CARGO_BIN_EXE_yerd"), &cover_shim_bin).expect("symlink cover shim");

        let output = run_in_home(&cover_shim_bin, &["artisan", "test", "--coverage"], &home);
        assert_cover_run(&output, &expected_phprc, "artisan test --coverage");
    }

    /// The `yerd coverage` subcommand front door reaches the same cover-shim
    /// logic as the `phpcover` argv[0] shim: invoked as the real `yerd` binary,
    /// it resolves the default PHP (8.4, the only installed version) and enables
    /// pcov via `PHPRC`, which survives the grandchild hop identically. The args
    /// after `coverage` are forwarded to PHP verbatim - the subcommand name must
    /// not leak into them.
    #[test]
    fn coverage_subcommand_enables_pcov_like_phpcover() {
        let (_tmp, home, expected_phprc) = faked_php_8_4_layout();

        let output = run_in_home(
            std::path::Path::new(env!("CARGO_BIN_EXE_yerd")),
            &["coverage", "artisan", "test", "--coverage"],
            &home,
        );
        assert_cover_run(&output, &expected_phprc, "artisan test --coverage");
    }

    /// Collect the `phprc=` values the stub PHP printed, in order.
    fn phprc_lines(output: &std::process::Output) -> Vec<String> {
        String::from_utf8_lossy(&output.stdout)
            .lines()
            .filter_map(|l| l.strip_prefix("phprc=").map(str::to_owned))
            .collect()
    }

    /// With no cover shim anywhere in the picture, the plain `php` shim honours
    /// `YERD_COVER=1` from the environment by deriving the cover ini itself.
    #[test]
    fn plain_php_shim_derives_a_cover_ini_under_yerd_cover() {
        let (tmp, home, expected_phprc) = faked_php_8_4_layout();

        let php_shim = tmp.path().join("php");
        symlink(env!("CARGO_BIN_EXE_yerd"), &php_shim).expect("symlink php shim");

        let output = run_in_home_with_env(&php_shim, &["--version"], &home, &[("YERD_COVER", "1")]);
        assert!(
            output.status.success(),
            "plain php shim failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let want = expected_phprc.to_str().expect("utf8 path");
        assert_eq!(
            phprc_lines(&output),
            vec![want, want],
            "the plain shim must point PHPRC at the cover ini it derived"
        );
        let ini = fs::read_to_string(&expected_phprc).expect("read cover.ini");
        assert!(
            ini.contains("pcov.enabled = 1"),
            "cover ini must enable pcov, got: {ini}"
        );
    }

    /// The `PATH` hop from issue #221, across two PHP minors: `php8.5cover` execs
    /// the 8.5 interpreter, which spawns `php8.4` resolved from `PATH`. That child
    /// re-enters the plain CLI shim, which must derive the cover ini for ITS OWN
    /// version (8.4's, built from 8.4's ABI-specific `pcov.so`) rather than
    /// inheriting or reusing the parent's 8.5 one.
    #[test]
    fn path_hop_child_derives_the_cover_ini_for_its_own_version() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let home = tmp.path().join("home");
        fs::create_dir_all(&home).expect("mkdir home");
        let dirs = PlatformDirs::for_user(&home, 0);

        install_stub_php(&dirs, "8.5", STUB_PHP_PATH_HOP);
        install_stub_php(&dirs, "8.4", STUB_PHP_PRINT_ONLY);

        let path_dir = tmp.path().join("path");
        fs::create_dir_all(&path_dir).expect("mkdir path dir");
        symlink(env!("CARGO_BIN_EXE_yerd"), path_dir.join("php8.4")).expect("symlink php8.4 shim");

        let cover_shim_bin = tmp.path().join("php8.5cover");
        symlink(env!("CARGO_BIN_EXE_yerd"), &cover_shim_bin).expect("symlink cover shim");

        let path = format!("{}:/usr/bin:/bin", path_dir.display());
        let output = run_in_home_with_env(
            &cover_shim_bin,
            &["artisan", "test"],
            &home,
            &[("PATH", path.as_str())],
        );
        assert!(
            output.status.success(),
            "cross-version PATH hop failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );

        let parent_ini = dirs.data.join("php-ext").join("php-8.5").join("cover.ini");
        let child_ini = dirs.data.join("php-ext").join("php-8.4").join("cover.ini");
        assert_eq!(
            phprc_lines(&output),
            vec![
                parent_ini.to_str().expect("utf8 path"),
                child_ini.to_str().expect("utf8 path"),
            ],
            "the child must use its own version's cover ini, not the parent's"
        );
    }

    /// `YERD_COVER=1` with no `pcov.so` for the resolved version must not fail:
    /// the shim prints one notice and runs PHP on the normal per-version ini
    /// (here unset, since the faked layout generates no CLI ini).
    #[test]
    fn yerd_cover_falls_back_with_a_notice_when_pcov_is_missing() {
        let (tmp, home, expected_phprc) = faked_php_8_4_layout();
        fs::remove_file(expected_phprc.with_file_name("pcov.so")).expect("remove stub pcov.so");

        let php_shim = tmp.path().join("php");
        symlink(env!("CARGO_BIN_EXE_yerd"), &php_shim).expect("symlink php shim");

        let output = run_in_home_with_env(&php_shim, &["--version"], &home, &[("YERD_COVER", "1")]);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            output.status.success(),
            "the fallback must still run PHP, got: {stderr}"
        );
        let notices: Vec<&str> = stderr
            .lines()
            .filter(|l| l.contains("YERD_COVER"))
            .collect();
        assert_eq!(notices.len(), 1, "expected one notice line, got: {stderr}");
        assert!(
            notices[0].contains("pcov") && notices[0].contains("coverage"),
            "the notice must name pcov and coverage, got: {stderr}"
        );
        assert_eq!(
            phprc_lines(&output),
            vec!["", ""],
            "PHPRC must fall back to the normal CLI ini path"
        );
        assert!(
            !expected_phprc.exists(),
            "no cover ini may be written when pcov is missing"
        );
    }
}
