//! End-to-end: run the real built `yerd` binary under the `phpdebug` and
//! `php8.4debug` names against a fully faked `PlatformDirs` layout, with a stub
//! shell script standing in for the PHP CLI. Proves the debug launcher forwards
//! the caller's arguments verbatim and execs PHP with `PHPRC` pointing at the
//! version's generated `debug.ini`, which loads the bundled `xdebug.so`. The
//! shim resolves `ActivePaths::new().resolve()` internally with no
//! dirs-injection seam, so it is driven as a child process, like the cover
//! shim e2e tests.

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
    use std::path::{Path, PathBuf};
    use std::process::Command;

    use yerd_platform::PlatformDirs;

    /// A stand-in PHP CLI that prints the `PHPRC` it was started with, then
    /// each argument it received on its own `arg=[…]` line, so argument
    /// boundaries survive.
    const STUB_PHP: &str = "#!/bin/sh\n\
        printf 'phprc=%s\\n' \"$PHPRC\"\n\
        for a in \"$@\"; do printf 'arg=[%s]\\n' \"$a\"; done\n";

    /// A faked home with a stub PHP 8.4 CLI, its bundled `xdebug.so` and Yerd's
    /// base CLI ini. Returns `(tempdir, home, dirs)`.
    fn faked_layout() -> (tempfile::TempDir, PathBuf, PlatformDirs) {
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
        fs::write(ext_dir.join("xdebug.so"), b"").expect("write stub xdebug.so");
        fs::write(dirs.data.join("php-cli.ini"), "memory_limit = 512M\n").expect("write cli ini");
        (tmp, home, dirs)
    }

    fn run_in_home(program: &Path, args: &[&str], home: &Path) -> std::process::Output {
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

    fn assert_debug_run(output: &std::process::Output, dirs: &PlatformDirs, want_args: &[&str]) {
        assert!(
            output.status.success(),
            "debug run failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let stdout = String::from_utf8_lossy(&output.stdout);
        let debug_ini = dirs.data.join("php-ext").join("php-8.4").join("debug.ini");
        let want_phprc = format!("phprc={}", debug_ini.display());
        assert!(
            stdout.lines().any(|l| l == want_phprc),
            "PHPRC must point at debug.ini, got: {stdout}"
        );
        let got_args: Vec<&str> = stdout
            .lines()
            .filter_map(|l| l.strip_prefix("arg=[")?.strip_suffix(']'))
            .collect();
        assert_eq!(
            got_args, want_args,
            "args must be forwarded verbatim, got: {stdout}"
        );
        let ini = fs::read_to_string(&debug_ini).expect("debug.ini written");
        let so = dirs.data.join("php-ext").join("php-8.4").join("xdebug.so");
        assert!(
            ini.starts_with("zend_extension = \""),
            "debug.ini must start with the Xdebug load line, got: {ini}"
        );
        assert_eq!(
            yerd_core::php_xdebug::ini_load_paths(&ini),
            vec![so.to_string_lossy().into_owned()],
            "debug.ini must load the bundled Xdebug, got: {ini}"
        );
        assert!(ini.contains("memory_limit = 512M"), "got: {ini}");
    }

    #[test]
    fn versioned_debug_shim_execs_php_with_the_debug_ini() {
        let (tmp, home, dirs) = faked_layout();
        let shim = tmp.path().join("php8.4debug");
        symlink(env!("CARGO_BIN_EXE_yerd"), &shim).expect("symlink debug shim");
        let output = run_in_home(&shim, &["artisan", "test", "--filter=Foo"], &home);
        assert_debug_run(&output, &dirs, &["artisan", "test", "--filter=Foo"]);
    }

    #[test]
    fn default_debug_shim_resolves_the_only_installed_version() {
        let (tmp, home, dirs) = faked_layout();
        let shim = tmp.path().join("phpdebug");
        symlink(env!("CARGO_BIN_EXE_yerd"), &shim).expect("symlink debug shim");
        let output = run_in_home(&shim, &["-r", "echo 1;"], &home);
        assert_debug_run(&output, &dirs, &["-r", "echo 1;"]);
    }

    #[test]
    fn debug_shim_without_the_base_cli_ini_asks_to_start_yerd() {
        let (tmp, home, dirs) = faked_layout();
        fs::remove_file(dirs.data.join("php-cli.ini")).expect("remove cli ini");
        let shim = tmp.path().join("php8.4debug");
        symlink(env!("CARGO_BIN_EXE_yerd"), &shim).expect("symlink debug shim");
        let output = run_in_home(&shim, &["-v"], &home);
        assert!(!output.status.success());
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains("start Yerd"), "got: {stderr}");
    }
}
