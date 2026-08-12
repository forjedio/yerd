//! Pure PHP CGI ini rendering for Windows.

use std::fmt::Write;
use std::path::Path;

use crate::pool::PoolConfig;

/// Render the Windows `php-cgi.exe` ini file.
#[must_use]
pub fn render(cfg: &PoolConfig, runtime_dir: &Path) -> String {
    let mut out = String::with_capacity(512);
    let _ = writeln!(out, "cgi.force_redirect=0");
    let _ = writeln!(out, "cgi.fix_pathinfo=1");
    let extension_dir = runtime_dir.join("ext").to_string_lossy().replace('\\', "/");
    let _ = writeln!(out, "extension_dir={extension_dir}");
    for extension in windows_extensions(cfg.version) {
        let _ = writeln!(out, "extension={extension}");
    }

    if let Some(path) = &cfg.ca_bundle {
        if let Some(path) = yerd_core::php_settings::sanitize_ca_bundle_path(path) {
            let _ = writeln!(out, "openssl.cafile={path}");
            let _ = writeln!(out, "curl.cainfo={path}");
        }
    }
    for (key, value) in &cfg.ini {
        if yerd_core::php_settings::directive(key).is_some()
            && yerd_core::php_settings::validate_value(key, value).is_ok()
        {
            let _ = writeln!(out, "{key}={value}");
        }
    }
    for (key, value) in &cfg.directives {
        if yerd_core::php_directives::validate_name(key).is_ok()
            && yerd_core::php_directives::validate_value(value).is_ok()
            && yerd_core::php_directives::reserved(key).is_none()
        {
            let _ = writeln!(out, "{key}={value}");
        }
    }
    out
}

/// Extensions bundled by official Windows PHP builds that local development expects.
#[must_use]
pub fn windows_extensions(version: yerd_core::PhpVersion) -> Vec<&'static str> {
    let mut extensions = vec![
        "php_curl.dll",
        "php_fileinfo.dll",
        "php_mbstring.dll",
        "php_mysqli.dll",
        "php_openssl.dll",
        "php_pdo_mysql.dll",
        "php_pdo_pgsql.dll",
        "php_pdo_sqlite.dll",
        "php_sodium.dll",
    ];
    if (version.major, version.minor) >= (8, 2) {
        extensions.push("php_zip.dll");
    }
    extensions
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use crate::listen::Listen;
    use crate::pool::PoolConfig;
    use std::net::{Ipv4Addr, SocketAddrV4};
    use std::path::PathBuf;
    use yerd_core::PhpVersion;
    use yerd_platform::PlatformDirs;

    #[test]
    fn renders_windows_cgi_settings() {
        let dirs = PlatformDirs {
            config: PathBuf::from(r"C:\Users\Dev\App Data\Yerd"),
            data: PathBuf::new(),
            state: PathBuf::new(),
            cache: PathBuf::new(),
            runtime: PathBuf::new(),
        };
        let mut cfg = PoolConfig::dev_defaults(
            PhpVersion::new(8, 4),
            Listen::TcpLoopback(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 9000).into()),
            &dirs,
            1,
        );
        cfg.ini.push(("memory_limit".into(), "512M".into()));
        cfg.ca_bundle = Some(PathBuf::from(r"C:\Users\Dev\App Data\ca.pem"));
        let rendered = render(&cfg, Path::new(r"C:\php-8.4"));
        assert!(rendered.contains("cgi.force_redirect=0"));
        assert!(rendered.contains("memory_limit=512M"));
        assert!(rendered.contains(r"openssl.cafile=C:\Users\Dev\App Data\ca.pem"));
        assert!(rendered.contains("extension=php_openssl.dll"));
        assert!(rendered.contains("extension=php_zip.dll"));
    }

    #[test]
    fn omits_zip_before_php_82() {
        assert!(!windows_extensions(PhpVersion::new(8, 1)).contains(&"php_zip.dll"));
        assert!(windows_extensions(PhpVersion::new(8, 2)).contains(&"php_zip.dll"));
    }
}
