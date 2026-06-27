//! PEM-decoding edge cases.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

mod common;

use common::standard_validity;
use yerd_tls::{CertAuthority, ParseErrorReason, TlsError};

#[test]
fn multi_block_pem_first_block_wins() {
    let ca = CertAuthority::generate("Yerd Local CA", standard_validity()).unwrap();
    let leaf = ca
        .issue_leaf(&["foo.test".to_string()], standard_validity())
        .unwrap();
    let chain = leaf.chain_pem(ca.cert_pem());
    let err = CertAuthority::from_pem(&chain, ca.key_pem()).unwrap_err();
    assert!(matches!(
        err,
        TlsError::Parse {
            reason: ParseErrorReason::KeyDoesNotMatchCertificate
        }
    ));
}

#[test]
fn pem_with_encrypted_body_rejected() {
    const ENCRYPTED_PEM: &str = "\
-----BEGIN PRIVATE KEY-----
ZW5jcnlwdGVkLXBsYWNlaG9sZGVyLW5vdC1hLXZhbGlkLXBrY3M4LWJsb2NrLXNv
LXJjZ2VuLXdpbGwtcmVqZWN0LWl0LWNsZWFybHk=
-----END PRIVATE KEY-----
";
    let ca = CertAuthority::generate("CA", standard_validity()).unwrap();
    let err = CertAuthority::from_pem(ca.cert_pem(), ENCRYPTED_PEM).unwrap_err();
    assert!(matches!(
        err,
        TlsError::Parse {
            reason: ParseErrorReason::InvalidPrivateKeyPem
        }
    ));
}

#[test]
fn pem_with_whitespace_in_label_rejected() {
    const BAD: &str = "\
-----BEGIN CERTIFICATE -----
MIIBgTCC ...
-----END CERTIFICATE -----
";
    let ca = CertAuthority::generate("CA", standard_validity()).unwrap();
    let err = CertAuthority::from_pem(BAD, ca.key_pem()).unwrap_err();
    assert!(matches!(
        err,
        TlsError::Parse {
            reason: ParseErrorReason::InvalidCertificatePem
        }
    ));
}

#[test]
fn trailing_newline_or_not_both_load() {
    let ca = CertAuthority::generate("Yerd Local CA", standard_validity()).unwrap();
    let with_nl = ca.cert_pem().to_owned();
    let stripped = with_nl.trim_end_matches('\n').to_owned();
    CertAuthority::from_pem(&with_nl, ca.key_pem()).unwrap();
    CertAuthority::from_pem(&stripped, ca.key_pem()).unwrap();
}
