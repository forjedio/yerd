//! Pure mutation logic for the daemon's IPC mutation handlers.
//!
//! This module decides *how a [`Request`] changes the config* and is
//! deliberately I/O-free: paths arrive already canonicalised, the live router
//! and config are borrowed, and nothing here touches the filesystem, clock, or
//! environment. The thin I/O wrapper that canonicalises paths, validates,
//! persists, and swaps the live router lives in [`crate::ipc_server`].
//!
//! ## Name normalisation
//!
//! The router and `cfg.linked` are keyed by the **lowercased** site name
//! (`scan_sites` lowercases discovered directory names; the `Site`
//! constructors lowercase too). Unlike the proxy's host path, the IPC mutation
//! path has no `host::normalise`, so [`apply`] lowercases the request `name`
//! itself before every lookup - otherwise `yerd use Blog 8.4` would look up
//! `"Blog"`, miss the stored `"blog"`, and wrongly report "not found".

use std::path::PathBuf;

use yerd_config::{Config, DomainDelta};
use yerd_core::{Domain, PhpVersion, Site, SiteRouter};
use yerd_ipc::{ErrorCode, Request};

/// A mutation that could not be applied. The inner string is a
/// human-readable message; [`error_code`] maps the variant to the wire
/// [`ErrorCode`].
#[derive(Debug, thiserror::Error)]
pub enum MutateError {
    /// The named site (or resource) does not exist.
    #[error("{0}")]
    NotFound(String),
    /// A site with that name is already registered.
    #[error("{0}")]
    AlreadyExists(String),
    /// The request was structurally rejected (bad path or bad site name).
    #[error("{0}")]
    Invalid(String),
}

/// A successfully applied mutation, carrying a one-line human summary for the
/// CLI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Applied {
    /// Short human-readable description of what changed.
    pub summary: String,
}

/// The `cfg.overrides` key for a parked site: its `document_root` stringified
/// with `to_string_lossy`. This MUST match the key `startup::scan_sites`
/// computes when it re-applies overrides - both derive from the same
/// `DirEntry::path()` (the router's parked site was built from it, and
/// `Site::document_root` returns it verbatim, uncanonicalised), so the strings
/// are byte-identical. Do not canonicalise one side independently.
fn override_key(site: &Site) -> String {
    site.document_root().to_string_lossy().into_owned()
}

/// Apply a mutation [`Request`] to `cfg` in place.
///
/// `router` is the **pre-mutation** live router - read here so a `SetPhp` on a
/// parked site can recover that site's `document_root`. `canonical` is the
/// already-canonicalised directory for `Park`/`Link`. `default_php` is the
/// version assigned to newly linked sites.
///
/// Pure: no filesystem, clock, or environment access. Only the site- and
/// group-mutation variants are handled; anything else is [`MutateError::Invalid`]
/// (the I/O wrapper never routes `Ping`/`ListSites` here). The group variants
/// ignore `router`/`canonical`/`default_php` - groups are a config-only overlay.
pub fn apply(
    cfg: &mut Config,
    router: &SiteRouter,
    req: &Request,
    canonical: Option<PathBuf>,
    default_php: PhpVersion,
) -> Result<Applied, MutateError> {
    match req {
        Request::Park { .. } => apply_park(cfg, canonical),
        Request::Link { name, .. } => apply_link(cfg, name, canonical, default_php),
        Request::Unpark { path } => Ok(apply_unpark(cfg, path)),
        Request::Unlink { name } => apply_unlink(cfg, router, name),
        Request::SetPhp { name, version } => apply_set_php(cfg, router, name, *version),
        Request::SetSecure { name, secure } => apply_set_secure(cfg, router, name, *secure),
        Request::SetWordpressAutoLogin {
            name,
            enabled,
            user,
        } => apply_set_wordpress_auto_login(cfg, router, name, *enabled, user.clone()),
        Request::AddDomain { name, domain } => apply_add_domain(cfg, router, name, domain),
        Request::RemoveDomain { name, domain } => apply_remove_domain(cfg, router, name, domain),
        Request::SetPrimaryDomain { name, domain } => {
            apply_set_primary_domain(cfg, router, name, domain)
        }
        Request::ResetDomains { name } => apply_reset_domains(cfg, router, name),
        Request::CreateGroup { name } => apply_create_group(cfg, name),
        Request::DeleteGroup { name } => Ok(apply_delete_group(cfg, name)),
        Request::SetGroupOrder { order } => apply_set_group_order(cfg, order),
        Request::SetSiteGroup { site, group } => apply_set_site_group(cfg, site, group.as_deref()),
        Request::RenameGroup { from, to } => apply_rename_group(cfg, from, to),
        _ => Err(MutateError::Invalid("unsupported request".into())),
    }
}

fn apply_park(cfg: &mut Config, canonical: Option<PathBuf>) -> Result<Applied, MutateError> {
    let path = canonical.ok_or_else(|| MutateError::Invalid("park requires a path".into()))?;
    let stored = path.to_string_lossy().into_owned();
    let added = cfg.parked.paths.insert(stored.clone());
    Ok(Applied {
        summary: if added {
            format!("parked {stored}")
        } else {
            format!("already parked {stored}")
        },
    })
}

fn apply_link(
    cfg: &mut Config,
    name: &str,
    canonical: Option<PathBuf>,
    default_php: PhpVersion,
) -> Result<Applied, MutateError> {
    let path = canonical.ok_or_else(|| MutateError::Invalid("link requires a path".into()))?;
    let site = Site::linked(name, path, default_php)
        .map_err(|e| MutateError::Invalid(format!("invalid site name: {e}")))?;
    let name_lc = site.name().to_owned();
    if cfg.linked.iter().any(|s| s.name() == name_lc) {
        return Err(MutateError::AlreadyExists(format!(
            "site already linked: {name_lc}"
        )));
    }
    // Promote any parked-side domain delta (keyed by document-root) to the linked
    // side (keyed by name), so a customised parked site keeps its domains when
    // linked. See NEW-C in the plan.
    let docroot_key = override_key(&site);
    if let Some(delta) = cfg.domains.parked.remove(&docroot_key) {
        cfg.domains.linked.insert(name_lc.clone(), delta);
    }
    cfg.linked.push(site);
    Ok(Applied {
        summary: format!("linked {name_lc}"),
    })
}

/// Operates on the request `path` verbatim (not `canonical`): parked roots are
/// stored as the canonical String produced at park time, so an exact `remove` is
/// an identity match. Deliberately *not* canonicalised by the I/O wrapper, so a
/// root deleted from disk is still removable. Idempotent - an absent path is a
/// successful no-op, mirroring `Park`'s insert.
fn apply_unpark(cfg: &mut Config, path: &str) -> Applied {
    let removed = cfg.parked.paths.remove(path);
    // Drop parked-side domain deltas for sites under this root (their document
    // roots are the root itself or a child), so a later re-park of the root does
    // not inherit stale domains.
    cfg.domains
        .parked
        .retain(|docroot, _| !is_under_root(docroot, path));
    Applied {
        summary: if removed {
            format!("un-parked {path}")
        } else {
            format!("{path} was not parked")
        },
    }
}

/// True when `docroot` is `root` itself or a path directly beneath it. Pure
/// string containment (no filesystem): a parked site's document root is
/// `<root>/<dir>`, so `docroot == root` or `docroot` starts with `root` plus a
/// path separator.
fn is_under_root(docroot: &str, root: &str) -> bool {
    if docroot == root {
        return true;
    }
    let sep = std::path::MAIN_SEPARATOR;
    docroot
        .strip_prefix(root)
        .is_some_and(|rest| rest.starts_with(sep))
}

fn apply_unlink(cfg: &mut Config, router: &SiteRouter, name: &str) -> Result<Applied, MutateError> {
    let name_lc = name.to_ascii_lowercase();
    if cfg.linked.iter().any(|s| s.name() == name_lc) {
        cfg.linked.retain(|s| s.name() != name_lc);
        // Drop the site's linked-side domain delta. The reverse migration
        // (linked -> parked, when the directory is still parked) is left to the
        // I/O wrapper, which can check parked-ness; here the delta simply resets.
        cfg.domains.linked.remove(&name_lc);
        Ok(Applied {
            summary: format!("unlinked {name_lc}"),
        })
    } else if router.get(&name_lc).is_some() {
        Err(MutateError::NotFound(format!(
            "{name_lc} is a parked site, not linked — unpark its directory instead"
        )))
    } else {
        Err(MutateError::NotFound(format!("no site named {name_lc}")))
    }
}

fn apply_set_php(
    cfg: &mut Config,
    router: &SiteRouter,
    name: &str,
    version: PhpVersion,
) -> Result<Applied, MutateError> {
    let name_lc = name.to_ascii_lowercase();
    if let Some(site) = cfg.linked.iter_mut().find(|s| s.name() == name_lc) {
        site.set_php(version);
        Ok(Applied {
            summary: format!("{name_lc} now uses PHP {version}"),
        })
    } else if let Some(parked) = router.get(&name_lc) {
        let key = override_key(parked);
        cfg.overrides.entry(key).or_default().php = Some(version);
        Ok(Applied {
            summary: format!("{name_lc} now uses PHP {version}"),
        })
    } else {
        Err(MutateError::NotFound(format!("no site named {name_lc}")))
    }
}

fn apply_set_secure(
    cfg: &mut Config,
    router: &SiteRouter,
    name: &str,
    secure: bool,
) -> Result<Applied, MutateError> {
    let name_lc = name.to_ascii_lowercase();
    if let Some(site) = cfg.linked.iter_mut().find(|s| s.name() == name_lc) {
        site.set_secure(secure);
        Ok(Applied {
            summary: format!("{name_lc} secure={secure}"),
        })
    } else if let Some(parked) = router.get(&name_lc) {
        let key = override_key(parked);
        cfg.overrides.entry(key).or_default().secure = Some(secure);
        Ok(Applied {
            summary: format!("{name_lc} secure={secure}"),
        })
    } else {
        Err(MutateError::NotFound(format!("no site named {name_lc}")))
    }
}

fn apply_set_wordpress_auto_login(
    cfg: &mut Config,
    router: &SiteRouter,
    name: &str,
    enabled: bool,
    user: Option<String>,
) -> Result<Applied, MutateError> {
    let name_lc = name.to_ascii_lowercase();
    if let Some(site) = cfg.linked.iter_mut().find(|s| s.name() == name_lc) {
        site.set_wp_auto_login(enabled);
        site.set_wp_auto_login_user(user);
        Ok(Applied {
            summary: format!("{name_lc} wp_auto_login={enabled}"),
        })
    } else if let Some(parked) = router.get(&name_lc) {
        let key = override_key(parked);
        let ov = cfg.overrides.entry(key).or_default();
        ov.wp_auto_login = Some(enabled);
        ov.wp_auto_login_user = user;
        Ok(Applied {
            summary: format!("{name_lc} wp_auto_login={enabled}"),
        })
    } else {
        Err(MutateError::NotFound(format!("no site named {name_lc}")))
    }
}

/// Which `[domains]` map (and key) a site's delta lives in: linked sites key by
/// name, parked sites by document-root (mirroring `overrides`).
enum DomainTarget {
    Linked(String),
    Parked(String),
}

/// Locate a site (linked first, then parked via the router) and return where its
/// domain delta is stored. `NotFound` when no such site exists.
fn resolve_domain_target(
    cfg: &Config,
    router: &SiteRouter,
    name_lc: &str,
) -> Result<DomainTarget, MutateError> {
    if cfg.linked.iter().any(|s| s.name() == name_lc) {
        Ok(DomainTarget::Linked(name_lc.to_owned()))
    } else if let Some(parked) = router.get(name_lc) {
        Ok(DomainTarget::Parked(override_key(parked)))
    } else {
        Err(MutateError::NotFound(format!("no site named {name_lc}")))
    }
}

fn delta_mut<'a>(cfg: &'a mut Config, target: &DomainTarget) -> &'a mut DomainDelta {
    match target {
        DomainTarget::Linked(name) => cfg.domains.linked.entry(name.clone()).or_default(),
        DomainTarget::Parked(key) => cfg.domains.parked.entry(key.clone()).or_default(),
    }
}

/// Drop the delta entry entirely if it carries no customisation, so an
/// effectively-default site leaves no `[domains]` record.
fn prune_delta(cfg: &mut Config, target: &DomainTarget) {
    match target {
        DomainTarget::Linked(name) => {
            if cfg
                .domains
                .linked
                .get(name)
                .is_some_and(DomainDelta::is_empty)
            {
                cfg.domains.linked.remove(name);
            }
        }
        DomainTarget::Parked(key) => {
            if cfg
                .domains
                .parked
                .get(key)
                .is_some_and(DomainDelta::is_empty)
            {
                cfg.domains.parked.remove(key);
            }
        }
    }
}

/// Parse a full-FQDN domain under the config TLD, mapping failures to `Invalid`.
fn parse_domain(cfg: &Config, domain: &str) -> Result<Domain, MutateError> {
    Domain::parse(domain, cfg.tld.as_str())
        .map_err(|e| MutateError::Invalid(format!("invalid domain: {e}")))
}

/// Reject a domain already routed to a **different** site (the pre-mutation
/// router is authoritative). Same-site ownership is fine (idempotent).
fn reject_if_claimed_elsewhere(
    router: &SiteRouter,
    name_lc: &str,
    dom: &Domain,
    tld: &str,
) -> Result<(), MutateError> {
    if let Some(owner) = router.domain_owner(dom) {
        if owner != name_lc {
            return Err(MutateError::AlreadyExists(format!(
                "{} already routes to {owner}",
                dom.to_fqdn(tld)
            )));
        }
    }
    Ok(())
}

/// Number of exact (non-wildcard) domains a delta yields, ignoring
/// zero-exact normalization (used to enforce the "keep >= 1 exact" rule at
/// mutation time). The apex counts unless suppressed.
fn exact_count(name_lc: &str, added: &[Domain], suppressed: &[Domain]) -> usize {
    let apex = Domain::apex(name_lc);
    let has_apex = usize::from(!suppressed.contains(&apex));
    has_apex + added.iter().filter(|d| !d.is_wildcard()).count()
}

fn apply_add_domain(
    cfg: &mut Config,
    router: &SiteRouter,
    name: &str,
    domain: &str,
) -> Result<Applied, MutateError> {
    let name_lc = name.to_ascii_lowercase();
    let target = resolve_domain_target(cfg, router, &name_lc)?;
    let dom = parse_domain(cfg, domain)?;
    let tld = cfg.tld.as_str().to_owned();
    reject_if_claimed_elsewhere(router, &name_lc, &dom, &tld)?;

    let apex = Domain::apex(&name_lc);
    let fqdn = dom.to_fqdn(&tld);
    let delta = delta_mut(cfg, &target);
    if dom == apex {
        delta.suppressed.retain(|d| d != &apex);
    } else if !delta.added.contains(&dom) {
        delta.added.push(dom);
    }
    prune_delta(cfg, &target);
    Ok(Applied {
        summary: format!("added {fqdn} to {name_lc}"),
    })
}

fn apply_remove_domain(
    cfg: &mut Config,
    router: &SiteRouter,
    name: &str,
    domain: &str,
) -> Result<Applied, MutateError> {
    let name_lc = name.to_ascii_lowercase();
    let target = resolve_domain_target(cfg, router, &name_lc)?;
    let dom = parse_domain(cfg, domain)?;
    let tld = cfg.tld.as_str().to_owned();
    let apex = Domain::apex(&name_lc);
    let fqdn = dom.to_fqdn(&tld);

    let delta = delta_mut(cfg, &target);
    let mut added = delta.added.clone();
    let mut suppressed = delta.suppressed.clone();
    if added.contains(&dom) {
        added.retain(|d| d != &dom);
    } else if dom == apex {
        if !suppressed.contains(&apex) {
            suppressed.push(apex.clone());
        }
    } else {
        return Err(MutateError::Invalid(format!(
            "{fqdn} is not a domain of {name_lc}"
        )));
    }
    if exact_count(&name_lc, &added, &suppressed) == 0 {
        return Err(MutateError::Invalid(format!(
            "{name_lc} must keep at least one exact domain"
        )));
    }

    delta.added = added;
    delta.suppressed = suppressed;
    if delta.primary.as_ref() == Some(&dom) {
        delta.primary = None;
    }
    prune_delta(cfg, &target);
    Ok(Applied {
        summary: format!("removed {fqdn} from {name_lc}"),
    })
}

fn apply_set_primary_domain(
    cfg: &mut Config,
    router: &SiteRouter,
    name: &str,
    domain: &str,
) -> Result<Applied, MutateError> {
    let name_lc = name.to_ascii_lowercase();
    let target = resolve_domain_target(cfg, router, &name_lc)?;
    let dom = parse_domain(cfg, domain)?;
    if dom.is_wildcard() {
        return Err(MutateError::Invalid(
            "a primary domain must be exact, not a wildcard".into(),
        ));
    }
    let tld = cfg.tld.as_str().to_owned();
    reject_if_claimed_elsewhere(router, &name_lc, &dom, &tld)?;

    let apex = Domain::apex(&name_lc);
    let fqdn = dom.to_fqdn(&tld);
    let delta = delta_mut(cfg, &target);
    if dom == apex {
        // The apex is the natural primary; keep it active and let it derive.
        delta.suppressed.retain(|d| d != &apex);
        delta.primary = None;
    } else {
        if !delta.added.contains(&dom) {
            delta.added.push(dom.clone());
        }
        delta.primary = Some(dom);
    }
    prune_delta(cfg, &target);
    Ok(Applied {
        summary: format!("{name_lc} primary domain is {fqdn}"),
    })
}

fn apply_reset_domains(
    cfg: &mut Config,
    router: &SiteRouter,
    name: &str,
) -> Result<Applied, MutateError> {
    let name_lc = name.to_ascii_lowercase();
    let target = resolve_domain_target(cfg, router, &name_lc)?;
    match target {
        DomainTarget::Linked(n) => {
            cfg.domains.linked.remove(&n);
        }
        DomainTarget::Parked(k) => {
            cfg.domains.parked.remove(&k);
        }
    }
    Ok(Applied {
        summary: format!("{name_lc} domains reset to default"),
    })
}

/// Create a site group, appended last in display order. Rejects an empty name,
/// the reserved `Unallocated` (case-insensitive), and a case-insensitive
/// duplicate of an existing group. The entered case is preserved. Group names
/// are display strings (validated cross-field by `Config::validate` too).
fn apply_create_group(cfg: &mut Config, name: &str) -> Result<Applied, MutateError> {
    let name = name.trim();
    if name.is_empty() {
        return Err(MutateError::Invalid("group name must not be empty".into()));
    }
    if name.eq_ignore_ascii_case(yerd_config::RESERVED_GROUP_NAME) {
        return Err(MutateError::Invalid(format!(
            "\"{}\" is a reserved group name",
            yerd_config::RESERVED_GROUP_NAME
        )));
    }
    if cfg
        .groups
        .order
        .iter()
        .any(|g| g.eq_ignore_ascii_case(name))
    {
        return Err(MutateError::AlreadyExists(format!(
            "group already exists: {name}"
        )));
    }
    cfg.groups.order.push(name.to_owned());
    Ok(Applied {
        summary: format!("created group {name}"),
    })
}

/// Delete a site group (matched ASCII-case-insensitively, like create/assign) and
/// drop every membership pointing at it, so its sites fall back to the synthetic
/// "Unallocated" bucket. Idempotent - an absent group is a successful no-op.
fn apply_delete_group(cfg: &mut Config, name: &str) -> Applied {
    let existed = cfg
        .groups
        .order
        .iter()
        .any(|g| g.eq_ignore_ascii_case(name));
    cfg.groups.order.retain(|g| !g.eq_ignore_ascii_case(name));
    cfg.groups
        .members
        .retain(|_, g| !g.eq_ignore_ascii_case(name));
    Applied {
        summary: if existed {
            format!("deleted group {name}")
        } else {
            format!("{name} was not a group")
        },
    }
}

/// Replace the group display order. `order` must be an exact permutation of the
/// current group names (same multiset), so it can only reorder - never add,
/// drop, or rename a group.
fn apply_set_group_order(cfg: &mut Config, order: &[String]) -> Result<Applied, MutateError> {
    let mut want: Vec<&str> = order.iter().map(String::as_str).collect();
    let mut have: Vec<&str> = cfg.groups.order.iter().map(String::as_str).collect();
    want.sort_unstable();
    have.sort_unstable();
    if want != have {
        return Err(MutateError::Invalid(
            "group order must be a permutation of the existing groups".into(),
        ));
    }
    cfg.groups.order = order.to_vec();
    Ok(Applied {
        summary: "reordered groups".into(),
    })
}

/// Set or clear a site's group membership (a site belongs to at most one group).
/// `Some(group)` must name an existing group (matched ASCII-case-insensitively);
/// the **canonical stored casing** from `order` is what's recorded, so a member
/// value always exactly equals its `order` entry (the GUI keys sections off that
/// exact string). `None` moves the site to "Unallocated". The `site` key is
/// lowercased to match the router's lowercased site identities. Membership is
/// intentionally not validated against live sites (a transiently-unscanned parked
/// site keeps its group), mirroring `overrides`.
fn apply_set_site_group(
    cfg: &mut Config,
    site: &str,
    group: Option<&str>,
) -> Result<Applied, MutateError> {
    let site_lc = site.to_ascii_lowercase();
    if let Some(g) = group {
        let canonical = match cfg
            .groups
            .order
            .iter()
            .find(|existing| existing.eq_ignore_ascii_case(g))
        {
            Some(c) => c.clone(),
            None => return Err(MutateError::NotFound(format!("no group named {g}"))),
        };
        cfg.groups
            .members
            .insert(site_lc.clone(), canonical.clone());
        Ok(Applied {
            summary: format!("{site_lc} added to {canonical}"),
        })
    } else {
        cfg.groups.members.remove(&site_lc);
        Ok(Applied {
            summary: format!("{site_lc} ungrouped"),
        })
    }
}

/// Rename a site group in place, keeping its display position and moving every
/// member with it. The new name is validated like `apply_create_group` (trimmed,
/// non-empty, not the reserved `Unallocated`), except a case-insensitive
/// collision is only rejected against a *different* group, so a case-only rename
/// (`blog` -> `Blog`) is allowed. `NotFound` if `from` names no group. The
/// entered case of `to` becomes the canonical casing in both `order` and every
/// matching `members` value (so members keep exactly equalling their `order`
/// entry, as `apply_set_site_group` documents).
fn apply_rename_group(cfg: &mut Config, from: &str, to: &str) -> Result<Applied, MutateError> {
    let to = to.trim();
    if to.is_empty() {
        return Err(MutateError::Invalid("group name must not be empty".into()));
    }
    if to.eq_ignore_ascii_case(yerd_config::RESERVED_GROUP_NAME) {
        return Err(MutateError::Invalid(format!(
            "\"{}\" is a reserved group name",
            yerd_config::RESERVED_GROUP_NAME
        )));
    }
    let idx = cfg
        .groups
        .order
        .iter()
        .position(|g| g.eq_ignore_ascii_case(from))
        .ok_or_else(|| MutateError::NotFound(format!("no group named {from}")))?;
    if cfg
        .groups
        .order
        .iter()
        .enumerate()
        .any(|(i, g)| i != idx && g.eq_ignore_ascii_case(to))
    {
        return Err(MutateError::AlreadyExists(format!(
            "group already exists: {to}"
        )));
    }
    let Some(slot) = cfg.groups.order.get_mut(idx) else {
        return Err(MutateError::NotFound(format!("no group named {from}")));
    };
    to.clone_into(slot);
    for g in cfg.groups.members.values_mut() {
        if g.eq_ignore_ascii_case(from) {
            to.clone_into(g);
        }
    }
    Ok(Applied {
        summary: format!("renamed group {from} to {to}"),
    })
}

/// Map a [`MutateError`] to the wire [`ErrorCode`]. `Invalid` collapses to
/// `InvalidPath` (the frozen `ErrorCode` set has no `InvalidName`; the CLI
/// validates names client-side so users get a precise message).
#[must_use]
pub fn error_code(e: &MutateError) -> ErrorCode {
    match e {
        MutateError::NotFound(_) => ErrorCode::NotFound,
        MutateError::AlreadyExists(_) => ErrorCode::AlreadyExists,
        MutateError::Invalid(_) => ErrorCode::InvalidPath,
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
    use std::path::Path;
    use yerd_core::{RouterConfig, Tld};

    fn v(major: u8, minor: u8) -> PhpVersion {
        PhpVersion::new(major, minor)
    }

    fn empty_router() -> SiteRouter {
        SiteRouter::new(RouterConfig::with_tld(Tld::new("test").unwrap()))
    }

    fn router_with_parked(name: &str, root: &str) -> SiteRouter {
        let mut r = empty_router();
        r.insert(Site::parked(name, root, v(8, 3)).unwrap())
            .unwrap();
        r
    }

    #[test]
    fn park_adds_path_and_is_idempotent() {
        let mut cfg = Config::default();
        let r = empty_router();
        let req = Request::Park {
            path: PathBuf::from("/ignored"),
        };
        let a = apply(
            &mut cfg,
            &r,
            &req,
            Some(PathBuf::from("/srv/sites")),
            v(8, 3),
        )
        .unwrap();
        assert!(a.summary.starts_with("parked"));
        assert!(cfg.parked.paths.contains("/srv/sites"));
        let a2 = apply(
            &mut cfg,
            &r,
            &req,
            Some(PathBuf::from("/srv/sites")),
            v(8, 3),
        )
        .unwrap();
        assert!(a2.summary.starts_with("already parked"));
        assert_eq!(cfg.parked.paths.len(), 1);
    }

    #[test]
    fn unpark_removes_path_and_is_idempotent() {
        let mut cfg = Config::default();
        let r = empty_router();
        cfg.parked.paths.insert("/srv/sites".to_string());
        cfg.parked.paths.insert("/srv/other".to_string());

        let a = apply(
            &mut cfg,
            &r,
            &Request::Unpark {
                path: "/srv/sites".into(),
            },
            None,
            v(8, 3),
        )
        .unwrap();
        assert!(a.summary.starts_with("un-parked"));
        assert!(!cfg.parked.paths.contains("/srv/sites"));
        assert!(cfg.parked.paths.contains("/srv/other"));

        let a2 = apply(
            &mut cfg,
            &r,
            &Request::Unpark {
                path: "/srv/sites".into(),
            },
            None,
            v(8, 3),
        )
        .unwrap();
        assert!(a2.summary.contains("was not parked"));
        assert_eq!(cfg.parked.paths.len(), 1);
    }

    #[test]
    fn link_adds_then_rejects_duplicate() {
        let mut cfg = Config::default();
        let r = empty_router();
        let req = Request::Link {
            name: "foo".into(),
            path: PathBuf::from("/ignored"),
        };
        apply(&mut cfg, &r, &req, Some(PathBuf::from("/srv/foo")), v(8, 3)).unwrap();
        assert_eq!(cfg.linked.len(), 1);
        assert_eq!(cfg.linked[0].name(), "foo");
        assert_eq!(cfg.linked[0].document_root(), Path::new("/srv/foo"));
        match apply(&mut cfg, &r, &req, Some(PathBuf::from("/srv/foo")), v(8, 3)) {
            Err(MutateError::AlreadyExists(_)) => {}
            other => panic!("expected AlreadyExists, got {other:?}"),
        }
    }

    #[test]
    fn link_rejects_bad_name() {
        let mut cfg = Config::default();
        let r = empty_router();
        let req = Request::Link {
            name: "bad name".into(),
            path: PathBuf::from("/ignored"),
        };
        match apply(&mut cfg, &r, &req, Some(PathBuf::from("/srv/x")), v(8, 3)) {
            Err(MutateError::Invalid(_)) => {}
            other => panic!("expected Invalid, got {other:?}"),
        }
    }

    #[test]
    fn unlink_linked_removes_it() {
        let mut cfg = Config::default();
        let r = empty_router();
        cfg.linked
            .push(Site::linked("foo", "/srv/foo", v(8, 3)).unwrap());
        let a = apply(
            &mut cfg,
            &r,
            &Request::Unlink { name: "foo".into() },
            None,
            v(8, 3),
        )
        .unwrap();
        assert!(a.summary.contains("unlinked"));
        assert!(cfg.linked.is_empty());
    }

    #[test]
    fn unlink_parked_is_not_found_with_hint() {
        let mut cfg = Config::default();
        let r = router_with_parked("blog", "/srv/blog");
        match apply(
            &mut cfg,
            &r,
            &Request::Unlink {
                name: "blog".into(),
            },
            None,
            v(8, 3),
        ) {
            Err(MutateError::NotFound(msg)) => assert!(msg.contains("parked")),
            other => panic!("expected NotFound, got {other:?}"),
        }
    }

    #[test]
    fn unlink_unknown_is_not_found() {
        let mut cfg = Config::default();
        let r = empty_router();
        match apply(
            &mut cfg,
            &r,
            &Request::Unlink {
                name: "nope".into(),
            },
            None,
            v(8, 3),
        ) {
            Err(MutateError::NotFound(_)) => {}
            other => panic!("expected NotFound, got {other:?}"),
        }
    }

    #[test]
    fn setphp_updates_linked_in_place() {
        let mut cfg = Config::default();
        let r = empty_router();
        cfg.linked
            .push(Site::linked("foo", "/srv/foo", v(8, 3)).unwrap());
        apply(
            &mut cfg,
            &r,
            &Request::SetPhp {
                name: "foo".into(),
                version: v(8, 4),
            },
            None,
            v(8, 3),
        )
        .unwrap();
        assert_eq!(cfg.linked.len(), 1);
        assert_eq!(cfg.linked[0].php(), v(8, 4));
    }

    #[test]
    fn setphp_records_override_keeping_parked() {
        let mut cfg = Config::default();
        let r = router_with_parked("blog", "/srv/blog");
        let a = apply(
            &mut cfg,
            &r,
            &Request::SetPhp {
                name: "blog".into(),
                version: v(8, 4),
            },
            None,
            v(8, 3),
        )
        .unwrap();
        assert!(!a.summary.contains("linked"));
        assert!(cfg.linked.is_empty());
        let ov = cfg.overrides.get("/srv/blog").expect("override stored");
        assert_eq!(ov.php, Some(v(8, 4)));
        assert_eq!(ov.secure, None);
    }

    #[test]
    fn upsert_merges_php_and_secure() {
        let mut cfg = Config::default();
        let r = router_with_parked("blog", "/srv/blog");
        apply(
            &mut cfg,
            &r,
            &Request::SetPhp {
                name: "blog".into(),
                version: v(8, 4),
            },
            None,
            v(8, 3),
        )
        .unwrap();
        apply(
            &mut cfg,
            &r,
            &Request::SetSecure {
                name: "blog".into(),
                secure: true,
            },
            None,
            v(8, 3),
        )
        .unwrap();
        assert_eq!(cfg.overrides.len(), 1);
        let ov = cfg.overrides.get("/srv/blog").unwrap();
        assert_eq!(ov.php, Some(v(8, 4)));
        assert_eq!(ov.secure, Some(true));
    }

    #[test]
    fn setphp_unknown_is_not_found() {
        let mut cfg = Config::default();
        let r = empty_router();
        match apply(
            &mut cfg,
            &r,
            &Request::SetPhp {
                name: "ghost".into(),
                version: v(8, 4),
            },
            None,
            v(8, 3),
        ) {
            Err(MutateError::NotFound(_)) => {}
            other => panic!("expected NotFound, got {other:?}"),
        }
    }

    #[test]
    fn setsecure_updates_linked_in_place() {
        let mut cfg = Config::default();
        let r = empty_router();
        cfg.linked
            .push(Site::linked("foo", "/srv/foo", v(8, 3)).unwrap());
        apply(
            &mut cfg,
            &r,
            &Request::SetSecure {
                name: "foo".into(),
                secure: true,
            },
            None,
            v(8, 3),
        )
        .unwrap();
        assert_eq!(cfg.linked.len(), 1);
        assert!(cfg.linked[0].secure());

        apply(
            &mut cfg,
            &r,
            &Request::SetSecure {
                name: "foo".into(),
                secure: false,
            },
            None,
            v(8, 3),
        )
        .unwrap();
        assert_eq!(cfg.linked.len(), 1);
        assert!(!cfg.linked[0].secure());
    }

    #[test]
    fn setsecure_records_override_keeping_parked() {
        let mut cfg = Config::default();
        let r = router_with_parked("blog", "/srv/blog");
        let a = apply(
            &mut cfg,
            &r,
            &Request::SetSecure {
                name: "blog".into(),
                secure: true,
            },
            None,
            v(8, 4),
        )
        .unwrap();
        assert!(!a.summary.contains("linked"));
        assert!(cfg.linked.is_empty());
        let ov = cfg.overrides.get("/srv/blog").expect("override stored");
        assert_eq!(ov.secure, Some(true));
        assert_eq!(ov.php, None);
    }

    #[test]
    fn setsecure_false_is_stored_verbatim() {
        let mut cfg = Config::default();
        let r = router_with_parked("blog", "/srv/blog");
        apply(
            &mut cfg,
            &r,
            &Request::SetSecure {
                name: "blog".into(),
                secure: false,
            },
            None,
            v(8, 3),
        )
        .unwrap();
        assert_eq!(cfg.overrides.get("/srv/blog").unwrap().secure, Some(false));
    }

    #[test]
    fn setsecure_unknown_is_not_found() {
        let mut cfg = Config::default();
        let r = empty_router();
        match apply(
            &mut cfg,
            &r,
            &Request::SetSecure {
                name: "ghost".into(),
                secure: true,
            },
            None,
            v(8, 3),
        ) {
            Err(MutateError::NotFound(_)) => {}
            other => panic!("expected NotFound, got {other:?}"),
        }
    }

    #[test]
    fn set_wordpress_auto_login_updates_linked_in_place() {
        let mut cfg = Config::default();
        let r = empty_router();
        cfg.linked
            .push(Site::linked("blog", "/srv/blog", v(8, 3)).unwrap());
        apply(
            &mut cfg,
            &r,
            &Request::SetWordpressAutoLogin {
                name: "blog".into(),
                enabled: true,
                user: Some("editor".into()),
            },
            None,
            v(8, 3),
        )
        .unwrap();
        assert_eq!(cfg.linked.len(), 1);
        assert!(cfg.linked[0].wp_auto_login());
        assert_eq!(cfg.linked[0].wp_auto_login_user(), Some("editor"));

        apply(
            &mut cfg,
            &r,
            &Request::SetWordpressAutoLogin {
                name: "blog".into(),
                enabled: false,
                user: None,
            },
            None,
            v(8, 3),
        )
        .unwrap();
        assert!(!cfg.linked[0].wp_auto_login());
        assert_eq!(cfg.linked[0].wp_auto_login_user(), None);
    }

    #[test]
    fn set_wordpress_auto_login_records_override_keeping_parked() {
        let mut cfg = Config::default();
        let r = router_with_parked("blog", "/srv/blog");
        let a = apply(
            &mut cfg,
            &r,
            &Request::SetWordpressAutoLogin {
                name: "blog".into(),
                enabled: true,
                user: Some("editor".into()),
            },
            None,
            v(8, 3),
        )
        .unwrap();
        assert!(!a.summary.contains("linked"));
        assert!(cfg.linked.is_empty());
        let ov = cfg.overrides.get("/srv/blog").expect("override stored");
        assert_eq!(ov.wp_auto_login, Some(true));
        assert_eq!(ov.wp_auto_login_user.as_deref(), Some("editor"));
    }

    #[test]
    fn set_wordpress_auto_login_unknown_is_not_found() {
        let mut cfg = Config::default();
        let r = empty_router();
        match apply(
            &mut cfg,
            &r,
            &Request::SetWordpressAutoLogin {
                name: "ghost".into(),
                enabled: true,
                user: None,
            },
            None,
            v(8, 3),
        ) {
            Err(MutateError::NotFound(_)) => {}
            other => panic!("expected NotFound, got {other:?}"),
        }
    }

    #[test]
    fn mixed_case_name_resolves_lowercased_site() {
        let mut cfg = Config::default();
        let r = router_with_parked("blog", "/srv/blog");
        apply(
            &mut cfg,
            &r,
            &Request::SetPhp {
                name: "Blog".into(),
                version: v(8, 4),
            },
            None,
            v(8, 3),
        )
        .unwrap();
        assert!(cfg.linked.is_empty());
        assert_eq!(cfg.overrides.get("/srv/blog").unwrap().php, Some(v(8, 4)));

        cfg.linked
            .push(Site::linked("foo", "/srv/foo", v(8, 3)).unwrap());
        apply(
            &mut cfg,
            &r,
            &Request::Unlink { name: "FOO".into() },
            None,
            v(8, 3),
        )
        .unwrap();
        assert!(cfg.linked.iter().all(|s| s.name() != "foo"));
    }

    // ------------------ groups ------------------

    fn create_group(cfg: &mut Config, name: &str) -> Result<Applied, MutateError> {
        let r = empty_router();
        apply(
            cfg,
            &r,
            &Request::CreateGroup { name: name.into() },
            None,
            v(8, 3),
        )
    }

    #[test]
    fn create_group_appends_in_order() {
        let mut cfg = Config::default();
        create_group(&mut cfg, "Blog").unwrap();
        create_group(&mut cfg, "Shop").unwrap();
        assert_eq!(
            cfg.groups.order,
            vec!["Blog".to_string(), "Shop".to_string()]
        );
    }

    #[test]
    fn create_group_rejects_empty_reserved_and_duplicate() {
        let mut cfg = Config::default();
        assert!(matches!(
            create_group(&mut cfg, "   "),
            Err(MutateError::Invalid(_))
        ));
        assert!(matches!(
            create_group(&mut cfg, "unallocated"),
            Err(MutateError::Invalid(_))
        ));
        create_group(&mut cfg, "Blog").unwrap();
        assert!(matches!(
            create_group(&mut cfg, "blog"),
            Err(MutateError::AlreadyExists(_))
        ));
        assert_eq!(cfg.groups.order, vec!["Blog".to_string()]);
    }

    #[test]
    fn create_group_trims_name() {
        let mut cfg = Config::default();
        create_group(&mut cfg, "  Blog  ").unwrap();
        assert_eq!(cfg.groups.order, vec!["Blog".to_string()]);
    }

    #[test]
    fn delete_group_moves_members_to_unallocated_and_is_idempotent() {
        let mut cfg = Config::default();
        create_group(&mut cfg, "Blog").unwrap();
        cfg.groups.members.insert("api".into(), "Blog".into());
        cfg.groups.members.insert("shop".into(), "Blog".into());
        let r = empty_router();
        let a = apply(
            &mut cfg,
            &r,
            &Request::DeleteGroup {
                name: "Blog".into(),
            },
            None,
            v(8, 3),
        )
        .unwrap();
        assert!(a.summary.contains("deleted"));
        assert!(cfg.groups.order.is_empty());
        assert!(cfg.groups.members.is_empty());
        let a2 = apply(
            &mut cfg,
            &r,
            &Request::DeleteGroup {
                name: "Blog".into(),
            },
            None,
            v(8, 3),
        )
        .unwrap();
        assert!(a2.summary.contains("was not a group"));
    }

    #[test]
    fn set_group_order_requires_permutation() {
        let mut cfg = Config::default();
        create_group(&mut cfg, "Blog").unwrap();
        create_group(&mut cfg, "Shop").unwrap();
        let r = empty_router();
        apply(
            &mut cfg,
            &r,
            &Request::SetGroupOrder {
                order: vec!["Shop".into(), "Blog".into()],
            },
            None,
            v(8, 3),
        )
        .unwrap();
        assert_eq!(
            cfg.groups.order,
            vec!["Shop".to_string(), "Blog".to_string()]
        );

        for bad in [
            vec!["Shop".to_string()],
            vec!["Blog".to_string(), "Nope".to_string()],
            vec!["Blog".to_string(), "Shop".to_string(), "Extra".to_string()],
        ] {
            assert!(
                matches!(
                    apply(
                        &mut cfg,
                        &r,
                        &Request::SetGroupOrder { order: bad.clone() },
                        None,
                        v(8, 3),
                    ),
                    Err(MutateError::Invalid(_))
                ),
                "expected Invalid for {bad:?}"
            );
        }
    }

    #[test]
    fn set_site_group_assigns_clears_and_lowercases() {
        let mut cfg = Config::default();
        create_group(&mut cfg, "Blog").unwrap();
        let r = empty_router();
        apply(
            &mut cfg,
            &r,
            &Request::SetSiteGroup {
                site: "API".into(),
                group: Some("Blog".into()),
            },
            None,
            v(8, 3),
        )
        .unwrap();
        assert_eq!(
            cfg.groups.members.get("api").map(String::as_str),
            Some("Blog")
        );

        apply(
            &mut cfg,
            &r,
            &Request::SetSiteGroup {
                site: "api".into(),
                group: None,
            },
            None,
            v(8, 3),
        )
        .unwrap();
        assert!(cfg.groups.members.is_empty());
    }

    #[test]
    fn group_matching_is_case_insensitive_and_canonicalises() {
        let mut cfg = Config::default();
        create_group(&mut cfg, "Blog").unwrap();
        let r = empty_router();
        // Assign with a different casing: the canonical order casing is stored, so
        // the member value always matches its order entry exactly.
        apply(
            &mut cfg,
            &r,
            &Request::SetSiteGroup {
                site: "api".into(),
                group: Some("BLOG".into()),
            },
            None,
            v(8, 3),
        )
        .unwrap();
        assert_eq!(
            cfg.groups.members.get("api").map(String::as_str),
            Some("Blog")
        );

        // Delete with yet another casing removes the group and its members.
        apply(
            &mut cfg,
            &r,
            &Request::DeleteGroup {
                name: "blog".into(),
            },
            None,
            v(8, 3),
        )
        .unwrap();
        assert!(cfg.groups.order.is_empty());
        assert!(cfg.groups.members.is_empty());
    }

    #[test]
    fn set_site_group_unknown_group_is_not_found() {
        let mut cfg = Config::default();
        let r = empty_router();
        match apply(
            &mut cfg,
            &r,
            &Request::SetSiteGroup {
                site: "api".into(),
                group: Some("Ghost".into()),
            },
            None,
            v(8, 3),
        ) {
            Err(MutateError::NotFound(_)) => {}
            other => panic!("expected NotFound, got {other:?}"),
        }
    }

    fn rename_group(cfg: &mut Config, from: &str, to: &str) -> Result<Applied, MutateError> {
        let r = empty_router();
        apply(
            cfg,
            &r,
            &Request::RenameGroup {
                from: from.into(),
                to: to.into(),
            },
            None,
            v(8, 3),
        )
    }

    #[test]
    fn rename_group_keeps_position_and_moves_members() {
        let mut cfg = Config::default();
        create_group(&mut cfg, "Blog").unwrap();
        create_group(&mut cfg, "Shop").unwrap();
        cfg.groups.members.insert("api".into(), "Blog".into());
        cfg.groups.members.insert("cart".into(), "Shop".into());
        let a = rename_group(&mut cfg, "Blog", "Journal").unwrap();
        assert!(a.summary.contains("renamed group Blog to Journal"));
        assert_eq!(
            cfg.groups.order,
            vec!["Journal".to_string(), "Shop".to_string()]
        );
        assert_eq!(
            cfg.groups.members.get("api").map(String::as_str),
            Some("Journal")
        );
        assert_eq!(
            cfg.groups.members.get("cart").map(String::as_str),
            Some("Shop")
        );
    }

    #[test]
    fn rename_group_is_case_insensitive_and_canonicalises_members() {
        let mut cfg = Config::default();
        create_group(&mut cfg, "Blog").unwrap();
        cfg.groups.members.insert("api".into(), "Blog".into());
        // Match `from` in a different casing; the entered `to` casing becomes
        // canonical in both order and members.
        rename_group(&mut cfg, "blog", "jOURNAL").unwrap();
        assert_eq!(cfg.groups.order, vec!["jOURNAL".to_string()]);
        assert_eq!(
            cfg.groups.members.get("api").map(String::as_str),
            Some("jOURNAL")
        );
    }

    #[test]
    fn rename_group_allows_case_only_change() {
        let mut cfg = Config::default();
        create_group(&mut cfg, "blog").unwrap();
        cfg.groups.members.insert("api".into(), "blog".into());
        rename_group(&mut cfg, "blog", "Blog").unwrap();
        assert_eq!(cfg.groups.order, vec!["Blog".to_string()]);
        assert_eq!(
            cfg.groups.members.get("api").map(String::as_str),
            Some("Blog")
        );
    }

    #[test]
    fn rename_group_trims_new_name() {
        let mut cfg = Config::default();
        create_group(&mut cfg, "Blog").unwrap();
        rename_group(&mut cfg, "Blog", "  Journal  ").unwrap();
        assert_eq!(cfg.groups.order, vec!["Journal".to_string()]);
    }

    #[test]
    fn rename_group_rejects_collision_with_other_group() {
        let mut cfg = Config::default();
        create_group(&mut cfg, "Blog").unwrap();
        create_group(&mut cfg, "Shop").unwrap();
        assert!(matches!(
            rename_group(&mut cfg, "Blog", "shop"),
            Err(MutateError::AlreadyExists(_))
        ));
        assert_eq!(
            cfg.groups.order,
            vec!["Blog".to_string(), "Shop".to_string()]
        );
    }

    #[test]
    fn rename_group_rejects_empty_and_reserved() {
        let mut cfg = Config::default();
        create_group(&mut cfg, "Blog").unwrap();
        assert!(matches!(
            rename_group(&mut cfg, "Blog", "   "),
            Err(MutateError::Invalid(_))
        ));
        assert!(matches!(
            rename_group(&mut cfg, "Blog", "unallocated"),
            Err(MutateError::Invalid(_))
        ));
        assert_eq!(cfg.groups.order, vec!["Blog".to_string()]);
    }

    #[test]
    fn rename_group_unknown_from_is_not_found() {
        let mut cfg = Config::default();
        assert!(matches!(
            rename_group(&mut cfg, "Ghost", "Journal"),
            Err(MutateError::NotFound(_))
        ));
    }

    // ------------------ domains ------------------

    fn router_with_domains(sites: &[(&str, &str, &[&str])]) -> SiteRouter {
        let mut r = empty_router();
        for (name, root, subs) in sites {
            let effective: Vec<Domain> = subs
                .iter()
                .map(|s| Domain::parse_subpart(s).unwrap())
                .collect();
            let primary = effective
                .iter()
                .find(|d| !d.is_wildcard())
                .cloned()
                .unwrap_or_else(|| Domain::apex(name));
            r.insert_with_domains(
                Site::parked(name, root, v(8, 3)).unwrap(),
                effective,
                primary,
            )
            .unwrap();
        }
        r
    }

    fn add_domain(
        cfg: &mut Config,
        r: &SiteRouter,
        name: &str,
        domain: &str,
    ) -> Result<Applied, MutateError> {
        apply(
            cfg,
            r,
            &Request::AddDomain {
                name: name.into(),
                domain: domain.into(),
            },
            None,
            v(8, 3),
        )
    }

    #[test]
    fn add_domain_records_delta_for_linked() {
        let mut cfg = Config::default();
        cfg.linked
            .push(Site::linked("foo", "/srv/foo", v(8, 3)).unwrap());
        let r = router_with_domains(&[("foo", "/srv/foo", &["foo"])]);
        add_domain(&mut cfg, &r, "foo", "corp.test").unwrap();
        add_domain(&mut cfg, &r, "foo", "*.foo.test").unwrap();
        let delta = cfg.domains.linked.get("foo").unwrap();
        assert_eq!(delta.added.len(), 2);
        assert_eq!(delta.added[0].as_str(), "corp");
        assert_eq!(delta.added[1].as_str(), "*.foo");
    }

    #[test]
    fn add_domain_parked_keys_by_docroot() {
        let mut cfg = Config::default();
        let r = router_with_domains(&[("blog", "/srv/blog", &["blog"])]);
        add_domain(&mut cfg, &r, "blog", "corp.test").unwrap();
        assert!(cfg.domains.linked.is_empty());
        assert!(cfg.domains.parked.contains_key("/srv/blog"));
    }

    #[test]
    fn add_domain_rejects_claim_by_other_site() {
        let mut cfg = Config::default();
        let r =
            router_with_domains(&[("foo", "/srv/foo", &["foo"]), ("bar", "/srv/bar", &["bar"])]);
        match add_domain(&mut cfg, &r, "bar", "foo.test") {
            Err(MutateError::AlreadyExists(_)) => {}
            other => panic!("expected AlreadyExists, got {other:?}"),
        }
    }

    #[test]
    fn add_domain_rejects_not_under_tld() {
        let mut cfg = Config::default();
        let r = router_with_domains(&[("foo", "/srv/foo", &["foo"])]);
        match add_domain(&mut cfg, &r, "foo", "foo.example") {
            Err(MutateError::Invalid(_)) => {}
            other => panic!("expected Invalid, got {other:?}"),
        }
    }

    #[test]
    fn remove_added_domain_and_reject_last_exact() {
        let mut cfg = Config::default();
        let r = router_with_domains(&[("foo", "/srv/foo", &["foo"])]);
        add_domain(&mut cfg, &r, "foo", "corp.test").unwrap();
        // Remove the added exact: fine, apex remains.
        apply(
            &mut cfg,
            &r,
            &Request::RemoveDomain {
                name: "foo".into(),
                domain: "corp.test".into(),
            },
            None,
            v(8, 3),
        )
        .unwrap();
        assert!(!cfg.domains.parked.contains_key("/srv/foo"));
        // Removing the apex when it is the only exact is rejected.
        match apply(
            &mut cfg,
            &r,
            &Request::RemoveDomain {
                name: "foo".into(),
                domain: "foo.test".into(),
            },
            None,
            v(8, 3),
        ) {
            Err(MutateError::Invalid(_)) => {}
            other => panic!("expected Invalid keeping an exact, got {other:?}"),
        }
    }

    #[test]
    fn change_primary_and_suppress_apex() {
        let mut cfg = Config::default();
        let r = router_with_domains(&[("foo", "/srv/foo", &["foo"])]);
        apply(
            &mut cfg,
            &r,
            &Request::SetPrimaryDomain {
                name: "foo".into(),
                domain: "corp.test".into(),
            },
            None,
            v(8, 3),
        )
        .unwrap();
        apply(
            &mut cfg,
            &r,
            &Request::RemoveDomain {
                name: "foo".into(),
                domain: "foo.test".into(),
            },
            None,
            v(8, 3),
        )
        .unwrap();
        let delta = cfg.domains.parked.get("/srv/foo").unwrap();
        assert_eq!(delta.added, vec![Domain::parse_subpart("corp").unwrap()]);
        assert_eq!(delta.suppressed, vec![Domain::apex("foo")]);
        assert_eq!(delta.primary, Some(Domain::parse_subpart("corp").unwrap()));
    }

    #[test]
    fn set_primary_rejects_wildcard() {
        let mut cfg = Config::default();
        let r = router_with_domains(&[("foo", "/srv/foo", &["foo"])]);
        match apply(
            &mut cfg,
            &r,
            &Request::SetPrimaryDomain {
                name: "foo".into(),
                domain: "*.foo.test".into(),
            },
            None,
            v(8, 3),
        ) {
            Err(MutateError::Invalid(_)) => {}
            other => panic!("expected Invalid for wildcard primary, got {other:?}"),
        }
    }

    #[test]
    fn reset_domains_clears_delta() {
        let mut cfg = Config::default();
        let r = router_with_domains(&[("foo", "/srv/foo", &["foo"])]);
        add_domain(&mut cfg, &r, "foo", "corp.test").unwrap();
        apply(
            &mut cfg,
            &r,
            &Request::ResetDomains { name: "foo".into() },
            None,
            v(8, 3),
        )
        .unwrap();
        assert!(cfg.domains.is_empty());
    }

    #[test]
    fn add_domain_unknown_site_is_not_found() {
        let mut cfg = Config::default();
        let r = empty_router();
        match add_domain(&mut cfg, &r, "ghost", "corp.test") {
            Err(MutateError::NotFound(_)) => {}
            other => panic!("expected NotFound, got {other:?}"),
        }
    }

    #[test]
    fn link_migrates_parked_domain_delta() {
        let mut cfg = Config::default();
        cfg.domains.parked.insert(
            "/srv/foo".into(),
            DomainDelta {
                added: vec![Domain::parse_subpart("corp").unwrap()],
                suppressed: vec![],
                primary: None,
            },
        );
        let r = empty_router();
        apply(
            &mut cfg,
            &r,
            &Request::Link {
                name: "foo".into(),
                path: PathBuf::from("/ignored"),
            },
            Some(PathBuf::from("/srv/foo")),
            v(8, 3),
        )
        .unwrap();
        assert!(cfg.domains.parked.is_empty());
        assert!(cfg.domains.linked.contains_key("foo"));
    }

    #[test]
    fn error_code_mapping() {
        assert_eq!(
            error_code(&MutateError::NotFound("x".into())),
            ErrorCode::NotFound
        );
        assert_eq!(
            error_code(&MutateError::AlreadyExists("x".into())),
            ErrorCode::AlreadyExists
        );
        assert_eq!(
            error_code(&MutateError::Invalid("x".into())),
            ErrorCode::InvalidPath
        );
    }
}
