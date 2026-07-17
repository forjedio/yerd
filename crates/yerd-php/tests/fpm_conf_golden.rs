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
fn ini_settings_render_exact() {
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
";
    assert_eq!(render_fpm_conf(&cfg), want);
}
