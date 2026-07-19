//! `rcgen::CertificateParams` builders for CA and leaf certificates.
//!
//! Kept private to the crate. These functions own:
//!
//! - common-name length + emptiness validation (CA path),
//! - SAN slice non-emptiness + per-entry `IA5String` validation (leaf path),
//! - the AKI toggle on leaves (without it rcgen does not emit the extension,
//!   `rcgen-0.13.2/src/certificate.rs:680–704`).

use rcgen::{
    BasicConstraints, CertificateParams, DnType, ExtendedKeyUsagePurpose, Ia5String, IsCa,
    KeyUsagePurpose, SanType,
};

use crate::error::{GenerateErrorReason, TlsError};
use crate::validity::Validity;

/// Upper bound on the `commonName` `AttributeValue` per RFC 5280 §A.1
/// (`ub-common-name` = 64).
pub(crate) const CN_MAX_BYTES: usize = 64;

/// Build `CertificateParams` for a self-signed CA.
pub(crate) fn ca_params(
    common_name: &str,
    validity: Validity,
) -> Result<CertificateParams, TlsError> {
    if common_name.is_empty() {
        return Err(TlsError::Generate {
            reason: GenerateErrorReason::EmptyCommonName,
        });
    }
    if common_name.len() > CN_MAX_BYTES {
        return Err(TlsError::Generate {
            reason: GenerateErrorReason::CommonNameTooLong { max: CN_MAX_BYTES },
        });
    }

    let mut params = CertificateParams::default();
    params.not_before = validity.not_before();
    params.not_after = validity.not_after();
    params.is_ca = IsCa::Ca(BasicConstraints::Constrained(0));
    params.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign];
    params.extended_key_usages = Vec::new();
    params.subject_alt_names = Vec::new();
    params.use_authority_key_identifier_extension = false;
    params.distinguished_name = {
        let mut dn = rcgen::DistinguishedName::new();
        dn.push(DnType::CommonName, common_name);
        dn
    };

    Ok(params)
}

/// Build `CertificateParams` for a leaf signed by a CA.
///
/// Each name that parses as an IP literal becomes an `iPAddress` SAN; everything
/// else becomes a `dNSName`. A TLS client connecting to a raw IP (e.g. the LAN
/// bootstrap endpoint, which the device reaches before it can resolve `.test`)
/// matches only `iPAddress` SANs, never a `dNSName` carrying the address text.
pub(crate) fn leaf_params(
    names: &[String],
    validity: Validity,
) -> Result<CertificateParams, TlsError> {
    if names.is_empty() {
        return Err(TlsError::Generate {
            reason: GenerateErrorReason::EmptyNameSet,
        });
    }

    let mut sans = Vec::with_capacity(names.len());
    for (index, name) in names.iter().enumerate() {
        if let Ok(ip) = name.parse::<std::net::IpAddr>() {
            sans.push(SanType::IpAddress(ip));
            continue;
        }
        let ia5 = Ia5String::try_from(name.as_str()).map_err(|_| TlsError::Generate {
            reason: GenerateErrorReason::InvalidDnsName { index },
        })?;
        sans.push(SanType::DnsName(ia5));
    }

    let mut params = CertificateParams::default();
    params.not_before = validity.not_before();
    params.not_after = validity.not_after();
    params.is_ca = IsCa::ExplicitNoCa;
    params.subject_alt_names = sans;
    params.key_usages = vec![
        KeyUsagePurpose::DigitalSignature,
        KeyUsagePurpose::KeyEncipherment,
    ];
    params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
    params.use_authority_key_identifier_extension = true;
    // Set a CN to the first name. macOS Security (Chrome/Safari) rejects an
    // empty-subject leaf (RFC 5280 4.1.2.6 wants a critical SAN, but rcgen
    // emits it non-critical), so a non-empty CN sidesteps it. Names over
    // 64 bytes (ub-common-name) fall back to an empty subject.
    params.distinguished_name = {
        let mut dn = rcgen::DistinguishedName::new();
        if let Some(cn) = names.first() {
            if !cn.is_empty() && cn.len() <= CN_MAX_BYTES {
                dn.push(DnType::CommonName, cn.as_str());
            }
        }
        dn
    };

    Ok(params)
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
    use crate::error::GenerateErrorReason;

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
    fn ca_params_rejects_empty_cn() {
        let err = ca_params("", v()).unwrap_err();
        match err {
            TlsError::Generate { reason } => {
                assert_eq!(reason, GenerateErrorReason::EmptyCommonName);
            }
            other => panic!("expected Generate error, got {other:?}"),
        }
    }

    #[test]
    fn ca_params_rejects_cn_over_64() {
        let cn = "a".repeat(65);
        let err = ca_params(&cn, v()).unwrap_err();
        match err {
            TlsError::Generate { reason } => {
                assert_eq!(
                    reason,
                    GenerateErrorReason::CommonNameTooLong { max: CN_MAX_BYTES }
                );
            }
            other => panic!("expected Generate error, got {other:?}"),
        }
    }

    #[test]
    fn ca_params_accepts_cn_at_64() {
        let cn = "a".repeat(64);
        let p = ca_params(&cn, v()).unwrap();
        assert!(matches!(
            p.is_ca,
            IsCa::Ca(BasicConstraints::Constrained(0))
        ));
    }

    #[test]
    fn ca_params_sets_ca_constraint() {
        let p = ca_params("Yerd Local CA", v()).unwrap();
        assert!(matches!(
            p.is_ca,
            IsCa::Ca(BasicConstraints::Constrained(0))
        ));
        assert!(p.key_usages.contains(&KeyUsagePurpose::KeyCertSign));
        assert!(p.key_usages.contains(&KeyUsagePurpose::CrlSign));
    }

    #[test]
    fn leaf_params_rejects_empty_names() {
        let err = leaf_params(&[], v()).unwrap_err();
        match err {
            TlsError::Generate { reason } => {
                assert_eq!(reason, GenerateErrorReason::EmptyNameSet);
            }
            other => panic!("expected Generate error, got {other:?}"),
        }
    }

    #[test]
    fn leaf_params_rejects_non_ia5_with_index() {
        let names = vec!["ok.test".to_string(), "f\u{00f6}\u{00f6}.test".to_string()];
        let err = leaf_params(&names, v()).unwrap_err();
        match err {
            TlsError::Generate { reason } => {
                assert_eq!(reason, GenerateErrorReason::InvalidDnsName { index: 1 });
            }
            other => panic!("expected Generate error, got {other:?}"),
        }
    }

    #[test]
    fn leaf_params_sets_no_ca() {
        let names = vec!["foo.test".to_string(), "*.foo.test".to_string()];
        let p = leaf_params(&names, v()).unwrap();
        assert!(matches!(p.is_ca, IsCa::ExplicitNoCa));
        assert!(p.key_usages.contains(&KeyUsagePurpose::DigitalSignature));
        assert!(p.key_usages.contains(&KeyUsagePurpose::KeyEncipherment));
        assert_eq!(
            p.extended_key_usages,
            vec![ExtendedKeyUsagePurpose::ServerAuth]
        );
        assert_eq!(p.subject_alt_names.len(), 2);
    }

    #[test]
    fn leaf_params_enables_aki() {
        let names = vec!["foo.test".to_string()];
        let p = leaf_params(&names, v()).unwrap();
        assert!(
            p.use_authority_key_identifier_extension,
            "AKI toggle must be true on leaves; without it rcgen does not emit the extension"
        );
    }

    #[test]
    fn leaf_params_emits_ip_san_for_ip_literal() {
        let names = vec!["192.168.1.42".to_string()];
        let p = leaf_params(&names, v()).unwrap();
        assert_eq!(p.subject_alt_names.len(), 1);
        match &p.subject_alt_names[0] {
            SanType::IpAddress(ip) => {
                assert_eq!(*ip, "192.168.1.42".parse::<std::net::IpAddr>().unwrap());
            }
            other => panic!("expected IpAddress SAN, got {other:?}"),
        }
    }

    #[test]
    fn leaf_params_keeps_dns_san_for_hostname() {
        let names = vec!["app.test".to_string()];
        let p = leaf_params(&names, v()).unwrap();
        assert!(matches!(p.subject_alt_names[0], SanType::DnsName(_)));
    }
}
