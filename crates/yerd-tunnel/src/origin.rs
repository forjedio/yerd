//! The origin a tunnel forwards to: Yerd's own loopback proxy listener.
//!
//! Yerd's proxy routes purely by the HTTP `Host` header matching the configured
//! TLD (e.g. `app.test`). A public tunnel request arrives with the *public*
//! hostname, which the proxy would 404. To route correctly without any
//! proxy-side host-aliasing, `cloudflared` is told to rewrite the `Host` header
//! to the site's canonical `{name}.{tld}` and connect to the loopback listener.
//!
//! For a secure site we target the HTTPS listener (the HTTP listener would issue
//! a 301 to HTTPS, useless to a public client) and skip public-trust validation
//! on the loopback hop, since Yerd serves a locally-trusted (private CA) cert.

/// Which proxy listener a tunnel forwards to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scheme {
    /// Plain HTTP listener (non-secure sites).
    Http,
    /// TLS listener (secure sites).
    Https,
}

impl Scheme {
    /// The URL scheme token.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Scheme::Http => "http",
            Scheme::Https => "https",
        }
    }
}

/// Where and how `cloudflared` should reach a single local site.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OriginTarget {
    /// Listener scheme (HTTP for non-secure sites, HTTPS for secure).
    pub scheme: Scheme,
    /// Bound loopback port of that listener.
    pub port: u16,
    /// Canonical `{name}.{tld}` to send as the rewritten `Host` header so the
    /// proxy routes to this site.
    pub host_header: String,
    /// SNI / expected origin cert name; `Some` only for a secure (HTTPS) origin.
    pub origin_server_name: Option<String>,
    /// Whether to skip origin TLS verification. `true` only for a secure origin,
    /// because Yerd's per-site cert is signed by a private CA the system trust
    /// store does not know; the hop is loopback so this is safe.
    pub no_tls_verify: bool,
}

impl OriginTarget {
    /// Build the origin for `name`.`tld`, choosing the listener by `secure`.
    ///
    /// `http_bound` / `https_bound` are the *actually bound* proxy ports (which
    /// may be the unprivileged fallbacks 8080/8443).
    #[must_use]
    #[allow(clippy::similar_names)]
    pub fn for_site(
        name: &str,
        tld: &str,
        secure: bool,
        http_bound: u16,
        https_bound: u16,
    ) -> Self {
        let host_header = format!("{name}.{tld}");
        if secure {
            Self {
                scheme: Scheme::Https,
                port: https_bound,
                origin_server_name: Some(host_header.clone()),
                no_tls_verify: true,
                host_header,
            }
        } else {
            Self {
                scheme: Scheme::Http,
                port: http_bound,
                origin_server_name: None,
                no_tls_verify: false,
                host_header,
            }
        }
    }

    /// The `service:` URL `cloudflared` connects to, e.g.
    /// `https://127.0.0.1:8443`.
    #[must_use]
    pub fn url(&self) -> String {
        format!("{}://127.0.0.1:{}", self.scheme.as_str(), self.port)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secure_site_targets_https_listener_with_sni_and_no_verify() {
        let o = OriginTarget::for_site("app", "test", true, 8080, 8443);
        assert_eq!(o.scheme, Scheme::Https);
        assert_eq!(o.port, 8443);
        assert_eq!(o.url(), "https://127.0.0.1:8443");
        assert_eq!(o.host_header, "app.test");
        assert_eq!(o.origin_server_name.as_deref(), Some("app.test"));
        assert!(o.no_tls_verify);
    }

    #[test]
    fn non_secure_site_targets_http_listener_plainly() {
        let o = OriginTarget::for_site("blog", "test", false, 8080, 8443);
        assert_eq!(o.scheme, Scheme::Http);
        assert_eq!(o.port, 8080);
        assert_eq!(o.url(), "http://127.0.0.1:8080");
        assert_eq!(o.host_header, "blog.test");
        assert_eq!(o.origin_server_name, None);
        assert!(!o.no_tls_verify);
    }

    #[test]
    fn honours_custom_tld_and_bound_ports() {
        let o = OriginTarget::for_site("api", "local", true, 80, 443);
        assert_eq!(o.url(), "https://127.0.0.1:443");
        assert_eq!(o.host_header, "api.local");
    }
}
