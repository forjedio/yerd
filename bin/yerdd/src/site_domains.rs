//! Infallible construction of the domain-aware [`SiteRouter`].
//!
//! Given the scanned sites, the configured whole-host proxies, and the persisted
//! `[domains]` deltas, this computes each claimant's effective domain set and
//! feeds a **de-conflicted** `(claimant, domains, primary)` list into the router,
//! so building can never error (no boot-brick from a duplicate name or a
//! hand-edited domain collision). Core's `insert_with_domains` /
//! `insert_proxy_with_domains` keep their `DuplicateSite`/`DuplicateDomain`
//! errors as safety nets; this layer resolves collisions first so they do not
//! fire.
//!
//! Collision rules (deterministic). Sites and proxies form **one claimant
//! sequence**: every site (scan order) followed by every proxy (config order).
//! - **Identity** (two sites, or two proxies, resolving to the same name): first
//!   wins; later ones are dropped and logged. Linked sites already shadow
//!   same-named parked sites during the scan, so for sites this only bites two
//!   parked dirs of the same name.
//! - **Name shadow**: a proxy whose name matches a planned site's is excluded
//!   from winning any claim and is never inserted. The router would reject it
//!   wholesale on `DuplicateSite`, orphaning any domain it had won; the case is
//!   reachable because `Config::validate` cannot see **parked** sites.
//! - **Domain index**: an **explicit** (added) domain beats an **implicit** apex;
//!   among same-priority claims the earlier claimant wins, so a site beats a
//!   proxy at equal priority and proxy-vs-proxy ties resolve in config order. A
//!   claimant whose apex is claimed by another loses it from the index (a shadow,
//!   surfaced via `SiteRouter::apex_shadowed_by`), but keeps it in its effective
//!   set so its primary/address stays concrete.

use std::collections::{BTreeMap, HashMap, HashSet};

use yerd_config::{Config, DomainDelta};
use yerd_core::{
    choose_primary, effective_domains, Domain, ProxySite, RouterConfig, Site, SiteRouter,
};

/// Build the router from scanned sites and configured proxies plus the config's
/// `[domains]` deltas. Infallible: collisions are resolved by the rules above,
/// not by erroring. `build_claims` de-conflicts every routing key before
/// insertion, so the `insert_with_domains` / `insert_proxy_with_domains` error
/// arms are unreachable in practice; they log and drop the claimant rather than
/// panicking, so a latent bug degrades gracefully instead of bricking boot.
///
/// Sites are inserted first, each with the subset of its effective domains it
/// won, then every non-name-shadowed proxy with the subset **it** won: a proxy
/// that loses one domain keeps the rest. Each site's path-prefix rules are then
/// attached (linked keyed by name, parked by document-root); a rule set for a
/// dropped/absent site is harmless, as it is only ever consulted for a site the
/// router actually resolved.
#[must_use]
pub(crate) fn build(cfg: &Config, sites: Vec<Site>) -> SiteRouter {
    let plans = plan_sites(cfg, sites);
    let proxy_plans = plan_proxies(cfg, &plans);
    let claims = build_claims(&claimants(&plans, &proxy_plans));

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
            tracing::error!(site = plan.site.name(), error = %e, "dropping site: router insert failed");
        }
    }

    for (offset, plan) in proxy_plans.iter().enumerate() {
        if plan.shadowed {
            continue;
        }
        let idx = plans.len() + offset;
        let won: Vec<Domain> = plan
            .effective
            .iter()
            .filter(|d| claims.get(d.as_str()).is_some_and(|owner| *owner == idx))
            .cloned()
            .collect();
        let primary = choose_primary(plan.proxy.name(), &won, Some(&plan.primary));
        if let Err(e) = router.insert_proxy_with_domains(plan.proxy.clone(), won, primary) {
            tracing::error!(proxy = plan.proxy.name(), error = %e, "dropping proxy: router insert failed");
        }
    }

    for plan in &plans {
        let rules = match plan.site.kind() {
            yerd_core::SiteKind::Linked => cfg.proxy_rules.linked.get(plan.site.name()),
            yerd_core::SiteKind::Parked => cfg
                .proxy_rules
                .parked
                .get(&plan.site.document_root().to_string_lossy().into_owned()),
        };
        if let Some(rules) = rules {
            router.set_proxy_rules(plan.site.name(), rules.clone());
        }
        let route_rules = match plan.site.kind() {
            yerd_core::SiteKind::Linked => cfg.route_rules.linked.get(plan.site.name()),
            yerd_core::SiteKind::Parked => cfg
                .route_rules
                .parked
                .get(&plan.site.document_root().to_string_lossy().into_owned()),
        };
        if let Some(route_rules) = route_rules {
            router.set_route_rules(plan.site.name(), route_rules.clone());
        }
    }
    router
}

/// A domain wanted by more than one claimant. `build` resolves this
/// deterministically for a given scan order (explicit beats implicit apex, else
/// the earlier claimant wins), but the scan order of parked directories under one
/// root is filesystem dependent, so `winner` can differ across restarts. Surfaced
/// by the doctor so the user can make the domains unique. `losers` wanted
/// `domain` but were dropped from routing.
pub(crate) struct DomainCollision {
    /// The claimant that currently owns the contested domain in the router: a
    /// site's name, or `proxy:<name>` for a whole-host proxy.
    pub winner: String,
    /// The other claimants that wanted the domain and lost it, labelled the same
    /// way as `winner`.
    pub losers: Vec<String>,
}

/// Detect domains wanted by more than one claimant, using the same effective-set
/// and claim rules as [`build`] over the same unified site-then-proxy list. Empty
/// for a well-formed config where every domain has a single claimant. Proxies
/// appear on either side and can lose on any domain, not just their apex; a
/// name-shadowed proxy still reports its losses even though it is never inserted.
#[must_use]
pub(crate) fn collisions(cfg: &Config, sites: Vec<Site>) -> Vec<DomainCollision> {
    let plans = plan_sites(cfg, sites);
    let proxy_plans = plan_proxies(cfg, &plans);
    let claimants = claimants(&plans, &proxy_plans);
    let claims = build_claims(&claimants);

    let mut wanted: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    for (idx, claimant) in claimants.iter().enumerate() {
        for dom in claimant.effective {
            wanted.entry(dom.as_str().to_owned()).or_default().push(idx);
        }
    }

    let mut out = Vec::new();
    for (domain, idxs) in wanted {
        if idxs.len() < 2 {
            continue;
        }
        let Some(winner_idx) = claims.get(&domain).copied() else {
            continue;
        };
        let Some(winner) = claimants.get(winner_idx).map(|c| c.label.clone()) else {
            continue;
        };
        let losers: Vec<String> = idxs
            .iter()
            .filter(|&&i| i != winner_idx)
            .filter_map(|&i| claimants.get(i).map(|c| c.label.clone()))
            .collect();
        if losers.is_empty() {
            continue;
        }
        out.push(DomainCollision { winner, losers });
    }
    out
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

/// A whole-host proxy plus its computed effective domain set and chosen primary.
struct ProxyPlan {
    proxy: ProxySite,
    effective: Vec<Domain>,
    primary: Domain,
    /// `true` when a planned site already answers to this proxy's name. Such a
    /// proxy still reports collisions but never wins a claim and is never
    /// inserted, since the router would reject it on `DuplicateSite` and orphan
    /// whatever it had won.
    shadowed: bool,
}

/// Resolve proxy identity collisions (first name wins), flag proxies shadowed by
/// a planned site's name, and compute each survivor's effective domain set and
/// primary from its stored delta.
fn plan_proxies(cfg: &Config, site_plans: &[SitePlan]) -> Vec<ProxyPlan> {
    let site_names: HashSet<&str> = site_plans.iter().map(|p| p.site.name()).collect();
    let mut seen: HashSet<String> = HashSet::new();
    let mut plans = Vec::with_capacity(cfg.proxies.len());
    for proxy in &cfg.proxies {
        if !seen.insert(proxy.name().to_owned()) {
            tracing::warn!(
                proxy = proxy.name(),
                "duplicate proxy name; keeping the first"
            );
            continue;
        }
        let (added, suppressed, stored_primary) = match cfg.domains.proxy.get(proxy.name()) {
            Some(d) => (
                d.added.as_slice(),
                d.suppressed.as_slice(),
                d.primary.as_ref(),
            ),
            None => ([].as_slice(), [].as_slice(), None),
        };
        let effective = effective_domains(proxy.name(), added, suppressed);
        let primary = choose_primary(proxy.name(), &effective, stored_primary);
        let shadowed = site_names.contains(proxy.name());
        if shadowed {
            tracing::warn!(
                proxy = proxy.name(),
                "proxy name is taken by a site; the proxy is not routed"
            );
        }
        plans.push(ProxyPlan {
            proxy: proxy.clone(),
            effective,
            primary,
            shadowed,
        });
    }
    plans
}

/// One entry of the unified claimant sequence consumed by [`build_claims`] and
/// [`collisions`]: every site plan (indices `0..S`) followed by every proxy plan
/// (`S..S+P`).
struct Claimant<'a> {
    /// Collision label: a site's name, or `proxy:<name>` for a proxy.
    label: String,
    /// The claimant's own apex, which marks its one implicit claim.
    apex: Domain,
    effective: &'a [Domain],
    /// `false` for a name-shadowed proxy, which never wins a claim.
    eligible: bool,
}

/// Flatten the site and proxy plans into the one ordered claimant sequence that
/// fixes the "earlier claimant wins" tie-break: sites before proxies.
fn claimants<'a>(site_plans: &'a [SitePlan], proxy_plans: &'a [ProxyPlan]) -> Vec<Claimant<'a>> {
    let sites = site_plans.iter().map(|p| Claimant {
        label: p.site.name().to_owned(),
        apex: Domain::apex(p.site.name()),
        effective: p.effective.as_slice(),
        eligible: true,
    });
    let proxies = proxy_plans.iter().map(|p| Claimant {
        label: format!("proxy:{}", p.proxy.name()),
        apex: Domain::apex(p.proxy.name()),
        effective: p.effective.as_slice(),
        eligible: !p.shadowed,
    });
    sites.chain(proxies).collect()
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

/// Build the domain-key → winning-claimant-index map over the unified claimant
/// sequence. Explicit (non-apex) claims beat implicit apex claims; among same
/// priority the earlier claimant wins, so a site beats a proxy and proxy-vs-proxy
/// ties resolve in config order. Ineligible (name-shadowed) claimants are skipped
/// entirely, so their domains fall through to the next claimant or to nobody.
fn build_claims(claimants: &[Claimant<'_>]) -> HashMap<String, usize> {
    struct Claim {
        idx: usize,
        explicit: bool,
    }
    let mut claims: HashMap<String, Claim> = HashMap::new();
    for (idx, claimant) in claimants.iter().enumerate() {
        if !claimant.eligible {
            continue;
        }
        for d in claimant.effective {
            let explicit = *d != claimant.apex;
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

    fn target(url: &str) -> yerd_core::UpstreamTarget {
        yerd_core::UpstreamTarget::from_url_str(url).unwrap()
    }

    fn proxy(name: &str) -> ProxySite {
        ProxySite::new(name, target("http://127.0.0.1:3000")).unwrap()
    }

    fn added(domains: &[&str]) -> DomainDelta {
        DomainDelta {
            added: domains.iter().map(|s| d(s)).collect(),
            suppressed: vec![],
            primary: None,
        }
    }

    fn is_proxy(r: &SiteRouter, host: &str) -> bool {
        matches!(r.resolve_route(host), Some(yerd_core::Route::Proxy(_)))
    }

    #[test]
    fn proxies_and_rules_fold_into_router() {
        let mut cfg = cfg_with_tld("test");
        cfg.proxies
            .push(yerd_core::ProxySite::new("reverb", target("http://127.0.0.1:3000")).unwrap());
        cfg.proxy_rules.linked.insert(
            "foo".into(),
            vec![yerd_core::ProxyRule::new("/ws", target("http://127.0.0.1:3001")).unwrap()],
        );
        let r = build(&cfg, vec![linked("foo", "/srv/foo")]);
        assert!(matches!(
            r.resolve_route("reverb.test"),
            Some(yerd_core::Route::Proxy(_))
        ));
        assert_eq!(r.resolve("foo.test").map(Site::name), Some("foo"));
        assert_eq!(r.rules_for("foo").len(), 1);
        assert!(yerd_core::match_rule(r.rules_for("foo"), "/ws/x").is_some());
    }

    #[test]
    fn route_rules_fold_into_router_for_both_site_kinds() {
        let mut cfg = cfg_with_tld("test");
        cfg.route_rules.linked.insert(
            "foo".into(),
            vec![yerd_core::RouteRule::new("/api", "api/index.php").unwrap()],
        );
        cfg.route_rules.parked.insert(
            "/srv/blog".into(),
            vec![yerd_core::RouteRule::new("/", "index.html").unwrap()],
        );
        let r = build(
            &cfg,
            vec![
                linked("foo", "/srv/foo"),
                Site::parked("blog", "/srv/blog", v()).unwrap(),
            ],
        );
        assert_eq!(r.route_rules_for("foo").len(), 1);
        assert_eq!(r.route_rules_for("foo")[0].target(), "api/index.php");
        assert_eq!(
            r.route_rules_for("blog").len(),
            1,
            "a parked site's rules key by document root but land under its name"
        );
        assert!(r.route_rules_for("absent").is_empty());
    }

    #[test]
    fn real_site_beats_a_same_named_proxy() {
        let mut cfg = cfg_with_tld("test");
        cfg.proxies
            .push(yerd_core::ProxySite::new("app", target("http://127.0.0.1:3000")).unwrap());
        let r = build(&cfg, vec![linked("app", "/srv/app")]);
        assert!(matches!(
            r.resolve_route("app.test"),
            Some(yerd_core::Route::Php(_))
        ));
        let cols = collisions(&cfg, vec![linked("app", "/srv/app")]);
        assert!(cols
            .iter()
            .any(|c| c.winner == "app" && c.losers.iter().any(|l| l == "proxy:app")));
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
    fn wildcard_only_site_with_shadowed_apex_has_a_hijacked_primary_fqdn() {
        // `a` owns only its wildcard `*.a` after `b` explicitly claims exact `a`,
        // so `a`'s primary FQDN resolves to `b`. This is why `tunnel::resolve_site`
        // verifies the primary routes back to the site before using it as an origin.
        let mut cfg = cfg_with_tld("test");
        cfg.domains.linked.insert(
            "a".into(),
            DomainDelta {
                added: vec![d("*.a")],
                suppressed: vec![],
                primary: None,
            },
        );
        cfg.domains.linked.insert(
            "b".into(),
            DomainDelta {
                added: vec![d("a")],
                suppressed: vec![],
                primary: None,
            },
        );
        let r = build(&cfg, vec![linked("a", "/srv/a"), linked("b", "/srv/b")]);
        assert_eq!(r.resolve("x.a.test").map(Site::name), Some("a"));
        assert_eq!(r.primary_fqdn("a"), "a.test");
        assert_eq!(r.resolve("a.test").map(Site::name), Some("b"));
    }

    #[test]
    fn collisions_reports_two_sites_claiming_one_explicit_domain() {
        let mut cfg = cfg_with_tld("test");
        cfg.domains.parked.insert(
            "/srv/a".into(),
            DomainDelta {
                added: vec![d("corp")],
                suppressed: vec![],
                primary: None,
            },
        );
        cfg.domains.parked.insert(
            "/srv/b".into(),
            DomainDelta {
                added: vec![d("corp")],
                suppressed: vec![],
                primary: None,
            },
        );
        let sites = vec![
            Site::parked("a", "/srv/a", v()).unwrap(),
            Site::parked("b", "/srv/b", v()).unwrap(),
        ];
        let cs = collisions(&cfg, sites);
        assert_eq!(cs.len(), 1, "only `corp` collides; apexes a/b are unique");
        let corp = cs.first().expect("collision");
        assert_eq!(corp.winner, "a");
        assert_eq!(corp.losers, vec!["b".to_owned()]);
    }

    #[test]
    fn collisions_empty_for_apex_and_wildcard_on_different_sites() {
        let mut cfg = cfg_with_tld("test");
        cfg.domains.linked.insert(
            "wild".into(),
            DomainDelta {
                added: vec![d("*.foo")],
                suppressed: vec![],
                primary: None,
            },
        );
        let sites = vec![linked("foo", "/srv/foo"), linked("wild", "/srv/wild")];
        assert!(collisions(&cfg, sites).is_empty());
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

    #[test]
    fn proxy_delta_adds_exact_and_wildcard_domains() {
        let mut cfg = cfg_with_tld("test");
        cfg.proxies.push(proxy("account-dev"));
        cfg.domains.proxy.insert(
            "account-dev".into(),
            added(&["custom-domain", "*.account-dev"]),
        );
        let r = build(&cfg, vec![]);
        assert!(is_proxy(&r, "account-dev.test"));
        assert!(is_proxy(&r, "custom-domain.test"));
        assert!(is_proxy(&r, "api.account-dev.test"));
        assert_eq!(r.primary_fqdn("account-dev"), "account-dev.test");
    }

    #[test]
    fn proxy_keeps_its_other_domains_when_one_collides() {
        let mut cfg = cfg_with_tld("test");
        cfg.proxies.push(proxy("api"));
        cfg.domains.linked.insert("shop".into(), added(&["corp"]));
        cfg.domains
            .proxy
            .insert("api".into(), added(&["corp", "extra"]));
        let r = build(&cfg, vec![linked("shop", "/srv/shop")]);
        assert_eq!(r.resolve("corp.test").map(Site::name), Some("shop"));
        assert!(is_proxy(&r, "api.test"));
        assert!(is_proxy(&r, "extra.test"));
    }

    #[test]
    fn explicit_proxy_domain_beats_an_implicit_site_apex() {
        let mut cfg = cfg_with_tld("test");
        cfg.proxies.push(proxy("api"));
        cfg.domains.proxy.insert("api".into(), added(&["blog"]));
        let r = build(&cfg, vec![linked("blog", "/srv/blog")]);
        assert!(is_proxy(&r, "blog.test"));
        assert_eq!(r.apex_shadowed_by("blog"), Some("api"));
        assert!(is_proxy(&r, "api.test"));
    }

    #[test]
    fn proxy_versus_proxy_collision_resolves_in_config_order() {
        let mut cfg = cfg_with_tld("test");
        cfg.proxies.push(proxy("first"));
        cfg.proxies.push(proxy("second"));
        cfg.domains.proxy.insert("first".into(), added(&["shared"]));
        cfg.domains
            .proxy
            .insert("second".into(), added(&["shared"]));
        let r = build(&cfg, vec![]);
        assert_eq!(r.domain_owner(&d("shared")), Some("first"));
        assert!(is_proxy(&r, "second.test"));
    }

    #[test]
    fn name_shadowed_proxy_never_owns_its_extra_domains() {
        let mut cfg = cfg_with_tld("test");
        cfg.proxies.push(proxy("app"));
        cfg.domains.proxy.insert("app".into(), added(&["corp"]));
        let parked_app = || Site::parked("app", "/srv/app", v()).unwrap();

        let r = build(&cfg, vec![parked_app()]);
        assert_eq!(r.resolve("app.test").map(Site::name), Some("app"));
        assert_eq!(
            r.domain_owner(&d("corp")),
            None,
            "the dropped proxy must not be left owning `corp`"
        );
        assert!(r.resolve_route("corp.test").is_none());

        let r = build(&cfg, vec![parked_app(), linked("corp", "/srv/corp")]);
        assert_eq!(r.resolve("corp.test").map(Site::name), Some("corp"));
    }

    #[test]
    fn collisions_label_proxy_winners_and_losers() {
        let mut cfg = cfg_with_tld("test");
        cfg.proxies.push(proxy("api"));
        cfg.proxies.push(proxy("shop"));
        cfg.domains.proxy.insert("api".into(), added(&["blog"]));
        cfg.domains.proxy.insert("shop".into(), added(&["blog"]));
        let cs = collisions(&cfg, vec![linked("blog", "/srv/blog")]);
        assert_eq!(cs.len(), 1);
        let blog = cs.first().expect("collision");
        assert_eq!(blog.winner, "proxy:api");
        assert_eq!(
            blog.losers,
            vec!["blog".to_owned(), "proxy:shop".to_owned()]
        );
    }
}
