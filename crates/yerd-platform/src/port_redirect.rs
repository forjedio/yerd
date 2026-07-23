//! `PortRedirector` trait - probe whether the privileged-port redirect is live.
//!
//! On macOS the unprivileged daemon can't bind 80/443, so `yerd elevate ports`
//! installs a pf `rdr` redirect to its rootless ports (see
//! `crate::pure::pf_anchor`). Because the daemon still *binds* the high ports,
//! `StatusReport.http.fell_back` stays `true` even once the redirect works - so
//! the doctor needs a separate signal to know 80/443 are actually reachable.
//!
//! This probe is **active and unprivileged**, and it confirms the redirect
//! reaches **this proxy** rather than merely confirming *something* answers:
//! a bare TCP connect to `127.0.0.1:80` succeeds for any listener (a foreign
//! web server, or a stale `pf` rule the user thinks they removed), so it would
//! report a redirect as live after it had actually been torn down. Instead we
//! speak HTTP to the port and require the yerd proxy's `Server:` marker
//! ([`yerd_core::PROXY_SERVER_ID`]) on the response - reading pf's own state
//! would need root, which the daemon doesn't have.

use std::io::{Read as _, Write as _};
use std::net::{Ipv4Addr, SocketAddr, TcpStream};
use std::time::Duration;

/// Bound on each connect/read/write probe so status assembly never stalls.
const PROBE_TIMEOUT: Duration = Duration::from_millis(250);

/// A minimal HTTP/1.0 request with no `Host` header. The yerd proxy answers it
/// with a `400` carrying `Server: yerd`; a foreign listener won't. HTTP/1.0 is
/// used so the missing `Host` is legal at the protocol level and reaches our
/// dispatch (which then emits the marked `bad_request` response).
const PROBE_REQUEST: &[u8] = b"GET / HTTP/1.0\r\n\r\n";

/// Privileged-port redirect probe.
pub trait PortRedirector {
    /// Whether the privileged-port redirect is currently live - i.e. 80/443 on
    /// loopback are carried to this daemon's proxy. `None` = not applicable on
    /// this OS (Linux binds the privileged ports directly after `setcap`) or
    /// undeterminable.
    fn is_active(&self) -> Option<bool>;

    /// Whether a privileged web port (80/443) is currently held by a listener
    /// that is **not** this daemon's proxy - a foreign process (or a stale `pf`
    /// rule) squatting the port Yerd wants. Cross-platform, unlike
    /// [`Self::is_active`]: a port answers, but the proxy's `Server:` marker
    /// ([`yerd_core::PROXY_SERVER_ID`]) is absent.
    ///
    /// The default impl is correct on every OS where Yerd serves over loopback,
    /// so platform impls only override [`Self::is_active`]. `None` is reserved
    /// for platforms that don't run the proxy (see the unsupported impl).
    fn foreign_web_listener(&self) -> Option<bool> {
        if loopback_redirect_reaches_proxy(80) {
            return Some(false);
        }
        Some(loopback_port_reachable(80) || loopback_port_reachable(443))
    }

    /// Destination ports `(http_to, https_to)` of the installed loopback
    /// (`dev.yerd`) pf anchor, read from disk. Compared against the daemon's
    /// live bound ports to detect a stale redirect. `None` = not installed,
    /// unreadable, or not applicable on this OS (only macOS uses a pf anchor).
    fn redirect_targets(&self) -> Option<(u16, u16)> {
        None
    }

    /// Same as [`Self::redirect_targets`] for the LAN (`dev.yerd.lan`) anchor.
    fn lan_redirect_targets(&self) -> Option<(u16, u16)> {
        None
    }
}

/// Returns `true` iff a TCP connection to `127.0.0.1:port` succeeds within
/// [`PROBE_TIMEOUT`]. The stream is dropped immediately so we never hold a
/// half-open connection against the daemon's own listener.
#[must_use]
pub fn loopback_port_reachable(port: u16) -> bool {
    let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
    TcpStream::connect_timeout(&addr, PROBE_TIMEOUT).is_ok()
}

/// Returns `true` iff a connection to `127.0.0.1:port` is answered by *the yerd
/// proxy* - verified via the `Server: yerd` ([`yerd_core::PROXY_SERVER_ID`])
/// marker on the proxy's synthetic responses. This distinguishes a live yerd
/// privileged-port redirect from any other process (or stale `pf` rule) merely
/// holding the port, which [`loopback_port_reachable`] alone cannot.
#[must_use]
pub fn loopback_redirect_reaches_proxy(port: u16) -> bool {
    let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
    let Ok(mut stream) = TcpStream::connect_timeout(&addr, PROBE_TIMEOUT) else {
        return false;
    };
    if stream.set_read_timeout(Some(PROBE_TIMEOUT)).is_err()
        || stream.set_write_timeout(Some(PROBE_TIMEOUT)).is_err()
        || stream.write_all(PROBE_REQUEST).is_err()
    {
        return false;
    }
    let mut buf = Vec::new();
    let _ = stream.take(512).read_to_end(&mut buf);
    response_identifies_proxy(&buf)
}

/// Whether a response head carries the proxy's `Server:` marker. Tolerant of
/// header casing/spacing: looks for a `server` header and the id token
/// independently (case-insensitively), so it survives hyper's encoding choices
/// without coupling to an exact byte sequence.
fn response_identifies_proxy(head: &[u8]) -> bool {
    contains_ci(head, b"server") && contains_ci(head, yerd_core::PROXY_SERVER_ID.as_bytes())
}

/// Case-insensitive subslice search (`needle` is matched ignoring ASCII case).
fn contains_ci(haystack: &[u8], needle: &[u8]) -> bool {
    if needle.is_empty() {
        return true;
    }
    if haystack.len() < needle.len() {
        return false;
    }
    haystack
        .windows(needle.len())
        .any(|w| w.eq_ignore_ascii_case(needle))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn marker_matches_hyper_lowercase_header() {
        let resp = b"HTTP/1.0 400 Bad Request\r\nserver: yerd\r\ncontent-type: text/plain\r\n\r\nMissing or invalid Host header.\n";
        assert!(response_identifies_proxy(resp));
    }

    #[test]
    fn marker_matches_title_case_header() {
        let resp = b"HTTP/1.1 404 Not Found\r\nServer: yerd\r\n\r\n";
        assert!(response_identifies_proxy(resp));
    }

    #[test]
    fn foreign_server_is_rejected() {
        let resp = b"HTTP/1.1 400 Bad Request\r\nServer: nginx/1.25.3\r\n\r\n";
        assert!(!response_identifies_proxy(resp));
    }

    #[test]
    fn empty_or_truncated_head_is_rejected() {
        assert!(!response_identifies_proxy(b""));
        assert!(!response_identifies_proxy(
            b"HTTP/1.1 400 Bad Request\r\nServer: ye"
        ));
    }

    #[test]
    fn contains_ci_basics() {
        assert!(contains_ci(b"abcDEF", b"cde"));
        assert!(contains_ci(b"abc", b""));
        assert!(!contains_ci(b"abc", b"abcd"));
        assert!(!contains_ci(b"abc", b"xyz"));
    }
}
