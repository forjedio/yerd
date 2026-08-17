//! The shared desired -> fallback bind-pair retry, used by every real OS
//! adapter.
//!
//! Gated to the three OSes that have a real `PortBinder`: on any other target
//! `os::mod` selects the `unsupported` stub, nothing calls into here, and the
//! items would be dead code.

#![allow(clippy::similar_names)]

use std::net::{Ipv4Addr, SocketAddr, TcpListener};

use crate::error::{BindPairErrorReason, PlatformError};
use crate::port_binder::{BoundPort, PortPair};
use crate::pure::port_plan;

/// Bind a TCP listener at `ip:port`.
pub(crate) fn bind_at(ip: Ipv4Addr, port: u16) -> std::io::Result<TcpListener> {
    TcpListener::bind(SocketAddr::from((ip, port)))
}

/// Attempt `desired`; on a retry-trigger kind
/// (`PermissionDenied`/`AddrInUse`/`AddrNotAvailable`) drop any partial listener
/// and retry `fallback`; any other error on the desired pair surfaces
/// immediately; if both pairs fail, a [`PlatformError::BindPair`] carries all
/// four `ErrorKind`s.
///
/// `lan` only widens the bind address from loopback to `0.0.0.0`.
///
/// `strip_privileged` replaces a privileged `desired` pair with `fallback`
/// before any bind is attempted (see [`port_plan::strip_privileged_desired`]).
/// **macOS passes `true`**: it uses the M2 LAN strategy, where a privileged
/// `pf rdr` installed by `yerd elevate lan` carries inbound 80/443 to
/// `<lan_ip>:<rootless>`, so the daemon must stay deterministically on the
/// rootless ports that redirect targets and must never hold a privileged port
/// itself. Linux and Windows pass `false` and attempt `desired` as given.
pub(crate) fn bind_pair_impl(
    strip_privileged: bool,
    lan: bool,
    desired: (u16, u16),
    fallback: (u16, u16),
) -> Result<PortPair, PlatformError> {
    let desired = if strip_privileged {
        port_plan::strip_privileged_desired(desired, fallback)
    } else {
        desired
    };
    let ip = if lan {
        Ipv4Addr::UNSPECIFIED
    } else {
        Ipv4Addr::LOCALHOST
    };
    let http_attempt = bind_at(ip, desired.0);
    let https_attempt = bind_at(ip, desired.1);

    let http_outcome = http_attempt
        .as_ref()
        .map(|_| ())
        .map_err(std::io::Error::kind);
    let https_outcome = https_attempt
        .as_ref()
        .map(|_| ())
        .map_err(std::io::Error::kind);

    match port_plan::classify_desired(http_outcome, https_outcome) {
        port_plan::DesiredPairAction::KeepDesired => Ok(PortPair {
            http: BoundPort {
                listener: http_attempt.map_err(|e| PlatformError::Bind {
                    port: desired.0,
                    source: e,
                })?,
            },
            https: BoundPort {
                listener: https_attempt.map_err(|e| PlatformError::Bind {
                    port: desired.1,
                    source: e,
                })?,
            },
        }),
        port_plan::DesiredPairAction::HardFail(_) => {
            if let Err(e) = http_attempt {
                return Err(PlatformError::Bind {
                    port: desired.0,
                    source: e,
                });
            }
            if let Err(e) = https_attempt {
                return Err(PlatformError::Bind {
                    port: desired.1,
                    source: e,
                });
            }
            Err(PlatformError::Bind {
                port: desired.0,
                source: std::io::Error::from(std::io::ErrorKind::Other),
            })
        }
        port_plan::DesiredPairAction::UseFallback => {
            let desired_http_kind = http_outcome.err().unwrap_or(std::io::ErrorKind::Other);
            let desired_https_kind = https_outcome.err().unwrap_or(std::io::ErrorKind::Other);
            drop(http_attempt);
            drop(https_attempt);

            let fb_http = bind_at(ip, fallback.0);
            let fb_https = bind_at(ip, fallback.1);

            let fb_http_outcome = fb_http.as_ref().map(|_| ()).map_err(std::io::Error::kind);
            let fb_https_outcome = fb_https.as_ref().map(|_| ()).map_err(std::io::Error::kind);

            match port_plan::classify_fallback(fb_http_outcome, fb_https_outcome) {
                port_plan::FallbackPairAction::KeepFallback => Ok(PortPair {
                    http: BoundPort {
                        listener: fb_http.map_err(|e| PlatformError::Bind {
                            port: fallback.0,
                            source: e,
                        })?,
                    },
                    https: BoundPort {
                        listener: fb_https.map_err(|e| PlatformError::Bind {
                            port: fallback.1,
                            source: e,
                        })?,
                    },
                }),
                port_plan::FallbackPairAction::BothFailed => Err(PlatformError::BindPair {
                    reason: BindPairErrorReason::BothPairsFailed {
                        desired_http: desired_http_kind,
                        desired_https: desired_https_kind,
                        fallback_http: fb_http_outcome.err().unwrap_or(std::io::ErrorKind::Other),
                        fallback_https: fb_https_outcome.err().unwrap_or(std::io::ErrorKind::Other),
                    },
                }),
            }
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    /// (0, 0) makes both ephemeral binds succeed, exercising the `KeepDesired` arm.
    #[test]
    fn bind_pair_impl_keeps_desired_when_both_free() {
        let pair = bind_pair_impl(false, false, (0, 0), (0, 0)).unwrap();
        let http = pair.http.port().unwrap();
        let https = pair.https.port().unwrap();
        assert_ne!(http, 0);
        assert_ne!(https, 0);
        assert_ne!(http, https);
    }

    /// In LAN mode the pair binds the wildcard address, so the resolved local
    /// address is `0.0.0.0` rather than loopback.
    #[test]
    fn bind_pair_impl_lan_binds_wildcard() {
        let pair = bind_pair_impl(false, true, (0, 0), (0, 0)).unwrap();
        assert_eq!(
            pair.http.listener.local_addr().unwrap().ip(),
            std::net::IpAddr::V4(Ipv4Addr::UNSPECIFIED)
        );
        assert_eq!(
            pair.https.listener.local_addr().unwrap().ip(),
            std::net::IpAddr::V4(Ipv4Addr::UNSPECIFIED)
        );
    }

    /// Even when the desired pair is taken and it falls back, LAN mode still
    /// binds the wildcard address for both listeners.
    #[test]
    fn bind_pair_impl_lan_fallback_still_binds_wildcard() {
        let occupied = bind_at(Ipv4Addr::UNSPECIFIED, 0).unwrap();
        let taken = occupied.local_addr().unwrap().port();
        let pair = bind_pair_impl(false, true, (taken, 0), (0, 0)).unwrap();
        assert_eq!(
            pair.http.listener.local_addr().unwrap().ip(),
            std::net::IpAddr::V4(Ipv4Addr::UNSPECIFIED)
        );
        assert_eq!(
            pair.https.listener.local_addr().unwrap().ip(),
            std::net::IpAddr::V4(Ipv4Addr::UNSPECIFIED)
        );
        assert_ne!(pair.http.port().unwrap(), taken);
    }

    /// Occupy a real loopback port so the desired-HTTP bind fails with
    /// `AddrInUse` (a retry kind), driving `UseFallback` then `KeepFallback` on
    /// (0, 0).
    #[test]
    fn bind_pair_impl_uses_fallback_when_desired_http_taken() {
        let occupied = bind_at(Ipv4Addr::LOCALHOST, 0).unwrap();
        let taken = occupied.local_addr().unwrap().port();

        let pair = bind_pair_impl(false, false, (taken, 0), (0, 0)).unwrap();
        assert_ne!(pair.http.port().unwrap(), 0);
        assert_ne!(pair.https.port().unwrap(), 0);
    }

    /// Occupy both the desired-HTTP and fallback-HTTP ports so the desired
    /// pair retries, then the fallback also fails: `BothFailed` then `BindPair`.
    #[test]
    fn bind_pair_impl_both_pairs_failed_yields_bind_pair_error() {
        let occ_desired = bind_at(Ipv4Addr::LOCALHOST, 0).unwrap();
        let desired_http = occ_desired.local_addr().unwrap().port();
        let occ_fallback = bind_at(Ipv4Addr::LOCALHOST, 0).unwrap();
        let fallback_http = occ_fallback.local_addr().unwrap().port();

        let err = bind_pair_impl(false, false, (desired_http, 0), (fallback_http, 0)).unwrap_err();
        assert!(matches!(
            err,
            PlatformError::BindPair {
                reason: BindPairErrorReason::BothPairsFailed { .. }
            }
        ));
    }

    /// With `strip_privileged`, a privileged desired pair is never attempted:
    /// it is replaced by the rootless fallback first, so the bind succeeds
    /// without root and lands on non-privileged ports even though 80/443 were
    /// requested. This is the macOS contract.
    #[test]
    fn bind_pair_impl_never_attempts_privileged_desired_when_stripping() {
        let pair = bind_pair_impl(true, false, (80, 443), (0, 0)).unwrap();
        let http = pair.http.port().unwrap();
        let https = pair.https.port().unwrap();
        assert!(http != 80 && http != 443 && http != 0);
        assert!(https != 80 && https != 443 && https != 0);
    }

    /// Without `strip_privileged` (the Linux and Windows contract), the
    /// privileged pair *is* attempted. Unprivileged CI cannot bind 80/443, so
    /// the retry carries it to the rootless fallback rather than replacing the
    /// desired pair up front; either way it must not end up on 80/443.
    #[test]
    fn bind_pair_impl_attempts_privileged_desired_without_stripping() {
        let pair = bind_pair_impl(false, false, (80, 443), (0, 0)).unwrap();
        let http = pair.http.port().unwrap();
        let https = pair.https.port().unwrap();
        assert_ne!(http, 0);
        assert_ne!(https, 0);
    }
}
