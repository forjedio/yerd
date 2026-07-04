//! End-to-end: prove that `PHPRC`, set by the `phpcover`/`php<ver>cover` shim on
//! the process it `exec`s, is inherited by a subsequent process that one
//! re-execs itself - the actual mechanism that lets coverage survive `artisan
//! test`'s child PHPUnit/Pest/paratest hop. Spawns the real built `yerd`
//! binary under a `php8.4cover` name against a fully faked `PlatformDirs`
//! layout (a stub shell script standing in for the PHP interpreter), rather
//! than calling `cover_shim::dispatch()` in-process, because it resolves
//! `ActivePaths::new().resolve()` internally with no dirs-injection seam.

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

    #[test]
    fn phprc_survives_a_re_exec_grandchild_hop() {
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

        let cover_shim_bin = tmp.path().join("php8.4cover");
        symlink(env!("CARGO_BIN_EXE_yerd"), &cover_shim_bin).expect("symlink cover shim");

        let output = Command::new(&cover_shim_bin)
            .env_clear()
            .env("HOME", &home)
            .env("XDG_DATA_HOME", home.join(".local").join("share"))
            .env("XDG_CONFIG_HOME", home.join(".config"))
            .env("XDG_STATE_HOME", home.join(".local").join("state"))
            .env("XDG_CACHE_HOME", home.join(".cache"))
            .output()
            .expect("run php8.4cover");

        assert!(
            output.status.success(),
            "php8.4cover failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );

        let expected_phprc = ext_dir.join("cover.ini");
        let stdout = String::from_utf8_lossy(&output.stdout);
        let lines: Vec<&str> = stdout.lines().collect();
        assert_eq!(
            lines,
            vec![
                expected_phprc.to_str().expect("utf8 path"),
                expected_phprc.to_str().expect("utf8 path"),
            ],
            "PHPRC must be identical across the re-exec hop (top-level process and its grandchild)"
        );
        assert!(expected_phprc.is_file(), "cover.ini must have been written");
    }
}
