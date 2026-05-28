//! `TrustStore` trait and associated data types.
//!
//! The system store install/uninstall always returns `NeedsHelper` in
//! Phase 1; the per-user NSS install is a separately-callable method
//! whose partial-success story is captured by [`NssOutcome`].

use std::path::PathBuf;

use crate::PlatformError;

/// SHA-256 fingerprint of a CA certificate's DER body.
///
/// Newtype with a private field so callers can't construct an unchecked
/// fingerprint by accident.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CaFingerprint([u8; 32]);

impl CaFingerprint {
    /// Wrap a raw 32-byte SHA-256 digest as a `CaFingerprint`.
    #[must_use]
    pub const fn new(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    /// Borrow the underlying bytes.
    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    /// Render the fingerprint as 64 lowercase hex characters.
    ///
    /// This is the form used in `HelperInvocation` argv.
    #[must_use]
    pub fn to_hex(&self) -> String {
        hex::encode(self.0)
    }
}

/// Outcome of [`TrustStore::install_firefox_nss`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NssOutcome {
    /// Number of NSS databases discovered and attempted (Firefox
    /// profiles + `~/.pki/nssdb`).
    pub profiles_attempted: usize,
    /// Number of those attempts that succeeded.
    pub profiles_succeeded: usize,
    /// Per-failure detail, in attempt order. Empty on full success.
    pub failures: Vec<(PathBuf, NssFailure)>,
    /// `certutil` was not on `PATH`. With this set, every database in
    /// `failures` has [`NssFailure::CertutilMissing`] as its reason. The
    /// caller logs and continues rather than treating this as an error.
    pub certutil_missing: bool,
}

/// Single-profile failure mode for [`NssOutcome::failures`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NssFailure {
    /// `certutil` was not on `PATH` when this database was processed.
    CertutilMissing,
    /// `certutil` exited non-zero. Carries the exit code.
    CertutilExit(i32),
    /// The candidate NSS database directory did not exist.
    DbMissing,
}

/// Trust-store abstraction.
///
/// `install_system` / `uninstall_system` always return `NeedsHelper` in
/// Phase 1; daemon orchestrates the `yerd-helper` invocation. The probe
/// `is_present_system` runs unprivileged and is a *presence* check, not a
/// trust-policy check — a true result means the certificate is in the
/// store, not necessarily trusted for SSL by every consumer.
pub trait TrustStore {
    /// Request system-store install of `ca_pem`.
    ///
    /// Phase 1: always returns
    /// `Err(PlatformError::NeedsHelper { operation: "install-ca" })`.
    /// The daemon materialises an [`crate::HelperInvocation::InstallCa`]
    /// from `ca_pem` and `fp` and runs `yerd-helper`.
    fn install_system(&self, ca_pem: &str, fp: &CaFingerprint) -> Result<(), PlatformError>;

    /// Request system-store uninstall by fingerprint.
    ///
    /// Phase 1: always returns
    /// `Err(PlatformError::NeedsHelper { operation: "uninstall-ca" })`.
    fn uninstall_system(&self, fp: &CaFingerprint) -> Result<(), PlatformError>;

    /// Report whether a CA matching `fp` is **present** in the system
    /// store. Read-only, unprivileged.
    ///
    /// macOS: enumerates `/Library/Keychains/System.keychain` via
    /// `security-framework`. Linux: iterates the anchor directory
    /// configured for the running distro and hashes each PEM's DER body.
    fn is_present_system(&self, fp: &CaFingerprint) -> Result<bool, PlatformError>;

    /// Install `ca_pem` into every discovered NSS database (Firefox
    /// profiles + `~/.pki/nssdb`).
    ///
    /// Best-effort: returns `Ok(NssOutcome)` even when `certutil` is
    /// missing or some profiles fail. The caller decides whether to
    /// surface the degraded outcome to the user.
    fn install_firefox_nss(&self, ca_pem: &str) -> Result<NssOutcome, PlatformError>;
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
    fn fingerprint_hex_is_64_lowercase_chars() {
        let fp = CaFingerprint::new([0xAB; 32]);
        let hex = fp.to_hex();
        assert_eq!(hex.len(), 64);
        assert!(hex.chars().all(|c| c.is_ascii_hexdigit()));
        assert!(hex.chars().all(|c| !c.is_ascii_uppercase()));
        assert_eq!(hex, "ab".repeat(32));
    }

    #[test]
    fn fingerprint_as_bytes_borrow_matches_input() {
        let bytes = [7u8; 32];
        let fp = CaFingerprint::new(bytes);
        assert_eq!(fp.as_bytes(), &bytes);
    }

    #[test]
    fn nss_outcome_default_construction() {
        let o = NssOutcome {
            profiles_attempted: 0,
            profiles_succeeded: 0,
            failures: vec![],
            certutil_missing: false,
        };
        assert_eq!(o.profiles_attempted, 0);
    }

    #[test]
    fn nss_failure_variants() {
        let _ = NssFailure::CertutilMissing;
        let _ = NssFailure::CertutilExit(2);
        let _ = NssFailure::DbMissing;
    }
}
