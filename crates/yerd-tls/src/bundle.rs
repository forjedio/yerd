//! CA-bundle composition for the bundled PHP trust store.
//!
//! The bundled static PHP verifies TLS against an OpenSSL CA file, not the OS
//! keychain, so Yerd points it at a managed bundle of the host's public roots
//! plus the Yerd CA. [`compose_ca_bundle`] builds that file's contents.
//!
//! This is a **non-validating byte concatenation**: it does not parse either
//! input, check PEM structure, or deduplicate. Callers decide whether the roots
//! are usable (e.g. by requiring at least one `CERTIFICATE` block) before
//! pointing PHP at the result.

/// Concatenate the host's public roots with the Yerd CA into one PEM bundle.
///
/// Guarantees exactly one `\n` separating the two inputs and a single trailing
/// newline, tolerating a missing trailing newline on either side.
/// `system_roots_pem` may be empty (yielding just the CA, newline-terminated).
/// The CA always appears last. No PEM validation is performed; see the module
/// docs.
#[must_use]
pub fn compose_ca_bundle(system_roots_pem: &str, ca_cert_pem: &str) -> String {
    let roots = system_roots_pem.trim_end_matches(['\r', '\n']);
    let ca = ca_cert_pem.trim_end_matches(['\r', '\n']);
    let mut out = String::with_capacity(roots.len() + ca.len() + 2);
    if !roots.is_empty() {
        out.push_str(roots);
        out.push('\n');
    }
    out.push_str(ca);
    out.push('\n');
    out
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    const CA: &str = "-----BEGIN CERTIFICATE-----\nCA\n-----END CERTIFICATE-----";
    const ROOTS: &str = "-----BEGIN CERTIFICATE-----\nROOT\n-----END CERTIFICATE-----";

    #[test]
    fn concatenates_roots_then_ca_with_single_separator() {
        let out = compose_ca_bundle(ROOTS, CA);
        assert_eq!(out, format!("{ROOTS}\n{CA}\n"));
        assert!(out.ends_with("END CERTIFICATE-----\n"));
        assert_eq!(out.matches("BEGIN CERTIFICATE").count(), 2);
    }

    #[test]
    fn ca_appears_last() {
        let out = compose_ca_bundle(ROOTS, CA);
        let ca_pos = out.find("CA\n").unwrap();
        let root_pos = out.find("ROOT\n").unwrap();
        assert!(root_pos < ca_pos);
    }

    #[test]
    fn normalises_trailing_newlines_on_both_inputs() {
        let out = compose_ca_bundle(&format!("{ROOTS}\n\n"), &format!("{CA}\r\n"));
        assert_eq!(out, format!("{ROOTS}\n{CA}\n"));
    }

    #[test]
    fn empty_roots_yields_ca_only() {
        let out = compose_ca_bundle("", CA);
        assert_eq!(out, format!("{CA}\n"));
    }

    #[test]
    fn whitespace_only_roots_treated_as_empty() {
        let out = compose_ca_bundle("\n\n", CA);
        assert_eq!(out, format!("{CA}\n"));
    }
}
