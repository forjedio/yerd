//! Defence-in-depth validators.
//!
//! The helper does not trust the daemon. Every typed value re-parses
//! and re-validates here before any side effect happens.

#![allow(clippy::similar_names)]

use std::fs;
use std::path::Path;

use yerd_core::Tld;
use yerd_platform::pure::pem_match;
use yerd_platform::CaFingerprint;

use crate::error::{HelperError, ValidationReason};

/// Reject relative paths and missing files. We do NOT canonicalise —
/// canonicalisation against a path an attacker controls introduces
/// TOCTOU (the path could be swapped between canonicalize and open).
pub fn require_existing_file(path: &Path) -> Result<(), HelperError> {
    if !path.is_absolute() {
        return Err(HelperError::Validation {
            reason: ValidationReason::PathNotAbsolute(path.to_path_buf()),
        });
    }
    let meta = fs::symlink_metadata(path).map_err(|_| HelperError::Validation {
        reason: ValidationReason::PathMissing(path.to_path_buf()),
    })?;
    if !meta.file_type().is_file() {
        return Err(HelperError::Validation {
            reason: ValidationReason::PathNotFile(path.to_path_buf()),
        });
    }
    Ok(())
}

/// `setcap` is only useful against the `yerdd` binary; refusing other
/// basenames here bounds the blast radius if the daemon is ever tricked
/// into asking for setcap on an arbitrary binary.
pub fn require_basename_yerdd(path: &Path) -> Result<(), HelperError> {
    let basename = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
    if basename != "yerdd" {
        return Err(HelperError::Validation {
            reason: ValidationReason::BinaryNameUnexpected(basename.to_string()),
        });
    }
    Ok(())
}

/// Re-parse the TLD through `yerd-core::Tld` so the helper itself
/// validates it; the daemon's validation is not in our trust base.
pub fn require_valid_tld(raw: &str) -> Result<Tld, HelperError> {
    Tld::new(raw).map_err(|_| HelperError::Validation {
        reason: ValidationReason::TldInvalid(raw.to_string()),
    })
}

/// Read a PEM file, require exactly one CERTIFICATE block, and verify
/// its SHA-256 matches the argv-supplied fingerprint. On success
/// returns the DER body for downstream use.
///
/// This closes the "drop a different PEM into runtime dir" attack
/// vector: the helper provably installs the certificate whose
/// fingerprint the daemon chose.
pub fn require_pem_matches_fingerprint(
    pem_path: &Path,
    expected: &CaFingerprint,
) -> Result<Vec<u8>, HelperError> {
    let bytes = fs::read(pem_path).map_err(|source| HelperError::Io {
        path: pem_path.to_path_buf(),
        source,
    })?;
    let blocks = pem::parse_many(bytes).map_err(|_| HelperError::Validation {
        reason: ValidationReason::PemParseFailed,
    })?;
    let certs: Vec<&pem::Pem> = blocks.iter().filter(|b| b.tag() == "CERTIFICATE").collect();
    if certs.len() != 1 {
        return Err(HelperError::Validation {
            reason: ValidationReason::ExpectedSingleCertPem { count: certs.len() },
        });
    }
    let der = certs
        .first()
        .map_or_else(Vec::new, |c| c.contents().to_vec());
    let actual = pem_match::sha256(&der);
    if &actual != expected.as_bytes() {
        return Err(HelperError::FingerprintMismatch {
            expected: hex::encode(expected.as_bytes()),
            actual: hex::encode(actual),
        });
    }
    Ok(der)
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
    use std::io::Write;

    fn write_pem_with_blocks(path: &Path, ders: &[&[u8]]) {
        let mut f = std::fs::File::create(path).unwrap();
        for der in ders {
            let block = pem::Pem::new("CERTIFICATE", der.to_vec());
            f.write_all(pem::encode(&block).as_bytes()).unwrap();
        }
    }

    #[test]
    fn require_existing_file_rejects_relative() {
        let err = require_existing_file(Path::new("foo")).unwrap_err();
        assert!(matches!(
            err,
            HelperError::Validation {
                reason: ValidationReason::PathNotAbsolute(_)
            }
        ));
    }

    #[test]
    fn require_existing_file_rejects_missing() {
        let err = require_existing_file(Path::new("/tmp/yerd-no-such-file-xyz")).unwrap_err();
        assert!(matches!(
            err,
            HelperError::Validation {
                reason: ValidationReason::PathMissing(_)
            }
        ));
    }

    #[test]
    fn require_existing_file_rejects_directory() {
        let err = require_existing_file(Path::new("/tmp")).unwrap_err();
        assert!(matches!(
            err,
            HelperError::Validation {
                reason: ValidationReason::PathNotFile(_)
            }
        ));
    }

    #[test]
    fn require_existing_file_accepts_real_file() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("real.crt");
        std::fs::write(&p, b"x").unwrap();
        assert!(require_existing_file(&p).is_ok());
    }

    #[test]
    fn require_basename_yerdd_accepts() {
        assert!(require_basename_yerdd(Path::new("/usr/bin/yerdd")).is_ok());
    }

    #[test]
    fn require_basename_yerdd_rejects_other() {
        let err = require_basename_yerdd(Path::new("/usr/bin/zerdd")).unwrap_err();
        assert!(matches!(
            err,
            HelperError::Validation {
                reason: ValidationReason::BinaryNameUnexpected(_)
            }
        ));
    }

    #[test]
    fn require_valid_tld_accepts_test() {
        let tld = require_valid_tld("test").unwrap();
        assert_eq!(tld.as_str(), "test");
    }

    #[test]
    fn require_valid_tld_rejects_traversal() {
        let err = require_valid_tld("../etc/passwd").unwrap_err();
        assert!(matches!(
            err,
            HelperError::Validation {
                reason: ValidationReason::TldInvalid(_)
            }
        ));
    }

    #[test]
    fn require_pem_matches_fingerprint_happy_path() {
        let der: &[u8] = b"some der bytes";
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("ca.pem");
        write_pem_with_blocks(&p, &[der]);
        let fp = CaFingerprint::new(pem_match::sha256(der));
        let returned = require_pem_matches_fingerprint(&p, &fp).unwrap();
        assert_eq!(returned, der);
    }

    #[test]
    fn require_pem_matches_fingerprint_mismatch() {
        let der: &[u8] = b"actual der";
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("ca.pem");
        write_pem_with_blocks(&p, &[der]);
        let wrong = CaFingerprint::new([0u8; 32]);
        let err = require_pem_matches_fingerprint(&p, &wrong).unwrap_err();
        assert!(matches!(err, HelperError::FingerprintMismatch { .. }));
    }

    #[test]
    fn require_pem_matches_fingerprint_rejects_multi_cert_pem() {
        let der_a: &[u8] = b"cert a";
        let der_b: &[u8] = b"cert b";
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("ca.pem");
        write_pem_with_blocks(&p, &[der_a, der_b]);
        let fp = CaFingerprint::new(pem_match::sha256(der_a));
        let err = require_pem_matches_fingerprint(&p, &fp).unwrap_err();
        assert!(matches!(
            err,
            HelperError::Validation {
                reason: ValidationReason::ExpectedSingleCertPem { count: 2 }
            }
        ));
    }

    #[test]
    fn require_pem_matches_fingerprint_rejects_zero_cert_pem() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("ca.pem");
        // Write a PRIVATE KEY block but no CERTIFICATE.
        let block = pem::Pem::new("PRIVATE KEY", b"key".to_vec());
        std::fs::write(&p, pem::encode(&block)).unwrap();
        let fp = CaFingerprint::new([0u8; 32]);
        let err = require_pem_matches_fingerprint(&p, &fp).unwrap_err();
        assert!(matches!(
            err,
            HelperError::Validation {
                reason: ValidationReason::ExpectedSingleCertPem { count: 0 }
            }
        ));
    }

    #[test]
    fn require_pem_matches_fingerprint_io_error_on_missing() {
        let err = require_pem_matches_fingerprint(
            Path::new("/tmp/yerd-no-such-pem-xyz"),
            &CaFingerprint::new([0u8; 32]),
        )
        .unwrap_err();
        assert!(matches!(err, HelperError::Io { .. }));
    }
}
