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
}
