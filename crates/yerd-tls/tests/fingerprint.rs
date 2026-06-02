//! SHA-256 fingerprint invariants.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

mod common;

use common::standard_validity;
use sha2::{Digest, Sha256};
use yerd_tls::CertAuthority;

#[test]
fn fingerprint_matches_hand_computed_sha256() {
    let ca = CertAuthority::generate("Yerd Local CA", standard_validity()).unwrap();
    let mut h = Sha256::new();
    h.update(ca.cert_der());
    let expected: [u8; 32] = h.finalize().into();
    assert_eq!(ca.fingerprint_sha256(), expected);
}

#[test]
fn fingerprint_differs_between_independent_generations() {
    let a = CertAuthority::generate("CA A", standard_validity()).unwrap();
    let b = CertAuthority::generate("CA A", standard_validity()).unwrap();
    // Even with the same common-name and validity, fresh CAs differ because
    // the key pair and serial number are random.
    assert_ne!(a.fingerprint_sha256(), b.fingerprint_sha256());
}

#[test]
fn fingerprint_stable_after_from_pem() {
    let ca = CertAuthority::generate("Yerd Local CA", standard_validity()).unwrap();
    let fp = ca.fingerprint_sha256();
    let reloaded = CertAuthority::from_pem(ca.cert_pem(), ca.key_pem()).unwrap();
    assert_eq!(reloaded.fingerprint_sha256(), fp);
}
