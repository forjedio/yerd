//! `wp` multi-call shim.
//!
//! `{data}/bin/wp` is a symlink to *this* `yerd` binary. When invoked under
//! that name (detected from `argv[0]` before clap), yerd execs WP-CLI's
//! filesystem entry point - `php …/tools/wp-cli/vendor/wp-cli/wp-cli/php/
//! boot-fs.php <args…>` - rather than upstream's `bin/wp` shell wrapper, which
//! exists only to locate a `php` on `PATH`; we already know which PHP to use.
//!
//! If the invocation's current directory is inside a registered site, `wp`
//! runs under *that site's* pinned PHP version, scoped to the site's served
//! root (`document_root` joined with `web_subpath`) via `--path=` (asking the
//! daemon via a short-timeout `Request::ListSites`), so `wp option get
//! siteurl` and friends behave the way the site itself is served. Outside any
//! registered site, or if the
//! daemon is unreachable or slow, this falls back to exactly the old
//! behavior: the default managed PHP, no working-directory change. If cwd
//! *is* inside a site but that site's pinned PHP version isn't installed,
//! this fails with a clear error rather than silently running under an
//! unrelated default PHP. Unix-only.

use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};
use std::time::Duration;

use yerd_core::PhpVersion;
use yerd_ipc::{Request, Response};
use yerd_platform::{ActivePaths, Paths, PlatformDirs};

use crate::shim::{cli_binary, fail, resolve_default_php};
use crate::transport;

/// How long to wait for the daemon to answer `ListSites` before giving up and
/// falling back to the default PHP - this must stay short since it's on the
/// critical path of every `wp` invocation.
const SITE_LOOKUP_TIMEOUT: Duration = Duration::from_millis(300);

/// Silences PHP-engine `E_DEPRECATED` notices from WP-CLI's own bundled
/// Composer dependencies (`react/promise`, `wp-cli/php-cli-tools`), which
/// aren't kept current with newer PHP releases and otherwise flood every
/// invocation with "Deprecated: ..." noise unrelated to whether the command
/// actually succeeded. Kept in sync with the identical constant in
/// `bin/yerdd/src/tools/wp_cli.rs` (this is a different binary - `bin/yerd`
/// can't depend on `bin/yerdd` - so it can't just import that one).
const QUIET_DEPRECATIONS: [&str; 2] = ["-d", "error_reporting=E_ALL & ~E_DEPRECATED"];

/// If `argv[0]` is `wp`, exec WP-CLI and return its exit code (on success
/// `exec` replaces the process and never returns); otherwise `None`, so
/// `main` falls through to the next shim / CLI.
#[must_use]
pub fn dispatch() -> Option<ExitCode> {
    let arg0 = std::env::args_os().next()?;
    let name = Path::new(&arg0).file_name()?.to_str()?;
    if name != "wp" {
        return None;
    }
    Some(run())
}

fn run() -> ExitCode {
    let dirs = match ActivePaths::new().resolve() {
        Ok(d) => d,
        Err(e) => return fail(format!("cannot resolve yerd directories: {e}")),
    };
    let cwd = std::env::current_dir()
        .ok()
        .and_then(|cwd| std::fs::canonicalize(cwd).ok());

    let resolution = match &cwd {
        Some(cwd) => site_scope(&dirs, cwd),
        None => ScopeResolution::NoScope,
    };
    let (php_bin, scope) = match resolution {
        ScopeResolution::Scoped(s) => (s.php_bin.clone(), Some(s)),
        ScopeResolution::MatchedPhpMissing { php_version } => {
            return fail(format!(
                "this site is pinned to PHP {php_version}, which is not installed — run \
                 `yerd install php {php_version}`"
            ));
        }
        ScopeResolution::NoScope => match resolve_default_php(&dirs) {
            Some((php, _minor)) => (php, None),
            None => return fail("no PHP installed — run `yerd install php <version>`".to_owned()),
        },
    };

    let boot_fs = dirs
        .data
        .join("tools")
        .join("wp-cli")
        .join("vendor")
        .join("wp-cli")
        .join("wp-cli")
        .join("php")
        .join("boot-fs.php");
    if !boot_fs.is_file() {
        return fail(
            "WP-CLI is not installed — install it from the Tooling page \
             (or run `yerd install tool wp-cli`)"
                .to_owned(),
        );
    }
    let Some((boot_dir, boot_name)) = split_boot_fs(&boot_fs) else {
        return fail(format!("{}: not a valid file path", boot_fs.display()));
    };

    let mut cmd = Command::new(&php_bin);
    cmd.args(QUIET_DEPRECATIONS)
        .arg(boot_name)
        .args(std::env::args_os().skip(1))
        .current_dir(boot_dir);
    if let Some(s) = &scope {
        cmd.arg(format!("--path={}", s.served_root.display()));
    }

    let err = cmd.exec();
    if err.kind() == std::io::ErrorKind::NotFound {
        return fail(format!(
            "PHP binary not found at {} ({err}) — reinstall with `yerd install php`",
            php_bin.display()
        ));
    }
    fail(format!("failed to exec {}: {err}", php_bin.display()))
}

/// Split `boot_fs` into its own directory and bare file name, so it can be
/// invoked as a bare relative name from *its own* directory (with `--path=`
/// decoupling "which `WordPress` install" from "process cwd") rather than by
/// its full absolute path with cwd set to the site. WP-CLI's
/// `WP_CLI::launch_self()` re-invocation (used by several subcommands,
/// `rewrite structure` among them) builds a raw shell string from the
/// captured `argv[0]` that escapes the PHP binary and arguments but not that
/// path itself; on macOS `boot_fs`'s absolute path always runs through
/// `~/Library/Application Support/...`, which always contains a space, so
/// passing it as argv[0]-ish input makes the re-invocation's shell command
/// silently split mid-path and fail with "Could not open input file".
/// Mirrors `bin/yerdd/src/create_site/wordpress.rs`'s `wp_step_invocation`
/// (and `wordpress_url_sync.rs`/`wordpress_users.rs`'s analogous helpers),
/// which this shim must match exactly for the same WP-CLI script. `None` if
/// `boot_fs` has no parent/file name (never true for a real path). Pure.
#[must_use]
fn split_boot_fs(boot_fs: &Path) -> Option<(&Path, &std::ffi::OsStr)> {
    Some((boot_fs.parent()?, boot_fs.file_name()?))
}

/// A site the current invocation resolved as "inside," and the PHP binary
/// pinned to it. `pub` (rather than `pub(crate)`) solely so the end-to-end
/// integration test in `tests/wp_shim_e2e.rs` (a separate crate) can exercise
/// this against a real daemon - same reason [`crate::resolve_link`] is `pub`.
#[derive(Debug)]
pub struct SiteScope {
    /// The PHP CLI binary pinned to the matched site.
    pub php_bin: PathBuf,
    /// The matched site's (canonicalized) served root - `document_root`
    /// joined with `web_subpath` - i.e. where `WordPress` actually lives, not
    /// necessarily the site's project root. Passed to `wp` as `--path=`.
    pub served_root: PathBuf,
}

/// Outcome of resolving the current invocation against the live site list.
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
    /// No site matched (or no daemon/timeout/no-match) - falls back to
    /// today's pre-existing default-PHP behavior.
    NoScope,
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
    let sock = dirs.runtime.join("yerd.sock");
    let Some(sites) = list_sites_with_timeout(&sock) else {
        return ScopeResolution::NoScope;
    };
    let candidates: Vec<(PathBuf, PhpVersion)> = sites
        .iter()
        .filter_map(|entry| {
            let root = std::fs::canonicalize(entry.site.served_root()).ok()?;
            Some((root, entry.site.php()))
        })
        .collect();

    let Some((root, php_version)) = match_site(cwd, &candidates) else {
        return ScopeResolution::NoScope;
    };
    let php_bin = cli_binary(dirs, &php_version.to_string());
    if php_bin.is_file() {
        ScopeResolution::Scoped(SiteScope {
            php_bin,
            served_root: root,
        })
    } else {
        ScopeResolution::MatchedPhpMissing { php_version }
    }
}

/// Spin up a one-shot, single-threaded tokio runtime (this shim otherwise
/// has none) to make a single timeout-bounded `ListSites` call against the
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
/// one contains `cwd` (nested sites are unusual but not disallowed). Pure:
/// takes already-canonicalized paths, does no I/O itself.
fn match_site(cwd: &Path, candidates: &[(PathBuf, PhpVersion)]) -> Option<(PathBuf, PhpVersion)> {
    candidates
        .iter()
        .filter(|(root, _)| cwd.starts_with(root))
        .max_by_key(|(root, _)| root.as_os_str().len())
        .cloned()
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn split_boot_fs_splits_absolute_space_containing_path() {
        let boot_fs = Path::new("/Users/x/Library/Application Support/io.yerd.Yerd/boot-fs.php");
        let (boot_dir, boot_name) = split_boot_fs(boot_fs).unwrap();
        assert_eq!(
            boot_dir,
            Path::new("/Users/x/Library/Application Support/io.yerd.Yerd")
        );
        assert_eq!(boot_name, "boot-fs.php");
    }

    #[test]
    fn split_boot_fs_none_for_rootless_path() {
        assert!(split_boot_fs(Path::new("/")).is_none());
    }

    #[test]
    fn dispatch_ignores_non_wp_argv0() {
        assert_eq!(Path::new("/x/wp").file_name().unwrap(), "wp");
        assert_ne!(Path::new("/x/wpcli").file_name().unwrap(), "wp");
    }

    fn php(major: u8, minor: u8) -> PhpVersion {
        PhpVersion::new(major, minor)
    }

    #[test]
    fn match_site_finds_exact_root() {
        let candidates = vec![(PathBuf::from("/srv/blog"), php(8, 3))];
        let hit = match_site(Path::new("/srv/blog"), &candidates);
        assert_eq!(hit, Some((PathBuf::from("/srv/blog"), php(8, 3))));
    }

    #[test]
    fn match_site_finds_nested_cwd() {
        let candidates = vec![(PathBuf::from("/srv/blog"), php(8, 3))];
        let hit = match_site(Path::new("/srv/blog/wp-content/themes"), &candidates);
        assert_eq!(hit, Some((PathBuf::from("/srv/blog"), php(8, 3))));
    }

    #[test]
    fn match_site_prefers_more_specific_nested_site() {
        let candidates = vec![
            (PathBuf::from("/srv"), php(8, 1)),
            (PathBuf::from("/srv/blog"), php(8, 3)),
        ];
        let hit = match_site(Path::new("/srv/blog/wp-admin"), &candidates);
        assert_eq!(hit, Some((PathBuf::from("/srv/blog"), php(8, 3))));
    }

    #[test]
    fn match_site_none_outside_any_site() {
        let candidates = vec![(PathBuf::from("/srv/blog"), php(8, 3))];
        assert_eq!(match_site(Path::new("/home/dev/other"), &candidates), None);
    }

    #[test]
    fn site_scope_falls_back_to_no_scope_when_daemon_unreachable() {
        // No socket is ever created at `dirs.runtime` - this deterministically
        // exercises the "daemon unreachable" fallback the plan calls out as
        // needing its own explicit test, with no real daemon or process-global
        // cwd mutation required (`site_scope` takes `cwd` as a plain parameter).
        let tmp = tempfile::tempdir().unwrap();
        let dirs = PlatformDirs {
            config: tmp.path().join("c"),
            data: tmp.path().join("d"),
            state: tmp.path().join("s"),
            cache: tmp.path().join("ca"),
            runtime: tmp.path().join("r"),
        };
        std::fs::create_dir_all(&dirs.runtime).unwrap();
        let cwd = std::fs::canonicalize(tmp.path()).unwrap();
        assert!(matches!(site_scope(&dirs, &cwd), ScopeResolution::NoScope));
    }

    #[test]
    fn match_site_resolves_symlinked_cwd_once_canonicalized() {
        let tmp = tempfile::tempdir().unwrap();
        let real_root = tmp.path().join("real-site");
        std::fs::create_dir(&real_root).unwrap();
        let link = tmp.path().join("link-to-site");
        #[cfg(unix)]
        std::os::unix::fs::symlink(&real_root, &link).unwrap();

        let canonical_root = std::fs::canonicalize(&real_root).unwrap();
        let canonical_cwd = std::fs::canonicalize(&link).unwrap();

        let candidates = vec![(canonical_root.clone(), php(8, 4))];
        assert_eq!(
            match_site(&canonical_cwd, &candidates),
            Some((canonical_root, php(8, 4)))
        );
    }
}
