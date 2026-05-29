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
//! itself before every lookup — otherwise `yerd use Blog 8.4` would look up
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

/// Apply a mutation [`Request`] to `cfg` in place.
///
/// `router` is the **pre-mutation** live router — read here so a `SetPhp` on a
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
        Request::Park { .. } => {
            let path =
                canonical.ok_or_else(|| MutateError::Invalid("park requires a path".into()))?;
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
        Request::Link { name, .. } => {
            let path =
                canonical.ok_or_else(|| MutateError::Invalid("link requires a path".into()))?;
            // `Site::linked` validates and lowercases the name.
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
        Request::Unlink { name } => {
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
        Request::SetPhp { name, version } => {
            let name_lc = name.to_ascii_lowercase();
            if let Some(site) = cfg.linked.iter_mut().find(|s| s.name() == name_lc) {
                site.set_php(*version);
                Ok(Applied {
                    summary: format!("{name_lc} now uses PHP {version}"),
                })
            } else if let Some(parked) = router.get(&name_lc) {
                // Promote the parked site to a linked entry that captures its
                // discovered document_root, so the override persists and wins
                // over the parked directory on the next scan.
                let site = Site::linked(&name_lc, parked.document_root().to_path_buf(), *version)
                    .map_err(|e| MutateError::Invalid(format!("invalid site name: {e}")))?;
                cfg.linked.push(site);
                Ok(Applied {
                    summary: format!("{name_lc} now uses PHP {version} (linked)"),
                })
            } else {
                Err(MutateError::NotFound(format!("no site named {name_lc}")))
            }
        }
        _ => Err(MutateError::Invalid("unsupported request".into())),
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
    use yerd_core::{RouterConfig, SiteKind, Tld};

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
        // Second time: still Ok, but "already parked".
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
    fn setphp_promotes_parked_to_linked() {
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
        assert!(a.summary.contains("linked"));
        assert_eq!(cfg.linked.len(), 1);
        assert_eq!(cfg.linked[0].name(), "blog");
        assert_eq!(cfg.linked[0].php(), v(8, 4));
        assert_eq!(cfg.linked[0].document_root(), Path::new("/srv/blog"));
        assert_eq!(cfg.linked[0].kind(), SiteKind::Linked);
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
    fn mixed_case_name_resolves_lowercased_site() {
        // `use Blog` must find the stored parked `blog` and promote it.
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
        assert_eq!(cfg.linked[0].name(), "blog");

        // `unlink FOO` must remove the stored linked `foo`.
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
