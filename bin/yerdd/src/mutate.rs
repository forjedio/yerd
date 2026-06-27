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

use yerd_config::Config;
use yerd_core::{PhpVersion, Site, SiteRouter};
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
/// Pure: no filesystem, clock, or environment access. Only the four mutation
/// variants are handled; anything else is [`MutateError::Invalid`] (the I/O
/// wrapper never routes `Ping`/`ListSites` here).
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
    Applied {
        summary: if removed {
            format!("un-parked {path}")
        } else {
            format!("{path} was not parked")
        },
    }
}

fn apply_unlink(cfg: &mut Config, router: &SiteRouter, name: &str) -> Result<Applied, MutateError> {
    let name_lc = name.to_ascii_lowercase();
    if cfg.linked.iter().any(|s| s.name() == name_lc) {
        cfg.linked.retain(|s| s.name() != name_lc);
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
