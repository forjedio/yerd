//! Site router and configuration.
//!
//! [`RouterConfig`] holds the TLD plus a cached `".{tld}"` suffix that
//! [`SiteRouter::resolve`] uses on the hot path. [`SiteRouter`] keeps the site
//! identity map (keyed by `site.name()`) plus two domain indices built from each
//! site's **effective domain set**: `exact` (sub-part → site) and `wildcards`
//! (`"*.rest"` → site).
//!
//! ## Routing model
//!
//! A site answers **only** the domains in its effective set. By default that is
//! just its apex (`{name}.{tld}`) - there is no implicit subdomain catch-all, so
//! `api.foo.test` does not route to `foo` unless `foo` explicitly holds
//! `api.foo` or the single-label wildcard `*.foo`. Resolution tries an exact
//! match first, then exactly one single-label wildcard candidate (the host with
//! its leftmost label replaced by `*`); exact always wins.

use std::collections::{BTreeMap, HashMap};

use crate::domain::Domain;
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
/// `Default` is deliberately not derived - callers should pass a
/// [`RouterConfig`] consciously rather than relying on implicit `"test"`.
#[derive(Debug, Clone)]
pub struct SiteRouter {
    config: RouterConfig,
    sites: BTreeMap<String, Site>,
    domains: BTreeMap<String, Vec<Domain>>,
    primaries: BTreeMap<String, Domain>,
    exact: HashMap<String, String>,
    wildcards: HashMap<String, String>,
}

impl SiteRouter {
    /// Constructs an empty router under the given configuration.
    #[must_use]
    pub fn new(config: RouterConfig) -> Self {
        Self {
            config,
            sites: BTreeMap::new(),
            domains: BTreeMap::new(),
            primaries: BTreeMap::new(),
            exact: HashMap::new(),
            wildcards: HashMap::new(),
        }
    }

    /// Inserts each site with the default apex-only domain set. The first
    /// duplicate name aborts with [`CoreError::DuplicateSite`].
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

    /// Inserts a site with its **default** domain set (apex only, primary =
    /// apex). Errors with [`CoreError::DuplicateSite`] if the name is taken or
    /// [`CoreError::DuplicateDomain`] if the apex is already claimed.
    pub fn insert(&mut self, site: Site) -> Result<(), CoreError> {
        let apex = Domain::apex(site.name());
        self.insert_with_domains(site, vec![apex.clone()], apex)
    }

    /// Inserts a site with an explicit effective domain set and primary. The
    /// daemon computes these (defaults ± delta) and feeds a de-conflicted set.
    ///
    /// Errors (safety nets - the daemon pre-resolves so these do not fire in
    /// production):
    /// - [`CoreError::DuplicateSite`] if the name is already present;
    /// - [`CoreError::DuplicateDomain`] if any domain key is already claimed by
    ///   another site. No partial state is left on error.
    pub fn insert_with_domains(
        &mut self,
        site: Site,
        effective: Vec<Domain>,
        primary: Domain,
    ) -> Result<(), CoreError> {
        if self.sites.contains_key(site.name()) {
            return Err(CoreError::DuplicateSite {
                name: site.name().to_owned(),
            });
        }
        for d in &effective {
            let index = if d.is_wildcard() {
                &self.wildcards
            } else {
                &self.exact
            };
            if index.contains_key(d.as_str()) {
                return Err(CoreError::DuplicateDomain {
                    domain: d.as_str().to_owned(),
                });
            }
        }

        let name = site.name().to_owned();
        for d in &effective {
            if d.is_wildcard() {
                self.wildcards.insert(d.as_str().to_owned(), name.clone());
            } else {
                self.exact.insert(d.as_str().to_owned(), name.clone());
            }
        }
        self.primaries.insert(name.clone(), primary);
        self.domains.insert(name.clone(), effective);
        self.sites.insert(name, site);
        Ok(())
    }

    /// Removes a site by name, together with its domain-index entries. Errors
    /// with [`CoreError::SiteNotFound`] if missing. Returns the removed [`Site`].
    pub fn remove(&mut self, name: &str) -> Result<Site, CoreError> {
        let site = self
            .sites
            .remove(name)
            .ok_or_else(|| CoreError::SiteNotFound {
                name: name.to_owned(),
            })?;
        if let Some(domains) = self.domains.remove(name) {
            for d in domains {
                let index = if d.is_wildcard() {
                    &mut self.wildcards
                } else {
                    &mut self.exact
                };
                if index.get(d.as_str()).is_some_and(|owner| owner == name) {
                    index.remove(d.as_str());
                }
            }
        }
        self.primaries.remove(name);
        Ok(site)
    }

    /// Borrows a site by name.
    #[must_use]
    pub fn get(&self, name: &str) -> Option<&Site> {
        self.sites.get(name)
    }

    /// Mutably borrows a site by name. Invariant-safe because [`Site::name`] is
    /// private with no setter and domains are keyed separately, so neither the
    /// routing key nor the domain indices can drift.
    pub fn get_mut(&mut self, name: &str) -> Option<&mut Site> {
        self.sites.get_mut(name)
    }

    /// The site's primary (canonical, displayed) domain, if the site exists.
    #[must_use]
    pub fn primary_domain(&self, name: &str) -> Option<&Domain> {
        self.primaries.get(name)
    }

    /// The site's primary domain as a full FQDN under the router's TLD, falling
    /// back to `<name>.<tld>` when the site has no stored primary. Centralizes the
    /// host every `{name}.{tld}` address producer needs (`WordPress` URL sync,
    /// tunnel origin) so the fallback lives in one place.
    #[must_use]
    pub fn primary_fqdn(&self, name: &str) -> String {
        let tld = self.config.tld();
        self.primary_domain(name)
            .map_or_else(|| format!("{name}.{tld}"), |d| d.to_fqdn(tld))
    }

    /// The site's effective routable domain set (primary first), if it exists.
    #[must_use]
    pub fn effective_domains(&self, name: &str) -> Option<&[Domain]> {
        self.domains.get(name).map(Vec::as_slice)
    }

    /// The site that currently owns `domain` (in the effective routing indices),
    /// or `None` if unclaimed. Used by mutation handlers to reject a domain that
    /// already routes to a different site.
    #[must_use]
    pub fn domain_owner(&self, domain: &Domain) -> Option<&str> {
        let index = if domain.is_wildcard() {
            &self.wildcards
        } else {
            &self.exact
        };
        index.get(domain.as_str()).map(String::as_str)
    }

    /// If the site's apex label is claimed in the exact index by a **different**
    /// site, returns that other site's name (the shadow). `None` when the site
    /// owns its own apex or nobody claims it.
    #[must_use]
    pub fn apex_shadowed_by(&self, name: &str) -> Option<&str> {
        self.exact
            .get(name)
            .filter(|owner| owner.as_str() != name)
            .map(String::as_str)
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
    ///
    /// Exact domain match first; then a single-label wildcard match (the host
    /// with its leftmost label replaced by `*`). No implicit catch-all: a host
    /// with no matching exact or wildcard domain is unresolved.
    #[must_use]
    pub fn resolve(&self, host: &str) -> Option<&Site> {
        let host = match host::normalise(host) {
            HostKind::Hostname(c) => c,
            HostKind::Unroutable => return None,
        };
        let tld = self.config.tld();
        let dotted = self.config.dotted_tld();

        if host.as_ref() == tld {
            return None;
        }

        let sub = host.as_ref().strip_suffix(dotted)?;
        if sub.is_empty() {
            return None;
        }

        if let Some(name) = self.exact.get(sub) {
            return self.sites.get(name);
        }

        if let Some((_, rest)) = sub.split_once('.') {
            let mut key = String::with_capacity(rest.len() + 2);
            key.push_str("*.");
            key.push_str(rest);
            if let Some(name) = self.wildcards.get(&key) {
                return self.sites.get(name);
            }
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

    fn dom(sub: &str) -> Domain {
        Domain::parse_subpart(sub).unwrap()
    }

    /// Insert a site with an explicit effective set (primary = first exact).
    fn insert_domains(r: &mut SiteRouter, name: &str, subs: &[&str]) {
        let effective: Vec<Domain> = subs.iter().map(|s| dom(s)).collect();
        let primary = effective
            .iter()
            .find(|d| !d.is_wildcard())
            .cloned()
            .unwrap_or_else(|| Domain::apex(name));
        r.insert_with_domains(parked(name), effective, primary)
            .unwrap();
    }

    fn router_with(tld: &str, sites: &[&str]) -> SiteRouter {
        let cfg = RouterConfig::new(tld).unwrap();
        let mut r = SiteRouter::new(cfg);
        for n in sites {
            r.insert(parked(n)).unwrap();
        }
        r
    }

    /// Default (apex-only) resolution: exact apex resolves, subdomains do not.
    #[test]
    fn resolve_apex_only_default() {
        let r = router_with("test", &["foo", "api-foo"]);
        let cases: &[(&str, Option<&str>)] = &[
            ("foo.test", Some("foo")),
            ("foo.test:8443", Some("foo")),
            ("foo.test.", Some("foo")),
            ("FOO.TEST", Some("foo")),
            ("api-foo.test", Some("api-foo")),
            ("api.foo.test", None), // NO implicit catch-all
            ("a.b.foo.test", None), // NO implicit catch-all
            ("bar.test", None),
            ("test", None),
            ("test.", None),
            ("", None),
            ("föö.test", None),
            ("foo.example", None),
            ("foo.notthetest", None),
            ("foo..test", None),
            ("[::1]", None),
        ];
        for (host, want) in cases {
            assert_eq!(r.resolve(host).map(Site::name), *want, "host {host:?}");
        }
    }

    /// Single-label wildcard: `*.foo` matches one label, not deeper; exact wins.
    #[test]
    fn resolve_wildcard_single_label_and_precedence() {
        let cfg = RouterConfig::new("test").unwrap();
        let mut r = SiteRouter::new(cfg);
        insert_domains(&mut r, "foo", &["foo"]); // apex A
        insert_domains(&mut r, "wild", &["wild", "*.foo"]); // wildcard site B
        insert_domains(&mut r, "api", &["api", "api.foo"]); // exact carve-out C

        let cases: &[(&str, Option<&str>)] = &[
            ("foo.test", Some("foo")),      // exact apex A
            ("xyz.foo.test", Some("wild")), // wildcard *.foo -> B
            ("api.foo.test", Some("api")),  // exact beats wildcard -> C
            ("x.api.foo.test", None),       // single-label: *.foo does NOT match 2 labels
            ("wild.test", Some("wild")),
            ("api.test", Some("api")),
        ];
        for (host, want) in cases {
            assert_eq!(r.resolve(host).map(Site::name), *want, "host {host:?}");
        }
    }

    /// Nested wildcard resolves its own level; `foo.test` and `*.foo.test` are
    /// independent sites (the user's core requirement).
    #[test]
    fn resolve_nested_wildcard_and_independent_sites() {
        let cfg = RouterConfig::new("test").unwrap();
        let mut r = SiteRouter::new(cfg);
        insert_domains(&mut r, "a", &["foo"]); // foo.test -> A
        insert_domains(&mut r, "b", &["b", "*.foo"]); // *.foo.test -> B
        insert_domains(&mut r, "c", &["c", "*.api.foo"]); // *.api.foo.test -> C

        assert_eq!(r.resolve("foo.test").map(Site::name), Some("a"));
        assert_eq!(r.resolve("x.foo.test").map(Site::name), Some("b"));
        assert_eq!(r.resolve("x.api.foo.test").map(Site::name), Some("c"));
        // api.foo.test: exact? no. wildcard *.foo -> B (one label `api`).
        assert_eq!(r.resolve("api.foo.test").map(Site::name), Some("b"));
    }

    #[test]
    fn multi_label_tld_resolution() {
        let cfg = RouterConfig::new("dev.local").unwrap();
        let mut r = SiteRouter::new(cfg);
        insert_domains(&mut r, "foo", &["foo", "*.foo"]);
        assert_eq!(r.resolve("foo.dev.local").map(Site::name), Some("foo"));
        assert_eq!(r.resolve("api.foo.dev.local").map(Site::name), Some("foo"));
        assert_eq!(r.resolve("a.b.foo.dev.local").map(Site::name), None);
    }

    #[test]
    fn insert_rejects_duplicate_name() {
        let mut r = SiteRouter::new(RouterConfig::default());
        r.insert(parked("foo")).unwrap();
        let dup = Site::parked("FOO", "/srv/foo", v83()).unwrap();
        match r.insert(dup) {
            Err(CoreError::DuplicateSite { name }) => assert_eq!(name, "foo"),
            other => panic!("expected DuplicateSite, got {other:?}"),
        }
    }

    #[test]
    fn insert_rejects_duplicate_domain() {
        let cfg = RouterConfig::new("test").unwrap();
        let mut r = SiteRouter::new(cfg);
        insert_domains(&mut r, "a", &["a", "shared"]);
        // A different site claiming the same exact domain collides.
        let effective = vec![dom("b"), dom("shared")];
        match r.insert_with_domains(parked("b"), effective, dom("b")) {
            Err(CoreError::DuplicateDomain { domain }) => assert_eq!(domain, "shared"),
            other => panic!("expected DuplicateDomain, got {other:?}"),
        }
        // ... and no partial state was left: `b` is absent, `shared` still -> a.
        assert!(r.get("b").is_none());
        assert_eq!(r.resolve("shared.test").map(Site::name), Some("a"));
    }

    #[test]
    fn exact_and_wildcard_same_base_coexist() {
        // foo (exact) and *.foo (wildcard) on different sites: no collision.
        let cfg = RouterConfig::new("test").unwrap();
        let mut r = SiteRouter::new(cfg);
        insert_domains(&mut r, "a", &["foo"]);
        insert_domains(&mut r, "b", &["b", "*.foo"]);
        assert_eq!(r.resolve("foo.test").map(Site::name), Some("a"));
        assert_eq!(r.resolve("x.foo.test").map(Site::name), Some("b"));
    }

    #[test]
    fn remove_clears_domain_indices() {
        let cfg = RouterConfig::new("test").unwrap();
        let mut r = SiteRouter::new(cfg);
        insert_domains(&mut r, "foo", &["foo", "corp", "*.foo"]);
        assert_eq!(r.resolve("corp.test").map(Site::name), Some("foo"));
        let removed = r.remove("foo").unwrap();
        assert_eq!(removed.name(), "foo");
        assert!(r.is_empty());
        assert_eq!(r.resolve("corp.test"), None);
        assert_eq!(r.resolve("x.foo.test"), None);
        // The freed key can be re-claimed by a new site.
        insert_domains(&mut r, "corp", &["corp"]);
        assert_eq!(r.resolve("corp.test").map(Site::name), Some("corp"));
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
    fn primary_and_effective_accessors() {
        let cfg = RouterConfig::new("test").unwrap();
        let mut r = SiteRouter::new(cfg);
        r.insert_with_domains(parked("foo"), vec![dom("corp"), dom("*.foo")], dom("corp"))
            .unwrap();
        assert_eq!(r.primary_domain("foo"), Some(&dom("corp")));
        assert_eq!(
            r.effective_domains("foo"),
            Some(&[dom("corp"), dom("*.foo")][..])
        );
        assert_eq!(r.primary_domain("missing"), None);
    }

    #[test]
    fn apex_shadowed_by_reports_claimant() {
        let cfg = RouterConfig::new("test").unwrap();
        let mut r = SiteRouter::new(cfg);
        // shop explicitly claims exact `blog`; site blog's apex was dropped.
        insert_domains(&mut r, "shop", &["shop", "blog"]);
        insert_domains(&mut r, "blog", &["*.blog"]); // apex suppressed -> only wildcard... but normalization is a daemon concern; here we feed it directly
        assert_eq!(r.apex_shadowed_by("blog"), Some("shop"));
        assert_eq!(r.apex_shadowed_by("shop"), None);
    }

    #[test]
    fn get_mut_allows_field_update_without_rename() {
        let cfg = RouterConfig::new("test").unwrap();
        let mut r = SiteRouter::new(cfg);
        insert_domains(&mut r, "foo", &["foo", "*.foo"]);
        r.get_mut("foo").unwrap().set_php(PhpVersion::new(8, 4));
        assert_eq!(r.get("foo").unwrap().php(), PhpVersion::new(8, 4));
        assert_eq!(r.resolve("foo.test").map(Site::name), Some("foo"));
        assert_eq!(r.resolve("x.foo.test").map(Site::name), Some("foo"));
    }

    #[test]
    fn iter_yields_sites_in_name_order() {
        let r = router_with("test", &["charlie", "alpha", "bravo"]);
        let names: Vec<&str> = r.iter().map(Site::name).collect();
        assert_eq!(names, vec!["alpha", "bravo", "charlie"]);
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
    fn linked_and_parked_route_alike() {
        let cfg = RouterConfig::default();
        let mut r = SiteRouter::new(cfg);
        r.insert(Site::linked("foo", "/srv/foo", v83()).unwrap())
            .unwrap();
        assert_eq!(
            r.resolve("foo.test").map(Site::kind),
            Some(SiteKind::Linked)
        );
    }

    #[test]
    fn new_creates_empty_router() {
        let r = SiteRouter::new(RouterConfig::default());
        assert_eq!(r.len(), 0);
        assert!(r.is_empty());
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
    fn routerconfig_serialize_omits_dotted_tld() {
        let json = serde_json::to_string(&RouterConfig::default()).unwrap();
        assert_eq!(json, r#"{"tld":"test"}"#);
    }

    #[test]
    fn routerconfig_deserialize_rejects_unknown_field() {
        let res: Result<RouterConfig, _> = toml::from_str("tld = \"test\"\nextra = \"x\"");
        assert!(res.is_err(), "expected unknown-field rejection");
    }
}
