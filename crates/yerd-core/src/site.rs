//! Site type and kind.
//!
//! A [`Site`] is a routable target with a validated DNS-label `name`, a
//! `document_root`, a [`PhpVersion`](crate::PhpVersion), an HTTPS flag, and
//! a [`SiteKind`]. Fields are private to enforce the name invariant; mutation
//! goes through typed setters (no `set_name` - renaming is a router-level
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
/// `document_root` is **not** validated by `yerd-core` - this is a pure crate.
/// It may be empty, relative, or non-canonical. Path semantics, existence, and
/// platform normalisation are owned by `yerd-config` (load time) and
/// `yerd-platform` (runtime). Round-trip through `serde` uses `PathBuf`'s
/// default string representation, which is lossy for paths that cannot be
/// encoded as UTF-8 (notably Windows paths containing unpaired surrogates from
/// WTF-16). Callers needing a guaranteed-UTF-8 path should normalise upstream.
///
/// `web_subpath` is the directory actually served, **relative to**
/// `document_root` (empty = serve `document_root` itself). Modern frameworks
/// serve from a subdirectory (`public/`, `web/`, `webroot/`, `pub/`); the daemon
/// detects this and the proxy serves [`Self::served_root`]. Like
/// `document_root`, the value is not validated here - but [`Self::served_root`]
/// is deliberately defensive so it can never escape `document_root` even if the
/// stored subpath is absolute or contains `..` (see that method). Containment is
/// enforced authoritatively at config-load validation in `yerd-config`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Site {
    name: String,
    document_root: PathBuf,
    web_subpath: PathBuf,
    php: PhpVersion,
    secure: bool,
    kind: SiteKind,
    wp_auto_login: bool,
    wp_auto_login_user: Option<String>,
    front_controller: Option<bool>,
}

impl Site {
    /// Constructs a parked site. **Initialises `secure = false`** - promote
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
            web_subpath: PathBuf::new(),
            php,
            secure: false,
            kind: SiteKind::Parked,
            wp_auto_login: false,
            wp_auto_login_user: None,
            front_controller: None,
        })
    }

    /// Constructs a linked site. **Initialises `secure = false`** - promote
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
            web_subpath: PathBuf::new(),
            php,
            secure: false,
            kind: SiteKind::Linked,
            wp_auto_login: false,
            wp_auto_login_user: None,
            front_controller: None,
        })
    }

    /// The validated, lowercased DNS-label name.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The document root (unvalidated - see type-level docs).
    #[must_use]
    pub fn document_root(&self) -> &Path {
        &self.document_root
    }

    /// The served web root, **relative to** [`Self::document_root`]. Empty means
    /// "serve the document root itself".
    #[must_use]
    pub fn web_subpath(&self) -> &Path {
        &self.web_subpath
    }

    /// The absolute directory the proxy serves: [`Self::document_root`] joined
    /// with [`Self::web_subpath`].
    ///
    /// **Defensive by construction - never escapes the document root.** An empty
    /// subpath returns the document root verbatim (avoiding `join("")`, which
    /// would append a trailing separator). A subpath that is absolute or
    /// contains a `..`/root/prefix component is treated as empty: `Path::join`
    /// with an absolute argument discards the base (`"/a".join("/etc") ==
    /// "/etc"`) and `..` could climb out, so such values fall back to serving the
    /// document root. Legitimate subpaths are plain relative directories
    /// (`public`, `web`, …); config-load validation rejects the rest, this is the
    /// second line of defence.
    #[must_use]
    pub fn served_root(&self) -> PathBuf {
        if self.web_subpath.as_os_str().is_empty() || !is_safe_relative(&self.web_subpath) {
            self.document_root.clone()
        } else {
            self.document_root.join(&self.web_subpath)
        }
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

    /// Whether one-click `WordPress` admin login ("WP Admin" auto-login) is
    /// enabled for this site. `false` by default - a derived-vs-setting
    /// distinction: unlike `is_wordpress` (a detected fact, never stored
    /// here), this is a genuine per-site setting the user opts into.
    #[must_use]
    pub fn wp_auto_login(&self) -> bool {
        self.wp_auto_login
    }

    /// The `WordPress` login/username to sign in as when auto-login runs, or
    /// `None` to fall back to the earliest-created administrator.
    #[must_use]
    pub fn wp_auto_login_user(&self) -> Option<&str> {
        self.wp_auto_login_user.as_deref()
    }

    /// The stored front-controller override: `None` = auto (derive from
    /// detection), `Some(true)` = force front-controller mode, `Some(false)` =
    /// force direct script execution. See [`Self::uses_front_controller`].
    #[must_use]
    pub fn front_controller(&self) -> Option<bool> {
        self.front_controller
    }

    /// Whether requests should funnel through a single front controller
    /// (`index.php`) rather than executing the named `.php` directly.
    ///
    /// The stored override wins; absent it, the default is `!is_wordpress &&
    /// !web_subpath.is_empty()`: a framework served from a subdirectory
    /// (`public/`, `web/`, ...) is single-front-controller, while `WordPress`
    /// (any layout) and plain root-served PHP execute scripts directly.
    /// `is_wordpress` is a runtime fact the daemon injects, so this stays pure.
    #[must_use]
    pub fn uses_front_controller(&self, is_wordpress: bool) -> bool {
        self.front_controller
            .unwrap_or(!is_wordpress && !self.web_subpath.as_os_str().is_empty())
    }

    /// Replaces the document root. Not validated - see type-level docs.
    pub fn set_document_root(&mut self, p: impl Into<PathBuf>) {
        self.document_root = p.into();
    }

    /// Replaces the served web subpath (relative to the document root). Not
    /// validated here - see [`Self::served_root`] for the containment guarantee.
    pub fn set_web_subpath(&mut self, p: impl Into<PathBuf>) {
        self.web_subpath = p.into();
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

    /// Toggles `WordPress` one-click admin login.
    pub fn set_wp_auto_login(&mut self, enabled: bool) {
        self.wp_auto_login = enabled;
    }

    /// Sets the `WordPress` login/username to sign in as, or `None` to fall
    /// back to the earliest-created administrator.
    pub fn set_wp_auto_login_user(&mut self, user: Option<String>) {
        self.wp_auto_login_user = user;
    }

    /// Sets the front-controller override (`None` = auto). See
    /// [`Self::uses_front_controller`].
    pub fn set_front_controller(&mut self, front_controller: Option<bool>) {
        self.front_controller = front_controller;
    }
}

/// Validates and lowercases a site name. Checks run in a fixed, pinned order.
fn validate_and_lowercase_name(raw: &str) -> Result<String, CoreError> {
    if raw.is_empty() {
        return Err(err(raw, SiteNameErrorReason::Empty));
    }

    if raw.contains('.') {
        return Err(err(raw, SiteNameErrorReason::ContainsDot));
    }

    for &b in raw.as_bytes() {
        if !b.is_ascii() {
            return Err(err(raw, SiteNameErrorReason::InvalidCharacter));
        }
        let ok = b.is_ascii_alphanumeric() || b == b'-';
        if !ok {
            return Err(err(raw, SiteNameErrorReason::InvalidCharacter));
        }
    }

    let lowered = raw.to_ascii_lowercase();

    if lowered.starts_with('-') || lowered.ends_with('-') {
        return Err(err(raw, SiteNameErrorReason::LeadingOrTrailingHyphen));
    }

    // RFC 1035 single-label cap. Byte length equals char length here because
    // non-ASCII is rejected above.
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

/// Derive a valid site name from an arbitrary string (typically a directory's
/// last path component), for `yerd link`'s auto-naming: lowercases, replaces
/// every byte outside `[a-z0-9-]` with `-`, collapses runs of `-`, trims
/// leading/trailing `-`, and caps at the 63-byte DNS-label limit. Returns
/// `None` if nothing valid remains.
///
/// The output alphabet is always `[a-z0-9-]` with no leading/trailing/doubled
/// `-`, so unlike [`validate_and_lowercase_name`] it never needs to reject its
/// own result.
#[must_use]
pub fn slugify_site_name(raw: &str) -> Option<String> {
    let mut out = String::with_capacity(raw.len());
    for ch in raw.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
        } else if !out.is_empty() && !out.ends_with('-') {
            out.push('-');
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    if out.is_empty() {
        return None;
    }
    if out.chars().count() > 63 {
        out = out.chars().take(63).collect();
        while out.ends_with('-') {
            out.pop();
        }
    }
    if out.is_empty() {
        None
    } else {
        Some(out)
    }
}

/// A web subpath is safe to join onto the document root iff it is a plain
/// relative path: no root, no drive/UNC prefix, and no `..` component. Such a
/// path can only ever resolve to a descendant of the document root. Used by
/// [`Site::served_root`] as a containment backstop; `yerd-config` enforces the
/// same rule at load time. An empty path is reported safe (the caller handles
/// the empty case before calling this).
#[must_use]
pub(crate) fn is_safe_relative(p: &Path) -> bool {
    use std::path::Component;
    p.components()
        .all(|c| matches!(c, Component::Normal(_) | Component::CurDir))
}

impl serde::Serialize for Site {
    fn serialize<S: serde::Serializer>(&self, ser: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
        let emit_subpath = !self.web_subpath.as_os_str().is_empty();
        let emit_wp_auto_login = self.wp_auto_login;
        let emit_wp_auto_login_user = self.wp_auto_login_user.is_some();
        let emit_front_controller = self.front_controller.is_some();
        let fields = 5
            + usize::from(emit_subpath)
            + usize::from(emit_wp_auto_login)
            + usize::from(emit_wp_auto_login_user)
            + usize::from(emit_front_controller);
        let mut s = ser.serialize_struct("Site", fields)?;
        s.serialize_field("name", &self.name)?;
        s.serialize_field("document_root", &self.document_root)?;
        if emit_subpath {
            s.serialize_field("web_subpath", &self.web_subpath)?;
        }
        s.serialize_field("php", &self.php)?;
        s.serialize_field("secure", &self.secure)?;
        s.serialize_field("kind", &self.kind)?;
        if emit_wp_auto_login {
            s.serialize_field("wp_auto_login", &self.wp_auto_login)?;
        }
        if emit_wp_auto_login_user {
            s.serialize_field("wp_auto_login_user", &self.wp_auto_login_user)?;
        }
        if emit_front_controller {
            s.serialize_field("front_controller", &self.front_controller)?;
        }
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
            // Absent in pre-`web_subpath` wire/config → defaults to empty
            // (serve the document root). Other unknown fields are still rejected.
            #[serde(default)]
            web_subpath: PathBuf,
            php: PhpVersion,
            secure: bool,
            kind: SiteKind,
            // Absent in configs/wire payloads written before this field existed.
            #[serde(default)]
            wp_auto_login: bool,
            #[serde(default)]
            wp_auto_login_user: Option<String>,
            #[serde(default)]
            front_controller: Option<bool>,
        }
        let w = Wire::deserialize(de)?;
        let name = validate_and_lowercase_name(&w.name).map_err(serde::de::Error::custom)?;
        Ok(Self {
            name,
            document_root: w.document_root,
            web_subpath: w.web_subpath,
            php: w.php,
            secure: w.secure,
            kind: w.kind,
            wp_auto_login: w.wp_auto_login,
            wp_auto_login_user: w.wp_auto_login_user,
            front_controller: w.front_controller,
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
            Err(CoreError::InvalidSiteName {
                reason: LeadingOrTrailingHyphen,
                ..
            }) => {}
            other => panic!("LeadingOrTrailingHyphen expected, got {other:?}"),
        }
    }

    #[test]
    fn name_ordering_pin() {
        let long_dotted = format!("{}.", "a".repeat(64));
        match Site::parked(&long_dotted, "/x", v83()) {
            Err(CoreError::InvalidSiteName {
                reason: SiteNameErrorReason::ContainsDot,
                ..
            }) => {}
            other => panic!("ContainsDot expected, got {other:?}"),
        }
        match Site::parked("fü.bar", "/x", v83()) {
            Err(CoreError::InvalidSiteName {
                reason: SiteNameErrorReason::ContainsDot,
                ..
            }) => {}
            other => panic!("ContainsDot expected, got {other:?}"),
        }
    }

    #[test]
    fn slugify_site_name_cases() {
        let cases: &[(&str, Option<&str>)] = &[
            ("My Project", Some("my-project")),
            ("my_app", Some("my-app")),
            ("example.com", Some("example-com")),
            ("a..b", Some("a-b")),
            ("-leading", Some("leading")),
            ("trailing-", Some("trailing")),
            ("already-valid", Some("already-valid")),
            ("???", None),
            ("", None),
        ];
        for (input, expected) in cases {
            assert_eq!(
                slugify_site_name(input).as_deref(),
                *expected,
                "input {input:?}"
            );
        }
    }

    #[test]
    fn slugify_site_name_caps_at_63_bytes_without_trailing_hyphen() {
        let long = "a".repeat(65);
        let slug = slugify_site_name(&long).unwrap();
        assert_eq!(slug.len(), 63);
        assert!(!slug.ends_with('-'));
    }

    /// A separator that lands exactly on the 63-byte truncation boundary
    /// must not leave a dangling trailing hyphen in the result.
    #[test]
    fn slugify_site_name_boundary_separator_has_no_trailing_hyphen() {
        let boundary = format!("{} {}", "a".repeat(62), "b".repeat(5));
        let slug = slugify_site_name(&boundary).unwrap();
        assert_eq!(slug.len(), 62);
        assert!(!slug.ends_with('-'));
    }

    #[test]
    fn slugify_site_name_result_is_always_valid() {
        for input in ["My Project", "my_app", "example.com", "a..b"] {
            let slug = slugify_site_name(input).unwrap();
            assert!(Site::parked(&slug, "/x", v83()).is_ok(), "slug {slug:?}");
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

        s.set_wp_auto_login(true);
        assert!(s.wp_auto_login());

        s.set_wp_auto_login_user(Some("admin".to_owned()));
        assert_eq!(s.wp_auto_login_user(), Some("admin"));

        s.set_front_controller(Some(true));
        assert_eq!(s.front_controller(), Some(true));

        assert_eq!(s.name(), "foo");
    }

    #[test]
    fn wp_auto_login_defaults_false_and_no_user() {
        let s = Site::parked("foo", "/srv/foo", v83()).unwrap();
        assert!(!s.wp_auto_login());
        assert_eq!(s.wp_auto_login_user(), None);
    }

    #[test]
    fn front_controller_defaults_to_auto() {
        let s = Site::parked("foo", "/srv/foo", v83()).unwrap();
        assert_eq!(s.front_controller(), None);
    }

    /// Columns: `(web_subpath, is_wordpress, stored_override, expected)`. Rows 1-4
    /// exercise the auto default (a framework served from a subdir funnels; a
    /// root-served site - plain or `WordPress`, any layout - runs directly); rows
    /// 5-7 show an explicit override winning over the derived default.
    #[test]
    fn uses_front_controller_default_and_override() {
        let cases: &[(&str, bool, Option<bool>, bool)] = &[
            ("public", false, None, true),
            ("", false, None, false),
            ("", true, None, false),
            ("web", true, None, false),
            ("public", false, Some(false), false),
            ("", false, Some(true), true),
            ("", true, Some(true), true),
        ];
        for (subpath, is_wp, ov, expected) in cases {
            let mut s = Site::linked("foo", "/srv/foo", v83()).unwrap();
            s.set_web_subpath(*subpath);
            s.set_front_controller(*ov);
            assert_eq!(
                s.uses_front_controller(*is_wp),
                *expected,
                "subpath={subpath:?} is_wp={is_wp} override={ov:?}"
            );
        }
    }

    #[test]
    fn serde_front_controller_roundtrip_and_omitted_when_auto() {
        let mut s = Site::parked("foo", "/srv/foo", v83()).unwrap();
        let auto = serde_json::to_value(&s).unwrap();
        assert!(
            auto.get("front_controller").is_none(),
            "auto (None) must not be serialized"
        );
        assert_eq!(
            serde_json::from_value::<Site>(auto)
                .unwrap()
                .front_controller(),
            None
        );

        s.set_front_controller(Some(true));
        let v = serde_json::to_value(&s).unwrap();
        assert_eq!(v["front_controller"], true);
        assert_eq!(serde_json::from_value::<Site>(v).unwrap(), s);

        s.set_front_controller(Some(false));
        let v = serde_json::to_value(&s).unwrap();
        assert_eq!(
            v["front_controller"], false,
            "Some(false) must emit, not drop"
        );
        assert_eq!(serde_json::from_value::<Site>(v).unwrap(), s);
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
        assert_eq!(v["name"], "foo");
        assert_eq!(v["document_root"], "/srv/foo");
        assert_eq!(v["php"], "8.3");
        assert_eq!(v["secure"], false);
        assert_eq!(v["kind"], "parked");
        assert!(v.get("web_subpath").is_none());

        let back: Site = serde_json::from_value(v).unwrap();
        assert_eq!(back, s);
    }

    #[test]
    fn web_subpath_default_is_empty() {
        let s = Site::parked("foo", "/srv/foo", v83()).unwrap();
        assert_eq!(s.web_subpath(), Path::new(""));
    }

    #[test]
    fn served_root_empty_subpath_is_document_root() {
        let s = Site::parked("foo", "/srv/foo", v83()).unwrap();
        assert_eq!(s.served_root(), PathBuf::from("/srv/foo"));
    }

    #[test]
    fn served_root_joins_relative_subpath() {
        let mut s = Site::linked("foo", "/srv/foo", v83()).unwrap();
        s.set_web_subpath("public");
        assert_eq!(s.served_root(), PathBuf::from("/srv/foo/public"));
        assert_eq!(s.web_subpath(), Path::new("public"));
    }

    #[test]
    fn served_root_is_defensive_against_escapes() {
        let mut s = Site::linked("foo", "/srv/foo", v83()).unwrap();
        s.set_web_subpath("/etc");
        assert_eq!(s.served_root(), PathBuf::from("/srv/foo"));

        s.set_web_subpath("../../etc");
        assert_eq!(s.served_root(), PathBuf::from("/srv/foo"));

        s.set_web_subpath("app/public");
        assert_eq!(s.served_root(), PathBuf::from("/srv/foo/app/public"));
    }

    #[test]
    fn serde_roundtrip_with_web_subpath() {
        let mut s = Site::linked("foo", "/srv/foo", v83()).unwrap();
        s.set_web_subpath("public");
        let v = serde_json::to_value(&s).unwrap();
        assert_eq!(v["web_subpath"], "public");
        let json = serde_json::to_string(&s).unwrap();
        assert_eq!(
            json,
            r#"{"name":"foo","document_root":"/srv/foo","web_subpath":"public","php":"8.3","secure":false,"kind":"linked"}"#
        );
        let back: Site = serde_json::from_value(v).unwrap();
        assert_eq!(back, s);
    }

    #[test]
    fn serde_roundtrip_with_wp_auto_login() {
        let mut s = Site::linked("foo", "/srv/foo", v83()).unwrap();
        s.set_wp_auto_login(true);
        s.set_wp_auto_login_user(Some("admin".to_owned()));
        let json = serde_json::to_string(&s).unwrap();
        assert_eq!(
            json,
            r#"{"name":"foo","document_root":"/srv/foo","php":"8.3","secure":false,"kind":"linked","wp_auto_login":true,"wp_auto_login_user":"admin"}"#
        );
        let back: Site = serde_json::from_str(&json).unwrap();
        assert_eq!(back, s);
    }

    #[test]
    fn serde_full_site_roundtrip_omits_wp_auto_login_fields_by_default() {
        let s = Site::parked("foo", "/srv/foo", v83()).unwrap();
        let v = serde_json::to_value(&s).unwrap();
        assert!(v.get("wp_auto_login").is_none());
        assert!(v.get("wp_auto_login_user").is_none());
    }

    #[test]
    fn deserialize_absent_wp_auto_login_defaults_false_and_no_user() {
        let json = r#"{"name":"foo","document_root":"/srv/foo","php":"8.3","secure":false,"kind":"parked"}"#;
        let s: Site = serde_json::from_str(json).unwrap();
        assert!(!s.wp_auto_login());
        assert_eq!(s.wp_auto_login_user(), None);
    }

    #[test]
    fn deserialize_absent_web_subpath_defaults_empty() {
        let json = r#"{"name":"foo","document_root":"/srv/foo","php":"8.3","secure":false,"kind":"parked"}"#;
        let s: Site = serde_json::from_str(json).unwrap();
        assert_eq!(s.web_subpath(), Path::new(""));
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
