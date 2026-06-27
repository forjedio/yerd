//! Error types for `yerd-tls`.
//!
//! [`TlsError`] is the single error type exposed by every fallible public API
//! in this crate. Each variant carries a typed `*Reason` sub-enum so callers
//! can match on precise failure modes without parsing message strings. We
//! deliberately do not wrap [`rcgen::Error`] - instead, [`rcgen_detail`] maps
//! every variant to a `&'static str` tag exposed through Reason variants like
//! [`GenerateErrorReason::SelfSignFailed`]. This keeps [`TlsError`] fully
//! `Clone + PartialEq + Eq` and preserves diagnostic detail without leaking
//! `rcgen`'s API.
//!
//! Every public error enum carries `#[non_exhaustive]` so additions are
//! semver-compatible.

use std::fmt;

use thiserror::Error;

/// Errors produced by `yerd-tls`.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum TlsError {
    /// Failed while building or signing new certificate material.
    #[error("could not generate certificate material: {reason}")]
    Generate {
        /// Specific generation failure.
        reason: GenerateErrorReason,
    },

    /// Failed while parsing CA material handed in via PEM strings.
    #[error("could not parse certificate material: {reason}")]
    Parse {
        /// Specific parse failure.
        reason: ParseErrorReason,
    },

    /// Validity window failed construction-time validation.
    #[error("invalid validity window: {reason}")]
    Validity {
        /// Specific validity failure.
        reason: ValidityErrorReason,
    },
}

/// Specific failure modes for cert/key *generation*.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum GenerateErrorReason {
    /// `common_name` argument was empty.
    EmptyCommonName,
    /// `common_name` exceeded the RFC 5280 `ub-common-name` byte cap.
    CommonNameTooLong {
        /// The byte cap that was exceeded.
        max: usize,
    },
    /// `names` slice passed to `issue_leaf` was empty (every leaf needs at
    /// least one SAN entry to be useful as a TLS server cert).
    EmptyNameSet,
    /// `names[index]` was not a valid `IA5String` (ASCII) DNS name.
    InvalidDnsName {
        /// The 0-based index into the `names` slice that failed validation.
        index: usize,
    },
    /// `rcgen::KeyPair::generate*` failed. `detail` is the rcgen variant name
    /// (e.g. `"RingUnspecified"`); see [`rcgen_detail`].
    KeyGenerationFailed {
        /// Static tag identifying the underlying rcgen variant.
        detail: &'static str,
    },
    /// `params.self_signed(...)` failed (CA generation path). `detail` is the
    /// rcgen variant name.
    SelfSignFailed {
        /// Static tag identifying the underlying rcgen variant.
        detail: &'static str,
    },
    /// `params.signed_by(...)` failed (leaf signing path). `detail` is the
    /// rcgen variant name.
    SignByCaFailed {
        /// Static tag identifying the underlying rcgen variant.
        detail: &'static str,
    },
}

impl fmt::Display for GenerateErrorReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyCommonName => f.write_str("common name must not be empty"),
            Self::CommonNameTooLong { max } => write!(f, "common name exceeds {max} bytes"),
            Self::EmptyNameSet => f.write_str("name set must not be empty"),
            Self::InvalidDnsName { index } => {
                write!(f, "name at index {index} is not a valid IA5 DNS name")
            }
            Self::KeyGenerationFailed { detail } => write!(f, "key generation failed: {detail}"),
            Self::SelfSignFailed { detail } => write!(f, "self-signing failed: {detail}"),
            Self::SignByCaFailed { detail } => write!(f, "signing leaf with CA failed: {detail}"),
        }
    }
}

/// Specific failure modes for parsing CA material from PEM strings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ParseErrorReason {
    /// The certificate PEM string failed to decode, or carried a tag other
    /// than `"CERTIFICATE"`.
    InvalidCertificatePem,
    /// The certificate DER bytes failed `rcgen::CertificateParams::from_ca_cert_der`.
    /// Typically a multi-AVA `RDN` in the subject or other unsupported feature.
    InvalidCertificateDer {
        /// Static tag identifying the underlying rcgen variant.
        detail: &'static str,
    },
    /// The private-key PEM string failed to decode, carried a tag other than
    /// `"PRIVATE KEY"`, or had a body `rcgen::KeyPair::from_pem` rejected.
    InvalidPrivateKeyPem,
    /// The key pair's public key did not match the certificate's
    /// `SubjectPublicKeyInfo`. SPKI byte-comparison is the primary safeguard
    /// against `from_pem(a.cert_pem, b.key_pem)` succeeding silently.
    KeyDoesNotMatchCertificate,
}

impl fmt::Display for ParseErrorReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidCertificatePem => {
                f.write_str("certificate PEM is malformed or has the wrong tag")
            }
            Self::InvalidCertificateDer { detail } => {
                write!(f, "certificate DER could not be parsed by rcgen: {detail}")
            }
            Self::InvalidPrivateKeyPem => {
                f.write_str("private-key PEM is malformed or has the wrong tag")
            }
            Self::KeyDoesNotMatchCertificate => {
                f.write_str("key pair's public key does not match the certificate's SPKI")
            }
        }
    }
}

/// Specific failure modes for [`Validity::new`](crate::Validity::new).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ValidityErrorReason {
    /// `not_before > not_after`.
    NotBeforeAfterNotAfter,
    /// `not_before.year() > 9998` or `not_after.year() > 9998`. Reserves a
    /// one-year gap below `time`'s representable ceiling so callers can't
    /// emit `99991231235959Z` `GeneralizedTime` that trust stores mis-parse.
    YearAbove9998,
}

impl fmt::Display for ValidityErrorReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotBeforeAfterNotAfter => f.write_str("not_before must not exceed not_after"),
            Self::YearAbove9998 => f.write_str("year must not exceed 9998"),
        }
    }
}

/// Map an [`rcgen::Error`] to a stable static-string tag.
///
/// The dispatch table covers every variant present under our feature set
/// (`pem`, `crypto`, `ring`, `x509-parser`). `MissingSerialNumber` is
/// cfg-gated to `not(crypto)` upstream and is excluded.
///
/// New rcgen variants fall through to `"Unknown"`. The patch pin
/// `rcgen = "=0.13.2"` in the workspace `Cargo.toml` forces a deliberate
/// edit at bump-time; the `rcgen_error_detail_table_is_current` tripwire
/// test then catches the omission if the maintainer doesn't pre-defeat it.
pub(crate) fn rcgen_detail(err: &rcgen::Error) -> &'static str {
    match err {
        rcgen::Error::CouldNotParseCertificate => "CouldNotParseCertificate",
        rcgen::Error::CouldNotParseCertificationRequest => "CouldNotParseCertificationRequest",
        rcgen::Error::CouldNotParseKeyPair => "CouldNotParseKeyPair",
        rcgen::Error::InvalidNameType => "InvalidNameType",
        rcgen::Error::InvalidAsn1String(_) => "InvalidAsn1String",
        rcgen::Error::InvalidIpAddressOctetLength(_) => "InvalidIpAddressOctetLength",
        rcgen::Error::KeyGenerationUnavailable => "KeyGenerationUnavailable",
        rcgen::Error::UnsupportedExtension => "UnsupportedExtension",
        rcgen::Error::UnsupportedSignatureAlgorithm => "UnsupportedSignatureAlgorithm",
        rcgen::Error::RingUnspecified => "RingUnspecified",
        rcgen::Error::RingKeyRejected(_) => "RingKeyRejected",
        rcgen::Error::Time => "Time",
        rcgen::Error::PemError(_) => "PemError",
        rcgen::Error::RemoteKeyError => "RemoteKeyError",
        rcgen::Error::UnsupportedInCsr => "UnsupportedInCsr",
        rcgen::Error::InvalidCrlNextUpdate => "InvalidCrlNextUpdate",
        rcgen::Error::IssuerNotCrlSigner => "IssuerNotCrlSigner",
        rcgen::Error::X509(_) => "X509",
        _ => "Unknown",
    }
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
    fn display_generate_each_variant_non_empty() {
        for r in [
            GenerateErrorReason::EmptyCommonName,
            GenerateErrorReason::CommonNameTooLong { max: 64 },
            GenerateErrorReason::EmptyNameSet,
            GenerateErrorReason::InvalidDnsName { index: 0 },
            GenerateErrorReason::KeyGenerationFailed { detail: "X" },
            GenerateErrorReason::SelfSignFailed { detail: "X" },
            GenerateErrorReason::SignByCaFailed { detail: "X" },
        ] {
            assert!(!r.to_string().is_empty());
            let _ = format!("{r:?}");
        }
    }

    #[test]
    fn display_parse_each_variant_non_empty() {
        for r in [
            ParseErrorReason::InvalidCertificatePem,
            ParseErrorReason::InvalidCertificateDer { detail: "X" },
            ParseErrorReason::InvalidPrivateKeyPem,
            ParseErrorReason::KeyDoesNotMatchCertificate,
        ] {
            assert!(!r.to_string().is_empty());
            let _ = format!("{r:?}");
        }
    }

    #[test]
    fn display_validity_each_variant_non_empty() {
        for r in [
            ValidityErrorReason::NotBeforeAfterNotAfter,
            ValidityErrorReason::YearAbove9998,
        ] {
            assert!(!r.to_string().is_empty());
            let _ = format!("{r:?}");
        }
    }

    #[test]
    fn display_tls_error_carries_reason() {
        let g = TlsError::Generate {
            reason: GenerateErrorReason::EmptyCommonName,
        };
        let p = TlsError::Parse {
            reason: ParseErrorReason::InvalidCertificatePem,
        };
        let v = TlsError::Validity {
            reason: ValidityErrorReason::NotBeforeAfterNotAfter,
        };
        assert!(g.to_string().contains("common name"));
        assert!(p.to_string().contains("malformed"));
        assert!(v.to_string().contains("not_before"));
    }

    /// Trait-bounds assertion: `TlsError` stays Eq-friendly because we deliberately
    /// do not wrap `rcgen::Error` (which is not `Clone`).
    #[test]
    fn tls_error_is_clone_partial_eq_eq() {
        fn assert_traits<T: Clone + PartialEq + Eq + Send + Sync>() {}
        assert_traits::<TlsError>();
    }

    #[test]
    fn reason_enums_are_copy_eq() {
        fn assert_traits<T: Copy + PartialEq + Eq + Send + Sync>() {}
        assert_traits::<GenerateErrorReason>();
        assert_traits::<ParseErrorReason>();
        assert_traits::<ValidityErrorReason>();
    }

    /// Tripwire: constructs every `TlsError` variant and every `Reason` variant.
    /// Adding a new variant without updating this test drops coverage and
    /// makes the omission visible.
    #[test]
    fn construct_every_tls_error_variant() {
        let _ = TlsError::Generate {
            reason: GenerateErrorReason::EmptyCommonName,
        };
        let _ = TlsError::Parse {
            reason: ParseErrorReason::InvalidCertificatePem,
        };
        let _ = TlsError::Validity {
            reason: ValidityErrorReason::NotBeforeAfterNotAfter,
        };

        for _ in [
            GenerateErrorReason::EmptyCommonName,
            GenerateErrorReason::CommonNameTooLong { max: 64 },
            GenerateErrorReason::EmptyNameSet,
            GenerateErrorReason::InvalidDnsName { index: 0 },
            GenerateErrorReason::KeyGenerationFailed { detail: "X" },
            GenerateErrorReason::SelfSignFailed { detail: "X" },
            GenerateErrorReason::SignByCaFailed { detail: "X" },
        ] {}

        for _ in [
            ParseErrorReason::InvalidCertificatePem,
            ParseErrorReason::InvalidCertificateDer { detail: "X" },
            ParseErrorReason::InvalidPrivateKeyPem,
            ParseErrorReason::KeyDoesNotMatchCertificate,
        ] {}

        for _ in [
            ValidityErrorReason::NotBeforeAfterNotAfter,
            ValidityErrorReason::YearAbove9998,
        ] {}
    }

    /// rcgen-variant dispatch tripwire. Enumerates every `rcgen::Error` variant
    /// present under our feature set and asserts each maps to a non-`"Unknown"`
    /// detail string. A future rcgen bump that adds a new variant will require
    /// re-pointing the workspace pin; this test then fires until the new
    /// variant is mapped in `rcgen_detail`.
    #[test]
    fn rcgen_error_detail_table_is_current() {
        use rcgen::{Error::*, InvalidAsn1String};

        let cases: &[rcgen::Error] = &[
            CouldNotParseCertificate,
            CouldNotParseCertificationRequest,
            CouldNotParseKeyPair,
            InvalidNameType,
            InvalidAsn1String(InvalidAsn1String::Ia5String(String::new())),
            InvalidIpAddressOctetLength(0),
            KeyGenerationUnavailable,
            UnsupportedExtension,
            UnsupportedSignatureAlgorithm,
            RingUnspecified,
            RingKeyRejected(String::new()),
            Time,
            PemError(String::new()),
            RemoteKeyError,
            UnsupportedInCsr,
            InvalidCrlNextUpdate,
            IssuerNotCrlSigner,
            X509(String::new()),
        ];

        for err in cases {
            let detail = rcgen_detail(err);
            assert_ne!(
                detail, "Unknown",
                "rcgen variant {err:?} fell through to Unknown — update rcgen_detail and this test"
            );
            assert!(!detail.is_empty());
        }
    }
}
