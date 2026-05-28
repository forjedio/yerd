//! Subject Alt Name + extension content invariants.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

mod common;

use common::standard_validity;
use x509_parser::extensions::{GeneralName, ParsedExtension};
use yerd_tls::CertAuthority;

fn parse_cert(der: &[u8]) -> x509_parser::certificate::X509Certificate<'_> {
    let (_, x) = x509_parser::parse_x509_certificate(der).unwrap();
    x
}

#[test]
fn every_passed_name_appears_in_san() {
    let ca = CertAuthority::generate("Yerd Local CA", standard_validity()).unwrap();
    let names = vec![
        "foo.test".to_string(),
        "*.foo.test".to_string(),
        "alt.local".to_string(),
    ];
    let leaf = ca.issue_leaf(&names, standard_validity()).unwrap();
    let block = pem::parse(leaf.cert_pem()).unwrap();
    let cert = parse_cert(block.contents());

    let mut sans: Vec<String> = Vec::new();
    for ext in cert.iter_extensions() {
        if let ParsedExtension::SubjectAlternativeName(san) = ext.parsed_extension() {
            for gn in &san.general_names {
                if let GeneralName::DNSName(s) = gn {
                    sans.push((*s).to_string());
                }
            }
        }
    }

    assert_eq!(sans.len(), names.len(), "SAN count mismatch");
    for n in &names {
        assert!(sans.contains(n), "missing SAN: {n}");
    }
}

#[test]
fn wildcard_san_preserved_as_dnsname() {
    let ca = CertAuthority::generate("Yerd Local CA", standard_validity()).unwrap();
    let names = vec!["*.foo.test".to_string()];
    let leaf = ca.issue_leaf(&names, standard_validity()).unwrap();
    let block = pem::parse(leaf.cert_pem()).unwrap();
    let cert = parse_cert(block.contents());

    let mut found_wildcard = false;
    for ext in cert.iter_extensions() {
        if let ParsedExtension::SubjectAlternativeName(san) = ext.parsed_extension() {
            for gn in &san.general_names {
                if matches!(gn, GeneralName::DNSName("*.foo.test")) {
                    found_wildcard = true;
                }
            }
        }
    }
    assert!(found_wildcard, "wildcard SAN must round-trip as DNSName");
}

#[test]
fn ca_basic_constraints_ca_true_pathlen_zero() {
    let ca = CertAuthority::generate("Yerd Local CA", standard_validity()).unwrap();
    let cert = parse_cert(ca.cert_der());
    let bc = cert
        .basic_constraints()
        .unwrap()
        .expect("CA must carry BasicConstraints");
    assert!(bc.value.ca, "ca:TRUE required on CA");
    assert_eq!(bc.value.path_len_constraint, Some(0));
}

#[test]
fn ca_key_usage_cert_sign_and_crl_sign() {
    let ca = CertAuthority::generate("Yerd Local CA", standard_validity()).unwrap();
    let cert = parse_cert(ca.cert_der());
    let ku = cert.key_usage().unwrap().expect("CA must carry KeyUsage");
    assert!(ku.value.key_cert_sign(), "keyCertSign required");
    assert!(ku.value.crl_sign(), "cRLSign required");
}

#[test]
fn leaf_basic_constraints_ca_false() {
    let ca = CertAuthority::generate("Yerd Local CA", standard_validity()).unwrap();
    let leaf = ca
        .issue_leaf(&["foo.test".to_string()], standard_validity())
        .unwrap();
    let block = pem::parse(leaf.cert_pem()).unwrap();
    let cert = parse_cert(block.contents());
    let bc = cert
        .basic_constraints()
        .unwrap()
        .expect("leaf must carry BasicConstraints (ExplicitNoCa)");
    assert!(!bc.value.ca, "ca:FALSE required on leaves");
}

#[test]
fn leaf_extended_key_usage_server_auth_only() {
    let ca = CertAuthority::generate("Yerd Local CA", standard_validity()).unwrap();
    let leaf = ca
        .issue_leaf(&["foo.test".to_string()], standard_validity())
        .unwrap();
    let block = pem::parse(leaf.cert_pem()).unwrap();
    let cert = parse_cert(block.contents());
    let eku = cert
        .extended_key_usage()
        .unwrap()
        .expect("leaf must carry EKU");
    assert!(eku.value.server_auth, "serverAuth required");
    assert!(!eku.value.client_auth, "clientAuth must not leak in");
    assert!(!eku.value.code_signing, "codeSigning must not leak in");
    assert!(
        !eku.value.email_protection,
        "emailProtection must not leak in"
    );
}

#[test]
fn ca_has_no_san_extension() {
    let ca = CertAuthority::generate("Yerd Local CA", standard_validity()).unwrap();
    let cert = parse_cert(ca.cert_der());
    for ext in cert.iter_extensions() {
        assert!(
            !matches!(
                ext.parsed_extension(),
                ParsedExtension::SubjectAlternativeName(_)
            ),
            "CA must not carry a SAN extension"
        );
    }
}
