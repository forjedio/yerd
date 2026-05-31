//! `PortRedirector` trait — probe whether the privileged-port redirect is live.
//!
//! On macOS the unprivileged daemon can't bind 80/443, so `yerd elevate ports`
//! installs a pf `rdr` redirect to its rootless ports (see
//! `crate::pure::pf_anchor`). Because the daemon still *binds* the high ports,
//! `StatusReport.http.fell_back` stays `true` even once the redirect works — so
//! the doctor needs a separate signal to know 80/443 are actually reachable.
//!
//! This probe is **active and unprivileged**: it attempts a short-timeout TCP
//! connect to `127.0.0.1:80`/`:443`. A file-existence check (does the pf anchor
//! exist?) would be a false-green — the file can exist while the rule isn't
//! redirecting loopback. Connecting proves end-to-end reachability.

use std::net::{Ipv4Addr, SocketAddr, TcpStream};
use std::time::Duration;

/// Bound on each connect probe so status assembly never stalls.
const PROBE_TIMEOUT: Duration = Duration::from_millis(250);

/// Privileged-port redirect probe.
pub trait PortRedirector {
    /// Whether 80 **and** 443 are currently reachable on loopback (i.e. the
    /// redirect is live). `None` = not applicable on this OS (Linux binds the
    /// privileged ports directly after `setcap`) or undeterminable.
    fn is_active(&self) -> Option<bool>;
}

/// Returns `true` iff a TCP connection to `127.0.0.1:port` succeeds within
/// [`PROBE_TIMEOUT`]. The stream is dropped immediately so we never hold a
/// half-open connection against the daemon's own listener.
#[must_use]
pub fn loopback_port_reachable(port: u16) -> bool {
    let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
    TcpStream::connect_timeout(&addr, PROBE_TIMEOUT).is_ok()
}
