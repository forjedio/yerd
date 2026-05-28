//! Site type and kind.
//!
//! A [`Site`] is a routable target with a validated DNS-label `name`, a
//! `document_root`, a [`PhpVersion`](crate::PhpVersion), an HTTPS flag, and
//! a [`SiteKind`]. Fields are private to enforce the name invariant; mutation
//! goes through typed setters (no `set_name` — renaming is a router-level
//! operation).

use std::path::{Path, PathBuf};

use crate::error::{CoreError, SiteNameErrorReason};
use crate::php::PhpVersion;

/// Whether a site is `Parked` (auto-discovered under a parked directory) or
/// `Linked` (explicitly registered).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SiteKind {
    /// Auto-discovered under a parked directory.
    Parked,
    /// Explicitly registered.
    Linked,
}

/// A routable site.
///
/// `name` is a DNS-safe label (`[a-z0-9-]`, length 1–63, no leading/trailing
/// `-`). It is validated and lowercased at construction. It is **immutable**
/// after that: there is no `set_name` method, because the name doubles as the
/// router's lookup key. To rename a site, remove it from the router and
/// reinsert with a fresh `Site`.
///
/// `document_root` is **not** validated by `yerd-core` — this is a pure crate.
/// It may be empty, relative, or non-canonical. Path semantics, existence, and
/// platform normalisation are owned by `yerd-config` (load time) and
/// `yerd-platform` (runtime). Round-trip through `serde` uses `PathBuf`'s
/// default string representation, which is lossy for paths that cannot be
/// encoded as UTF-8 (notably Windows paths containing unpaired surrogates from
/// WTF-16). Callers needing a guaranteed-UTF-8 path should normalise upstream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Site {
    name: String,
    document_root: PathBuf,
    php: PhpVersion,
    secure: bool,
    kind: SiteKind,
}

impl Site {
    /// Constructs a parked site. **Initialises `secure = false`** — promote
    /// via [`Self::set_secure`].
    pub fn parked(
        name: &str,
        document_root: impl Into<PathBuf>,
        php: PhpVersion,
    ) -> Result<Self, CoreError> {
        let name = validate_and_lowercase_name(name)?;
        Ok(Self {
            name,
            document_root: document_root.into(),
            php,
            secure: false,
            kind: SiteKind::Parked,
        })
    }

    /// Constructs a linked site. **Initialises `secure = false`** — promote
    /// via [`Self::set_secure`].
    pub fn linked(
        name: &str,
        document_root: impl Into<PathBuf>,
        php: PhpVersion,
    ) -> Result<Self, CoreError> {
        let name = validate_and_lowercase_name(name)?;
        Ok(Self {
            name,
            document_root: document_root.into(),
            php,
            secure: false,
            kind: SiteKind::Linked,
        })
    }

    /// The validated, lowercased DNS-label name.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The document root (unvalidated — see type-level docs).
    #[must_use]
    pub fn document_root(&self) -> &Path {
        &self.document_root
    }

    /// The PHP version this site is served under.
    #[must_use]
    pub fn php(&self) -> PhpVersion {
        self.php
    }

    /// Whether the site is served over HTTPS.
    #[must_use]
    pub fn secure(&self) -> bool {
        self.secure
    }

    /// Whether the site is `Parked` or `Linked`.
    #[must_use]
    pub fn kind(&self) -> SiteKind {
        self.kind
    }

    /// Replaces the document root. Not validated — see type-level docs.
    pub fn set_document_root(&mut self, p: impl Into<PathBuf>) {
        self.document_root = p.into();
    }

    /// Replaces the PHP version.
    pub fn set_php(&mut self, v: PhpVersion) {
        self.php = v;
    }

    /// Toggles the HTTPS flag.
    pub fn set_secure(&mut self, secure: bool) {
        self.secure = secure;
    }

    /// Replaces the kind.
    pub fn set_kind(&mut self, k: SiteKind) {
        self.kind = k;
    }
}

/// Pinned ordered algorithm — see plan §2.4.
fn validate_and_lowercase_name(raw: &str) -> Result<String, CoreError> {
    // 1.
    if raw.is_empty() {
        return Err(err(raw, SiteNameErrorReason::Empty));
    }

    // 2. dot rejection (sites are single DNS labels)
    if raw.contains('.') {
        return Err(err(raw, SiteNameErrorReason::ContainsDot));
    }

    // 3. ASCII alphanumeric ∪ '-' only (rejects whitespace, '_', ':', '/', '\\',
    //    '+', '@', etc., and non-ASCII)
    for &b in raw.as_bytes() {
        if !b.is_ascii() {
            return Err(err(raw, SiteNameErrorReason::InvalidCharacter));
        }
        let ok = b.is_ascii_alphanumeric() || b == b'-';
        if !ok {
            return Err(err(raw, SiteNameErrorReason::InvalidCharacter));
        }
    }

    // 4. lowercase
    let lowered = raw.to_ascii_lowercase();

    // 5. leading/trailing hyphen
    if lowered.starts_with('-') || lowered.ends_with('-') {
        return Err(err(raw, SiteNameErrorReason::LeadingOrTrailingHyphen));
    }

    // 6. length cap (RFC 1035 single label). Byte length equals char length
    //    because non-ASCII is rejected at step 3.
    if lowered.len() > 63 {
        return Err(err(raw, SiteNameErrorReason::LabelTooLong));
    }

    Ok(lowered)
}

fn err(name: &str, reason: SiteNameErrorReason) -> CoreError {
    CoreError::InvalidSiteName {
        name: name.to_owned(),
        reason,
    }
}

impl serde::Serialize for Site {
    fn serialize<S: serde::Serializer>(&self, ser: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
        let mut s = ser.serialize_struct("Site", 5)?;
        s.serialize_field("name", &self.name)?;
        s.serialize_field("document_root", &self.document_root)?;
        s.serialize_field("php", &self.php)?;
        s.serialize_field("secure", &self.secure)?;
        s.serialize_field("kind", &self.kind)?;
        s.end()
    }
}

impl<'de> serde::Deserialize<'de> for Site {
    fn deserialize<D>(de: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(serde::Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Wire {
            name: String,
            document_root: PathBuf,
            php: PhpVersion,
            secure: bool,
            kind: SiteKind,
        }
        let w = Wire::deserialize(de)?;
        let name = validate_and_lowercase_name(&w.name).map_err(serde::de::Error::custom)?;
        Ok(Self {
            name,
            document_root: w.document_root,
            php: w.php,
            secure: w.secure,
            kind: w.kind,
        })
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
    use serde_test::{assert_tokens, Token};

    fn v83() -> PhpVersion {
        PhpVersion::new(8, 3)
    }

    #[test]
    fn parked_constructs() {
        let s = Site::parked("foo", "/srv/foo", v83()).unwrap();
        assert_eq!(s.name(), "foo");
        assert_eq!(s.document_root(), Path::new("/srv/foo"));
        assert_eq!(s.php(), v83());
        assert_eq!(s.kind(), SiteKind::Parked);
    }

    #[test]
    fn linked_constructs() {
        let s = Site::linked("bar", "/srv/bar", v83()).unwrap();
        assert_eq!(s.name(), "bar");
        assert_eq!(s.kind(), SiteKind::Linked);
    }

    #[test]
    fn constructor_defaults_secure_false() {
        assert!(!Site::parked("foo", "/x", v83()).unwrap().secure());
        assert!(!Site::linked("foo", "/x", v83()).unwrap().secure());
    }

    #[test]
    fn name_is_lowercased() {
        let s = Site::parked("FooBar", "/x", v83()).unwrap();
        assert_eq!(s.name(), "foobar");
    }

    #[test]
    fn name_rejects_each_reason() {
        use SiteNameErrorReason::*;
        let cases: &[(&str, SiteNameErrorReason)] = &[
            ("", Empty),
            ("foo.bar", ContainsDot),
            // InvalidCharacter — every ASCII whitespace + ASCII non-[a-z0-9-] + non-ASCII
            ("foo bar", InvalidCharacter),
            ("foo\tbar", InvalidCharacter),
            ("foo\nbar", InvalidCharacter),
            ("foo\rbar", InvalidCharacter),
            ("foo\x0Bbar", InvalidCharacter),
            ("foo\x0Cbar", InvalidCharacter),
            ("foo_bar", InvalidCharacter),
            ("foo:bar", InvalidCharacter),
            ("foo/bar", InvalidCharacter),
            ("foo\\bar", InvalidCharacter),
            ("fü", InvalidCharacter),
            // LeadingOrTrailingHyphen
            ("-foo", LeadingOrTrailingHyphen),
            ("foo-", LeadingOrTrailingHyphen),
        ];
        for (input, expected) in cases {
            match Site::parked(input, "/x", v83()) {
                Err(CoreError::InvalidSiteName { reason, .. }) => {
                    assert_eq!(reason, *expected, "input {input:?}");
                }
                other => panic!("input {input:?}: expected {expected:?}, got {other:?}"),
            }
        }

        // Length-based cases
        let long_name = "a".repeat(64);
        match Site::parked(&long_name, "/x", v83()) {
            Err(CoreError::InvalidSiteName {
                reason: LabelTooLong,
                ..
            }) => {}
            other => panic!("LabelTooLong expected, got {other:?}"),
        }
        let dashes64 = "-".repeat(64);
        match Site::parked(&dashes64, "/x", v83()) {
            // step 5 (hyphen) beats step 6 (length)
            Err(CoreError::InvalidSiteName {
                reason: LeadingOrTrailingHyphen,
                ..
            }) => {}
            other => panic!("LeadingOrTrailingHyphen expected, got {other:?}"),
        }
    }

    #[test]
    fn name_ordering_pin() {
        // step 2 (ContainsDot) beats step 6 (LabelTooLong)
        let long_dotted = format!("{}.", "a".repeat(64));
        match Site::parked(&long_dotted, "/x", v83()) {
            Err(CoreError::InvalidSiteName {
                reason: SiteNameErrorReason::ContainsDot,
                ..
            }) => {}
            other => panic!("ContainsDot expected, got {other:?}"),
        }
        // step 2 (ContainsDot) beats step 3 (InvalidCharacter for non-ASCII)
        match Site::parked("fü.bar", "/x", v83()) {
            Err(CoreError::InvalidSiteName {
                reason: SiteNameErrorReason::ContainsDot,
                ..
            }) => {}
            other => panic!("ContainsDot expected, got {other:?}"),
        }
    }

    #[test]
    fn accessors_return_expected_values() {
        let s = Site::linked("foo", "/srv/foo", PhpVersion::new(7, 4)).unwrap();
        assert_eq!(s.name(), "foo");
        assert_eq!(s.document_root(), Path::new("/srv/foo"));
        assert_eq!(s.php(), PhpVersion::new(7, 4));
        assert!(!s.secure());
        assert_eq!(s.kind(), SiteKind::Linked);
    }

    #[test]
    fn setters_mutate_only_intended_field() {
        let mut s = Site::parked("foo", "/srv/foo", v83()).unwrap();

        s.set_document_root("/srv/new");
        assert_eq!(s.document_root(), Path::new("/srv/new"));
        assert_eq!(s.name(), "foo");

        s.set_php(PhpVersion::new(8, 4));
        assert_eq!(s.php(), PhpVersion::new(8, 4));
        assert_eq!(s.document_root(), Path::new("/srv/new"));

        s.set_secure(true);
        assert!(s.secure());

        s.set_kind(SiteKind::Linked);
        assert_eq!(s.kind(), SiteKind::Linked);

        // Name unchanged through it all.
        assert_eq!(s.name(), "foo");
    }

    #[test]
    fn serde_wire_shape_sitekind() {
        assert_tokens(
            &SiteKind::Parked,
            &[Token::UnitVariant {
                name: "SiteKind",
                variant: "parked",
            }],
        );
        assert_tokens(
            &SiteKind::Linked,
            &[Token::UnitVariant {
                name: "SiteKind",
                variant: "linked",
            }],
        );
    }

    #[test]
    fn serde_full_site_roundtrip() {
        let s = Site::parked("foo", "/srv/foo", v83()).unwrap();
        let v = serde_json::to_value(&s).unwrap();
        // Field names and `php` rendered as the string "8.3"
        assert_eq!(v["name"], "foo");
        assert_eq!(v["document_root"], "/srv/foo");
        assert_eq!(v["php"], "8.3");
        assert_eq!(v["secure"], false);
        assert_eq!(v["kind"], "parked");

        let back: Site = serde_json::from_value(v).unwrap();
        assert_eq!(back, s);
    }

    #[test]
    fn deserialize_rejects_invalid_name() {
        let json = r#"{"name":"Foo.Bar","document_root":"/srv/foo","php":"8.3","secure":false,"kind":"parked"}"#;
        let res: Result<Site, _> = serde_json::from_str(json);
        assert!(res.is_err());
    }

    #[test]
    fn deserialize_lowercases_name() {
        let json = r#"{"name":"FOO","document_root":"/srv/foo","php":"8.3","secure":false,"kind":"parked"}"#;
        let s: Site = serde_json::from_str(json).unwrap();
        assert_eq!(s.name(), "foo");
    }

    #[test]
    fn deserialize_rejects_unknown_field() {
        let json = r#"{"name":"foo","document_root":"/srv/foo","php":"8.3","secure":false,"kind":"parked","extra":"x"}"#;
        let res: Result<Site, _> = serde_json::from_str(json);
        assert!(res.is_err(), "expected unknown-field rejection");
    }
}
