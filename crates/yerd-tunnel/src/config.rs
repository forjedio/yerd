//! Pure `cloudflared` `config.yml` rendering for named tunnels.
//!
//! Hand-rendered as a string (rather than via a YAML serializer) so it stays a
//! trivially-testable pure function with no extra dependency. The ingress list
//! maps one public hostname to one local site via the same Host-header rewrite
//! used by Quick Tunnels, and must end in the mandatory catch-all
//! `service: http_status:404` or `cloudflared` refuses to start.

use std::fmt::Write as _;
use std::path::Path;

use crate::origin::OriginTarget;

/// Render the single-site ingress config for a named tunnel.
#[must_use]
pub fn render_ingress_config(
    tunnel_uuid: &str,
    credentials_file: &Path,
    hostname: &str,
    origin: &OriginTarget,
) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "tunnel: {tunnel_uuid}");
    let _ = writeln!(out, "credentials-file: {}", credentials_file.display());
    out.push_str("ingress:\n");
    let _ = writeln!(out, "  - hostname: {hostname}");
    let _ = writeln!(out, "    service: {}", origin.url());
    out.push_str("    originRequest:\n");
    let _ = writeln!(out, "      httpHostHeader: {}", origin.host_header);
    if let Some(sni) = origin.origin_server_name.as_ref() {
        let _ = writeln!(out, "      originServerName: {sni}");
    }
    if origin.no_tls_verify {
        out.push_str("      noTLSVerify: true\n");
    }
    out.push_str("  - service: http_status:404\n");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secure_config_has_host_rewrite_sni_and_catch_all() {
        let origin = OriginTarget::for_site("app", "test", true, 8080, 8443);
        let yaml = render_ingress_config(
            "uuid-123",
            Path::new("/t/creds/uuid-123.json"),
            "app.example.com",
            &origin,
        );
        assert!(yaml.contains("tunnel: uuid-123"));
        assert!(yaml.contains("credentials-file: /t/creds/uuid-123.json"));
        assert!(yaml.contains("- hostname: app.example.com"));
        assert!(yaml.contains("service: https://127.0.0.1:8443"));
        assert!(yaml.contains("httpHostHeader: app.test"));
        assert!(yaml.contains("originServerName: app.test"));
        assert!(yaml.contains("noTLSVerify: true"));
        assert!(yaml.trim_end().ends_with("- service: http_status:404"));
    }

    #[test]
    fn non_secure_config_omits_tls_knobs() {
        let origin = OriginTarget::for_site("blog", "test", false, 8080, 8443);
        let yaml = render_ingress_config(
            "uuid-9",
            Path::new("/t/creds/uuid-9.json"),
            "blog.example.com",
            &origin,
        );
        assert!(yaml.contains("service: http://127.0.0.1:8080"));
        assert!(yaml.contains("httpHostHeader: blog.test"));
        assert!(!yaml.contains("originServerName"));
        assert!(!yaml.contains("noTLSVerify"));
        assert!(yaml.contains("- service: http_status:404"));
    }
}
