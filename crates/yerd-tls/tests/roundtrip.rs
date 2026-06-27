//! PEM and DER round-trip invariants on `CertAuthority`.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

mod common;

use common::standard_validity;
use yerd_tls::CertAuthority;

#[test]
fn cert_pem_round_trip_byte_identical() {
    let ca = CertAuthority::generate("Yerd Local CA", standard_validity()).unwrap();
    let cert_pem = ca.cert_pem().to_owned();
    let key_pem = ca.key_pem().to_owned();
    let reloaded = CertAuthority::from_pem(&cert_pem, &key_pem).unwrap();
    assert_eq!(reloaded.cert_pem(), cert_pem);
}

#[test]
fn key_pem_round_trip_byte_identical() {
    let ca = CertAuthority::generate("Yerd Local CA", standard_validity()).unwrap();
    let cert_pem = ca.cert_pem().to_owned();
    let key_pem = ca.key_pem().to_owned();
    let reloaded = CertAuthority::from_pem(&cert_pem, &key_pem).unwrap();
    assert_eq!(reloaded.key_pem(), key_pem);
}

#[test]
fn cert_der_round_trip_byte_identical() {
    let ca = CertAuthority::generate("Yerd Local CA", standard_validity()).unwrap();
    let original_der = ca.cert_der().to_vec();
    let reloaded = CertAuthority::from_pem(ca.cert_pem(), ca.key_pem()).unwrap();
    assert_eq!(reloaded.cert_der(), original_der.as_slice());
}

#[test]
fn fingerprint_round_trip_byte_identical() {
    let ca = CertAuthority::generate("Yerd Local CA", standard_validity()).unwrap();
    let fp = ca.fingerprint_sha256();
    let reloaded = CertAuthority::from_pem(ca.cert_pem(), ca.key_pem()).unwrap();
    assert_eq!(reloaded.fingerprint_sha256(), fp);
}

#[test]
fn from_pem_recovers_signing_capability() {
    let ca = CertAuthority::generate("Yerd Local CA", standard_validity()).unwrap();
    let reloaded = CertAuthority::from_pem(ca.cert_pem(), ca.key_pem()).unwrap();
    let names = vec!["foo.test".to_string(), "*.foo.test".to_string()];
    let leaf = reloaded.issue_leaf(&names, standard_validity()).unwrap();
    assert!(leaf.cert_pem().contains("BEGIN CERTIFICATE"));
    assert!(leaf.key_pem().contains("BEGIN PRIVATE KEY"));

    let leaf_block = pem::parse(leaf.cert_pem()).unwrap();
    let (_, leaf_x509) = x509_parser::parse_x509_certificate(leaf_block.contents()).unwrap();
    let (_, ca_x509) = x509_parser::parse_x509_certificate(ca.cert_der()).unwrap();
    assert_eq!(
        leaf_x509.tbs_certificate.issuer.as_raw(),
        ca_x509.tbs_certificate.subject.as_raw(),
        "leaf issuer DN must match the original CA's subject DN"
    );
}

#[test]
fn two_from_pem_calls_on_same_input_hash_identically() {
    let ca = CertAuthority::generate("Yerd Local CA", standard_validity()).unwrap();
    let r1 = CertAuthority::from_pem(ca.cert_pem(), ca.key_pem()).unwrap();
    let r2 = CertAuthority::from_pem(ca.cert_pem(), ca.key_pem()).unwrap();
    assert_eq!(r1.fingerprint_sha256(), r2.fingerprint_sha256());
}
