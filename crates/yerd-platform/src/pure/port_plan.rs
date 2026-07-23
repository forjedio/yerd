//! Decide whether a port-pair bind failure should fall back to the
//! rootless pair or surface as an immediate hard failure.
//!
//! `PortBinder::bind_pair` runs the actual `TcpListener::bind` calls; this
//! module decides what to do with the outcome. Keeping the classification
//! pure makes the precedence table testable without a network stack.

use std::io::ErrorKind;

/// Single-side bind outcome: `Ok` if we bound a real listener, else
/// `Err(kind)` with the OS-level error kind.
pub type BindOutcome = Result<(), ErrorKind>;

/// Decision produced after attempting the desired pair.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DesiredPairAction {
    /// Both desired binds succeeded; keep them, no fallback needed.
    KeepDesired,
    /// At least one desired bind failed with a retry-triggering kind
    /// (`PermissionDenied`, `AddrInUse`, or `AddrNotAvailable`). The
    /// caller drops any successful partial listener and retries with the
    /// fallback pair.
    UseFallback,
    /// At least one desired bind failed with a non-retry kind. The whole
    /// `bind_pair` call surfaces this through [`crate::PlatformError::Bind`]
    /// without trying the fallback. The first non-retry kind encountered
    /// (in `http, https` order) is returned.
    HardFail(ErrorKind),
}

/// Decision produced after attempting the fallback pair.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FallbackPairAction {
    /// Both fallback binds succeeded; return the pair.
    KeepFallback,
    /// At least one fallback bind failed. Surface
    /// [`crate::BindPairErrorReason::BothPairsFailed`] carrying all four
    /// error kinds (the desired pair's two + the fallback pair's two).
    BothFailed,
}

/// Classify a desired-pair attempt.
#[must_use]
pub fn classify_desired(http: BindOutcome, https: BindOutcome) -> DesiredPairAction {
    if http.is_ok() && https.is_ok() {
        return DesiredPairAction::KeepDesired;
    }
    if let Err(k) = http {
        if !is_retry_kind(k) {
            return DesiredPairAction::HardFail(k);
        }
    }
    if let Err(k) = https {
        if !is_retry_kind(k) {
            return DesiredPairAction::HardFail(k);
        }
    }
    DesiredPairAction::UseFallback
}

/// Classify a fallback-pair attempt.
#[must_use]
pub fn classify_fallback(http: BindOutcome, https: BindOutcome) -> FallbackPairAction {
    if http.is_ok() && https.is_ok() {
        FallbackPairAction::KeepFallback
    } else {
        FallbackPairAction::BothFailed
    }
}

/// The three `io::ErrorKind`s that cause `bind_pair` to retry with the
/// fallback pair. Any other kind is a hard fail.
#[must_use]
pub fn is_retry_kind(kind: ErrorKind) -> bool {
    matches!(
        kind,
        ErrorKind::PermissionDenied | ErrorKind::AddrInUse | ErrorKind::AddrNotAvailable
    )
}

/// Ports below this are privileged: binding one needs elevation on every
/// supported OS.
pub const PRIVILEGED_PORT_CEILING: u16 = 1024;

/// Replace a privileged desired pair with the rootless fallback pair.
///
/// On macOS the daemon must never hold a privileged port directly: the
/// invariant is that it binds its rootless pair and a privileged `pf rdr`
/// (installed by `yerd elevate ports` / `yerd elevate lan`) carries 80/443 to
/// it. A privileged desired bind normally fails with `PermissionDenied`, which
/// [`is_retry_kind`] already routes to the fallback, so for most daemons this
/// only formalizes the fallback that already happens. It additionally closes
/// the path where a privileged bind occasionally succeeds and leaves the daemon
/// squatting 80/443 in conflict with the redirect design, regardless of how
/// that bind came to be permitted.
///
/// The substitution is pair-level, mirroring `bind_pair`'s pair-level fallback:
/// if either side of `desired` is privileged, the whole `fallback` pair is
/// returned. Port `0` (ephemeral, used by tests) is not privileged and passes
/// through. For the default config the returned pair equals `fallback`, so
/// `bind_pair_impl`'s two-stage desired/fallback retry collapses to a single
/// effective attempt on macOS.
#[must_use]
pub fn strip_privileged_desired(desired: (u16, u16), fallback: (u16, u16)) -> (u16, u16) {
    if is_privileged(desired.0) || is_privileged(desired.1) {
        fallback
    } else {
        desired
    }
}

/// True if `port` is non-zero and below [`PRIVILEGED_PORT_CEILING`].
fn is_privileged(port: u16) -> bool {
    port != 0 && port < PRIVILEGED_PORT_CEILING
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

    #[test]
    fn both_ok_keeps_desired() {
        assert_eq!(
            classify_desired(Ok(()), Ok(())),
            DesiredPairAction::KeepDesired
        );
    }

    #[test]
    fn http_permission_denied_falls_back() {
        assert_eq!(
            classify_desired(Err(ErrorKind::PermissionDenied), Ok(())),
            DesiredPairAction::UseFallback
        );
    }

    #[test]
    fn https_addr_in_use_falls_back() {
        assert_eq!(
            classify_desired(Ok(()), Err(ErrorKind::AddrInUse)),
            DesiredPairAction::UseFallback
        );
    }

    #[test]
    fn addr_not_available_falls_back() {
        assert_eq!(
            classify_desired(Err(ErrorKind::AddrNotAvailable), Ok(())),
            DesiredPairAction::UseFallback
        );
    }

    #[test]
    fn both_retry_kinds_falls_back() {
        assert_eq!(
            classify_desired(Err(ErrorKind::PermissionDenied), Err(ErrorKind::AddrInUse)),
            DesiredPairAction::UseFallback
        );
    }

    #[test]
    fn hard_kind_on_http_short_circuits() {
        let action = classify_desired(Err(ErrorKind::InvalidInput), Err(ErrorKind::AddrInUse));
        assert_eq!(action, DesiredPairAction::HardFail(ErrorKind::InvalidInput));
    }

    #[test]
    fn hard_kind_on_https_when_http_ok() {
        let action = classify_desired(Ok(()), Err(ErrorKind::InvalidInput));
        assert_eq!(action, DesiredPairAction::HardFail(ErrorKind::InvalidInput));
    }

    #[test]
    fn http_hard_takes_precedence_over_https_hard() {
        let action = classify_desired(Err(ErrorKind::InvalidInput), Err(ErrorKind::Unsupported));
        assert_eq!(action, DesiredPairAction::HardFail(ErrorKind::InvalidInput));
    }

    #[test]
    fn retry_kind_on_http_lets_hard_kind_on_https_decide() {
        let action = classify_desired(
            Err(ErrorKind::PermissionDenied),
            Err(ErrorKind::InvalidInput),
        );
        assert_eq!(action, DesiredPairAction::HardFail(ErrorKind::InvalidInput));
    }

    #[test]
    fn fallback_both_ok_keeps() {
        assert_eq!(
            classify_fallback(Ok(()), Ok(())),
            FallbackPairAction::KeepFallback
        );
    }

    #[test]
    fn fallback_any_failure_is_both_failed() {
        assert_eq!(
            classify_fallback(Err(ErrorKind::AddrInUse), Ok(())),
            FallbackPairAction::BothFailed
        );
        assert_eq!(
            classify_fallback(Ok(()), Err(ErrorKind::PermissionDenied)),
            FallbackPairAction::BothFailed
        );
        assert_eq!(
            classify_fallback(Err(ErrorKind::AddrInUse), Err(ErrorKind::PermissionDenied)),
            FallbackPairAction::BothFailed
        );
    }

    #[test]
    fn retry_kind_classification() {
        assert!(is_retry_kind(ErrorKind::PermissionDenied));
        assert!(is_retry_kind(ErrorKind::AddrInUse));
        assert!(is_retry_kind(ErrorKind::AddrNotAvailable));
        assert!(!is_retry_kind(ErrorKind::InvalidInput));
        assert!(!is_retry_kind(ErrorKind::Unsupported));
        assert!(!is_retry_kind(ErrorKind::NotFound));
        assert!(!is_retry_kind(ErrorKind::Other));
    }

    #[test]
    fn default_privileged_pair_collapses_to_fallback() {
        assert_eq!(
            strip_privileged_desired((80, 443), (8080, 8443)),
            (8080, 8443)
        );
    }

    #[test]
    fn rootless_desired_passes_through() {
        assert_eq!(
            strip_privileged_desired((8080, 8443), (9080, 9443)),
            (8080, 8443)
        );
    }

    #[test]
    fn either_privileged_side_collapses_whole_pair() {
        assert_eq!(
            strip_privileged_desired((80, 8443), (8080, 8443)),
            (8080, 8443)
        );
        assert_eq!(
            strip_privileged_desired((8080, 443), (8080, 8443)),
            (8080, 8443)
        );
    }

    #[test]
    fn ephemeral_zero_is_not_privileged() {
        assert_eq!(strip_privileged_desired((0, 0), (8080, 8443)), (0, 0));
    }

    #[test]
    fn privileged_ceiling_is_exclusive() {
        assert_eq!(
            strip_privileged_desired((1023, 1023), (8080, 8443)),
            (8080, 8443)
        );
        assert_eq!(
            strip_privileged_desired((1024, 1024), (8080, 8443)),
            (1024, 1024)
        );
    }
}
