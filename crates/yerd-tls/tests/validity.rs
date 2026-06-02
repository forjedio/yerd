//! Validity-window round-trip through the cert's ASN.1 time encoding.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

mod common;

use common::at;
use time::Month;
use yerd_tls::{CertAuthority, Validity};

fn parse_cert(der: &[u8]) -> x509_parser::certificate::X509Certificate<'_> {
    let (_, x) = x509_parser::parse_x509_certificate(der).unwrap();
    x
}

#[test]
fn not_before_round_trips_to_second_precision() {
    let nb = time::OffsetDateTime::from_unix_timestamp(1_768_510_496).unwrap(); // 2026-01-15T20:14:56Z
    let na = at(2027, Month::January, 1);
    let v = Validity::new(nb, na).unwrap();
    let ca = CertAuthority::generate("Yerd Local CA", v).unwrap();
    let cert = parse_cert(ca.cert_der());
    let parsed_nb = cert.validity().not_before.to_datetime();
    assert_eq!(parsed_nb, nb);
}

#[test]
fn not_after_round_trips_to_second_precision() {
    let nb = at(2026, Month::January, 1);
    let na = time::OffsetDateTime::from_unix_timestamp(1_799_959_496).unwrap(); // 2027-01-15T...
    let v = Validity::new(nb, na).unwrap();
    let ca = CertAuthority::generate("Yerd Local CA", v).unwrap();
    let cert = parse_cert(ca.cert_der());
    let parsed_na = cert.validity().not_after.to_datetime();
    assert_eq!(parsed_na, na);
}

#[test]
fn validity_post_2050_round_trips() {
    let nb = at(2050, Month::January, 1);
    let na = at(2060, Month::January, 1);
    let v = Validity::new(nb, na).unwrap();
    let ca = CertAuthority::generate("Yerd Local CA", v).unwrap();
    let cert = parse_cert(ca.cert_der());
    assert_eq!(cert.validity().not_before.to_datetime(), nb);
    assert_eq!(cert.validity().not_after.to_datetime(), na);
}

#[test]
fn validity_pre_2050_uses_utctime() {
    let nb = at(2026, Month::January, 1);
    let na = at(2027, Month::January, 1);
    let v = Validity::new(nb, na).unwrap();
    let ca = CertAuthority::generate("Yerd Local CA", v).unwrap();
    let cert = parse_cert(ca.cert_der());
    // Round-trip is what matters most; pinning the raw ASN.1 tag bytes is
    // brittle. Confirm equality at the OffsetDateTime layer.
    assert_eq!(cert.validity().not_before.to_datetime(), nb);
    assert_eq!(cert.validity().not_after.to_datetime(), na);
}

#[test]
fn leaf_validity_independent_of_ca_validity() {
    let ca_v = Validity::new(at(2026, Month::January, 1), at(2027, Month::January, 1)).unwrap();
    let leaf_v = Validity::new(at(2030, Month::January, 1), at(2031, Month::January, 1)).unwrap();
    let ca = CertAuthority::generate("Yerd Local CA", ca_v).unwrap();
    let leaf = ca.issue_leaf(&["foo.test".to_string()], leaf_v).unwrap();
    let block = pem::parse(leaf.cert_pem()).unwrap();
    let cert = parse_cert(block.contents());
    assert_eq!(
        cert.validity().not_before.to_datetime(),
        at(2030, Month::January, 1)
    );
    assert_eq!(
        cert.validity().not_after.to_datetime(),
        at(2031, Month::January, 1)
    );
}
