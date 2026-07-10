//! Pure reverse-proxy domain types.
//!
//! [`UpstreamTarget`] is a validated `http[s]://host:port` forwarding target;
//! [`ProxyRule`] is a path-prefix rule attached to a PHP site (`/app` →
//! upstream); [`ProxySite`] is a whole-host proxy (`reverb.test` → upstream).
//!
//! These types are pure data: they carry no serde impls (the `yerd-config`
//! crate owns the string↔typed conversion on the wire) and do no I/O. The
//! forwarder in `yerd-proxy` consumes [`UpstreamTarget::is_local`] to pick a
//! client-TLS policy and [`UpstreamTarget::server_name`] to seed the upstream
//! TLS SNI.

use std::net::IpAddr;

use crate::error::{CoreError, ProxyRuleErrorReason, UpstreamTargetErrorReason};
use crate::site::validate_and_lowercase_name;

/// A validated reverse-proxy upstream: scheme (`http`/`https`), host, port.
///
/// Parsed from a URL string via [`Self::from_url_str`]. A path, query,
/// fragment, or credentials in the URL are rejected: the forwarder passes the
/// original request path verbatim, so a target path would be ambiguous.
///
/// IPv6 hosts are stored **without** brackets (so [`Self::server_name`] feeds
/// `rustls`'s `ServerName::try_from`, which accepts `::1` but not `[::1]`);
/// [`std::fmt::Display`] re-adds them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpstreamTarget {
    secure: bool,
    host: String,
    port: u16,
}

impl UpstreamTarget {
    /// Parses `http[s]://host[:port]`. Port defaults to 80 (http) / 443 (https).
    pub fn from_url_str(input: &str) -> Result<Self, CoreError> {
        let raw = input.trim();
        let reason = |r: UpstreamTargetErrorReason| CoreError::InvalidUpstreamTarget {
            input: input.to_owned(),
            reason: r,
        };
        if raw.is_empty() {
            return Err(reason(UpstreamTargetErrorReason::Empty));
        }
        let (scheme, rest) = raw
            .split_once("://")
            .ok_or_else(|| reason(UpstreamTargetErrorReason::MissingScheme))?;
        let secure = match scheme.to_ascii_lowercase().as_str() {
            "http" => false,
            "https" => true,
            _ => return Err(reason(UpstreamTargetErrorReason::UnsupportedScheme)),
        };
        if rest.contains('@') {
            return Err(reason(UpstreamTargetErrorReason::HasCredentials));
        }
        if rest.contains('/') || rest.contains('?') || rest.contains('#') {
            return Err(reason(UpstreamTargetErrorReason::HasPathOrQuery));
        }
        let default_port = if secure { 443 } else { 80 };
        let (host, port) = split_host_port(rest, default_port)
            .ok_or_else(|| reason(UpstreamTargetErrorReason::InvalidPort))?;
        if host.is_empty() {
            return Err(reason(UpstreamTargetErrorReason::MissingHost));
        }
        if !is_valid_host(host) {
            return Err(reason(UpstreamTargetErrorReason::InvalidHost));
        }
        if port == 0 {
            return Err(reason(UpstreamTargetErrorReason::InvalidPort));
        }
        Ok(Self {
            secure,
            host: host.to_ascii_lowercase(),
            port,
        })
    }

    /// Whether the upstream is reached over TLS (`https://`).
    #[must_use]
    pub fn secure(&self) -> bool {
        self.secure
    }

    /// The upstream host, unbracketed even for IPv6.
    #[must_use]
    pub fn host(&self) -> &str {
        &self.host
    }

    /// The upstream port.
    #[must_use]
    pub fn port(&self) -> u16 {
        self.port
    }

    /// The value to seed the upstream TLS SNI / `ServerName` with: the host,
    /// unbracketed. **Never** the client `Host` header.
    #[must_use]
    pub fn server_name(&self) -> &str {
        &self.host
    }

    /// Whether the host is loopback, a private/link-local IP, `localhost`, or a
    /// name under the given `tld`. The forwarder skips certificate verification
    /// for such upstreams (self-signed local dev backends are the norm) and
    /// verifies public hosts normally.
    #[must_use]
    pub fn is_local(&self, tld: &str) -> bool {
        if self.host == "localhost" {
            return true;
        }
        if self.host == tld || self.host.ends_with(&format!(".{tld}")) {
            return true;
        }
        match self.host.parse::<IpAddr>() {
            Ok(IpAddr::V4(v4)) => v4.is_loopback() || v4.is_private() || v4.is_link_local(),
            Ok(IpAddr::V6(v6)) => {
                if v6.is_loopback() {
                    return true;
                }
                let seg0 = v6.segments().first().copied().unwrap_or(0);
                let unique_local = (seg0 & 0xfe00) == 0xfc00;
                let link_local = (seg0 & 0xffc0) == 0xfe80;
                unique_local || link_local
            }
            Err(_) => false,
        }
    }
}

impl std::fmt::Display for UpstreamTarget {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let scheme = if self.secure { "https" } else { "http" };
        if self.host.contains(':') {
            write!(f, "{scheme}://[{}]:{}", self.host, self.port)
        } else {
            write!(f, "{scheme}://{}:{}", self.host, self.port)
        }
    }
}

/// Splits `host[:port]` (bracketed IPv6 allowed) into an unbracketed host and a
/// port, defaulting the port. Returns `None` when the port is malformed, or for
/// an unbracketed multi-colon host (an IPv6 literal that must be bracketed).
fn split_host_port(rest: &str, default_port: u16) -> Option<(&str, u16)> {
    if let Some(inner) = rest.strip_prefix('[') {
        let (host, after) = inner.split_once(']')?;
        let port = if after.is_empty() {
            default_port
        } else {
            after.strip_prefix(':')?.parse().ok()?
        };
        return Some((host, port));
    }
    match rest.matches(':').count() {
        0 => Some((rest, default_port)),
        1 => {
            let (host, p) = rest.rsplit_once(':')?;
            Some((host, p.parse().ok()?))
        }
        _ => None,
    }
}

/// A host is valid if it parses as an IP address, or is a plain hostname (ASCII
/// `[a-z0-9._-]`, non-empty, no leading/trailing dot). `_` is allowed for
/// container/service names (e.g. `my_service` from a docker-compose network),
/// which aren't strict DNS but resolve fine on a local dev network.
fn is_valid_host(host: &str) -> bool {
    if host.parse::<IpAddr>().is_ok() {
        return true;
    }
    if host.is_empty() || host.starts_with('.') || host.ends_with('.') {
        return false;
    }
    host.bytes()
        .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'.' || b == b'_')
}

/// A path-prefix reverse-proxy rule attached to an existing PHP site: requests
/// whose path is under [`Self::prefix`] are forwarded to [`Self::target`]; every
/// other path is served by PHP as usual.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProxyRule {
    prefix: String,
    target: UpstreamTarget,
}

impl ProxyRule {
    /// Validates `prefix` (absolute; no `..` component; no control chars, space,
    /// `?`, or `#` - none of which can appear in `uri.path()`, so such a prefix
    /// would be a silently dead rule) and normalizes a trailing slash away
    /// (`/app/` → `/app`; root stays `/`).
    pub fn new(prefix: &str, target: UpstreamTarget) -> Result<Self, CoreError> {
        let reason = |r: ProxyRuleErrorReason| CoreError::InvalidProxyRule {
            input: prefix.to_owned(),
            reason: r,
        };
        if prefix.is_empty() {
            return Err(reason(ProxyRuleErrorReason::Empty));
        }
        if !prefix.starts_with('/') {
            return Err(reason(ProxyRuleErrorReason::NotAbsolute));
        }
        if prefix
            .chars()
            .any(|c| c.is_control() || c == ' ' || c == '?' || c == '#')
        {
            return Err(reason(ProxyRuleErrorReason::ContainsControl));
        }
        if prefix.split('/').any(|seg| seg == "..") {
            return Err(reason(ProxyRuleErrorReason::ContainsDotDot));
        }
        let mut normalized = prefix.to_owned();
        while normalized.len() > 1 && normalized.ends_with('/') {
            normalized.pop();
        }
        Ok(Self {
            prefix: normalized,
            target,
        })
    }

    /// The normalized path prefix (e.g. `/app`; root is `/`).
    #[must_use]
    pub fn prefix(&self) -> &str {
        &self.prefix
    }

    /// The upstream this rule forwards to.
    #[must_use]
    pub fn target(&self) -> &UpstreamTarget {
        &self.target
    }

    /// Whether this rule matches `path`. Boundary-correct: `/app` matches `/app`
    /// and `/app/x` but not `/apple`. Root (`/`) is a catch-all.
    #[must_use]
    pub fn matches_path(&self, path: &str) -> bool {
        if self.prefix == "/" {
            return path.starts_with('/');
        }
        path == self.prefix || path.starts_with(&format!("{}/", self.prefix))
    }
}

/// Longest-prefix match of `path` against `rules`. Returns the rule with the
/// longest matching prefix, or `None` if none match. Callers pass the raw,
/// case-sensitive, percent-encoded `uri.path()` (no normalization) — an
/// under-match (an encoded path not matching) is acceptable for a dev tool; the
/// matcher never over-matches.
#[must_use]
pub fn match_rule<'a>(rules: &'a [ProxyRule], path: &str) -> Option<&'a ProxyRule> {
    rules
        .iter()
        .filter(|r| r.matches_path(path))
        .max_by_key(|r| r.prefix.len())
}

/// A whole-host reverse proxy: a `{name}.{tld}` host forwarded wholesale to
/// [`Self::target`], with no PHP/document-root. The router routes it alongside
/// PHP sites; the daemon forwards it without ever entering PHP-FPM resolution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProxySite {
    name: String,
    target: UpstreamTarget,
    secure: bool,
}

impl ProxySite {
    /// Constructs a proxy site. Validates and lowercases `name` as a DNS label.
    /// Initialises `secure = false` (promote via [`Self::set_secure`]).
    pub fn new(name: &str, target: UpstreamTarget) -> Result<Self, CoreError> {
        let name = validate_and_lowercase_name(name)?;
        Ok(Self {
            name,
            target,
            secure: false,
        })
    }

    /// The validated, lowercased DNS-label name.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The upstream this proxy forwards to.
    #[must_use]
    pub fn target(&self) -> &UpstreamTarget {
        &self.target
    }

    /// Whether the proxy is served over HTTPS.
    #[must_use]
    pub fn secure(&self) -> bool {
        self.secure
    }

    /// Toggles the HTTPS flag.
    pub fn set_secure(&mut self, secure: bool) {
        self.secure = secure;
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

    fn target(url: &str) -> UpstreamTarget {
        UpstreamTarget::from_url_str(url).unwrap()
    }

    #[test]
    fn parses_http_default_port() {
        let t = target("http://localhost");
        assert!(!t.secure());
        assert_eq!(t.host(), "localhost");
        assert_eq!(t.port(), 80);
    }

    #[test]
    fn parses_https_default_port() {
        let t = target("https://api.example.com");
        assert!(t.secure());
        assert_eq!(t.port(), 443);
    }

    #[test]
    fn parses_explicit_port() {
        let t = target("http://127.0.0.1:8080");
        assert_eq!(t.host(), "127.0.0.1");
        assert_eq!(t.port(), 8080);
    }

    #[test]
    fn parses_bracketed_ipv6() {
        let t = target("http://[::1]:8080");
        assert_eq!(t.host(), "::1");
        assert_eq!(t.port(), 8080);
        assert_eq!(t.server_name(), "::1");
        assert_eq!(t.to_string(), "http://[::1]:8080");
    }

    #[test]
    fn parses_bracketed_ipv6_default_port() {
        let t = target("https://[fe80::1]");
        assert_eq!(t.host(), "fe80::1");
        assert_eq!(t.port(), 443);
    }

    #[test]
    fn display_round_trips_through_parse() {
        for url in [
            "http://localhost:80",
            "https://example.com:443",
            "http://[::1]:9000",
        ] {
            let t = target(url);
            let reparsed = UpstreamTarget::from_url_str(&t.to_string()).unwrap();
            assert_eq!(t, reparsed);
        }
    }

    #[test]
    fn rejects_bad_targets() {
        for bad in [
            "",
            "localhost:8080",
            "ftp://localhost",
            "http://user:pass@host",
            "http://host/path",
            "http://host?q=1",
            "http://host:0",
            "http://host:99999",
            "http://host:",
            "http://::1:8080",
            "http://",
        ] {
            assert!(
                UpstreamTarget::from_url_str(bad).is_err(),
                "expected error for {bad:?}"
            );
        }
    }

    #[test]
    fn is_local_classifies_hosts() {
        assert!(target("http://localhost").is_local("test"));
        assert!(target("http://127.0.0.1").is_local("test"));
        assert!(target("http://10.1.2.3").is_local("test"));
        assert!(target("http://192.168.1.1").is_local("test"));
        assert!(target("http://169.254.1.1").is_local("test"));
        assert!(target("http://[::1]").is_local("test"));
        assert!(target("http://[fc00::1]").is_local("test"));
        assert!(target("http://[fe80::1]").is_local("test"));
        assert!(target("http://myapp.test").is_local("test"));
        assert!(!target("http://api.example.com").is_local("test"));
        assert!(!target("http://8.8.8.8").is_local("test"));
        assert!(!target("http://[2606:4700::1]").is_local("test"));
    }

    #[test]
    fn rule_normalizes_and_matches() {
        let r = ProxyRule::new("/app/", target("http://127.0.0.1:8080")).unwrap();
        assert_eq!(r.prefix(), "/app");
        assert!(r.matches_path("/app"));
        assert!(r.matches_path("/app/foo"));
        assert!(!r.matches_path("/apple"));
        assert!(!r.matches_path("/"));
    }

    #[test]
    fn root_rule_is_catch_all() {
        let r = ProxyRule::new("/", target("http://127.0.0.1:8080")).unwrap();
        assert_eq!(r.prefix(), "/");
        assert!(r.matches_path("/"));
        assert!(r.matches_path("/anything"));
    }

    #[test]
    fn rejects_bad_prefixes() {
        for bad in ["", "app", "/a/../b", "/x\ty"] {
            assert!(
                ProxyRule::new(bad, target("http://127.0.0.1:8080")).is_err(),
                "expected error for {bad:?}"
            );
        }
    }

    #[test]
    fn match_rule_picks_longest_prefix() {
        let rules = vec![
            ProxyRule::new("/app", target("http://127.0.0.1:8080")).unwrap(),
            ProxyRule::new("/app/admin", target("http://127.0.0.1:9090")).unwrap(),
        ];
        let hit = match_rule(&rules, "/app/admin/x").unwrap();
        assert_eq!(hit.prefix(), "/app/admin");
        let hit = match_rule(&rules, "/app/other").unwrap();
        assert_eq!(hit.prefix(), "/app");
        assert!(match_rule(&rules, "/nope").is_none());
    }

    #[test]
    fn proxysite_validates_name() {
        let p = ProxySite::new("Reverb", target("http://localhost:8080")).unwrap();
        assert_eq!(p.name(), "reverb");
        assert!(!p.secure());
        assert!(ProxySite::new("bad.name", target("http://localhost:8080")).is_err());
    }

    #[test]
    fn proxysite_secure_toggles() {
        let mut p = ProxySite::new("reverb", target("http://localhost:8080")).unwrap();
        p.set_secure(true);
        assert!(p.secure());
    }
}
