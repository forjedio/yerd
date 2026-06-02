//! Pure-Rust local CA and per-site leaf certificate issuance for Yerd.
//!
//! `yerd-tls` generates a self-signed CA, loads it back from PEM, computes its
//! SHA-256 fingerprint, and issues per-site leaf certs signed by it. It does
//! **no I/O**, **no clock reads**, and **no env reads** — callers pass
//! timestamps via [`Validity`]; persistence and trust-store install live in
//! `yerd-config` and `yerd-platform` respectively.
//!
//! ## Purity
//!
//! No `tokio`, no `std::fs`, no `std::time::SystemTime`/`Instant`. All
//! random material comes from rcgen's configured backend (`ring` under our
//! feature set). See `README.md` for the cryptography posture.

#![forbid(unsafe_code)]

mod ca;
mod error;
mod leaf;
mod params;
mod validity;

pub use ca::CertAuthority;
pub use error::{GenerateErrorReason, ParseErrorReason, TlsError, ValidityErrorReason};
pub use leaf::LeafCert;
pub use validity::Validity;

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod tests {
    use super::*;

    /// Compile-time check that the four public types are re-exported and
    /// nameable through the crate root.
    #[test]
    fn re_exports_compile() {
        fn _names_resolve() {
            let _: Option<CertAuthority> = None;
            let _: Option<LeafCert> = None;
            let _: Option<Validity> = None;
            let _: Option<TlsError> = None;
            let _: Option<GenerateErrorReason> = None;
            let _: Option<ParseErrorReason> = None;
            let _: Option<ValidityErrorReason> = None;
        }
    }
}
