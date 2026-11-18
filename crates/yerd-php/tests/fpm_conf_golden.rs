//! Byte-exact golden test for the rendered FPM config. Pins the
//! template format - future edits flip this test deliberately.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::path::PathBuf;

use yerd_core::PhpVersion;
use yerd_php::pure::fpm_conf::render_fpm_conf;
use yerd_php::{Listen, PoolConfig};
use yerd_platform::PlatformDirs;

#[test]
fn dev_defaults_unix_renders_exact() {
    let dirs = PlatformDirs {
        config: PathBuf::from("/yerd/cfg"),
        data: PathBuf::from("/yerd/data"),
        state: PathBuf::from("/yerd/state"),
        cache: PathBuf::from("/yerd/cache"),
        runtime: PathBuf::from("/yerd/run"),
    };
    let v = PhpVersion::new(8, 3);
    let listen = Listen::UnixSocket(PathBuf::from("/yerd/run/fpm-8.3-1234.sock"));
    let cfg = PoolConfig::dev_defaults(v, listen, &dirs, 1234);

    let want = "\
[global]
pid = /yerd/state/fpm-8.3-1234.pid
error_log = /yerd/state/fpm-8.3-1234.log
daemonize = no

[yerd-8.3]
listen = /yerd/run/fpm-8.3-1234.sock
pm = ondemand
pm.max_children = 16
clear_env = no
catch_workers_output = yes
";
    assert_eq!(render_fpm_conf(&cfg), want);
}

#[test]
fn overridden_max_children_renders_exact() {
    let dirs = PlatformDirs {
        config: PathBuf::from("/yerd/cfg"),
        data: PathBuf::from("/yerd/data"),
        state: PathBuf::from("/yerd/state"),
        cache: PathBuf::from("/yerd/cache"),
        runtime: PathBuf::from("/yerd/run"),
    };
    let v = PhpVersion::new(8, 3);
    let listen = Listen::UnixSocket(PathBuf::from("/yerd/run/fpm-8.3-1234.sock"));
    let mut cfg = PoolConfig::dev_defaults(v, listen, &dirs, 1234);
    cfg.max_children = 32;

    let want = "\
[global]
pid = /yerd/state/fpm-8.3-1234.pid
error_log = /yerd/state/fpm-8.3-1234.log
daemonize = no

[yerd-8.3]
listen = /yerd/run/fpm-8.3-1234.sock
pm = ondemand
pm.max_children = 32
clear_env = no
catch_workers_output = yes
";
    assert_eq!(render_fpm_conf(&cfg), want);
}

#[test]
fn settings_and_directives_render_exact() {
    let dirs = PlatformDirs {
        config: PathBuf::from("/yerd/cfg"),
        data: PathBuf::from("/yerd/data"),
        state: PathBuf::from("/yerd/state"),
        cache: PathBuf::from("/yerd/cache"),
        runtime: PathBuf::from("/yerd/run"),
    };
    let v = PhpVersion::new(8, 3);
    let listen = Listen::UnixSocket(PathBuf::from("/yerd/run/fpm-8.3-1234.sock"));
    let mut cfg = PoolConfig::dev_defaults(v, listen, &dirs, 1234);
    cfg.ini = vec![
        ("display_errors".to_string(), "On".to_string()),
        ("memory_limit".to_string(), "1G".to_string()),
    ];
    cfg.directives = vec![
        ("opcache.enable".to_string(), "1".to_string()),
        ("xdebug.mode".to_string(), "debug".to_string()),
        ("extension".to_string(), "/evil.so".to_string()),
        ("bad name".to_string(), "x".to_string()),
    ];

    let want = "\
[global]
pid = /yerd/state/fpm-8.3-1234.pid
error_log = /yerd/state/fpm-8.3-1234.log
daemonize = no

[yerd-8.3]
listen = /yerd/run/fpm-8.3-1234.sock
pm = ondemand
pm.max_children = 16
clear_env = no
catch_workers_output = yes
php_flag[display_errors] = On
php_value[memory_limit] = 1G
php_value[opcache.enable] = 1
php_value[xdebug.mode] = debug
";
    assert_eq!(render_fpm_conf(&cfg), want);
}

fn golden_dirs() -> PlatformDirs {
    PlatformDirs {
        config: PathBuf::from("/yerd/cfg"),
        data: PathBuf::from("/yerd/data"),
        state: PathBuf::from("/yerd/state"),
        cache: PathBuf::from("/yerd/cache"),
        runtime: PathBuf::from("/yerd/run"),
    }
}

#[test]
fn ffi_enable_on_renders_admin_value_exact() {
    let v = PhpVersion::new(8, 3);
    let listen = Listen::UnixSocket(PathBuf::from("/yerd/run/fpm-8.3-1234.sock"));
    let mut cfg = PoolConfig::dev_defaults(v, listen, &golden_dirs(), 1234);
    cfg.ini = vec![
        ("display_errors".to_string(), "On".to_string()),
        ("ffi.enable".to_string(), "On".to_string()),
        ("memory_limit".to_string(), "1G".to_string()),
    ];

    let want = "\
[global]
pid = /yerd/state/fpm-8.3-1234.pid
error_log = /yerd/state/fpm-8.3-1234.log
daemonize = no

[yerd-8.3]
listen = /yerd/run/fpm-8.3-1234.sock
pm = ondemand
pm.max_children = 16
clear_env = no
catch_workers_output = yes
php_flag[display_errors] = On
php_admin_value[ffi.enable] = true
php_value[memory_limit] = 1G
";
    assert_eq!(render_fpm_conf(&cfg), want);
}

#[test]
fn ffi_enable_off_or_legacy_renders_nothing() {
    let off = {
        let listen = Listen::UnixSocket(PathBuf::from("/yerd/run/fpm-8.3-1234.sock"));
        let mut cfg = PoolConfig::dev_defaults(PhpVersion::new(8, 3), listen, &golden_dirs(), 1234);
        cfg.ini = vec![("ffi.enable".to_string(), "Off".to_string())];
        cfg
    };
    let legacy = {
        let listen = Listen::UnixSocket(PathBuf::from("/yerd/run/fpm-8.1-1234.sock"));
        let mut cfg = PoolConfig::dev_defaults(PhpVersion::new(8, 1), listen, &golden_dirs(), 1234);
        cfg.ini = vec![("ffi.enable".to_string(), "On".to_string())];
        cfg
    };
    assert!(!render_fpm_conf(&off).contains("ffi.enable"));
    assert!(!render_fpm_conf(&legacy).contains("ffi.enable"));
}

#[test]
fn managed_xdebug_drops_managed_directives_exact() {
    let dirs = PlatformDirs {
        config: PathBuf::from("/yerd/cfg"),
        data: PathBuf::from("/yerd/data"),
        state: PathBuf::from("/yerd/state"),
        cache: PathBuf::from("/yerd/cache"),
        runtime: PathBuf::from("/yerd/run"),
    };
    let v = PhpVersion::new(8, 4);
    let listen = Listen::UnixSocket(PathBuf::from("/yerd/run/fpm-8.4-1234.sock"));
    let mut cfg = PoolConfig::dev_defaults(v, listen, &dirs, 1234);
    cfg.directives = vec![
        ("opcache.jit".to_string(), "tracing".to_string()),
        ("xdebug.idekey".to_string(), "PHPSTORM".to_string()),
        ("xdebug.mode".to_string(), "coverage".to_string()),
    ];
    cfg.xdebug = Some(yerd_php::XdebugLoad {
        path: PathBuf::from("/yerd/data/php-ext/php-8.4/xdebug.so"),
        defines: vec![("xdebug.mode".to_string(), "debug".to_string())],
    });

    let want = "\
[global]
pid = /yerd/state/fpm-8.4-1234.pid
error_log = /yerd/state/fpm-8.4-1234.log
daemonize = no

[yerd-8.4]
listen = /yerd/run/fpm-8.4-1234.sock
pm = ondemand
pm.max_children = 16
clear_env = no
catch_workers_output = yes
php_value[xdebug.idekey] = PHPSTORM
";
    assert_eq!(render_fpm_conf(&cfg), want);
}
