//! Pure `cloudflared` argument-vector generation.
//!
//! Each function returns the argument vector (excluding the `cloudflared`
//! program itself) for one invocation. The caller builds the actual
//! `std::process::Command` from the resolved binary path plus these args, so
//! this module performs no I/O and is trivially table-testable.

use std::ffi::OsString;
use std::path::Path;

use crate::origin::OriginTarget;

/// The shared prefix for every account-scoped `cloudflared` subcommand:
/// `--origincert <cert> tunnel`. Callers append their command-specific args.
fn origincert_tunnel_prefix(origincert: &Path) -> Vec<OsString> {
    vec![
        "--origincert".into(),
        origincert.as_os_str().to_os_string(),
        "tunnel".into(),
    ]
}

/// Args for a Quick Tunnel: `cloudflared tunnel --url <origin> ...`.
///
/// `cloudflared` logs (including the assigned `*.trycloudflare.com` URL banner)
/// go to stderr, which the supervisor captures by redirecting the child's
/// stderr to a file it tails, so no `--logfile` flag is passed (a second writer
/// to the same path would interleave). For a secure origin the SNI name and
/// `--no-tls-verify` are added (the loopback hop uses Yerd's private CA).
#[must_use]
pub fn quick_tunnel_args(origin: &OriginTarget) -> Vec<OsString> {
    let mut args: Vec<OsString> = vec![
        "tunnel".into(),
        "--no-autoupdate".into(),
        "--url".into(),
        origin.url().into(),
        "--http-host-header".into(),
        origin.host_header.clone().into(),
    ];
    push_origin_tls(&mut args, origin);
    args.push("--loglevel".into());
    args.push("info".into());
    args
}

/// Args to run a named tunnel from a local config:
/// `cloudflared --origincert <cert> --config <config> tunnel run`.
///
/// The config file carries the tunnel UUID, credentials-file path, and ingress
/// rules, so no tunnel name is needed on the command line.
#[must_use]
pub fn named_run_args(config_path: &Path, origincert: &Path) -> Vec<OsString> {
    vec![
        "--no-autoupdate".into(),
        "--origincert".into(),
        origincert.as_os_str().to_os_string(),
        "--config".into(),
        config_path.as_os_str().to_os_string(),
        "tunnel".into(),
        "run".into(),
    ]
}

/// Args for the interactive browser login:
/// `cloudflared --origincert <cert> tunnel login`.
///
/// `--origincert` pins where the account cert is written, keeping it out of
/// `~/.cloudflared`. The auth URL is printed to stderr; the supervisor parses it
/// and the GUI opens it.
#[must_use]
pub fn login_args(origincert: &Path) -> Vec<OsString> {
    let mut args = origincert_tunnel_prefix(origincert);
    args.push("login".into());
    args
}

/// Args to create a named tunnel writing its credentials to a chosen path:
/// `cloudflared --origincert <cert> tunnel create --credentials-file <file> <name>`.
#[must_use]
pub fn create_args(name: &str, origincert: &Path, credentials_file: &Path) -> Vec<OsString> {
    let mut args = origincert_tunnel_prefix(origincert);
    args.extend([
        "create".into(),
        "--credentials-file".into(),
        credentials_file.as_os_str().to_os_string(),
        name.into(),
    ]);
    args
}

/// Args to route a DNS hostname to a tunnel:
/// `cloudflared --origincert <cert> tunnel route dns --overwrite-dns <tunnel> <hostname>`.
///
/// `--overwrite-dns` repoints an existing CNAME rather than failing on it, so
/// re-exposing a hostname (e.g. after deleting and recreating the tunnel, which
/// leaves the old proxied record behind) succeeds instead of erroring on a
/// duplicate record.
#[must_use]
pub fn route_dns_args(tunnel: &str, hostname: &str, origincert: &Path) -> Vec<OsString> {
    let mut args = origincert_tunnel_prefix(origincert);
    args.extend([
        "route".into(),
        "dns".into(),
        "--overwrite-dns".into(),
        tunnel.into(),
        hostname.into(),
    ]);
    args
}

/// Args to clean up a tunnel's stale edge connections before deletion:
/// `cloudflared --origincert <cert> tunnel cleanup <tunnel>`.
///
/// Run before [`delete_args`] so `cloudflared tunnel delete` doesn't refuse a
/// tunnel that still shows active connections from a just-stopped process.
#[must_use]
pub fn cleanup_args(tunnel: &str, origincert: &Path) -> Vec<OsString> {
    let mut args = origincert_tunnel_prefix(origincert);
    args.extend(["cleanup".into(), tunnel.into()]);
    args
}

/// Args to delete a named tunnel from the account:
/// `cloudflared --origincert <cert> tunnel delete <tunnel>`.
#[must_use]
pub fn delete_args(tunnel: &str, origincert: &Path) -> Vec<OsString> {
    let mut args = origincert_tunnel_prefix(origincert);
    args.extend(["delete".into(), tunnel.into()]);
    args
}

/// Args to list the account's named tunnels as JSON:
/// `cloudflared --origincert <cert> tunnel list --output json`.
#[must_use]
pub fn list_args(origincert: &Path) -> Vec<OsString> {
    let mut args = origincert_tunnel_prefix(origincert);
    args.extend(["list".into(), "--output".into(), "json".into()]);
    args
}

/// Append the secure-origin TLS flags (`--origin-server-name`, `--no-tls-verify`)
/// when the origin is HTTPS; a no-op for a plain HTTP origin.
fn push_origin_tls(args: &mut Vec<OsString>, origin: &OriginTarget) {
    if let Some(sni) = origin.origin_server_name.as_ref() {
        args.push("--origin-server-name".into());
        args.push(sni.clone().into());
    }
    if origin.no_tls_verify {
        args.push("--no-tls-verify".into());
    }
}

#[cfg(test)]
#[allow(clippy::indexing_slicing)]
mod tests {
    use super::*;
    use crate::origin::OriginTarget;

    fn strings(args: &[OsString]) -> Vec<String> {
        args.iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn quick_secure_includes_host_rewrite_and_tls_flags() {
        let origin = OriginTarget::for_site("app", "test", true, 8080, 8443);
        let args = strings(&quick_tunnel_args(&origin));
        assert_eq!(args[0], "tunnel");
        assert!(args.iter().any(|a| a == "--no-autoupdate"));
        assert!(args.iter().any(|a| a == "--url"));
        assert!(args.contains(&"https://127.0.0.1:8443".to_string()));
        assert!(args.iter().any(|a| a == "--http-host-header"));
        assert!(args.contains(&"app.test".to_string()));
        assert!(args.iter().any(|a| a == "--origin-server-name"));
        assert!(args.iter().any(|a| a == "--no-tls-verify"));
    }

    #[test]
    fn quick_non_secure_omits_tls_flags() {
        let origin = OriginTarget::for_site("blog", "test", false, 8080, 8443);
        let args = strings(&quick_tunnel_args(&origin));
        assert!(args.contains(&"http://127.0.0.1:8080".to_string()));
        assert!(args.iter().any(|a| a == "--no-autoupdate"));
        assert!(!args.iter().any(|a| a == "--origin-server-name"));
        assert!(!args.iter().any(|a| a == "--no-tls-verify"));
        assert!(args.iter().any(|a| a == "--http-host-header"));
    }

    #[test]
    fn named_run_passes_config_and_cert_then_subcommand() {
        let args = strings(&named_run_args(
            Path::new("/t/app.yml"),
            Path::new("/t/cert.pem"),
        ));
        assert_eq!(
            args,
            vec![
                "--no-autoupdate",
                "--origincert",
                "/t/cert.pem",
                "--config",
                "/t/app.yml",
                "tunnel",
                "run",
            ]
        );
    }

    #[test]
    fn login_pins_origincert() {
        let args = strings(&login_args(Path::new("/t/cert.pem")));
        assert_eq!(args, vec!["--origincert", "/t/cert.pem", "tunnel", "login"]);
    }

    #[test]
    fn create_passes_credentials_file_and_name_last() {
        let args = strings(&create_args(
            "mysite",
            Path::new("/t/cert.pem"),
            Path::new("/t/creds/uuid.json"),
        ));
        assert_eq!(args[2], "tunnel");
        assert_eq!(args[3], "create");
        assert!(args.iter().any(|a| a == "--credentials-file"));
        assert_eq!(args.last().map(String::as_str), Some("mysite"));
    }

    #[test]
    fn route_dns_orders_tunnel_then_hostname() {
        let args = strings(&route_dns_args(
            "mysite",
            "app.example.com",
            Path::new("/t/cert.pem"),
        ));
        assert_eq!(
            args,
            vec![
                "--origincert",
                "/t/cert.pem",
                "tunnel",
                "route",
                "dns",
                "--overwrite-dns",
                "mysite",
                "app.example.com",
            ]
        );
    }

    #[test]
    fn list_requests_json_output() {
        let args = strings(&list_args(Path::new("/t/cert.pem")));
        assert!(args.iter().any(|a| a == "--output"));
        assert!(args.iter().any(|a| a == "json"));
    }

    #[test]
    fn cleanup_then_delete_target_the_named_tunnel() {
        let cert = Path::new("/t/cert.pem");
        assert_eq!(
            strings(&cleanup_args("mysite", cert)),
            vec!["--origincert", "/t/cert.pem", "tunnel", "cleanup", "mysite"]
        );
        assert_eq!(
            strings(&delete_args("mysite", cert)),
            vec!["--origincert", "/t/cert.pem", "tunnel", "delete", "mysite"]
        );
    }
}
