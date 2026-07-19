//! `TrustStore` trait and associated data types.
//!
//! The system store install/uninstall always returns `NeedsHelper` in
//! Phase 1; the per-user NSS install is a separately-callable method
//! whose partial-success story is captured by [`NssOutcome`].

use std::path::{Path, PathBuf};

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

    /// Compute the fingerprint of a raw DER certificate body (`sha256(der)`).
    ///
    /// This is the same definition the daemon uses (`yerd_tls`'s
    /// `fingerprint_sha256`); deriving it on disk lets `yerd uninstall` revert
    /// the trust store without a running daemon.
    #[must_use]
    pub fn from_der(der: &[u8]) -> Self {
        Self(crate::pure::pem_match::sha256(der))
    }

    /// Compute the fingerprint of the first `CERTIFICATE` block in a PEM
    /// document. Returns `None` if the text has no certificate block.
    #[must_use]
    pub fn from_pem(pem_text: &str) -> Option<Self> {
        crate::pure::pem_match::fingerprint_of_first_cert_in_pem(pem_text).map(Self)
    }

    /// Parse exactly 64 **lowercase** hex characters into a fingerprint - the
    /// inverse of [`Self::to_hex`]. Uppercase, wrong length, or non-hex input
    /// is rejected; this is the canonical wire form, so the strict lowercase
    /// rule keeps it byte-stable.
    pub fn from_hex(s: &str) -> Result<Self, FingerprintParseError> {
        if s.len() != 64
            || s.chars()
                .any(|c| !c.is_ascii_hexdigit() || c.is_ascii_uppercase())
        {
            return Err(FingerprintParseError);
        }
        let bytes = hex::decode(s).map_err(|_| FingerprintParseError)?;
        let arr: [u8; 32] = bytes.try_into().map_err(|_| FingerprintParseError)?;
        Ok(Self(arr))
    }
}

/// A string was not 64 lowercase hex characters (the canonical [`CaFingerprint`]
/// wire form).
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("invalid CA fingerprint: expected 64 lowercase hex characters")]
pub struct FingerprintParseError;

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

/// Whether the per-user browser (Chromium/Firefox) NSS stores trust the Yerd CA.
///
/// Three states rather than a `bool` so the daemon and doctor can distinguish
/// "not trusted, run trust" from "can't manage NSS at all - install the tool".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BrowserCaTrust {
    /// The CA is present and fingerprint-matches in a browser NSS store (or
    /// there are no browser NSS stores to worry about).
    Trusted,
    /// Browser NSS stores exist but none trust the Yerd CA - the user should
    /// run the trust flow.
    Untrusted,
    /// `certutil` (`libnss3-tools`) is not installed, so browser trust can
    /// neither be verified nor established.
    ToolMissing,
}

/// Trust-store abstraction.
///
/// `install_system` / `uninstall_system` always return `NeedsHelper` in
/// Phase 1; daemon orchestrates the `yerd-helper` invocation. The probe
/// `is_present_system` runs unprivileged and is a *presence* check, not a
/// trust-policy check - a true result means the certificate is in the
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

    /// Report whether the CA at `ca_path` is **effectively trusted** for
    /// SSL - not merely present. Read-only, unprivileged.
    ///
    /// macOS: runs `security verify-cert -c <ca_path> -p ssl`, which
    /// evaluates the user, admin, and system trust domains (presence
    /// without a trust setting reads as *not* trusted). Linux: presence in
    /// an anchor directory *is* system trust, so this delegates to
    /// [`Self::is_present_system`].
    ///
    /// This method has a default `Unsupported` body so non-macOS/Linux
    /// impls (and test fakes) need not override it - the only deliberate
    /// defaulted method on this trait.
    fn is_trusted(&self, ca_path: &Path, fp: &CaFingerprint) -> Result<bool, PlatformError> {
        let _ = (ca_path, fp);
        Err(PlatformError::Unsupported {
            operation: crate::error::ops::IS_TRUSTED,
        })
    }

    /// Install the CA (PEM file at `ca_path`) into every discovered per-user
    /// NSS database: the shared Chromium-family store `~/.pki/nssdb` (created
    /// and initialised if absent) plus every Firefox profile, including
    /// Snap/Flatpak-sandboxed copies. Fixes browsers that ignore the system
    /// trust store (Brave/Chrome/Chromium/Edge/Firefox on Linux).
    ///
    /// Best-effort: returns `Ok(NssOutcome)` even when `certutil` is missing
    /// (`certutil_missing`) or some databases fail. The caller decides whether
    /// to surface the degraded outcome to the user.
    fn install_firefox_nss(&self, ca_path: &Path) -> Result<NssOutcome, PlatformError>;

    /// Remove the Yerd CA from every discovered per-user NSS database (the
    /// inverse of [`Self::install_firefox_nss`]). Best-effort; delete-by-
    /// nickname, so it also clears a stale CA left by a prior rotation.
    fn uninstall_firefox_nss(&self) -> Result<NssOutcome, PlatformError>;

    /// Report whether the CA matching `fp` is trusted by the per-user
    /// **browser** NSS stores. Read-only, unprivileged.
    ///
    /// Distinct from [`Self::is_trusted`], which covers the *system* store
    /// (curl/PHP/OS): on Linux those are unrelated to what browsers trust.
    /// [`BrowserCaTrust::ToolMissing`] is a first-class outcome so the daemon
    /// can tell the user to install `libnss3-tools` rather than silently
    /// reporting healthy.
    ///
    /// Default `Unsupported` body so non-macOS/Linux impls and fakes need not
    /// override it.
    fn browser_ca_trust(&self, fp: &CaFingerprint) -> Result<BrowserCaTrust, PlatformError> {
        let _ = fp;
        Err(PlatformError::Unsupported {
            operation: crate::error::ops::BROWSER_CA_TRUST,
        })
    }

    /// Return the host's public CA roots as a PEM string, for composing the
    /// bundle the bundled PHP verifies against (see `yerd_tls::compose_ca_bundle`).
    /// Read-only, unprivileged.
    ///
    /// macOS enumerates the system root keychains in-process; Linux reads the
    /// first present `ca-certificates` bundle file; the `unsupported` stub
    /// returns `Ok(None)`. `Ok(None)` means "no host roots available" - the
    /// caller must then leave PHP's compiled-in default untouched rather than
    /// pointing it at a rootless bundle.
    fn system_root_bundle(&self) -> Result<Option<String>, PlatformError>;
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
    fn fingerprint_from_hex_round_trips_to_hex() {
        let fp = CaFingerprint::new([0xAB; 32]);
        assert_eq!(CaFingerprint::from_hex(&fp.to_hex()).unwrap(), fp);
    }

    #[test]
    fn from_der_matches_sha256_and_from_pem_round_trips() {
        let der = b"\x30\x82\x01\x0a fake-der-body for fingerprint test";
        let direct = CaFingerprint::from_der(der);
        assert_eq!(direct.as_bytes(), &crate::pure::pem_match::sha256(der));
        let pem = crate::pure::pem_match::der_to_pem(der);
        assert_eq!(CaFingerprint::from_pem(&pem), Some(direct));
    }

    #[test]
    fn from_pem_returns_none_without_certificate_block() {
        assert_eq!(CaFingerprint::from_pem("not a pem"), None);
    }

    #[test]
    fn fingerprint_from_hex_rejects_malformed() {
        assert!(CaFingerprint::from_hex("ab").is_err());
        assert!(CaFingerprint::from_hex(&"ab".repeat(33)).is_err());
        assert!(CaFingerprint::from_hex(&"AB".repeat(32)).is_err());
        assert!(CaFingerprint::from_hex(&"zz".repeat(32)).is_err());
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
