//! Site router and configuration.
//!
//! [`RouterConfig`] holds the TLD plus a cached `".{tld}"` suffix that
//! [`SiteRouter::resolve`] uses on the hot path. [`SiteRouter`] is a
//! `BTreeMap`-backed registry keyed by `site.name()`, with `insert`, `remove`,
//! `get`, `get_mut`, `iter`, and the host→site `resolve` algorithm.

use std::collections::BTreeMap;

use crate::error::CoreError;
use crate::host::{self, HostKind};
use crate::site::Site;
use crate::tld::Tld;

/// Router configuration.
///
/// INVARIANT: `dotted_tld == format!(".{}", tld.as_str())`. Construct **only**
/// via [`Self::with_tld`], [`Self::new`], [`Self::default`], or `Deserialize`.
/// Never construct field-by-field.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RouterConfig {
    tld: Tld,
    dotted_tld: String,
}

impl Default for RouterConfig {
    fn default() -> Self {
        Self::with_tld(Tld::default())
    }
}

impl RouterConfig {
    /// Validates the TLD string and returns a `RouterConfig`.
    pub fn new(tld: &str) -> Result<Self, CoreError> {
        Ok(Self::with_tld(Tld::new(tld)?))
    }

    /// Wraps an already-validated [`Tld`] and pre-computes the `dotted_tld`
    /// suffix used by `resolve`.
    #[must_use]
    pub fn with_tld(tld: Tld) -> Self {
        let mut dotted_tld = String::with_capacity(tld.as_str().len() + 1);
        dotted_tld.push('.');
        dotted_tld.push_str(tld.as_str());
        Self { tld, dotted_tld }
    }

    /// The TLD as a string slice.
    #[must_use]
    pub fn tld(&self) -> &str {
        self.tld.as_str()
    }

    /// The TLD as a typed [`Tld`].
    #[must_use]
    pub fn tld_typed(&self) -> &Tld {
        &self.tld
    }

    /// The pre-computed `".{tld}"` suffix used by `resolve`. Private to the
    /// crate; only [`SiteRouter::resolve`] (in this module) reads it.
    #[must_use]
    fn dotted_tld(&self) -> &str {
        &self.dotted_tld
    }
}

// Serialise emits exactly one field, `tld`. `dotted_tld` is the cache and is
// NEVER serialised.
impl serde::Serialize for RouterConfig {
    fn serialize<S: serde::Serializer>(&self, ser: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
        let mut s = ser.serialize_struct("RouterConfig", 1)?;
        s.serialize_field("tld", &self.tld)?;
        s.end()
    }
}

impl<'de> serde::Deserialize<'de> for RouterConfig {
    fn deserialize<D: serde::Deserializer<'de>>(de: D) -> Result<Self, D::Error> {
        #[derive(serde::Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Wire {
            tld: String,
        }
        let w = Wire::deserialize(de)?;
        RouterConfig::new(&w.tld).map_err(serde::de::Error::custom)
    }
}

/// Host→site router.
///
/// `Default` is deliberately not derived — callers should pass a
/// [`RouterConfig`] consciously rather than relying on implicit `"test"`.
#[derive(Debug, Clone)]
pub struct SiteRouter {
    config: RouterConfig,
    sites: BTreeMap<String, Site>,
}

impl SiteRouter {
    /// Constructs an empty router under the given configuration.
    #[must_use]
    pub fn new(config: RouterConfig) -> Self {
        Self {
            config,
            sites: BTreeMap::new(),
        }
    }

    /// Calls [`Self::insert`] in iteration order; the first duplicate aborts
    /// with [`CoreError::DuplicateSite`].
    pub fn from_sites(
        config: RouterConfig,
        sites: impl IntoIterator<Item = Site>,
    ) -> Result<Self, CoreError> {
        let mut r = Self::new(config);
        for s in sites {
            r.insert(s)?;
        }
        Ok(r)
    }

    /// Inserts a site keyed by `site.name()`. Errors with
    /// [`CoreError::DuplicateSite`] if a site with that name already exists.
    pub fn insert(&mut self, site: Site) -> Result<(), CoreError> {
        if self.sites.contains_key(site.name()) {
            return Err(CoreError::DuplicateSite {
                name: site.name().to_owned(),
            });
        }
        self.sites.insert(site.name().to_owned(), site);
        Ok(())
    }

    /// Removes a site by name. Errors with [`CoreError::SiteNotFound`] if
    /// missing. Returns the removed [`Site`].
    pub fn remove(&mut self, name: &str) -> Result<Site, CoreError> {
        self.sites
            .remove(name)
            .ok_or_else(|| CoreError::SiteNotFound {
                name: name.to_owned(),
            })
    }

    /// Borrows a site by name.
    #[must_use]
    pub fn get(&self, name: &str) -> Option<&Site> {
        self.sites.get(name)
    }

    /// Mutably borrows a site by name. Invariant-safe because [`Site::name`]
    /// is private and has no setter, so the routing key cannot drift.
    pub fn get_mut(&mut self, name: &str) -> Option<&mut Site> {
        self.sites.get_mut(name)
    }

    /// Iterates sites in lexicographic name order (BTreeMap-backed).
    pub fn iter(&self) -> impl Iterator<Item = &Site> + '_ {
        self.sites.values()
    }

    /// Number of registered sites.
    #[must_use]
    pub fn len(&self) -> usize {
        self.sites.len()
    }

    /// `true` if no sites are registered.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.sites.is_empty()
    }

    /// The router's configuration.
    #[must_use]
    pub fn config(&self) -> &RouterConfig {
        &self.config
    }

    /// Resolves a `Host:` header value to a site.
    #[must_use]
    pub fn resolve(&self, host: &str) -> Option<&Site> {
        let host = match host::normalise(host) {
            HostKind::Hostname(c) => c,
            HostKind::Unroutable => return None,
        };
        let tld = self.config.tld();
        let dotted = self.config.dotted_tld();

        // Bare TLD has no site label.
        if host.as_ref() == tld {
            return None;
        }

        // Must end with ".{tld}".
        let label = host.as_ref().strip_suffix(dotted)?;
        if label.is_empty() {
            return None;
        }

        // Exact match beats wildcard.
        if let Some(s) = self.sites.get(label) {
            return Some(s);
        }

        // Wildcard peel: strip leftmost label, walk right. Terminates because
        // site names cannot contain dots (validated at construction).
        let mut rest = label;
        while let Some((_, parent)) = rest.split_once('.') {
            if parent.is_empty() {
                return None;
            }
            if let Some(s) = self.sites.get(parent) {
                return Some(s);
            }
            rest = parent;
        }
        None
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
    use crate::php::PhpVersion;
    use crate::site::SiteKind;

    fn v83() -> PhpVersion {
        PhpVersion::new(8, 3)
    }

    fn parked(name: &str) -> Site {
        Site::parked(name, format!("/srv/{name}"), v83()).unwrap()
    }

    fn router_with(tld: &str, sites: &[&str]) -> SiteRouter {
        let cfg = RouterConfig::new(tld).unwrap();
        let mut r = SiteRouter::new(cfg);
        for n in sites {
            r.insert(parked(n)).unwrap();
        }
        r
    }

    /// Resolver table — 27 cases covering every rule (exact, port strip,
    /// FQDN dot, case-insensitive, TLD enforcement, exact-beats-wildcard,
    /// wildcard→parent) plus normalisation edge cases.
    #[test]
    fn resolve_table() {
        // Build a single fixture router for the default-TLD cases.
        let r = router_with("test", &["foo", "api-foo"]);

        // (host, expected site name) — None means no match
        let cases: &[(&str, Option<&str>)] = &[
            ("foo.test", Some("foo")),         // 1 Exact
            ("foo.test:8443", Some("foo")),    // 2 Port strip
            ("foo.test:", Some("foo")),        // 3 Trailing ':'
            ("foo.test:abc", None),            // 4 Port junk
            ("foo.test:80:80", None),          // 5 Double colon
            ("[::1]", None),                   // 6 IPv6 literal
            ("[::1]:8080", None),              // 7 IPv6 + port
            (":8080", None),                   // 8 Only port
            ("foo.test.", Some("foo")),        // 9 FQDN dot
            ("foo.test.:80", Some("foo")),     // 10 Trailing dot + port
            ("FOO.TEST", Some("foo")),         // 11 Uppercase
            ("Foo.Test.:443", Some("foo")),    // 12 Mixed
            ("föö.test", None),                // 13 Non-ASCII
            ("foo.example", None),             // 14 Wrong TLD
            ("foo.notthetest", None),          // 15 TLD-suffix collision
            ("test", None),                    // 16 Bare TLD
            ("test.", None),                   // 17 Bare TLD with dot
            ("", None),                        // 18 Empty
            ("bar.test", None),                // 19 Unknown site
            ("api.foo.test", Some("foo")),     // 20 Wildcard 1-level
            ("a.b.c.foo.test", Some("foo")),   // 21 Wildcard multi-level
            ("api.bar.test", None),            // 22 Wildcard unknown parent
            ("api-foo.test", Some("api-foo")), // 23 Exact beats wildcard
            ("foo..test", None),               // 27 Embedded ..
        ];
        for (host, want) in cases {
            let got = r.resolve(host).map(Site::name);
            assert_eq!(got, *want, "host {host:?}");
        }

        // Rows 24/25/26 — custom TLDs.
        let r_lh = router_with("localhost", &["foo"]);
        assert_eq!(
            r_lh.resolve("api.foo.localhost").map(Site::name),
            Some("foo")
        ); // 24

        let r_dl = router_with("dev.local", &["foo"]);
        assert_eq!(r_dl.resolve("foo.dev.local").map(Site::name), Some("foo")); // 25
        assert_eq!(
            r_dl.resolve("api.foo.dev.local").map(Site::name),
            Some("foo")
        ); // 26
    }

    #[test]
    fn new_creates_empty_router() {
        let r = SiteRouter::new(RouterConfig::default());
        assert_eq!(r.len(), 0);
        assert!(r.is_empty());
    }

    #[test]
    fn insert_increments_len() {
        let mut r = SiteRouter::new(RouterConfig::default());
        r.insert(parked("foo")).unwrap();
        r.insert(parked("bar")).unwrap();
        assert_eq!(r.len(), 2);
        assert!(!r.is_empty());
    }

    #[test]
    fn insert_rejects_duplicate() {
        let mut r = SiteRouter::new(RouterConfig::default());
        r.insert(parked("foo")).unwrap();
        // Different casing in input → same lowercased key → DuplicateSite.
        let dup = Site::parked("FOO", "/srv/foo", v83()).unwrap();
        match r.insert(dup) {
            Err(CoreError::DuplicateSite { name }) => assert_eq!(name, "foo"),
            other => panic!("expected DuplicateSite, got {other:?}"),
        }
    }

    #[test]
    fn from_sites_ok_three_sites() {
        let r = SiteRouter::from_sites(
            RouterConfig::default(),
            [parked("a"), parked("b"), parked("c")],
        )
        .unwrap();
        assert_eq!(r.len(), 3);
    }

    #[test]
    fn from_sites_returns_first_duplicate_name_in_error() {
        let res = SiteRouter::from_sites(
            RouterConfig::default(),
            [parked("a"), parked("b"), parked("a"), parked("c")],
        );
        match res {
            Err(CoreError::DuplicateSite { name }) => assert_eq!(name, "a"),
            other => panic!("expected DuplicateSite, got {other:?}"),
        }
    }

    #[test]
    fn get_returns_some_for_known_name() {
        let r = router_with("test", &["foo"]);
        assert!(r.get("foo").is_some());
    }

    #[test]
    fn get_returns_none_for_missing() {
        let r = router_with("test", &["foo"]);
        assert!(r.get("bar").is_none());
    }

    #[test]
    fn get_mut_allows_field_update_without_rename() {
        let mut r = router_with("test", &["foo"]);
        r.get_mut("foo").unwrap().set_php(PhpVersion::new(8, 4));
        assert_eq!(r.get("foo").unwrap().php(), PhpVersion::new(8, 4));
        // Routing still works under the original name.
        assert_eq!(r.resolve("foo.test").map(Site::name), Some("foo"));
    }

    #[test]
    fn remove_returns_removed_site() {
        let mut r = router_with("test", &["foo"]);
        let removed = r.remove("foo").unwrap();
        assert_eq!(removed.name(), "foo");
        assert!(r.is_empty());
    }

    #[test]
    fn remove_errors_when_missing() {
        let mut r = SiteRouter::new(RouterConfig::default());
        match r.remove("nope") {
            Err(CoreError::SiteNotFound { name }) => assert_eq!(name, "nope"),
            other => panic!("expected SiteNotFound, got {other:?}"),
        }
    }

    #[test]
    fn iter_yields_sites_in_name_order() {
        let r = router_with("test", &["charlie", "alpha", "bravo"]);
        let names: Vec<&str> = r.iter().map(Site::name).collect();
        assert_eq!(names, vec!["alpha", "bravo", "charlie"]);
    }

    #[test]
    fn iter_after_remove_skips_removed() {
        let mut r = router_with("test", &["alpha", "bravo", "charlie"]);
        let _ = r.remove("bravo").unwrap();
        let names: Vec<&str> = r.iter().map(Site::name).collect();
        assert_eq!(names, vec!["alpha", "charlie"]);
    }

    #[test]
    fn config_accessor() {
        let r = SiteRouter::new(RouterConfig::new("dev.local").unwrap());
        assert_eq!(r.config().tld(), "dev.local");
    }

    #[test]
    fn routerconfig_new_validates() {
        assert!(RouterConfig::new("").is_err());
        assert!(RouterConfig::new("..").is_err());
        assert!(RouterConfig::new("test").is_ok());
    }

    #[test]
    fn routerconfig_default_is_test() {
        assert_eq!(RouterConfig::default().tld(), "test");
    }

    #[test]
    fn routerconfig_with_tld_caches_dotted_tld() {
        let cfg = RouterConfig::with_tld(Tld::default());
        assert_eq!(cfg.dotted_tld(), ".test");
        let cfg2 = RouterConfig::with_tld(Tld::new("dev.local").unwrap());
        assert_eq!(cfg2.dotted_tld(), ".dev.local");
    }

    #[test]
    fn routerconfig_serde_round_trip_toml() {
        let cfg = RouterConfig::default();
        let s = toml::to_string(&cfg).unwrap();
        assert!(s.contains("tld = \"test\""), "got: {s}");
        let back: RouterConfig = toml::from_str(&s).unwrap();
        assert_eq!(back, cfg);
    }

    #[test]
    fn routerconfig_deserialize_rejects_invalid() {
        let res: Result<RouterConfig, _> = toml::from_str("tld = \"\"");
        assert!(res.is_err());
    }

    #[test]
    fn routerconfig_deserialize_rejects_unknown_field() {
        let res: Result<RouterConfig, _> = toml::from_str("tld = \"test\"\nextra = \"x\"");
        assert!(res.is_err(), "expected unknown-field rejection");
    }

    #[test]
    fn routerconfig_serialize_omits_dotted_tld() {
        let json = serde_json::to_string(&RouterConfig::default()).unwrap();
        assert_eq!(json, r#"{"tld":"test"}"#);
    }

    #[test]
    fn site_kind_is_routable_under_either_kind() {
        // Just exercises that Linked sites route exactly like Parked ones.
        let cfg = RouterConfig::default();
        let mut r = SiteRouter::new(cfg);
        r.insert(Site::linked("foo", "/srv/foo", v83()).unwrap())
            .unwrap();
        assert_eq!(
            r.resolve("foo.test").map(Site::kind),
            Some(SiteKind::Linked)
        );
    }
}
