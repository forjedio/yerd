//! Pure `cloudflared` config rendering.
//!
//! Hand-rendered as strings (rather than via a YAML serializer) so they stay
//! trivially-testable pure functions with no extra dependency.
//!
//! The named tunnel's ingress is a single catch-all rule pointing at Yerd's
//! tunnel listener, which routes by the public `Host` itself. Exposing or
//! unexposing a site therefore only changes DNS and the daemon's route map,
//! never this file, so the running `cloudflared` never needs a restart. A
//! catch-all rule with no `hostname` is itself the mandatory last rule, so no
//! `http_status:404` entry is needed.

use std::fmt::Write as _;
use std::path::Path;

/// Render the config for the named tunnel `tunnel_uuid`, authenticating with
/// `credentials_file`, whose only ingress rule sends everything to `origin`
/// (the tunnel listener URL, see [`crate::origin::origin_url`]).
#[must_use]
pub fn render_ingress_config(tunnel_uuid: &str, credentials_file: &Path, origin: &str) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "tunnel: {}", yaml_squote(tunnel_uuid));
    let _ = writeln!(
        out,
        "credentials-file: {}",
        yaml_squote(&credentials_file.display().to_string())
    );
    out.push_str("ingress:\n");
    let _ = writeln!(out, "  - service: {}", yaml_squote(origin));
    out
}

/// The Quick Tunnel config passed with `--config`: valid and ingress-free, so
/// `--url` applies and `cloudflared` never falls back to a default search path
/// such as `~/.cloudflared` or `/etc/cloudflared`.
#[must_use]
pub fn render_quick_config() -> &'static str {
    "no-autoupdate: true\n"
}

/// Render a string as a YAML single-quoted scalar, doubling any embedded single
/// quote per the YAML spec. The values here are a UUID, a daemon-owned path, and
/// a loopback URL, but quoting keeps a stray character from ever breaking out of
/// its line into a forged key.
fn yaml_squote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn named_config_is_a_single_catch_all_to_the_listener() {
        let yaml = render_ingress_config(
            "uuid-123",
            Path::new("/t/creds/uuid-123.json"),
            "http://127.0.0.1:49152",
        );
        assert_eq!(
            yaml,
            "tunnel: 'uuid-123'\n\
             credentials-file: '/t/creds/uuid-123.json'\n\
             ingress:\n  - service: 'http://127.0.0.1:49152'\n"
        );
    }

    #[test]
    fn named_config_has_no_per_host_or_tls_options() {
        let yaml = render_ingress_config("u", Path::new("/c.json"), "http://127.0.0.1:1");
        assert_eq!(yaml.matches("service:").count(), 1);
        for absent in [
            "hostname:",
            "httpHostHeader",
            "noTLSVerify",
            "originServerName",
            "originRequest",
            "http_status:404",
        ] {
            assert!(!yaml.contains(absent), "{absent}");
        }
    }

    #[test]
    fn values_are_single_quoted_with_quotes_doubled() {
        let yaml = render_ingress_config("a'b", Path::new("/it's/c.json"), "http://127.0.0.1:2");
        assert!(yaml.contains("tunnel: 'a''b'"));
        assert!(yaml.contains("credentials-file: '/it''s/c.json'"));
    }

    #[test]
    fn quick_config_is_ingress_free() {
        assert_eq!(render_quick_config(), "no-autoupdate: true\n");
        assert!(!render_quick_config().contains("ingress"));
    }
}
