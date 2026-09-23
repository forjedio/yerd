//! The URL a browser reaches a site on, for the URLs the daemon writes into
//! projects and job logs. Mirrors the GUI's `siteUrl`: the bound port is
//! included only on a rootless fallback (8080/8443) with no 80/443 redirect in
//! front of it.

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
    format_url(host, secure, url_port(bound, default, redirected == Some(true)))
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
