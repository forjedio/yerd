//! End-to-end: prove that `PHPRC`, set by the cover launcher on the process it
//! `exec`s, is inherited by a subsequent process that one re-execs itself - the
//! actual mechanism that lets coverage survive `artisan test`'s child
//! PHPUnit/Pest/paratest hop. Covers both front doors that reach the same
//! cover-shim logic: the `php<ver>cover` argv[0] shim and the `yerd coverage`
//! subcommand. Spawns the real built `yerd` binary against a fully faked
//! `PlatformDirs` layout (a stub standing in for the PHP interpreter: a shell
//! script on Unix, a `rustc`-compiled `php.exe` on Windows), rather than calling
//! `cover_shim::dispatch()` in-process, because it resolves
//! `ActivePaths::new().resolve()` internally with no dirs-injection seam. The
//! Windows leg also drives a real generated `.cmd` wrapper to prove the cmd hop
//! forwards args/`PHPRC` and does not swallow the child's exit code.

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
    /// `phprc=$PHPRC`; on the top-level pass (before the hop) it also prints
    /// `args=$*` so a test can assert the launcher forwarded the caller's
    /// arguments verbatim. It then re-execs itself once with `--grandchild`
    /// (the actual hop under test - a plain re-exec inherits the parent's
    /// environment, same as Symfony `Process` spawning `PHPUnit` via
    /// `PHP_BINARY`) and exits. The grandchild deliberately carries only
    /// `--grandchild`, so it exercises `PHPRC` inheritance, not arg forwarding.
    const STUB_PHP: &str = "#!/bin/sh\n\
        printf 'phprc=%s\\n' \"$PHPRC\"\n\
        case \"$1\" in\n\
        --grandchild) exit 0 ;;\n\
        esac\n\
        printf 'args=%s\\n' \"$*\"\n\
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

    /// Assert the stub PHP saw `PHPRC` pointing at the cover ini on both the
    /// top-level process and its re-exec'd grandchild (coverage surviving the
    /// hop), that the cover ini was written, and that `expected_args` reached the
    /// top-level PHP verbatim - i.e. the launcher forwarded the caller's args and
    /// leaked no shim or subcommand name into them.
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
}

#[cfg(windows)]
mod win_tests {
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::process::Command;

    use yerd_platform::pure::win_shim;

    /// A Rust stand-in for `php.exe`, compiled with `rustc` at test start (rustc
    /// is present on the CI/dev leg). On every process it prints `phprc=%PHPRC%`;
    /// on the top-level pass it also prints `args=<forwarded args>`, then spawns
    /// itself once with `--grandchild` (the inheritance hop under test - a plain
    /// child inherits the parent env, same as `artisan test`'s `PHPUnit` hop). A
    /// `--fail` argument makes it exit `3` immediately, exercising exit-code
    /// propagation through the `.cmd` wrapper hop.
    const STUB_PHP_RS: &str = r#"
use std::process::Command;
fn main() {
    let phprc = std::env::var("PHPRC").unwrap_or_default();
    println!("phprc={phprc}");
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "--fail") {
        std::process::exit(3);
    }
    if args.first().map(|s| s == "--grandchild").unwrap_or(false) {
        return;
    }
    println!("args={}", args.join(" "));
    let exe = std::env::current_exe().unwrap();
    let status = Command::new(exe).arg("--grandchild").status().unwrap();
    std::process::exit(status.code().unwrap_or(1));
}
"#;

    /// `%LOCALAPPDATA%\yerd\data` for a fake root, matching `WindowsPaths`.
    fn data_dir(root: &Path) -> PathBuf {
        root.join("local").join("yerd").join("data")
    }

    /// Build a faked Windows `PlatformDirs` layout under a fresh tempdir: a stub
    /// `php.exe` (flat layout `php-8.4/php.exe`) compiled via `rustc`, and a stub
    /// `pcov.dll`. Returns `(tempdir, root, expected cover.ini)`.
    fn faked_php_8_4_layout() -> (tempfile::TempDir, PathBuf, PathBuf) {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path().to_path_buf();
        for sub in ["appdata", "local", "temp"] {
            fs::create_dir_all(root.join(sub)).unwrap();
        }
        let data = data_dir(&root);

        let ver_root = data.join("php").join("php-8.4");
        fs::create_dir_all(&ver_root).unwrap();
        compile_stub_php(&ver_root.join("php.exe"));

        let ext_dir = data.join("php-ext").join("php-8.4");
        fs::create_dir_all(&ext_dir).unwrap();
        fs::write(ext_dir.join("pcov.dll"), b"").unwrap();

        let expected_phprc = ext_dir.join("cover.ini");
        (tmp, root, expected_phprc)
    }

    /// Build a faked layout whose only installed PHP is legacy 7.4.
    fn faked_legacy_only_layout() -> (tempfile::TempDir, PathBuf) {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path().to_path_buf();
        for sub in ["appdata", "local", "temp"] {
            fs::create_dir_all(root.join(sub)).unwrap();
        }
        let ver_root = data_dir(&root).join("php").join("php-7.4");
        fs::create_dir_all(&ver_root).unwrap();
        compile_stub_php(&ver_root.join("php.exe"));
        (tmp, root)
    }

    fn compile_stub_php(dest: &Path) {
        let src = dest
            .parent()
            .expect("dest has a parent")
            .join("php_stub_main.rs");
        fs::write(&src, STUB_PHP_RS).unwrap();
        let out = Command::new("rustc")
            .arg("-O")
            .arg("--crate-name")
            .arg("php_stub")
            .arg("-o")
            .arg(dest)
            .arg(&src)
            .output()
            .expect("run rustc to build the stub php.exe");
        assert!(
            out.status.success(),
            "rustc failed to build the stub php.exe: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    /// Run `program` with `args` under the faked root's Windows environment
    /// (`APPDATA`/`LOCALAPPDATA`/`TEMP`/`TMP` overridden, the rest inherited so
    /// `SystemRoot` etc. stay intact).
    fn run_in_root(program: &Path, args: &[&str], root: &Path) -> std::process::Output {
        Command::new(program)
            .args(args)
            .env("APPDATA", root.join("appdata"))
            .env("LOCALAPPDATA", root.join("local"))
            .env("TEMP", root.join("temp"))
            .env("TMP", root.join("temp"))
            .output()
            .expect("run yerd")
    }

    fn assert_cover_run(output: &std::process::Output, expected_phprc: &Path, expected_args: &str) {
        assert!(
            output.status.success(),
            "cover run failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let stdout = String::from_utf8_lossy(&output.stdout);
        let want = expected_phprc.to_str().expect("utf8 path");
        let phprc: Vec<&str> = stdout
            .lines()
            .filter_map(|l| l.strip_prefix("phprc="))
            .collect();
        assert_eq!(
            phprc,
            vec![want, want],
            "PHPRC must survive the child hop (top-level and grandchild): {stdout}"
        );
        let args: Vec<&str> = stdout
            .lines()
            .filter_map(|l| l.strip_prefix("args="))
            .collect();
        assert_eq!(
            args,
            vec![expected_args],
            "forwarded args must reach PHP verbatim, no shim/subcommand leaked"
        );
        assert!(expected_phprc.is_file(), "cover.ini must have been written");
    }

    fn yerd_exe() -> &'static Path {
        Path::new(env!("CARGO_BIN_EXE_yerd"))
    }

    #[test]
    fn php84cover_via_shim_sentinel_survives_hop() {
        let (_tmp, root, expected_phprc) = faked_php_8_4_layout();
        let output = run_in_root(
            yerd_exe(),
            &["__shim", "php8.4cover", "artisan", "test", "--coverage"],
            &root,
        );
        assert_cover_run(&output, &expected_phprc, "artisan test --coverage");
    }

    #[test]
    fn coverage_subcommand_enables_pcov() {
        let (_tmp, root, expected_phprc) = faked_php_8_4_layout();
        let output = run_in_root(
            yerd_exe(),
            &["coverage", "artisan", "test", "--coverage"],
            &root,
        );
        assert_cover_run(&output, &expected_phprc, "artisan test --coverage");
    }

    /// The `.cmd` wrapper hop: a real generated `php8.4cover.cmd` (as the daemon
    /// writes) invoked through `cmd.exe` must forward args + `PHPRC` and, crucially,
    /// propagate the child's exit code through `exit /b %ERRORLEVEL%`.
    #[test]
    fn cmd_wrapper_forwards_args_phprc_and_exit_code() {
        let (tmp, root, expected_phprc) = faked_php_8_4_layout();
        let wrapper = tmp.path().join(win_shim::wrapper_file_name("php8.4cover"));
        fs::write(&wrapper, win_shim::wrapper_body(yerd_exe(), "php8.4cover")).unwrap();

        let cmd = system32("cmd.exe");
        let ok = run_in_root(
            &cmd,
            &[
                "/c",
                wrapper.to_str().unwrap(),
                "artisan",
                "test",
                "--coverage",
            ],
            &root,
        );
        assert_cover_run(&ok, &expected_phprc, "artisan test --coverage");

        let failed = run_in_root(&cmd, &["/c", wrapper.to_str().unwrap(), "--fail"], &root);
        assert_eq!(
            failed.status.code(),
            Some(3),
            "the child's non-zero exit code must survive the .cmd hop"
        );
    }

    #[test]
    fn php74cover_errors_on_legacy() {
        let (_tmp, root) = faked_legacy_only_layout();
        let output = run_in_root(
            yerd_exe(),
            &["__shim", "php7.4cover", "artisan", "test"],
            &root,
        );
        assert!(
            !output.status.success(),
            "php7.4cover must fail on a legacy version"
        );
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains("pcov") && stderr.contains("legacy"),
            "stderr should explain pcov/legacy, got: {stderr}"
        );
    }

    fn system32(exe: &str) -> PathBuf {
        std::env::var_os("SystemRoot")
            .map_or_else(|| PathBuf::from(r"C:\Windows"), PathBuf::from)
            .join("System32")
            .join(exe)
    }
}
