//! [`CertAuthority`] — the local CA's generated or loaded material.
//!
//! Stores `cert_pem` + `cert_der` + `key_pem` + `key_pair`. The duplication
//! is deliberate: `cert_pem` and `cert_der` are the canonical wire forms
//! (returned by accessors), `key_pair` is the live signing context for
//! [`Self::issue_leaf`]. After `from_pem`, the cached strings/bytes are the
//! caller's input verbatim, so `cert_pem()` and `fingerprint_sha256()`
//! round-trip losslessly.

use std::fmt;

use rcgen::{CertificateParams, KeyPair, PublicKeyData};
use rustls_pki_types::CertificateDer;
use sha2::{Digest, Sha256};

use crate::error::{rcgen_detail, GenerateErrorReason, ParseErrorReason, TlsError};
use crate::leaf::LeafCert;
use crate::params;
use crate::validity::Validity;

/// Pinning the algorithm explicitly is robust against rcgen changing its
/// `KeyPair::generate()` default. Available via the `crypto` feature, which
/// we enable. (`PKCS_ECDSA_P256_SHA256` is a `static`, not a `const`, so we
/// take a reference at the call site rather than caching it in a const.)
fn key_alg() -> &'static rcgen::SignatureAlgorithm {
    &rcgen::PKCS_ECDSA_P256_SHA256
}

/// A locally-generated CA: cert + key pair, exposable as PEM, hashable to a
/// SHA-256 fingerprint, signs leaf certs.
///
/// `#[non_exhaustive]` because future fields (e.g. cached x509-parser-parsed
/// view) could be added without breaking `SemVer`.
#[non_exhaustive]
pub struct CertAuthority {
    cert_pem: String,
    cert_der: Vec<u8>,
    key_pem: String,
    key_pair: KeyPair,
}

impl CertAuthority {
    /// Generate a fresh CA. `validity` carries the issuance bounds (the
    /// crate has no clock; callers supply timestamps).
    pub fn generate(common_name: &str, validity: Validity) -> Result<Self, TlsError> {
        let params = params::ca_params(common_name, validity)?;

        let key_pair = KeyPair::generate_for(key_alg()).map_err(|e| TlsError::Generate {
            reason: GenerateErrorReason::KeyGenerationFailed {
                detail: rcgen_detail(&e),
            },
        })?;

        let cert = params
            .self_signed(&key_pair)
            .map_err(|e| TlsError::Generate {
                reason: GenerateErrorReason::SelfSignFailed {
                    detail: rcgen_detail(&e),
                },
            })?;

        let cert_pem = cert.pem();
        let cert_der = cert.der().to_vec();
        let key_pem = key_pair.serialize_pem();

        Ok(Self {
            cert_pem,
            cert_der,
            key_pem,
            key_pair,
        })
    }

    /// Load a previously-saved CA from PEM strings.
    ///
    /// Validates: cert PEM tag is `"CERTIFICATE"`; key PEM tag is
    /// `"PRIVATE KEY"`; key's `SPKI` byte-equals the cert's `SPKI` (the
    /// primary safeguard — rcgen does not check this for us).
    pub fn from_pem(cert_pem: &str, key_pem: &str) -> Result<Self, TlsError> {
        // Cert PEM: decode and tag-check.
        let cert_block = pem::parse(cert_pem).map_err(|_| TlsError::Parse {
            reason: ParseErrorReason::InvalidCertificatePem,
        })?;
        if cert_block.tag() != "CERTIFICATE" {
            return Err(TlsError::Parse {
                reason: ParseErrorReason::InvalidCertificatePem,
            });
        }
        let cert_der = cert_block.contents().to_vec();

        // Key PEM: tag-check first (defensive layer — rcgen's from_pem does
        // not enforce tags).
        let key_block = pem::parse(key_pem).map_err(|_| TlsError::Parse {
            reason: ParseErrorReason::InvalidPrivateKeyPem,
        })?;
        if key_block.tag() != "PRIVATE KEY" {
            return Err(TlsError::Parse {
                reason: ParseErrorReason::InvalidPrivateKeyPem,
            });
        }

        let key_pair = KeyPair::from_pem(key_pem).map_err(|_| TlsError::Parse {
            reason: ParseErrorReason::InvalidPrivateKeyPem,
        })?;

        // SPKI byte-comparison (D5 primary mechanism).
        let cert_der_typed = CertificateDer::from(cert_der.as_slice());
        let (_, parsed_cert) =
            x509_parser::parse_x509_certificate(&cert_der).map_err(|_| TlsError::Parse {
                reason: ParseErrorReason::InvalidCertificateDer {
                    detail: "x509_parser_parse_failed",
                },
            })?;
        let cert_spki_der = parsed_cert.tbs_certificate.subject_pki.raw;
        let key_spki_der = key_pair.public_key_der();
        if cert_spki_der != key_spki_der.as_slice() {
            return Err(TlsError::Parse {
                reason: ParseErrorReason::KeyDoesNotMatchCertificate,
            });
        }

        // Probe rcgen parseability so issue_leaf does not surprise the caller.
        // This catches multi-AVA RDN subjects, unsupported extensions, etc.
        CertificateParams::from_ca_cert_der(&cert_der_typed).map_err(|e| TlsError::Parse {
            reason: ParseErrorReason::InvalidCertificateDer {
                detail: rcgen_detail(&e),
            },
        })?;

        Ok(Self {
            cert_pem: cert_pem.to_owned(),
            cert_der,
            key_pem: key_pem.to_owned(),
            key_pair,
        })
    }

    /// The CA certificate, PEM-encoded.
    #[must_use]
    pub fn cert_pem(&self) -> &str {
        &self.cert_pem
    }

    /// The CA private key, PEM-encoded.
    #[must_use]
    pub fn key_pem(&self) -> &str {
        &self.key_pem
    }

    /// The CA certificate, DER-encoded.
    #[must_use]
    pub fn cert_der(&self) -> &[u8] {
        &self.cert_der
    }

    /// SHA-256 over the cached cert DER. Stable across [`Self::from_pem`]
    /// round-trip because the DER bytes are the input PEM decoded once.
    #[must_use]
    pub fn fingerprint_sha256(&self) -> [u8; 32] {
        sha256_der(&self.cert_der)
    }

    /// Issue a leaf cert signed by this CA, for `names` as Subject
    /// Alternative Names.
    pub fn issue_leaf(&self, names: &[String], validity: Validity) -> Result<LeafCert, TlsError> {
        let leaf_params = params::leaf_params(names, validity)?;

        // Reconstruct a synthetic issuer Certificate from the cached DER +
        // live KeyPair. The synthetic cert has a fresh random serial and
        // signature but the same SPKI/DN/key, which is all rcgen's leaf-
        // signing path reads. See plan §D2.
        let cert_der_typed = CertificateDer::from(self.cert_der.as_slice());
        let issuer_params =
            CertificateParams::from_ca_cert_der(&cert_der_typed).map_err(|e| TlsError::Parse {
                reason: ParseErrorReason::InvalidCertificateDer {
                    detail: rcgen_detail(&e),
                },
            })?;
        let issuer_cert =
            issuer_params
                .self_signed(&self.key_pair)
                .map_err(|e| TlsError::Generate {
                    reason: GenerateErrorReason::SelfSignFailed {
                        detail: rcgen_detail(&e),
                    },
                })?;

        // Fresh key for the leaf.
        let leaf_key = KeyPair::generate_for(key_alg()).map_err(|e| TlsError::Generate {
            reason: GenerateErrorReason::KeyGenerationFailed {
                detail: rcgen_detail(&e),
            },
        })?;

        let leaf_cert = leaf_params
            .signed_by(&PublicKeyHandle(&leaf_key), &issuer_cert, &self.key_pair)
            .map_err(|e| TlsError::Generate {
                reason: GenerateErrorReason::SignByCaFailed {
                    detail: rcgen_detail(&e),
                },
            })?;

        Ok(LeafCert::new(leaf_cert.pem(), leaf_key.serialize_pem()))
    }
}

/// Newtype wrapper to satisfy `&impl PublicKeyData` on
/// `CertificateParams::signed_by`. `KeyPair` implements `PublicKeyData`
/// directly; this wrapper simply enables the call shape we want.
struct PublicKeyHandle<'a>(&'a KeyPair);

impl PublicKeyData for PublicKeyHandle<'_> {
    fn der_bytes(&self) -> &[u8] {
        self.0.der_bytes()
    }
    fn algorithm(&self) -> &'static rcgen::SignatureAlgorithm {
        self.0.algorithm()
    }
}

/// Hand-written Debug that elides key material. The default derive would
/// print `key_pem: "...BEGIN PRIVATE KEY..."`, which the daemon's
/// `tracing::error!(?ca)` would leak into logs.
impl fmt::Debug for CertAuthority {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut hex = String::with_capacity(64);
        for b in self.fingerprint_sha256() {
            use std::fmt::Write as _;
            // Test exemption: only writes to a String, can't fail.
            let _ = write!(&mut hex, "{b:02x}");
        }
        f.debug_struct("CertAuthority")
            .field("fingerprint_sha256", &hex)
            .field("key", &"(elided)")
            .finish()
    }
}

fn sha256_der(der: &[u8]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(der);
    hasher.finalize().into()
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod tests {
    use time::{Date, Month, Time};

    use super::*;

    fn v() -> Validity {
        let nb = Date::from_calendar_date(2026, Month::January, 1)
            .unwrap()
            .with_time(Time::from_hms(0, 0, 0).unwrap())
            .assume_utc();
        let na = Date::from_calendar_date(2027, Month::January, 1)
            .unwrap()
            .with_time(Time::from_hms(0, 0, 0).unwrap())
            .assume_utc();
        Validity::new(nb, na).unwrap()
    }

    #[test]
    fn generate_then_pem_roundtrip() {
        let ca = CertAuthority::generate("Yerd Local CA", v()).unwrap();
        let cert_pem = ca.cert_pem().to_owned();
        let key_pem = ca.key_pem().to_owned();
        let reloaded = CertAuthority::from_pem(&cert_pem, &key_pem).unwrap();
        assert_eq!(reloaded.cert_pem(), cert_pem);
        assert_eq!(reloaded.key_pem(), key_pem);
    }

    #[test]
    fn fingerprint_stable_across_from_pem() {
        let ca = CertAuthority::generate("Yerd Local CA", v()).unwrap();
        let fp = ca.fingerprint_sha256();
        let reloaded = CertAuthority::from_pem(ca.cert_pem(), ca.key_pem()).unwrap();
        assert_eq!(reloaded.fingerprint_sha256(), fp);
    }

    #[test]
    fn from_pem_rejects_mismatched_key_and_cert() {
        let a = CertAuthority::generate("CA A", v()).unwrap();
        let b = CertAuthority::generate("CA B", v()).unwrap();
        let err = CertAuthority::from_pem(a.cert_pem(), b.key_pem()).unwrap_err();
        match err {
            TlsError::Parse { reason } => {
                assert_eq!(reason, ParseErrorReason::KeyDoesNotMatchCertificate);
            }
            other => panic!("expected KeyDoesNotMatchCertificate, got {other:?}"),
        }
    }

    #[test]
    fn from_pem_rejects_garbage_cert_pem() {
        let ca = CertAuthority::generate("CA", v()).unwrap();
        let err = CertAuthority::from_pem("not pem at all", ca.key_pem()).unwrap_err();
        match err {
            TlsError::Parse { reason } => {
                assert_eq!(reason, ParseErrorReason::InvalidCertificatePem);
            }
            other => panic!("expected InvalidCertificatePem, got {other:?}"),
        }
    }

    #[test]
    fn from_pem_rejects_garbage_key_pem() {
        let ca = CertAuthority::generate("CA", v()).unwrap();
        let err = CertAuthority::from_pem(ca.cert_pem(), "not pem at all").unwrap_err();
        match err {
            TlsError::Parse { reason } => {
                assert_eq!(reason, ParseErrorReason::InvalidPrivateKeyPem);
            }
            other => panic!("expected InvalidPrivateKeyPem, got {other:?}"),
        }
    }

    #[test]
    fn debug_redacts_key_material() {
        let ca = CertAuthority::generate("CA", v()).unwrap();
        let dbg = format!("{ca:?}");
        assert!(
            !dbg.contains("PRIVATE KEY"),
            "Debug leaks PEM header: {dbg}"
        );
        assert!(!dbg.contains("BEGIN"), "Debug leaks PEM header: {dbg}");
        assert!(
            !dbg.contains(ca.key_pem()),
            "Debug leaks the full key PEM: {dbg}"
        );
        // The base64 body — split on the BEGIN/END lines, concatenate the middle.
        let key_pem = ca.key_pem();
        let body: String = key_pem
            .lines()
            .filter(|l| !l.starts_with("-----"))
            .collect();
        assert!(!body.is_empty());
        assert!(!dbg.contains(&body), "Debug leaks the base64 body: {dbg}");
    }

    #[test]
    fn assert_send_sync_cert_authority() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<CertAuthority>();
    }
}
