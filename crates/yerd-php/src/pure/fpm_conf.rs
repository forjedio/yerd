//! Pure FPM config rendering.
//!
//! Given a [`PoolConfig`], produces the text of a `php-fpm.conf` file.
//! No I/O; the caller writes the returned string to disk.
//!
//! Layout:
//! - `[global]` block with `pid`, `error_log`, `daemonize = no`
//!   (we supervise in the foreground; `--nodaemonize` is **not** also
//!   passed on the CLI - single source of truth).
//! - `[yerd-<version>]` pool block with `listen`, `pm`, `pm.max_children`,
//!   `clear_env = no`, `catch_workers_output = yes`.
//!
//! `clear_env = no` is deliberate: it lets the manager pre-scrub the env
//! via [`crate::pure::env_scrub::allowlist`] before spawn instead of FPM
//! doing its own (more aggressive) scrub. The allowlist is the security
//! boundary - see `env_scrub` rustdoc for the retained keys.

use std::fmt::Write;

use crate::listen::Listen;
use crate::pool::{PoolConfig, ProcessManagerMode};

/// Render `cfg` to a PHP-FPM config-file string.
#[must_use]
pub fn render_fpm_conf(cfg: &PoolConfig) -> String {
    let mut out = String::with_capacity(512);
    let listen = render_listen(&cfg.listen);
    let pm = render_pm(cfg.pm);
    let pool = format!("yerd-{}", cfg.version);

    let _ = writeln!(out, "[global]");
    let _ = writeln!(out, "pid = {}", cfg.pid_file.display());
    let _ = writeln!(out, "error_log = {}", cfg.error_log.display());
    let _ = writeln!(out, "daemonize = no");
    let _ = writeln!(out);
    let _ = writeln!(out, "[{pool}]");
    let _ = writeln!(out, "listen = {listen}");
    let _ = writeln!(out, "pm = {pm}");
    let _ = writeln!(out, "pm.max_children = {}", cfg.max_children);
    let _ = writeln!(out, "clear_env = no");
    let _ = writeln!(out, "catch_workers_output = yes");

    if let Some(path) = &cfg.ca_bundle {
        let p = path.display().to_string();
        if !p.chars().any(char::is_control) {
            let _ = writeln!(out, "php_admin_value[openssl.cafile] = {p}");
            let _ = writeln!(out, "php_admin_value[curl.cainfo] = {p}");
        }
    }

    for (key, value) in &cfg.ini {
        if let Some(directive) = yerd_core::php_settings::directive(key) {
            if yerd_core::php_settings::validate_value(key, value).is_ok() {
                let _ = writeln!(out, "{directive}[{key}] = {value}");
            }
        }
    }

    out
}

fn render_listen(listen: &Listen) -> String {
    match listen {
        Listen::UnixSocket(p) => p.display().to_string(),
        Listen::TcpLoopback(addr) => addr.to_string(),
    }
}

fn render_pm(pm: ProcessManagerMode) -> &'static str {
    match pm {
        ProcessManagerMode::Static => "static",
        ProcessManagerMode::Dynamic => "dynamic",
        ProcessManagerMode::OnDemand => "ondemand",
    }
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
    use std::path::PathBuf;
    use yerd_core::PhpVersion;

    fn cfg_unix(pm: ProcessManagerMode) -> PoolConfig {
        PoolConfig {
            version: PhpVersion::new(8, 3),
            listen: Listen::UnixSocket(PathBuf::from("/run/fpm-8.3-1.sock")),
            pid_file: PathBuf::from("/state/fpm-8.3-1.pid"),
            error_log: PathBuf::from("/state/fpm-8.3-1.log"),
            config_path: PathBuf::from("/cfg/php-fpm-8.3-1.conf"),
            pm,
            max_children: 16,
            ini: Vec::new(),
            extension: None,
            ini_defines: Vec::new(),
            ca_bundle: None,
        }
    }

    fn cfg_tcp() -> PoolConfig {
        PoolConfig {
            version: PhpVersion::new(8, 3),
            listen: Listen::TcpLoopback("127.0.0.1:9501".parse().unwrap()),
            pid_file: PathBuf::from("/state/fpm-8.3-1.pid"),
            error_log: PathBuf::from("/state/fpm-8.3-1.log"),
            config_path: PathBuf::from("/cfg/php-fpm-8.3-1.conf"),
            pm: ProcessManagerMode::OnDemand,
            max_children: 16,
            ini: Vec::new(),
            extension: None,
            ini_defines: Vec::new(),
            ca_bundle: None,
        }
    }

    #[test]
    fn unix_ondemand_byte_exact() {
        let want = "\
[global]
pid = /state/fpm-8.3-1.pid
error_log = /state/fpm-8.3-1.log
daemonize = no

[yerd-8.3]
listen = /run/fpm-8.3-1.sock
pm = ondemand
pm.max_children = 16
clear_env = no
catch_workers_output = yes
";
        assert_eq!(
            render_fpm_conf(&cfg_unix(ProcessManagerMode::OnDemand)),
            want
        );
    }

    #[test]
    fn unix_static_renders_static_mode() {
        let s = render_fpm_conf(&cfg_unix(ProcessManagerMode::Static));
        assert!(s.contains("pm = static\n"), "got: {s}");
    }

    #[test]
    fn unix_dynamic_renders_dynamic_mode() {
        let s = render_fpm_conf(&cfg_unix(ProcessManagerMode::Dynamic));
        assert!(s.contains("pm = dynamic\n"), "got: {s}");
    }

    #[test]
    fn tcp_renders_loopback_literal() {
        let s = render_fpm_conf(&cfg_tcp());
        assert!(s.contains("listen = 127.0.0.1:9501\n"), "got: {s}");
    }

    #[test]
    fn pool_name_includes_version() {
        let s = render_fpm_conf(&cfg_unix(ProcessManagerMode::OnDemand));
        assert!(s.contains("[yerd-8.3]"), "got: {s}");
    }

    #[test]
    fn ini_settings_render_as_value_and_flag_directives() {
        let mut cfg = cfg_unix(ProcessManagerMode::OnDemand);
        cfg.ini = vec![
            ("display_errors".to_string(), "On".to_string()),
            ("memory_limit".to_string(), "512M".to_string()),
        ];
        let s = render_fpm_conf(&cfg);
        assert!(s.contains("php_value[memory_limit] = 512M\n"), "got: {s}");
        assert!(s.contains("php_flag[display_errors] = On\n"), "got: {s}");
        assert!(
            s.find("catch_workers_output").unwrap() < s.find("php_value[memory_limit]").unwrap()
        );
    }

    #[test]
    fn ini_settings_skip_unsupported_and_unsafe_values() {
        let mut cfg = cfg_unix(ProcessManagerMode::OnDemand);
        cfg.ini = vec![
            ("not_a_setting".to_string(), "x".to_string()),
            ("memory_limit".to_string(), "256M; evil".to_string()),
        ];
        let s = render_fpm_conf(&cfg);
        assert!(!s.contains("not_a_setting"), "unsupported key leaked: {s}");
        assert!(!s.contains("evil"), "unsafe value leaked: {s}");
    }

    #[test]
    fn ca_bundle_renders_admin_cafile_and_cainfo() {
        let mut cfg = cfg_unix(ProcessManagerMode::OnDemand);
        cfg.ca_bundle = Some(PathBuf::from("/data/dir/cacert.pem"));
        let s = render_fpm_conf(&cfg);
        assert!(
            s.contains("php_admin_value[openssl.cafile] = /data/dir/cacert.pem\n"),
            "got: {s}"
        );
        assert!(
            s.contains("php_admin_value[curl.cainfo] = /data/dir/cacert.pem\n"),
            "got: {s}"
        );
    }

    #[test]
    fn ca_bundle_lines_precede_user_ini() {
        let mut cfg = cfg_unix(ProcessManagerMode::OnDemand);
        cfg.ca_bundle = Some(PathBuf::from("/d/cacert.pem"));
        cfg.ini = vec![("memory_limit".to_string(), "512M".to_string())];
        let s = render_fpm_conf(&cfg);
        assert!(
            s.find("php_admin_value[openssl.cafile]").unwrap()
                < s.find("php_value[memory_limit]").unwrap()
        );
    }

    #[test]
    fn ca_bundle_absent_when_none() {
        let s = render_fpm_conf(&cfg_unix(ProcessManagerMode::OnDemand));
        assert!(!s.contains("openssl.cafile"), "got: {s}");
        assert!(!s.contains("curl.cainfo"), "got: {s}");
    }

    #[test]
    fn ca_bundle_with_control_char_in_path_is_skipped() {
        let mut cfg = cfg_unix(ProcessManagerMode::OnDemand);
        cfg.ca_bundle = Some(PathBuf::from("/d/ca\ncert.pem"));
        let s = render_fpm_conf(&cfg);
        assert!(!s.contains("openssl.cafile"), "injection not skipped: {s}");
        assert!(!s.contains("curl.cainfo"), "injection not skipped: {s}");
    }

    #[test]
    fn ca_bundle_path_with_spaces_is_unquoted() {
        let mut cfg = cfg_unix(ProcessManagerMode::OnDemand);
        cfg.ca_bundle = Some(PathBuf::from(
            "/Users/x/Library/Application Support/io.yerd.Yerd/cacert.pem",
        ));
        let s = render_fpm_conf(&cfg);
        assert!(
            s.contains(
                "php_admin_value[openssl.cafile] = /Users/x/Library/Application Support/io.yerd.Yerd/cacert.pem\n"
            ),
            "got: {s}"
        );
    }
}
