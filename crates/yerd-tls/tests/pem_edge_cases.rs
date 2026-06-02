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
    // Feed a `chain_pem`-shaped input (leaf+CA concatenated) to from_pem.
    // Implementation calls `pem::parse` which returns the first block, so
    // the leaf cert loads and the CA is ignored. Because the leaf's key and
    // SPKI don't match the CA's key, this then fails with
    // KeyDoesNotMatchCertificate (proving the *leaf*, not the CA, was loaded).
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
    // A PRIVATE KEY-labelled block whose body is not a valid PKCS#8
    // PrivateKeyInfo (the format rcgen accepts). EncryptedPrivateKeyInfo
    // wraps PrivateKeyInfo with a different outer ASN.1 SEQUENCE; rcgen
    // rejects it. We use a deterministic non-PKCS#8 body inline (base64 of
    // random ASCII), which exercises the same code path without depending on
    // a runtime openssl invocation.
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
    // `-----BEGIN CERTIFICATE -----` (trailing space inside the label). The
    // `pem` crate either parses with the trimmed label or rejects outright.
    // Either way `from_pem` must reject — either via our tag check (if the
    // parser hands us a "CERTIFICATE " tag) or via the parse error itself.
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
    // Both forms must load successfully.
    CertAuthority::from_pem(&with_nl, ca.key_pem()).unwrap();
    CertAuthority::from_pem(&stripped, ca.key_pem()).unwrap();
}
