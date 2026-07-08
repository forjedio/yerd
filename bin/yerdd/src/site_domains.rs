//! Infallible construction of the domain-aware [`SiteRouter`].
//!
//! Given the scanned sites and the persisted `[domains]` deltas, this computes
//! each site's effective domain set and feeds a **de-conflicted** `(site,
//! domains, primary)` list into the router, so building can never error (no
//! boot-brick from a duplicate name or a hand-edited domain collision). Core's
//! `insert_with_domains` keeps its `DuplicateSite`/`DuplicateDomain` errors as
//! safety nets; this layer resolves collisions first so they do not fire.
//!
//! Collision rules (deterministic):
//! - **Identity** (two sites resolving to the same name): first wins; later ones
//!   are dropped and logged. Linked sites already shadow same-named parked sites
//!   during the scan, so this only bites two parked dirs of the same name.
//! - **Domain index**: an **explicit** (added) domain beats an **implicit** apex;
//!   among same-priority claims the first site wins. A site whose apex is claimed
//!   by another loses it from the index (a shadow, surfaced via
//!   `SiteRouter::apex_shadowed_by`), but keeps it in its effective set so its
//!   primary/address stays concrete.

use std::collections::{HashMap, HashSet};

use yerd_config::{Config, DomainDelta};
use yerd_core::{choose_primary, effective_domains, Domain, RouterConfig, Site, SiteRouter};

/// Build the router from scanned sites plus the config's `[domains]` deltas.
/// Infallible: collisions are resolved by the rules above, not by erroring.
#[must_use]
pub(crate) fn build(cfg: &Config, sites: Vec<Site>) -> SiteRouter {
    let plans = plan_sites(cfg, sites);
    let claims = build_claims(&plans);

    let mut router = SiteRouter::new(RouterConfig::with_tld(cfg.tld.clone()));
    for (idx, plan) in plans.iter().enumerate() {
        let won: Vec<Domain> = plan
            .effective
            .iter()
            .filter(|d| claims.get(d.as_str()).is_some_and(|owner| *owner == idx))
            .cloned()
            .collect();
        let primary = choose_primary(plan.site.name(), &won, Some(&plan.primary));
        if let Err(e) = router.insert_with_domains(plan.site.clone(), won, primary) {
            // Unreachable given the pre-de-confliction above; logged rather than
            // panicking so a latent bug degrades gracefully instead of bricking boot.
            tracing::error!(site = plan.site.name(), error = %e, "dropping site: router insert failed");
        }
    }
    router
}

/// A site plus its computed effective domain set and chosen primary.
struct SitePlan {
    site: Site,
    effective: Vec<Domain>,
    primary: Domain,
}

/// Resolve identity collisions (first name wins) and compute each surviving
/// site's effective domain set and primary from its stored delta.
fn plan_sites(cfg: &Config, sites: Vec<Site>) -> Vec<SitePlan> {
    let mut seen: HashSet<String> = HashSet::new();
    let mut plans = Vec::with_capacity(sites.len());
    for site in sites {
        if !seen.insert(site.name().to_owned()) {
            tracing::warn!(site = site.name(), "duplicate site name; keeping the first");
            continue;
        }
        let delta = delta_for(cfg, &site);
        let (added, suppressed, stored_primary) = match delta {
            Some(d) => (
                d.added.as_slice(),
                d.suppressed.as_slice(),
                d.primary.as_ref(),
            ),
            None => ([].as_slice(), [].as_slice(), None),
        };
        let effective = effective_domains(site.name(), added, suppressed);
        let primary = choose_primary(site.name(), &effective, stored_primary);
        plans.push(SitePlan {
            site,
            effective,
            primary,
        });
    }
    plans
}

/// The stored delta for a site: linked sites key by name, parked by document
/// root (mirroring `overrides`).
fn delta_for<'a>(cfg: &'a Config, site: &Site) -> Option<&'a DomainDelta> {
    match site.kind() {
        yerd_core::SiteKind::Linked => cfg.domains.linked.get(site.name()),
        yerd_core::SiteKind::Parked => cfg
            .domains
            .parked
            .get(&site.document_root().to_string_lossy().into_owned()),
    }
}

/// Build the domain-key → winning-site-index map. Explicit (non-apex) claims beat
/// implicit apex claims; among same priority the first site wins.
fn build_claims(plans: &[SitePlan]) -> HashMap<String, usize> {
    struct Claim {
        idx: usize,
        explicit: bool,
    }
    let mut claims: HashMap<String, Claim> = HashMap::new();
    for (idx, plan) in plans.iter().enumerate() {
        let apex = Domain::apex(plan.site.name());
        for d in &plan.effective {
            let explicit = *d != apex;
            match claims.get(d.as_str()) {
                Some(existing) if existing.explicit || !explicit => {}
                _ => {
                    claims.insert(d.as_str().to_owned(), Claim { idx, explicit });
                }
            }
        }
    }
    claims.into_iter().map(|(k, c)| (k, c.idx)).collect()
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
    use yerd_core::PhpVersion;

    fn v() -> PhpVersion {
        PhpVersion::new(8, 3)
    }

    fn cfg_with_tld(tld: &str) -> Config {
        let mut c = Config::default();
        c.tld = yerd_core::Tld::new(tld).unwrap();
        c
    }

    fn linked(name: &str, root: &str) -> Site {
        Site::linked(name, root, v()).unwrap()
    }

    fn d(sub: &str) -> Domain {
        Domain::parse_subpart(sub).unwrap()
    }

    #[test]
    fn default_sites_are_apex_only() {
        let cfg = cfg_with_tld("test");
        let r = build(&cfg, vec![linked("foo", "/srv/foo")]);
        assert_eq!(r.resolve("foo.test").map(Site::name), Some("foo"));
        assert_eq!(r.resolve("api.foo.test"), None);
    }

    #[test]
    fn linked_delta_adds_domains() {
        let mut cfg = cfg_with_tld("test");
        cfg.domains.linked.insert(
            "foo".into(),
            DomainDelta {
                added: vec![d("corp"), d("*.foo")],
                suppressed: vec![],
                primary: Some(d("corp")),
            },
        );
        let r = build(&cfg, vec![linked("foo", "/srv/foo")]);
        assert_eq!(r.resolve("foo.test").map(Site::name), Some("foo"));
        assert_eq!(r.resolve("corp.test").map(Site::name), Some("foo"));
        assert_eq!(r.resolve("x.foo.test").map(Site::name), Some("foo"));
        assert_eq!(r.primary_domain("foo"), Some(&d("corp")));
    }

    #[test]
    fn explicit_beats_implicit_apex_and_shadows() {
        // shop explicitly claims exact `blog`; parked site blog's apex is dropped.
        let mut cfg = cfg_with_tld("test");
        cfg.domains.linked.insert(
            "shop".into(),
            DomainDelta {
                added: vec![d("blog")],
                suppressed: vec![],
                primary: None,
            },
        );
        let r = build(
            &cfg,
            vec![linked("blog", "/srv/blog"), linked("shop", "/srv/shop")],
        );
        assert_eq!(r.resolve("blog.test").map(Site::name), Some("shop"));
        assert_eq!(r.apex_shadowed_by("blog"), Some("shop"));
    }

    #[test]
    fn duplicate_name_keeps_first() {
        let cfg = cfg_with_tld("test");
        let r = build(
            &cfg,
            vec![linked("foo", "/srv/a/foo"), linked("foo", "/srv/b/foo")],
        );
        assert_eq!(r.len(), 1);
        assert_eq!(
            r.get("foo").unwrap().document_root().to_string_lossy(),
            "/srv/a/foo"
        );
    }

    #[test]
    fn apex_and_wildcard_on_different_sites() {
        let mut cfg = cfg_with_tld("test");
        cfg.domains.linked.insert(
            "wild".into(),
            DomainDelta {
                added: vec![d("*.foo")],
                suppressed: vec![],
                primary: None,
            },
        );
        // `foo` keeps its apex; `wild` owns `*.foo`.
        let r = build(
            &cfg,
            vec![linked("foo", "/srv/foo"), linked("wild", "/srv/wild")],
        );
        assert_eq!(r.resolve("foo.test").map(Site::name), Some("foo"));
        assert_eq!(r.resolve("api.foo.test").map(Site::name), Some("wild"));
    }
}
