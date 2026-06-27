//! Read identifying fields out of a DER-encoded X.509 certificate.
//!
//! Used by the privileged `yerd-helper` to confirm a certificate it is about to
//! remove from the system trust store is yerd's own CA (Subject CN ==
//! [`yerd_core::CA_COMMON_NAME`]) before deleting it - so a bad fingerprint
//! handed to the helper can never delete an unrelated trusted root.

/// The Subject Common Name of a DER-encoded X.509 certificate, if present.
///
/// Returns `None` when the DER can't be parsed or carries no CN - callers
/// treat that as "not confirmably yerd's" and refuse to delete. Never panics:
/// every fallible step degrades to `None` (this runs as root).
#[must_use]
pub fn subject_common_name(cert_der: &[u8]) -> Option<String> {
    let (_, cert) = x509_parser::parse_x509_certificate(cert_der).ok()?;
    let cn = cert
        .subject()
        .iter_common_name()
        .next()
        .and_then(|cn| cn.as_str().ok())
        .map(ToOwned::to_owned);
    cn
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    /// Mint a CA with `cn` via yerd-tls and return its DER body.
    fn ca_der(cn: &str) -> Vec<u8> {
        let nb = time::OffsetDateTime::UNIX_EPOCH;
        let na = nb + time::Duration::days(365);
        let validity = yerd_tls::Validity::new(nb, na).unwrap();
        let ca = yerd_tls::CertAuthority::generate(cn, validity).unwrap();
        pem::parse(ca.cert_pem().as_bytes())
            .unwrap()
            .into_contents()
    }

    #[test]
    fn reads_the_yerd_ca_common_name() {
        let der = ca_der(yerd_core::CA_COMMON_NAME);
        assert_eq!(
            subject_common_name(&der).as_deref(),
            Some(yerd_core::CA_COMMON_NAME)
        );
    }

    #[test]
    fn reads_a_different_common_name() {
        let der = ca_der("Some Other CA");
        assert_eq!(subject_common_name(&der).as_deref(), Some("Some Other CA"));
    }

    #[test]
    fn none_for_non_certificate_bytes() {
        assert_eq!(subject_common_name(b"not a der certificate"), None);
    }
}
