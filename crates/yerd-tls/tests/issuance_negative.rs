//! Negative-path tests: rejected inputs and failure-mode plumbing.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

mod common;

use common::{at, standard_validity};
use time::Month;
use yerd_tls::{
    CertAuthority, GenerateErrorReason, ParseErrorReason, TlsError, Validity, ValidityErrorReason,
};

#[test]
fn empty_names_rejected() {
    let ca = CertAuthority::generate("CA", standard_validity()).unwrap();
    let err = ca.issue_leaf(&[], standard_validity()).unwrap_err();
    assert!(matches!(
        err,
        TlsError::Generate {
            reason: GenerateErrorReason::EmptyNameSet
        }
    ));
}

#[test]
fn non_ia5_name_rejected_with_index() {
    let ca = CertAuthority::generate("CA", standard_validity()).unwrap();
    let names = vec!["ok.test".to_string(), "f\u{00f6}\u{00f6}.test".to_string()];
    let err = ca.issue_leaf(&names, standard_validity()).unwrap_err();
    assert!(matches!(
        err,
        TlsError::Generate {
            reason: GenerateErrorReason::InvalidDnsName { index: 1 }
        }
    ));
}

#[test]
fn empty_common_name_rejected() {
    let err = CertAuthority::generate("", standard_validity()).unwrap_err();
    assert!(matches!(
        err,
        TlsError::Generate {
            reason: GenerateErrorReason::EmptyCommonName
        }
    ));
}

#[test]
fn overlong_common_name_rejected() {
    let cn = "a".repeat(65);
    let err = CertAuthority::generate(&cn, standard_validity()).unwrap_err();
    assert!(matches!(
        err,
        TlsError::Generate {
            reason: GenerateErrorReason::CommonNameTooLong { max: 64 }
        }
    ));
}

#[test]
fn boundary_common_name_accepted() {
    let cn = "a".repeat(64);
    CertAuthority::generate(&cn, standard_validity()).unwrap();
}

#[test]
fn validity_reversed_rejected() {
    let err = Validity::new(at(2027, Month::January, 1), at(2026, Month::January, 1)).unwrap_err();
    assert!(matches!(
        err,
        TlsError::Validity {
            reason: ValidityErrorReason::NotBeforeAfterNotAfter
        }
    ));
}

#[test]
fn from_pem_garbage_cert_rejected() {
    let ca = CertAuthority::generate("CA", standard_validity()).unwrap();
    let err = CertAuthority::from_pem("rubbish", ca.key_pem()).unwrap_err();
    assert!(matches!(
        err,
        TlsError::Parse {
            reason: ParseErrorReason::InvalidCertificatePem
        }
    ));
}

#[test]
fn from_pem_garbage_key_rejected() {
    let ca = CertAuthority::generate("CA", standard_validity()).unwrap();
    let err = CertAuthority::from_pem(ca.cert_pem(), "rubbish").unwrap_err();
    assert!(matches!(
        err,
        TlsError::Parse {
            reason: ParseErrorReason::InvalidPrivateKeyPem
        }
    ));
}

#[test]
fn from_pem_key_does_not_match_cert_rejected() {
    let a = CertAuthority::generate("CA A", standard_validity()).unwrap();
    let b = CertAuthority::generate("CA B", standard_validity()).unwrap();
    let err = CertAuthority::from_pem(a.cert_pem(), b.key_pem()).unwrap_err();
    assert!(matches!(
        err,
        TlsError::Parse {
            reason: ParseErrorReason::KeyDoesNotMatchCertificate
        }
    ));
}

#[test]
fn from_pem_cert_with_wrong_tag_rejected() {
    let ca = CertAuthority::generate("CA", standard_validity()).unwrap();
    let err = CertAuthority::from_pem(ca.key_pem(), ca.key_pem()).unwrap_err();
    assert!(matches!(
        err,
        TlsError::Parse {
            reason: ParseErrorReason::InvalidCertificatePem
        }
    ));
}

#[test]
fn from_pem_cross_algorithm_rejected() {
    let p256 = CertAuthority::generate("CA", standard_validity()).unwrap();
    let ed25519 = rcgen::KeyPair::generate_for(&rcgen::PKCS_ED25519).unwrap();
    let ed25519_pem = ed25519.serialize_pem();
    let err = CertAuthority::from_pem(p256.cert_pem(), &ed25519_pem).unwrap_err();
    assert!(matches!(
        err,
        TlsError::Parse {
            reason: ParseErrorReason::KeyDoesNotMatchCertificate
        }
    ));
}

#[test]
fn from_pem_key_with_wrong_tag_rejected() {
    let ca = CertAuthority::generate("CA", standard_validity()).unwrap();
    let err = CertAuthority::from_pem(ca.cert_pem(), ca.cert_pem()).unwrap_err();
    assert!(matches!(
        err,
        TlsError::Parse {
            reason: ParseErrorReason::InvalidPrivateKeyPem
        }
    ));
}
