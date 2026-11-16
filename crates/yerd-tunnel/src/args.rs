//! Pure `cloudflared` argument-vector generation.
//!
//! Each function returns the argument vector (excluding the `cloudflared`
//! program itself) for one invocation. The caller builds the actual
//! `std::process::Command` from the resolved binary path plus these args, so
//! this module performs no I/O and is trivially table-testable.

use std::ffi::OsString;
use std::path::Path;

/// The shared prefix for every account-scoped `cloudflared` subcommand:
/// `--origincert <cert> tunnel`. Callers append their command-specific args.
fn origincert_tunnel_prefix(origincert: &Path) -> Vec<OsString> {
    vec![
        "--origincert".into(),
        origincert.as_os_str().to_os_string(),
        "tunnel".into(),
    ]
}

/// Args for a Quick Tunnel:
/// `cloudflared tunnel --no-autoupdate --config <quick.yml> --url <origin> --loglevel info`.
///
/// `origin` is the tunnel listener URL (see [`crate::origin::origin_url`]); the
/// listener routes by the public `Host`, so no `--http-host-header` or TLS flags
/// are needed. The explicit `--config` points at Yerd's own ingress-free config
/// (see [`crate::config::render_quick_config`]) so a user or stray
/// `~/.cloudflared/config.yml` or `/etc/cloudflared/config.yml` is never picked
/// up. The metrics address is not in argv: the manager sets `TUNNEL_METRICS` in
/// the child env on every spawn. `cloudflared` logs (including the assigned
/// `*.trycloudflare.com` URL banner) go to stderr, which the supervisor captures
/// by redirecting the child's stderr to a file it tails, so no `--logfile` flag
/// is passed (a second writer to the same path would interleave).
#[must_use]
pub fn quick_tunnel_args(origin: &str, config_path: &Path) -> Vec<OsString> {
    vec![
        "tunnel".into(),
        "--no-autoupdate".into(),
        "--config".into(),
        config_path.as_os_str().to_os_string(),
        "--url".into(),
        origin.into(),
        "--loglevel".into(),
        "info".into(),
    ]
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

#[cfg(test)]
#[allow(clippy::indexing_slicing)]
mod tests {
    use super::*;

    fn strings(args: &[OsString]) -> Vec<String> {
        args.iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn quick_points_at_the_tunnel_listener_with_an_explicit_config() {
        let args = strings(&quick_tunnel_args(
            "http://127.0.0.1:49152",
            Path::new("/d/tunnel/quick.yml"),
        ));
        assert_eq!(
            args,
            vec![
                "tunnel",
                "--no-autoupdate",
                "--config",
                "/d/tunnel/quick.yml",
                "--url",
                "http://127.0.0.1:49152",
                "--loglevel",
                "info",
            ]
        );
    }

    #[test]
    fn quick_has_no_host_rewrite_tls_or_metrics_flags() {
        let args = strings(&quick_tunnel_args(
            "http://127.0.0.1:1",
            Path::new("/q.yml"),
        ));
        for flag in [
            "--http-host-header",
            "--origin-server-name",
            "--no-tls-verify",
            "--metrics",
        ] {
            assert!(!args.iter().any(|a| a == flag), "{flag}");
        }
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
