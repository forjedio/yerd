//! Per-site leaf certificate: PEM cert + PEM key + a chain helper.

/// A signed leaf certificate plus its private key, as PEM strings.
///
/// Construction is via [`crate::CertAuthority::issue_leaf`]. The struct is
/// `#[non_exhaustive]` so future fields can be added without breaking `SemVer`.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct LeafCert {
    cert_pem: String,
    key_pem: String,
}

impl LeafCert {
    /// Construct from owned PEM strings. `pub(crate)` because user code
    /// must always go through `CertAuthority::issue_leaf`.
    pub(crate) fn new(cert_pem: String, key_pem: String) -> Self {
        Self { cert_pem, key_pem }
    }

    /// Hand-rolled constructor for inline unit tests only. Hidden behind
    /// `#[cfg(test)]`. The `#[cfg(test)]` gate (not the visibility keyword)
    /// is what hides this from integration tests — they compile against the
    /// library built without `--test`, so the item is absent from their view.
    #[cfg(test)]
    fn from_parts(cert_pem: String, key_pem: String) -> Self {
        Self { cert_pem, key_pem }
    }

    /// The leaf certificate, PEM-encoded.
    #[must_use]
    pub fn cert_pem(&self) -> &str {
        &self.cert_pem
    }

    /// The leaf's private key, PEM-encoded.
    #[must_use]
    pub fn key_pem(&self) -> &str {
        &self.key_pem
    }

    /// Returns `format!("{leaf_pem}\n{ca_cert_pem}")`. Always inserts a
    /// single `\n` between the two; accepted by `rustls`'s and the `pem`
    /// crate's parsers (RFC 7468 §3 permits but does not mandate whitespace
    /// tolerance between blocks; strict downstream parsers may differ).
    ///
    /// Does not validate `ca_cert_pem`. Callers are responsible for passing a
    /// well-formed PEM block.
    #[must_use]
    pub fn chain_pem(&self, ca_cert_pem: &str) -> String {
        format!("{}\n{}", self.cert_pem, ca_cert_pem)
    }
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod tests {
    use super::*;

    #[test]
    fn accessors_return_inputs() {
        let leaf = LeafCert::from_parts("CERT".to_string(), "KEY".to_string());
        assert_eq!(leaf.cert_pem(), "CERT");
        assert_eq!(leaf.key_pem(), "KEY");
    }

    #[test]
    fn chain_pem_equals_format_leaf_newline_ca() {
        let leaf = LeafCert::from_parts("LEAF".to_string(), "K".to_string());
        let chain = leaf.chain_pem("CA");
        assert_eq!(chain, "LEAF\nCA");
    }

    #[test]
    fn chain_pem_passes_through_arbitrary_ca_string() {
        // Pins the no-validation invariant. A future contributor adding
        // "smart" validation breaks this test.
        let leaf = LeafCert::from_parts("LEAF".to_string(), "K".to_string());
        let chain = leaf.chain_pem("not even pem");
        assert_eq!(chain, "LEAF\nnot even pem");
    }
}
