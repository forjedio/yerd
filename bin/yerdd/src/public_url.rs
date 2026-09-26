//! Site URLs for what the daemon writes into projects and job logs, mirroring
//! the GUI's `siteUrl`: the bound port is included only on a rootless fallback
//! (8080/8443) with no 80/443 redirect in front of it.
//!
//! [`site_url`] is the site's own address, for values baked into a project
//! (`CodeIgniter`'s `app.baseURL`). [`browser_url`] is what to open right now:
//! when `.test` names don't resolve (resolver not installed, or the DNS
//! responder failed to bind) it is the `http://localhost/~{host}` switch URL
//! instead. That switch URL pins a cookie and 303-redirects to the path after
//! it, so it must never be used as a base URL: a form POST through it would
//! arrive as a GET.

use crate::state::DaemonState;

/// `scheme://host[:port]` (no trailing slash) for `host` served with `secure`,
/// from the daemon's bound ports and the live port-redirect state.
pub async fn site_url(state: &DaemonState, host: &str, secure: bool) -> String {
    let (bound, default) = if secure {
        (state.https.bound, 443)
    } else {
        (state.http.bound, 80)
    };
    let redirected = tokio::task::spawn_blocking(|| {
        use yerd_platform::PortRedirector;
        yerd_platform::ActivePortRedirector::new().is_active()
    })
    .await
    .ok()
    .flatten();
    format_url(
        host,
        secure,
        url_port(bound, default, redirected == Some(true)),
    )
}

/// The URL to show a user for opening `host` now: [`site_url`], or the
/// localhost switch URL while `.test` names don't resolve.
pub async fn browser_url(state: &DaemonState, host: &str, secure: bool) -> String {
    let tld = state.router.read().await.config().tld().to_owned();
    let dns_addr = state.dns_addr;
    let resolver_installed = tokio::task::spawn_blocking(move || {
        use yerd_platform::ResolverInstaller;
        yerd_platform::ActiveResolverInstaller::new()
            .is_installed(&tld, dns_addr)
            .ok()
    })
    .await
    .ok()
    .flatten();
    if is_unbound(resolver_installed, state.dns_unbound.is_some()) {
        return unbound_url(host, state.http.bound);
    }
    site_url(state, host, secure).await
}

/// The GUI's `isUnbound`: only a resolver known to be installed counts as on,
/// and a DNS responder that failed to bind means names won't resolve anyway.
fn is_unbound(resolver_installed: Option<bool>, dns_unbound: bool) -> bool {
    resolver_installed != Some(true) || dns_unbound
}

/// The GUI's `unboundUrlFor`: always plain http (there is no localhost cert),
/// an unbound (`0`) port falls back to 8080, and port 80 is omitted.
fn unbound_url(host: &str, http_bound: u16) -> String {
    let port = if http_bound == 0 { 8080 } else { http_bound };
    let port = if port == 80 {
        String::new()
    } else {
        format!(":{port}")
    };
    format!("http://localhost{port}/~{host}")
}

/// No port when redirected, unbound (`0`) or the scheme default. Pure.
fn url_port(bound: u16, default: u16, redirected: bool) -> Option<u16> {
    (!redirected && bound != 0 && bound != default).then_some(bound)
}

fn format_url(host: &str, secure: bool, port: Option<u16>) -> String {
    let scheme = if secure { "https" } else { "http" };
    let port = port.map(|p| format!(":{p}")).unwrap_or_default();
    format!("{scheme}://{host}{port}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn url_port_only_for_unredirected_non_default_ports() {
        for (bound, default, redirected, expected) in [
            (443, 443, false, None),
            (80, 80, false, None),
            (8443, 443, false, Some(8443)),
            (8080, 80, false, Some(8080)),
            (8443, 443, true, None),
            (0, 443, false, None),
        ] {
            assert_eq!(
                url_port(bound, default, redirected),
                expected,
                "{bound} {default} {redirected}"
            );
        }
    }

    #[test]
    fn is_unbound_unless_the_resolver_is_known_installed_and_dns_bound() {
        for (installed, dns_unbound, expected) in [
            (Some(true), false, false),
            (Some(true), true, true),
            (Some(false), false, true),
            (None, false, true),
        ] {
            assert_eq!(
                is_unbound(installed, dns_unbound),
                expected,
                "{installed:?} {dns_unbound}"
            );
        }
    }

    #[test]
    fn unbound_url_is_plain_http_on_localhost() {
        for (host, bound, expected) in [
            ("blog.test", 80, "http://localhost/~blog.test"),
            ("blog.test", 8080, "http://localhost:8080/~blog.test"),
            ("blog.test", 0, "http://localhost:8080/~blog.test"),
            (
                "corp.dev.local",
                8081,
                "http://localhost:8081/~corp.dev.local",
            ),
        ] {
            assert_eq!(unbound_url(host, bound), expected, "{host} {bound}");
        }
    }

    #[test]
    fn format_url_uses_scheme_host_and_port() {
        for (host, secure, port, expected) in [
            ("blog.test", true, None, "https://blog.test"),
            ("blog.test", false, None, "http://blog.test"),
            ("blog.dev.local", true, None, "https://blog.dev.local"),
            ("blog.test", true, Some(8443), "https://blog.test:8443"),
            ("blog.test", false, Some(8080), "http://blog.test:8080"),
        ] {
            assert_eq!(format_url(host, secure, port), expected);
        }
    }
}
