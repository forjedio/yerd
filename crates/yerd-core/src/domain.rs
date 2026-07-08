//! Routable domain patterns attached to a [`Site`](crate::Site).
//!
//! A [`Domain`] is the **sub-part** of a routable host, i.e. everything to the
//! left of the configured TLD. For TLD `test`: `foo`, `api.foo`, `*.foo`,
//! `*.api.foo`. A leftmost `*` label is a **single-label wildcard** - `*.foo`
//! matches exactly one label (`api.foo`), never deeper (`x.api.foo`). Storing the
//! sub-part (not the FQDN) is canonical because the router strips the TLD before
//! matching, and it keeps the value TLD-agnostic.
//!
//! This module also owns the pure "effective set" algebra a site's routable
//! domains are computed with: `implicit_default ± delta` plus the guarantees the
//! router relies on (at least one exact domain; a concrete primary).

use crate::error::{CoreError, DomainErrorReason};

/// RFC 1035's 253-byte cap on a full domain name. Applied both to a stored
/// sub-part and, in [`Domain::parse`], to the whole FQDN (sub-part plus TLD).
const MAX_NAME_BYTES: usize = 253;
/// Maximum single-label length (RFC 1035).
const MAX_LABEL_BYTES: usize = 63;

/// A validated routable domain sub-part (below the global TLD).
///
/// Construct via [`Domain::parse`] (from a full FQDN under a TLD),
/// [`Domain::parse_subpart`] (from a stored sub-part), or [`Domain::apex`] (a
/// site's default exact domain). Always lowercased and ASCII.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Domain {
    sub: String,
}

impl Domain {
    /// The default exact domain for a site: its `name` label. `name` must already
    /// be a validated site label (lowercased); this does not re-validate.
    #[must_use]
    pub fn apex(name: &str) -> Self {
        Self {
            sub: name.to_ascii_lowercase(),
        }
    }

    /// Parse a full FQDN (e.g. `"api.foo.test"`) that must sit under `tld`,
    /// returning the stored sub-part (`"api.foo"`). Lowercases and strips one
    /// trailing dot first. Rejects the bare TLD, any host not under it, and a
    /// whole name over the RFC 1035 253-byte limit. A shape error is reported
    /// against the original FQDN, not the stripped sub-part.
    pub fn parse(fqdn: &str, tld: &str) -> Result<Self, CoreError> {
        let lowered = fqdn.to_ascii_lowercase();
        let trimmed = lowered.strip_suffix('.').unwrap_or(&lowered);
        let tld = tld.to_ascii_lowercase();

        if trimmed.is_empty() {
            return Err(err(fqdn, DomainErrorReason::Empty));
        }
        if trimmed.len() > MAX_NAME_BYTES {
            return Err(err(fqdn, DomainErrorReason::TooLong));
        }
        if trimmed == tld {
            return Err(err(fqdn, DomainErrorReason::NotUnderTld));
        }
        let dotted = format!(".{tld}");
        let sub = trimmed
            .strip_suffix(&dotted)
            .ok_or_else(|| err(fqdn, DomainErrorReason::NotUnderTld))?;

        Self::parse_subpart(sub).map_err(|e| match e {
            CoreError::InvalidDomain { reason, .. } => err(fqdn, reason),
            other => other,
        })
    }

    /// Parse an already-stripped sub-part (`"api.foo"`, `"*.foo"`). Used when
    /// loading persisted config, which stores sub-parts. Lowercases and validates
    /// shape; does not know or check the TLD.
    pub fn parse_subpart(sub: &str) -> Result<Self, CoreError> {
        validate_subpart(sub).map(|s| Self { sub: s })
    }

    /// The stored sub-part (`"api.foo"`, `"*.foo"`).
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.sub
    }

    /// Whether this is a wildcard domain (leftmost label is `*`).
    #[must_use]
    pub fn is_wildcard(&self) -> bool {
        self.sub.starts_with("*.")
    }

    /// The full FQDN under `tld` (`"api.foo.test"`).
    #[must_use]
    pub fn to_fqdn(&self, tld: &str) -> String {
        format!("{}.{}", self.sub, tld)
    }
}

fn err(input: &str, reason: DomainErrorReason) -> CoreError {
    CoreError::InvalidDomain {
        input: input.to_owned(),
        reason,
    }
}

/// Validate and lowercase a domain sub-part in a fixed, pinned order.
fn validate_subpart(raw: &str) -> Result<String, CoreError> {
    validate_steps(raw).map_err(|reason| err(raw, reason))
}

fn validate_steps(raw: &str) -> Result<String, DomainErrorReason> {
    if raw.is_empty() {
        return Err(DomainErrorReason::Empty);
    }
    if raw.bytes().any(|b| !b.is_ascii()) {
        return Err(DomainErrorReason::InvalidCharacter);
    }
    if raw.len() > MAX_NAME_BYTES {
        return Err(DomainErrorReason::TooLong);
    }

    let lowered = raw.to_ascii_lowercase();
    let labels: Vec<&str> = lowered.split('.').collect();
    for (i, label) in labels.iter().enumerate() {
        if label.is_empty() {
            return Err(DomainErrorReason::EmptyLabel);
        }
        if label.contains('*') {
            if *label != "*" {
                return Err(DomainErrorReason::MisplacedWildcard);
            }
            if i != 0 {
                return Err(DomainErrorReason::MisplacedWildcard);
            }
            if labels.len() == 1 {
                return Err(DomainErrorReason::BareWildcard);
            }
            continue;
        }
        validate_label(label)?;
    }
    Ok(lowered)
}

/// One non-wildcard DNS label: `[a-z0-9-]`, 1..=63 bytes, no leading/trailing `-`.
/// Input is already lowercased and ASCII.
fn validate_label(label: &str) -> Result<(), DomainErrorReason> {
    if label.len() > MAX_LABEL_BYTES {
        return Err(DomainErrorReason::LabelTooLong);
    }
    for &b in label.as_bytes() {
        let ok = b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-';
        if !ok {
            return Err(DomainErrorReason::InvalidCharacter);
        }
    }
    if label.starts_with('-') || label.ends_with('-') {
        return Err(DomainErrorReason::LeadingOrTrailingHyphen);
    }
    Ok(())
}

/// Compute a site's **effective** routable domain set from its `name` and the
/// stored delta: `(implicit_default - suppressed) + added`.
///
/// - `implicit_default(name) = { apex }` (the site's own label, exact). There is
///   **no** implicit subdomain catch-all: an uncustomized site answers only its
///   apex.
/// - `added` is appended in order, de-duplicated against what is already present.
/// - Zero-exact normalization: if the result would contain no exact
///   (non-wildcard) domain, the apex is restored so a site is always reachable
///   under at least one concrete host. Callers that want to *observe* a
///   normalization (to warn) can compare against `suppressed`.
///
/// Order is stable: apex first (when present), then `added` in insertion order.
#[must_use]
pub fn effective_domains(name: &str, added: &[Domain], suppressed: &[Domain]) -> Vec<Domain> {
    let apex = Domain::apex(name);
    let mut out: Vec<Domain> = Vec::with_capacity(added.len() + 1);

    if !suppressed.contains(&apex) {
        out.push(apex.clone());
    }
    for d in added {
        if !out.contains(d) {
            out.push(d.clone());
        }
    }

    if !out.iter().any(|d| !d.is_wildcard()) {
        out.insert(0, apex);
    }
    out
}

/// Choose a site's primary (canonical, displayed) domain from its effective set.
///
/// A wildcard is never a primary (producers need a concrete host). Preference:
/// the stored primary if it is exact and still present; else the apex if present;
/// else the first exact domain in the set. `effective` is assumed to contain at
/// least one exact domain (guaranteed by [`effective_domains`]).
#[must_use]
pub fn choose_primary(name: &str, effective: &[Domain], stored: Option<&Domain>) -> Domain {
    if let Some(p) = stored {
        if !p.is_wildcard() && effective.contains(p) {
            return p.clone();
        }
    }
    let apex = Domain::apex(name);
    if effective.contains(&apex) {
        return apex;
    }
    effective
        .iter()
        .find(|d| !d.is_wildcard())
        .cloned()
        .unwrap_or(apex)
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

    fn d(sub: &str) -> Domain {
        Domain::parse_subpart(sub).unwrap()
    }

    #[test]
    fn parse_subpart_accepts_valid() {
        for s in ["foo", "api.foo", "*.foo", "*.api.foo", "a-b.c-d", "x1.y2"] {
            assert!(Domain::parse_subpart(s).is_ok(), "should accept {s:?}");
        }
    }

    #[test]
    fn parse_subpart_lowercases() {
        assert_eq!(
            Domain::parse_subpart("API.Foo").unwrap().as_str(),
            "api.foo"
        );
    }

    #[test]
    fn parse_subpart_rejects_each_reason() {
        use DomainErrorReason::*;
        let cases: &[(&str, DomainErrorReason)] = &[
            ("", Empty),
            (".foo", EmptyLabel),
            ("foo.", EmptyLabel),
            ("a..b", EmptyLabel),
            ("*", BareWildcard),
            ("foo.*", MisplacedWildcard),
            ("*.*.foo", MisplacedWildcard),
            ("a*b.foo", MisplacedWildcard),
            ("*x.foo", MisplacedWildcard),
            ("foo_bar", InvalidCharacter),
            ("fö.foo", InvalidCharacter),
            ("-foo.bar", LeadingOrTrailingHyphen),
            ("foo-.bar", LeadingOrTrailingHyphen),
        ];
        for (input, expected) in cases {
            match Domain::parse_subpart(input) {
                Err(CoreError::InvalidDomain { reason, .. }) => {
                    assert_eq!(reason, *expected, "input {input:?}");
                }
                other => panic!("input {input:?}: expected {expected:?}, got {other:?}"),
            }
        }

        let long_label = format!("{}.foo", "a".repeat(64));
        match Domain::parse_subpart(&long_label) {
            Err(CoreError::InvalidDomain {
                reason: LabelTooLong,
                ..
            }) => {}
            other => panic!("LabelTooLong expected, got {other:?}"),
        }
    }

    #[test]
    fn parse_fqdn_strips_tld_and_trailing_dot() {
        assert_eq!(
            Domain::parse("api.foo.test", "test").unwrap().as_str(),
            "api.foo"
        );
        assert_eq!(
            Domain::parse("API.Foo.Test.", "test").unwrap().as_str(),
            "api.foo"
        );
        assert_eq!(
            Domain::parse("*.foo.test", "test").unwrap().as_str(),
            "*.foo"
        );
    }

    #[test]
    fn parse_fqdn_rejects_wrong_or_bare_tld() {
        for bad in ["foo.example", "test", "test."] {
            match Domain::parse(bad, "test") {
                Err(CoreError::InvalidDomain {
                    reason: DomainErrorReason::NotUnderTld,
                    ..
                }) => {}
                other => panic!("input {bad:?}: expected NotUnderTld, got {other:?}"),
            }
        }
    }

    #[test]
    fn parse_fqdn_supports_multi_label_tld() {
        assert_eq!(
            Domain::parse("api.foo.dev.local", "dev.local")
                .unwrap()
                .as_str(),
            "api.foo"
        );
    }

    #[test]
    fn parse_rejects_fqdn_over_name_cap_even_when_subpart_fits() {
        let label = "a".repeat(63);
        let sub = format!("{label}.{label}.{label}.{}", "b".repeat(58)); // 250 bytes, valid shape
        assert!(Domain::parse_subpart(&sub).is_ok());
        match Domain::parse(&format!("{sub}.test"), "test") {
            Err(CoreError::InvalidDomain {
                reason: DomainErrorReason::TooLong,
                ..
            }) => {}
            other => panic!("expected TooLong for an over-253-byte FQDN, got {other:?}"),
        }
    }

    #[test]
    fn is_wildcard_and_fqdn() {
        assert!(d("*.foo").is_wildcard());
        assert!(!d("api.foo").is_wildcard());
        assert_eq!(d("api.foo").to_fqdn("test"), "api.foo.test");
        assert_eq!(d("*.foo").to_fqdn("test"), "*.foo.test");
    }

    #[test]
    fn effective_default_is_apex_only() {
        let eff = effective_domains("foo", &[], &[]);
        assert_eq!(eff, vec![Domain::apex("foo")]);
    }

    #[test]
    fn effective_adds_in_order_deduped() {
        let added = vec![d("corp"), d("*.foo"), d("corp")];
        let eff = effective_domains("foo", &added, &[]);
        assert_eq!(eff, vec![d("foo"), d("corp"), d("*.foo")]);
    }

    #[test]
    fn effective_suppresses_apex() {
        let eff = effective_domains("foo", &[d("corp")], &[d("foo")]);
        assert_eq!(eff, vec![d("corp")]);
    }

    #[test]
    fn effective_zero_exact_restores_apex() {
        // Apex suppressed and only a wildcard added -> apex restored.
        let eff = effective_domains("foo", &[d("*.foo")], &[d("foo")]);
        assert_eq!(eff, vec![d("foo"), d("*.foo")]);
    }

    #[test]
    fn choose_primary_prefers_stored_exact() {
        let eff = vec![d("foo"), d("corp")];
        assert_eq!(choose_primary("foo", &eff, Some(&d("corp"))), d("corp"));
    }

    #[test]
    fn choose_primary_rejects_wildcard_stored_falls_back_to_apex() {
        let eff = vec![d("foo"), d("*.foo")];
        assert_eq!(choose_primary("foo", &eff, Some(&d("*.foo"))), d("foo"));
    }

    #[test]
    fn choose_primary_falls_back_to_first_exact_when_apex_absent() {
        let eff = vec![d("*.foo"), d("corp"), d("shop")];
        assert_eq!(choose_primary("foo", &eff, None), d("corp"));
    }
}
