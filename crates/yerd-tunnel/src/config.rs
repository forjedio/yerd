//! Pure `cloudflared` `config.yml` rendering for the consolidated named tunnel.
//!
//! Hand-rendered as a string (rather than via a YAML serializer) so it stays a
//! trivially-testable pure function with no extra dependency. The ingress list
//! maps each enabled site's public hostname to its local origin via the same
//! Host-header rewrite used by Quick Tunnels. A single tunnel serves every
//! enabled site (one process, one config) and the list must end in the mandatory
//! catch-all `service: http_status:404` or `cloudflared` refuses to start.

use std::fmt::Write as _;
use std::path::Path;

use crate::origin::OriginTarget;

/// One ingress mapping: a public hostname to a local site origin.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IngressRule {
    /// The public hostname (on the user's Cloudflare domain).
    pub hostname: String,
    /// Where and how `cloudflared` reaches the site locally.
    pub origin: OriginTarget,
}

/// Render the ingress config for a named tunnel that serves every rule in
/// `rules` (one rule per enabled site), ending in the mandatory catch-all.
#[must_use]
pub fn render_ingress_config(
    tunnel_uuid: &str,
    credentials_file: &Path,
    rules: &[IngressRule],
) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "tunnel: {}", yaml_squote(tunnel_uuid));
    let _ = writeln!(
        out,
        "credentials-file: {}",
        yaml_squote(&credentials_file.display().to_string())
    );
    out.push_str("ingress:\n");
    for rule in rules {
        let _ = writeln!(out, "  - hostname: {}", yaml_squote(&rule.hostname));
        let _ = writeln!(out, "    service: {}", yaml_squote(&rule.origin.url()));
        out.push_str("    originRequest:\n");
        let _ = writeln!(
            out,
            "      httpHostHeader: {}",
            yaml_squote(&rule.origin.host_header)
        );
        if let Some(sni) = rule.origin.origin_server_name.as_ref() {
            let _ = writeln!(out, "      originServerName: {}", yaml_squote(sni));
        }
        if rule.origin.no_tls_verify {
            out.push_str("      noTLSVerify: true\n");
        }
    }
    out.push_str("  - service: http_status:404\n");
    out
}

/// Render a string as a YAML single-quoted scalar, doubling any embedded single
/// quote per the YAML spec. The values here are upstream-validated hostnames and
/// loopback URLs, but quoting keeps a stray character from ever breaking out of
/// its line into a forged ingress key.
fn yaml_squote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(name: &str, tld: &str, secure: bool, hostname: &str) -> IngressRule {
        IngressRule {
            hostname: hostname.to_owned(),
            origin: OriginTarget::for_site(&format!("{name}.{tld}"), secure, 8080, 8443),
        }
    }

    #[test]
    fn secure_rule_has_host_rewrite_sni_and_catch_all() {
        let yaml = render_ingress_config(
            "uuid-123",
            Path::new("/t/creds/uuid-123.json"),
            &[rule("app", "test", true, "app.example.com")],
        );
        assert!(yaml.contains("tunnel: 'uuid-123'"));
        assert!(yaml.contains("credentials-file: '/t/creds/uuid-123.json'"));
        assert!(yaml.contains("- hostname: 'app.example.com'"));
        assert!(yaml.contains("service: 'https://127.0.0.1:8443'"));
        assert!(yaml.contains("httpHostHeader: 'app.test'"));
        assert!(yaml.contains("originServerName: 'app.test'"));
        assert!(yaml.contains("noTLSVerify: true"));
        assert!(yaml.trim_end().ends_with("- service: http_status:404"));
    }

    #[test]
    fn non_secure_rule_omits_tls_knobs() {
        let yaml = render_ingress_config(
            "uuid-9",
            Path::new("/t/creds/uuid-9.json"),
            &[rule("blog", "test", false, "blog.example.com")],
        );
        assert!(yaml.contains("service: 'http://127.0.0.1:8080'"));
        assert!(yaml.contains("httpHostHeader: 'blog.test'"));
        assert!(!yaml.contains("originServerName"));
        assert!(!yaml.contains("noTLSVerify"));
    }

    #[test]
    fn multiple_rules_each_get_an_ingress_block() {
        let yaml = render_ingress_config(
            "uuid-1",
            Path::new("/t/creds/uuid-1.json"),
            &[
                rule("app", "test", true, "app.example.com"),
                rule("blog", "test", false, "blog.example.com"),
            ],
        );
        assert!(yaml.contains("- hostname: 'app.example.com'"));
        assert!(yaml.contains("- hostname: 'blog.example.com'"));
        assert_eq!(yaml.matches("http_status:404").count(), 1);
        assert!(yaml.trim_end().ends_with("- service: http_status:404"));
    }

    #[test]
    fn empty_rules_render_only_the_catch_all() {
        let yaml = render_ingress_config("u", Path::new("/c.json"), &[]);
        assert!(yaml.contains("ingress:\n  - service: http_status:404"));
    }
}
