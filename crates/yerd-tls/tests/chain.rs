//! Chain-validation invariants: issuer DN, AKI/SKI alignment, signature
//! verification, serial uniqueness.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

mod common;

use common::standard_validity;
use sha2::{Digest, Sha256};
use x509_parser::extensions::ParsedExtension;
use yerd_tls::CertAuthority;

fn parse_cert(der: &[u8]) -> x509_parser::certificate::X509Certificate<'_> {
    let (_, x) = x509_parser::parse_x509_certificate(der).unwrap();
    x
}

fn leaf_for(ca: &CertAuthority, names: &[String]) -> (Vec<u8>, Vec<u8>) {
    let leaf = ca.issue_leaf(names, standard_validity()).unwrap();
    let leaf_block = pem::parse(leaf.cert_pem()).unwrap();
    (leaf_block.contents().to_vec(), ca.cert_der().to_vec())
}

#[test]
fn leaf_issuer_dn_matches_ca_subject_dn() {
    let ca = CertAuthority::generate("Yerd Local CA", standard_validity()).unwrap();
    let (leaf_der, ca_der) = leaf_for(&ca, &["foo.test".into()]);
    let leaf = parse_cert(&leaf_der);
    let ca_x = parse_cert(&ca_der);
    assert_eq!(
        leaf.tbs_certificate.issuer.as_raw(),
        ca_x.tbs_certificate.subject.as_raw()
    );
}

#[test]
fn leaf_aki_matches_freshly_generated_ca_ski() {
    let ca = CertAuthority::generate("Yerd Local CA", standard_validity()).unwrap();
    let (leaf_der, ca_der) = leaf_for(&ca, &["foo.test".into()]);
    let leaf = parse_cert(&leaf_der);
    let ca_x = parse_cert(&ca_der);

    let mut leaf_aki: Option<Vec<u8>> = None;
    for ext in leaf.iter_extensions() {
        if let ParsedExtension::AuthorityKeyIdentifier(aki) = ext.parsed_extension() {
            leaf_aki = aki.key_identifier.as_ref().map(|k| k.0.to_vec());
        }
    }
    let leaf_aki = leaf_aki.expect("leaf must carry an AKI extension");

    let mut ca_ski: Option<Vec<u8>> = None;
    for ext in ca_x.iter_extensions() {
        if let ParsedExtension::SubjectKeyIdentifier(ski) = ext.parsed_extension() {
            ca_ski = Some(ski.0.to_vec());
        }
    }
    let ca_ski = ca_ski.expect("CA must carry an SKI extension");

    assert_eq!(leaf_aki, ca_ski, "AKI must equal SKI byte-for-byte");

    let spki = ca_x.tbs_certificate.subject_pki.raw;
    let mut h = Sha256::new();
    h.update(spki);
    let digest = h.finalize();
    assert_eq!(
        &ca_ski,
        &digest[..20],
        "SKI must equal Sha256(SPKI)[..20] under KeyIdMethod::Sha256"
    );
}

#[test]
fn leaf_aki_matches_loaded_ca_ski() {
    let ca = CertAuthority::generate("Yerd Local CA", standard_validity()).unwrap();
    let reloaded = CertAuthority::from_pem(ca.cert_pem(), ca.key_pem()).unwrap();
    let (leaf_der, ca_der) = leaf_for(&reloaded, &["foo.test".into()]);
    let leaf = parse_cert(&leaf_der);
    let ca_x = parse_cert(&ca_der);

    let mut leaf_aki: Option<Vec<u8>> = None;
    for ext in leaf.iter_extensions() {
        if let ParsedExtension::AuthorityKeyIdentifier(aki) = ext.parsed_extension() {
            leaf_aki = aki.key_identifier.as_ref().map(|k| k.0.to_vec());
        }
    }
    let leaf_aki = leaf_aki.expect("leaf must carry an AKI extension");

    let mut ca_ski: Option<Vec<u8>> = None;
    for ext in ca_x.iter_extensions() {
        if let ParsedExtension::SubjectKeyIdentifier(ski) = ext.parsed_extension() {
            ca_ski = Some(ski.0.to_vec());
        }
    }
    let ca_ski = ca_ski.expect("CA must carry an SKI extension");

    assert_eq!(
        leaf_aki, ca_ski,
        "leaf AKI must mirror the loaded CA's SKI byte-for-byte"
    );
}

#[test]
fn leaf_signature_verifies_against_ca_public_key() {
    let ca = CertAuthority::generate("Yerd Local CA", standard_validity()).unwrap();
    let (leaf_der, ca_der) = leaf_for(&ca, &["foo.test".into()]);
    let leaf = parse_cert(&leaf_der);
    let ca_x = parse_cert(&ca_der);
    leaf.verify_signature(Some(&ca_x.tbs_certificate.subject_pki))
        .expect("leaf signature must verify against the CA's public key");
}

#[test]
fn ca_self_signed_signature_verifies() {
    let ca = CertAuthority::generate("Yerd Local CA", standard_validity()).unwrap();
    let ca_x = parse_cert(ca.cert_der());
    ca_x.verify_signature(Some(&ca_x.tbs_certificate.subject_pki))
        .expect("CA's self-signature must verify");
}

#[test]
fn serial_numbers_unique_per_leaf_issuance() {
    let ca = CertAuthority::generate("Yerd Local CA", standard_validity()).unwrap();
    let (l1_der, _) = leaf_for(&ca, &["foo.test".into()]);
    let (l2_der, _) = leaf_for(&ca, &["bar.test".into()]);
    let l1 = parse_cert(&l1_der);
    let l2 = parse_cert(&l2_der);
    assert_ne!(
        l1.tbs_certificate.raw_serial(),
        l2.tbs_certificate.raw_serial(),
        "leaves issued in succession must carry distinct serials"
    );
}
