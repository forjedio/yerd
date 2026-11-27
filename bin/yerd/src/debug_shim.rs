//! Xdebug "debug" CLI shims (`phpdebug`, `php<major>.<minor>debug`).
//!
//! These names are symlinks in `{data}/bin` pointing at *this* `yerd` binary,
//! like the `phpcover` shims. When `yerd` is invoked under such a name it
//! resolves the matching PHP CLI binary plus that version's bundled
//! `xdebug.so`, points `PHPRC` at an Xdebug-augmented copy of Yerd's CLI ini
//! (`{data}/php-ext/php-<minor>/debug.ini`), and `exec`s PHP. The web toggle
//! (`yerd xdebug on`) is not consulted: the shim loads Xdebug for this one
//! command either way, with the default settings (debug mode, start on every
//! request), which the user's own per-version `xdebug.*` CLI directives or an
//! `XDEBUG_MODE` environment variable can override.
//!
//! The shims never talk to the daemon. The `.so` is downloaded by the daemon the
//! first time `yerd xdebug on <minor>` runs, so a missing one is a clear error.
//!
//! Unlike the cover shims, no `YERD_*` re-entry variable is exported, and the
//! shim refuses to run until Yerd's base CLI ini (`{data}/php-cli.ini`) exists.
//! With that file present every plain `php`/`composer`/`laravel`/`wp` shim sets
//! its own `PHPRC`, so a child that re-enters through `PATH` overwrites the
//! inherited `PHPRC=debug.ini` and plain `php` never loads Xdebug. Unix-only:
//! debug shims are never created on other platforms.

use std::ffi::OsString;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

use yerd_core::{php_xdebug, PhpVersion};
use yerd_platform::{ActivePaths, Paths, PlatformDirs};

use crate::shim::{cli_binary, cli_phprc, fail, resolve_default_php};

/// Which PHP a debug alias targets.
enum DebugSpec {
    /// `phpdebug` - the default version (resolved at run time).
    Default,
    /// `php<major>.<minor>debug` - an explicit version.
    Version(u8, u8),
}

/// If `argv[0]` is a debug-alias name, run that PHP with Xdebug loaded and
/// return its exit code (on success `exec` replaces the process and never
/// returns); otherwise `None`, so `main` falls through to the next shim.
#[must_use]
pub fn dispatch() -> Option<ExitCode> {
    let arg0 = std::env::args_os().next()?;
    let name = Path::new(&arg0).file_name()?.to_str()?;
    let spec = parse_debug_name(name)?;
    let forward: Vec<OsString> = std::env::args_os().skip(1).collect();
    Some(run(&spec, &forward))
}

/// Parse a debug-alias basename. Matches `phpdebug` and
/// `php<MAJOR>.<MINOR>debug` exactly; returns `None` for anything else.
fn parse_debug_name(name: &str) -> Option<DebugSpec> {
    let rest = name.strip_prefix("php")?;
    let rest = rest.strip_suffix("debug")?;
    if rest.is_empty() {
        return Some(DebugSpec::Default);
    }
    let (maj, min) = rest.split_once('.')?;
    if maj.is_empty() || min.is_empty() {
        return None;
    }
    let major: u8 = maj.parse().ok()?;
    let minor: u8 = min.parse().ok()?;
    Some(DebugSpec::Version(major, minor))
}

fn run(spec: &DebugSpec, forward: &[OsString]) -> ExitCode {
    let dirs = match ActivePaths::new().resolve() {
        Ok(d) => d,
        Err(e) => return fail(format!("cannot resolve yerd directories: {e}")),
    };
    let (php_bin, minor) = match resolve_target(&dirs, spec) {
        Ok(t) => t,
        Err(msg) => return fail(msg),
    };
    let debug_ini_path = match prepare_debug_ini(&dirs, &minor) {
        Ok(p) => p,
        Err(msg) => return fail(msg),
    };

    let mut cmd = Command::new(&php_bin);
    cmd.env("PHPRC", &debug_ini_path).args(forward);
    crate::shim::apply_network_env(&mut cmd, &dirs);
    let err = cmd.exec();
    if err.kind() == std::io::ErrorKind::NotFound {
        return fail(format!(
            "PHP binary not found at {} ({err}) - reinstall with `yerd install php {minor}`",
            php_bin.display()
        ));
    }
    fail(format!("failed to exec {}: {err}", php_bin.display()))
}

/// The error when Yerd's base CLI ini doesn't exist: the daemon writes it while
/// starting, so a running daemon has to be restarted to rewrite it.
const MISSING_CLI_INI: &str = "Yerd's CLI php.ini is missing. If Yerd is running, restart it \
     (`yerd restart daemon`, or quit and reopen the app); if it isn't, start Yerd (open the \
     app, or start its background service). Wait for it to finish starting, then retry";

/// The error for a missing `xdebug.so`: whether one is published for `minor` on
/// this host decides between "not built here" and "run `yerd xdebug on` once".
fn missing_so_message(available: bool, minor: &str) -> String {
    if available {
        format!(
            "Xdebug isn't downloaded for PHP {minor} yet - run `yerd xdebug on {minor}` once. \
             That also turns on web debugging and restarts that version's pool; run \
             `yerd xdebug off {minor}` afterwards if you only want the CLI (the download is kept)"
        )
    } else {
        format!("Xdebug isn't built for PHP {minor} on this platform")
    }
}

/// Derive the Xdebug-enabled ini for `minor` and return its path
/// (`{data}/php-ext/php-<minor>/debug.ini`), writing it atomically.
///
/// Fails with a message when the version's `xdebug.so` is missing, when Yerd's
/// base CLI ini hasn't been written yet (see the module docs for why that is
/// required), when `yerd.toml` or the CLI ini already loads a hand-registered
/// Xdebug, when the `.so` path can't be rendered as an ini value, or when the
/// write fails.
fn prepare_debug_ini(dirs: &PlatformDirs, minor: &str) -> Result<PathBuf, String> {
    let ext_dir = dirs.data.join("php-ext").join(format!("php-{minor}"));
    let so = ext_dir.join("xdebug.so");
    if !so.is_file() {
        let available = minor.parse::<PhpVersion>().is_ok_and(|v| {
            php_xdebug::is_available(v, std::env::consts::OS, std::env::consts::ARCH)
        });
        return Err(missing_so_message(available, minor));
    }
    if !dirs.data.join("php-cli.ini").is_file() {
        return Err(MISSING_CLI_INI.to_owned());
    }
    let base = match cli_phprc(dirs, minor) {
        Some(ini) => std::fs::read_to_string(&ini)
            .map_err(|e| format!("cannot read Yerd's CLI php.ini {}: {e}", ini.display()))?,
        None => String::new(),
    };
    if let Some(name) = registered_xdebug(dirs, minor, &base) {
        return Err(format!(
            "Xdebug is already loaded for PHP {minor} by a hand-registered extension - remove it \
             with `yerd php ext remove {minor} {name}` or use plain `php`"
        ));
    }
    if php_xdebug::ini_loads_xdebug(&base) {
        return Err(format!(
            "Xdebug is already loaded for PHP {minor} by a hand-registered extension - remove it \
             with `yerd php ext remove {minor} <name>` (see `yerd php ext list`) or use plain `php`"
        ));
    }
    let Some(debug_ini) = php_xdebug::render_debug_ini(&base, &so) else {
        return Err(format!(
            "cannot load Xdebug: {} isn't safe to use as an ini value (it must be valid UTF-8 with no control characters) - move Yerd's data directory to a path without those",
            so.display()
        ));
    };
    let debug_ini_path = ext_dir.join("debug.ini");
    crate::cover_shim::atomic_write(&debug_ini_path, debug_ini.as_bytes())
        .map_err(|e| format!("cannot write {}: {e}", debug_ini_path.display()))?;
    Ok(debug_ini_path)
}

/// The name of an extension registered in `yerd.toml` for `minor` that is an
/// Xdebug (by name or `.so` basename), that `base`, the CLI ini this shim
/// read, loads by its exact path, and whose file exists. A registered Xdebug
/// the ini doesn't load (the ini predates it) or whose file is gone is ignored:
/// PHP can't load a missing file, and only a real second load would clash with
/// the bundled one. A missing or unreadable config counts as none.
///
/// This is narrower than the daemon's FPM-side check, which refuses any
/// registered Xdebug for the version whether or not a current ini loads it:
/// the daemon decides what gets saved and rendered into every future pool and
/// CLI ini, while the shim only guards the one PHP process it's about to run.
fn registered_xdebug(dirs: &PlatformDirs, minor: &str, base: &str) -> Option<String> {
    let v = minor.parse::<PhpVersion>().ok()?;
    let cfg = yerd_config::Config::load(&dirs.config.join("yerd.toml")).ok()?;
    let loaded = php_xdebug::ini_load_paths(base);
    cfg.php
        .extensions
        .get(&v)?
        .iter()
        .find(|e| {
            php_xdebug::is_xdebug_extension(&e.name, &e.path)
                && loaded.contains(&e.path)
                && Path::new(&e.path).is_file()
        })
        .map(|e| e.name.clone())
}

/// Resolve `(php_binary, "major.minor")` for the spec. Legacy versions are
/// rejected before the install check, since no Xdebug is ever built for them.
fn resolve_target(dirs: &PlatformDirs, spec: &DebugSpec) -> Result<(PathBuf, String), String> {
    match spec {
        DebugSpec::Version(maj, min) => {
            if PhpVersion::new(*maj, *min).is_legacy() {
                return Err(format!(
                    "Xdebug is not built for legacy PHP {maj}.{min} (< 8.2). Use a supported PHP \
                     version."
                ));
            }
            let minor = format!("{maj}.{min}");
            let php = cli_binary(dirs, &minor);
            if php.is_file() {
                Ok((php, minor))
            } else {
                Err(format!(
                    "PHP {minor} is not installed - run `yerd install php {minor}`"
                ))
            }
        }
        DebugSpec::Default => {
            resolve_default_php(dirs).ok_or_else(|| crate::shim::no_default_php_message(dirs))
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn parses_default_and_versioned_debug_names() {
        assert!(matches!(
            parse_debug_name("phpdebug"),
            Some(DebugSpec::Default)
        ));
        assert!(matches!(
            parse_debug_name("php8.4debug"),
            Some(DebugSpec::Version(8, 4))
        ));
    }

    #[test]
    fn ignores_non_debug_names() {
        for name in [
            "php",
            "php8.4",
            "php8.4cover",
            "phpcover",
            "php8.4debugx",
            "php8.debug",
            "phpx.4debug",
            "yerd",
        ] {
            assert!(parse_debug_name(name).is_none(), "{name}");
        }
    }

    fn dirs_at(tmp: &Path) -> PlatformDirs {
        PlatformDirs {
            config: tmp.join("c"),
            data: tmp.join("d"),
            state: tmp.join("s"),
            cache: tmp.join("ca"),
            runtime: tmp.join("r"),
        }
    }

    fn place_so(dirs: &PlatformDirs, minor: &str) -> PathBuf {
        let dir = dirs.data.join("php-ext").join(format!("php-{minor}"));
        std::fs::create_dir_all(&dir).unwrap();
        let so = dir.join("xdebug.so");
        std::fs::write(&so, b"so").unwrap();
        so
    }

    fn write_base_ini(dirs: &PlatformDirs, body: &str) {
        std::fs::create_dir_all(&dirs.data).unwrap();
        std::fs::write(dirs.data.join("php-cli.ini"), body).unwrap();
    }

    #[test]
    fn missing_so_message_depends_on_availability() {
        let available = missing_so_message(true, "8.4");
        assert!(available.contains("yerd xdebug on 8.4"), "got {available}");
        assert!(available.contains("yerd xdebug off 8.4"), "got {available}");
        let unavailable = missing_so_message(false, "8.4");
        assert!(unavailable.contains("isn't built"), "got {unavailable}");
        assert!(!unavailable.contains("yerd xdebug on"), "got {unavailable}");
    }

    #[test]
    fn missing_so_is_an_error_and_writes_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        let dirs = dirs_at(tmp.path());
        write_base_ini(&dirs, "memory_limit = 512M\n");
        let err = prepare_debug_ini(&dirs, "8.4").unwrap_err();
        assert!(err.contains("Xdebug"), "got {err}");
        assert!(!dirs
            .data
            .join("php-ext")
            .join("php-8.4")
            .join("debug.ini")
            .exists());
    }

    #[test]
    fn missing_base_cli_ini_asks_to_start_yerd() {
        let tmp = tempfile::tempdir().unwrap();
        let dirs = dirs_at(tmp.path());
        place_so(&dirs, "8.4");
        let err = prepare_debug_ini(&dirs, "8.4").unwrap_err();
        assert!(err.contains("start Yerd"), "got {err}");
        assert!(err.contains("background service"), "got {err}");
        assert!(err.contains("If Yerd is running, restart it"), "got {err}");
        assert!(err.contains("`yerd restart daemon`"), "got {err}");
        assert!(err.contains("then retry"), "got {err}");
        assert!(!err.contains("yerd status"), "got {err}");
    }

    #[test]
    fn present_so_writes_debug_ini_with_base_after_managed_lines() {
        let tmp = tempfile::tempdir().unwrap();
        let dirs = dirs_at(tmp.path());
        let so = place_so(&dirs, "8.4");
        write_base_ini(&dirs, "memory_limit = 512M\n");
        let path = prepare_debug_ini(&dirs, "8.4").unwrap();
        assert_eq!(
            path,
            dirs.data.join("php-ext").join("php-8.4").join("debug.ini")
        );
        let ini = std::fs::read_to_string(&path).unwrap();
        assert!(ini.starts_with("zend_extension = \""), "got {ini}");
        assert_eq!(
            php_xdebug::ini_load_paths(&ini),
            vec![so.to_string_lossy().into_owned()]
        );
        assert!(ini.contains("xdebug.mode = debug"), "got {ini}");
        assert!(ini.contains("opcache.jit = disable"), "got {ini}");
        let managed = ini.find("xdebug.mode").unwrap();
        let base = ini.find("memory_limit = 512M").unwrap();
        assert!(managed < base, "got {ini}");
        let jit = ini.find("opcache.jit = disable").unwrap();
        assert!(base < jit, "got {ini}");
    }

    #[test]
    fn per_version_cli_ini_is_preferred_as_the_base() {
        let tmp = tempfile::tempdir().unwrap();
        let dirs = dirs_at(tmp.path());
        place_so(&dirs, "8.4");
        write_base_ini(&dirs, "memory_limit = 128M\n");
        std::fs::write(
            dirs.data.join("php-cli-8.4.ini"),
            "xdebug.client_port = 9100\n",
        )
        .unwrap();
        let ini = std::fs::read_to_string(prepare_debug_ini(&dirs, "8.4").unwrap()).unwrap();
        assert!(ini.contains("xdebug.client_port = 9100"), "got {ini}");
        assert!(!ini.contains("memory_limit = 128M"), "got {ini}");
    }

    #[test]
    fn hand_registered_xdebug_is_refused() {
        let tmp = tempfile::tempdir().unwrap();
        let dirs = dirs_at(tmp.path());
        place_so(&dirs, "8.4");
        write_base_ini(&dirs, "zend_extension = \"/x/xdebug.so\"\n");
        let err = prepare_debug_ini(&dirs, "8.4").unwrap_err();
        assert!(err.contains("already loaded"), "got {err}");
        assert!(err.contains("yerd php ext remove 8.4"), "got {err}");
    }

    fn write_config_with_extension(dirs: &PlatformDirs, name: &str, path: &str) {
        let mut cfg = yerd_config::Config::default();
        cfg.php.extensions.insert(
            PhpVersion::new(8, 4),
            vec![yerd_config::ExtEntry {
                name: name.to_owned(),
                path: path.to_owned(),
                zend: true,
            }],
        );
        std::fs::create_dir_all(&dirs.config).unwrap();
        cfg.save(&dirs.config.join("yerd.toml")).unwrap();
    }

    #[test]
    fn xdebug_registered_by_name_at_another_path_is_refused() {
        let tmp = tempfile::tempdir().unwrap();
        let dirs = dirs_at(tmp.path());
        place_so(&dirs, "8.4");
        let custom = tmp.path().join("debugger-3.4.so");
        std::fs::write(&custom, b"so").unwrap();
        let custom = custom.to_string_lossy().into_owned();
        write_base_ini(&dirs, &format!("zend_extension = {custom}\n"));
        write_config_with_extension(&dirs, "xdebug", &custom);
        let err = prepare_debug_ini(&dirs, "8.4").unwrap_err();
        assert!(err.contains("already loaded"), "got {err}");
        assert!(err.contains("yerd php ext remove 8.4 xdebug"), "got {err}");
    }

    #[test]
    fn registered_xdebug_the_cli_ini_does_not_load_is_ignored() {
        let tmp = tempfile::tempdir().unwrap();
        let dirs = dirs_at(tmp.path());
        place_so(&dirs, "8.4");
        let custom = tmp.path().join("debugger-3.4.so");
        std::fs::write(&custom, b"so").unwrap();
        let custom = custom.to_string_lossy().into_owned();
        write_config_with_extension(&dirs, "xdebug", &custom);
        write_base_ini(&dirs, "memory_limit = 512M\n");
        assert!(prepare_debug_ini(&dirs, "8.4").is_ok());
        write_base_ini(
            &dirs,
            &format!("zend_extension = \"{custom}.old\"\nzend_extension = /x/a.so\n"),
        );
        assert!(prepare_debug_ini(&dirs, "8.4").is_ok());
        write_base_ini(&dirs, &format!("zend_extension = \"{custom}\"\n"));
        let err = prepare_debug_ini(&dirs, "8.4").unwrap_err();
        assert!(err.contains("yerd php ext remove 8.4 xdebug"), "got {err}");
    }

    #[test]
    fn registered_xdebug_with_a_missing_file_is_ignored() {
        let tmp = tempfile::tempdir().unwrap();
        let dirs = dirs_at(tmp.path());
        place_so(&dirs, "8.4");
        write_base_ini(&dirs, "memory_limit = 512M\n");
        let gone = tmp.path().join("gone").join("xdebug.so");
        write_config_with_extension(&dirs, "xdebug", &gone.to_string_lossy());
        assert!(prepare_debug_ini(&dirs, "8.4").is_ok());
    }

    #[test]
    fn registered_xdebug_the_ini_loads_but_whose_file_is_gone_is_ignored() {
        let tmp = tempfile::tempdir().unwrap();
        let dirs = dirs_at(tmp.path());
        place_so(&dirs, "8.4");
        let custom = tmp.path().join("debugger-3.4.so");
        std::fs::write(&custom, b"so").unwrap();
        let custom_str = custom.to_string_lossy().into_owned();
        write_config_with_extension(&dirs, "xdebug", &custom_str);
        write_base_ini(&dirs, &format!("zend_extension = \"{custom_str}\"\n"));
        assert!(prepare_debug_ini(&dirs, "8.4").is_err());

        std::fs::remove_file(&custom).unwrap();
        assert!(prepare_debug_ini(&dirs, "8.4").is_ok());
    }

    #[test]
    fn unrelated_registered_extension_and_bad_config_are_allowed() {
        let tmp = tempfile::tempdir().unwrap();
        let dirs = dirs_at(tmp.path());
        place_so(&dirs, "8.4");
        write_base_ini(&dirs, "memory_limit = 512M\n");
        write_config_with_extension(&dirs, "redis", "/opt/ext/redis.so");
        assert!(prepare_debug_ini(&dirs, "8.4").is_ok());
        std::fs::write(dirs.config.join("yerd.toml"), "not = [valid").unwrap();
        assert!(prepare_debug_ini(&dirs, "8.4").is_ok());
    }

    #[test]
    fn resolve_target_rejects_legacy_before_checking_install() {
        let tmp = tempfile::tempdir().unwrap();
        let dirs = dirs_at(tmp.path());
        match resolve_target(&dirs, &DebugSpec::Version(7, 4)) {
            Err(msg) => assert!(msg.contains("legacy"), "got {msg}"),
            Ok(_) => panic!("expected legacy rejection"),
        }
        match resolve_target(&dirs, &DebugSpec::Version(8, 4)) {
            Err(msg) => assert!(msg.contains("not installed"), "got {msg}"),
            Ok(_) => panic!("expected not-installed error"),
        }
    }
}
