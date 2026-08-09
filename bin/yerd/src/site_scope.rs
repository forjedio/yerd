//! Resolving "which site am I in, and which PHP is pinned to it".
//!
//! Shared by the `wp` shim (which scopes WP-CLI to the site containing the
//! current directory) and `yerd exec` / `yerd which` (which run CLI PHP and
//! Composer under a site's pinned version). Both ask the daemon for the live
//! site list over a short-timeout `Request::ListSites` and match the current
//! directory against the sites' document roots.
//!
//! Two entry points, with deliberately different failure behaviour:
//!
//! - [`site_scope`] resolves by current directory. Outside any site - or if the
//!   daemon is unreachable or slow - it returns [`ScopeResolution::NoScope`] so
//!   callers fall back to the global default PHP. A cwd that *is* inside a site
//!   whose pinned version isn't installed returns
//!   [`ScopeResolution::MatchedPhpMissing`] instead, so callers fail loudly
//!   rather than silently running under an unrelated default.
//! - [`site_scope_by_name`] resolves an explicitly named site and never falls
//!   back: an unknown name, an uninstalled pinned version, and an unreachable
//!   daemon are all errors. The user named a site, so quietly running something
//!   else would be wrong.
//!
//! Unix-only.

use std::path::{Path, PathBuf};
use std::time::Duration;

use yerd_core::PhpVersion;
use yerd_ipc::{Request, Response};
use yerd_platform::PlatformDirs;

use crate::shim::cli_binary;
use crate::transport;

/// How long to wait for the daemon to answer `ListSites` before giving up and
/// falling back to the default PHP - this must stay short since it's on the
/// critical path of every `wp` / `yerd exec` invocation.
const SITE_LOOKUP_TIMEOUT: Duration = Duration::from_millis(300);

/// A site the current invocation resolved as "inside" (or named explicitly),
/// and the PHP binary pinned to it. `pub` (rather than `pub(crate)`) so the
/// end-to-end integration tests in `tests/wp_shim_e2e.rs` and
/// `tests/exec_e2e.rs` (separate crates) can exercise this against a real
/// daemon - same reason [`crate::resolve_link`] is `pub`.
#[derive(Debug)]
pub struct SiteScope {
    /// The matched site's name, as stored by the daemon (lowercased).
    pub site_name: String,
    /// The PHP CLI binary pinned to the matched site.
    pub php_bin: PathBuf,
    /// The matched site's pinned PHP version as `"major.minor"` - the
    /// [`crate::shim::cli_phprc`] key for pointing `PHPRC` at that version's
    /// generated CLI ini.
    pub php_minor: String,
    /// The matched site's (canonicalized) served root - `document_root` joined
    /// with `web_subpath` - i.e. where the served files actually live, not
    /// necessarily the site's project root. Passed to `wp` as `--path=`.
    ///
    /// `None` when that directory doesn't exist on disk (an unbuilt `public/`,
    /// say). Matching never depends on it, so `yerd exec` still resolves the
    /// site's pinned PHP; the `wp` shim instead declines to scope, since a
    /// `--path=` pointing at the project root would aim WP-CLI somewhere
    /// `WordPress` isn't served from.
    pub served_root: Option<PathBuf>,
}

/// Outcome of resolving the current directory against the live site list.
/// `pub` for the same testability reason as [`SiteScope`].
#[derive(Debug)]
pub enum ScopeResolution {
    /// cwd is inside a site whose pinned PHP is installed.
    Scoped(SiteScope),
    /// cwd is inside a site, but that site's pinned PHP version isn't
    /// installed - this must fail loudly rather than silently falling back
    /// to an unrelated default PHP (which could run under the wrong version
    /// with no indication why site-scoping didn't apply).
    MatchedPhpMissing {
        /// The site's pinned (but not installed) PHP version.
        php_version: PhpVersion,
    },
    /// No site matched (or no daemon/timeout/no-match) - callers fall back to
    /// the global default PHP.
    NoScope,
}

/// Why [`site_scope_by_name`] could not resolve an explicitly named site.
/// Unlike [`ScopeResolution`] there is no "fall back to the default" variant:
/// naming a site is an explicit instruction, so every failure is an error.
#[derive(Debug, PartialEq, Eq)]
pub enum NamedScopeError {
    /// No registered site has that name.
    NotFound,
    /// The site exists but its pinned PHP version isn't installed.
    PhpMissing {
        /// The site's pinned (but not installed) PHP version.
        php_version: PhpVersion,
    },
    /// The daemon didn't answer in time, so the site list is unknown.
    DaemonUnavailable,
}

/// A registered site reduced to what matching needs.
///
/// The two roots are deliberately distinct. Matching uses `document_root` -
/// the site's project directory - because that's where the work happens: a
/// Laravel site is served from `public/`, but `artisan` and `composer.json`
/// live one level up, so matching on the served root would miss a cwd at the
/// project root (by far the common case). `served_root` is carried through for
/// the `wp` shim, whose `--path=` genuinely needs the directory `WordPress` is
/// served from - `None` if it doesn't exist on disk.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Candidate {
    name: String,
    document_root: PathBuf,
    served_root: Option<PathBuf>,
    php: PhpVersion,
}

/// Resolve the site (if any) `cwd` is inside, by asking the daemon for the
/// live site list. `cwd` is taken as an explicit, already-canonicalized
/// parameter (rather than reading `std::env::current_dir()` internally) so
/// this is fully testable with an arbitrary directory - no process-global
/// cwd mutation needed in tests. Returns `NoScope` on any daemon error,
/// timeout, or no match - callers treat that identically to "no site-scoping
/// available." `pub` for the same testability reason as [`SiteScope`].
#[must_use]
pub fn site_scope(dirs: &PlatformDirs, cwd: &Path) -> ScopeResolution {
    let Some(candidates) = candidates(dirs) else {
        return ScopeResolution::NoScope;
    };
    let Some(hit) = match_site(cwd, &candidates) else {
        return ScopeResolution::NoScope;
    };
    match scope_from(dirs, &hit) {
        Ok(scope) => ScopeResolution::Scoped(scope),
        Err(php_version) => ScopeResolution::MatchedPhpMissing { php_version },
    }
}

/// Resolve an explicitly named site. `name` is lowercased before comparison
/// since site names are stored lowercased. `pub` for the same testability
/// reason as [`SiteScope`].
///
/// # Errors
///
/// Returns [`NamedScopeError`] when the daemon is unreachable, no site has
/// that name, or the named site's pinned PHP version isn't installed.
pub fn site_scope_by_name(dirs: &PlatformDirs, name: &str) -> Result<SiteScope, NamedScopeError> {
    let candidates = candidates(dirs).ok_or(NamedScopeError::DaemonUnavailable)?;
    let wanted = name.to_lowercase();
    let hit = candidates
        .into_iter()
        .find(|c| c.name == wanted)
        .ok_or(NamedScopeError::NotFound)?;
    scope_from(dirs, &hit).map_err(|php_version| NamedScopeError::PhpMissing { php_version })
}

/// The live site list reduced to [`Candidate`]s with canonicalized roots, or
/// `None` if the daemon didn't answer.
///
/// Sites whose document root can't be canonicalized (e.g. deleted from disk)
/// are skipped entirely - without a project root there is nothing to match a
/// cwd against. A served root that can't be canonicalized (an unbuilt
/// `public/`, say) is recorded as `None` rather than dropping the site:
/// `yerd exec` only needs the document root, and a missing `public/` is no
/// reason to demote a pinned site to the global default. The `wp` shim, which
/// genuinely needs that directory for `--path=`, filters those out itself via
/// [`SiteScope::served_root`] being `None`.
fn candidates(dirs: &PlatformDirs) -> Option<Vec<Candidate>> {
    let sock = dirs.runtime.join("yerd.sock");
    let sites = list_sites_with_timeout(&sock)?;
    Some(
        sites
            .iter()
            .filter_map(|entry| {
                let document_root = std::fs::canonicalize(entry.site.document_root()).ok()?;
                let served_root = std::fs::canonicalize(entry.site.served_root()).ok();
                Some(Candidate {
                    name: entry.site.name().to_owned(),
                    document_root,
                    served_root,
                    php: entry.site.php(),
                })
            })
            .collect(),
    )
}

/// Turn a matched candidate into a [`SiteScope`], or `Err(version)` if that
/// version's CLI binary isn't installed.
fn scope_from(dirs: &PlatformDirs, hit: &Candidate) -> Result<SiteScope, PhpVersion> {
    let minor = hit.php.to_string();
    let php_bin = cli_binary(dirs, &minor);
    if php_bin.is_file() {
        Ok(SiteScope {
            site_name: hit.name.clone(),
            php_bin,
            php_minor: minor,
            served_root: hit.served_root.clone(),
        })
    } else {
        Err(hit.php)
    }
}

/// Spin up a one-shot, single-threaded tokio runtime (the shims otherwise
/// have none) to make a single timeout-bounded `ListSites` call against the
/// daemon socket at `sock` (matching [`transport::exchange`]'s own derivation
/// of `<runtime>/yerd.sock` - passed explicitly here so tests can point at an
/// isolated socket instead of the real, active one).
fn list_sites_with_timeout(sock: &Path) -> Option<Vec<yerd_ipc::SiteEntry>> {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .ok()?;
    let outcome = rt.block_on(async {
        tokio::time::timeout(
            SITE_LOOKUP_TIMEOUT,
            transport::exchange_at(sock, &Request::ListSites),
        )
        .await
    });
    match outcome {
        Ok(Ok(Response::Sites { sites })) => Some(sites),
        _ => None,
    }
}

/// Pick the site whose (already-canonicalized) document root is `cwd` or an
/// ancestor of it, preferring the most specific (longest) root when more than
/// one contains `cwd` (nested sites are unusual but not disallowed - and a
/// parked root's children are each their own site, so the child must win).
/// Pure: takes already-canonicalized paths, does no I/O itself.
fn match_site(cwd: &Path, candidates: &[Candidate]) -> Option<Candidate> {
    candidates
        .iter()
        .filter(|c| cwd.starts_with(&c.document_root))
        .max_by_key(|c| c.document_root.as_os_str().len())
        .cloned()
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;

    fn php(major: u8, minor: u8) -> PhpVersion {
        PhpVersion::new(major, minor)
    }

    fn candidate(name: &str, root: &str, version: PhpVersion) -> Candidate {
        Candidate {
            name: name.to_owned(),
            document_root: PathBuf::from(root),
            served_root: Some(PathBuf::from(root)),
            php: version,
        }
    }

    /// A site served from a subdirectory (`public/`, the Laravel layout).
    fn candidate_with_subpath(
        name: &str,
        document_root: &str,
        served: &str,
        version: PhpVersion,
    ) -> Candidate {
        Candidate {
            name: name.to_owned(),
            document_root: PathBuf::from(document_root),
            served_root: Some(PathBuf::from(served)),
            php: version,
        }
    }

    fn dirs_at(tmp: &Path) -> PlatformDirs {
        PlatformDirs {
            config: tmp.join("c"),
            data: tmp.join("d"),
            state: tmp.join("s"),
            cache: tmp.join("ca"),
            runtime: tmp.join("r"),
        }
    }

    #[test]
    fn match_site_finds_exact_root() {
        let candidates = vec![candidate("blog", "/srv/blog", php(8, 3))];
        let hit = match_site(Path::new("/srv/blog"), &candidates).unwrap();
        assert_eq!(hit.document_root, PathBuf::from("/srv/blog"));
        assert_eq!(hit.name, "blog");
        assert_eq!(hit.php, php(8, 3));
    }

    #[test]
    fn match_site_finds_nested_cwd() {
        let candidates = vec![candidate("blog", "/srv/blog", php(8, 3))];
        let hit = match_site(Path::new("/srv/blog/wp-content/themes"), &candidates).unwrap();
        assert_eq!(hit.document_root, PathBuf::from("/srv/blog"));
    }

    #[test]
    fn match_site_prefers_more_specific_nested_site() {
        let candidates = vec![
            candidate("srv", "/srv", php(8, 1)),
            candidate("blog", "/srv/blog", php(8, 3)),
        ];
        let hit = match_site(Path::new("/srv/blog/wp-admin"), &candidates).unwrap();
        assert_eq!(hit.name, "blog");
        assert_eq!(hit.php, php(8, 3));
    }

    /// The common Laravel layout: served from `public/`, but `artisan` and
    /// `composer.json` live at the project root, which is where the CLI is
    /// actually run from. Matching on the served root would miss it entirely
    /// and silently fall back to the global default.
    #[test]
    fn match_site_matches_the_project_root_of_a_subpath_served_site() {
        let candidates = vec![candidate_with_subpath(
            "my-app",
            "/srv/my-app",
            "/srv/my-app/public",
            php(8, 3),
        )];
        let hit = match_site(Path::new("/srv/my-app"), &candidates).unwrap();
        assert_eq!(hit.name, "my-app");
        assert_eq!(hit.php, php(8, 3));
        assert_eq!(
            hit.served_root,
            Some(PathBuf::from("/srv/my-app/public")),
            "the served root still travels through for wp's --path="
        );

        // ...and from inside the served directory itself.
        let hit = match_site(Path::new("/srv/my-app/public"), &candidates).unwrap();
        assert_eq!(hit.name, "my-app");
    }

    #[test]
    fn match_site_none_outside_any_site() {
        let candidates = vec![candidate("blog", "/srv/blog", php(8, 3))];
        assert_eq!(match_site(Path::new("/home/dev/other"), &candidates), None);
    }

    #[test]
    fn match_site_resolves_symlinked_cwd_once_canonicalized() {
        let tmp = tempfile::tempdir().unwrap();
        let real_root = tmp.path().join("real-site");
        std::fs::create_dir(&real_root).unwrap();
        let link = tmp.path().join("link-to-site");
        std::os::unix::fs::symlink(&real_root, &link).unwrap();

        let canonical_root = std::fs::canonicalize(&real_root).unwrap();
        let canonical_cwd = std::fs::canonicalize(&link).unwrap();

        let candidates = vec![Candidate {
            name: "real-site".to_owned(),
            document_root: canonical_root.clone(),
            served_root: Some(canonical_root.clone()),
            php: php(8, 4),
        }];
        let hit = match_site(&canonical_cwd, &candidates).unwrap();
        assert_eq!(hit.document_root, canonical_root);
    }

    #[test]
    fn site_scope_falls_back_to_no_scope_when_daemon_unreachable() {
        // No socket is ever created at `dirs.runtime` - this deterministically
        // exercises the "daemon unreachable" fallback, with no real daemon or
        // process-global cwd mutation required (`site_scope` takes `cwd` as a
        // plain parameter).
        let tmp = tempfile::tempdir().unwrap();
        let dirs = dirs_at(tmp.path());
        std::fs::create_dir_all(&dirs.runtime).unwrap();
        let cwd = std::fs::canonicalize(tmp.path()).unwrap();
        assert!(matches!(site_scope(&dirs, &cwd), ScopeResolution::NoScope));
    }

    /// The by-name lookup never falls back: with no daemon it must report
    /// `DaemonUnavailable`, not "not found" and not a default-PHP fallback.
    #[test]
    fn site_scope_by_name_errors_when_daemon_unreachable() {
        let tmp = tempfile::tempdir().unwrap();
        let dirs = dirs_at(tmp.path());
        std::fs::create_dir_all(&dirs.runtime).unwrap();
        assert!(matches!(
            site_scope_by_name(&dirs, "blog"),
            Err(NamedScopeError::DaemonUnavailable)
        ));
    }
}
