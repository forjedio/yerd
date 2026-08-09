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
//! root (`document_root` joined with `web_subpath`) via `--path=`, so `wp
//! option get siteurl` and friends behave the way the site itself is served.
//! The resolution itself lives in [`crate::site_scope`], shared with `yerd
//! exec` / `yerd which`; see that module for the fallback and failure rules.
//! Unix-only.

use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

use yerd_platform::{ActivePaths, Paths, PlatformDirs};

use crate::shim::{cli_phprc, fail, resolve_default_php};
use crate::site_scope::{site_scope, ScopeResolution};

/// Silences PHP-engine `E_DEPRECATED` notices from WP-CLI's own bundled
/// Composer dependencies (`react/promise`, `wp-cli/php-cli-tools`), which
/// aren't kept current with newer PHP releases and otherwise flood every
/// invocation with "Deprecated: ..." noise unrelated to whether the command
/// actually succeeded. Kept in sync with the identical constant in
/// `bin/yerdd/src/tools/wp_cli.rs` (this is a different binary - `bin/yerd`
/// can't depend on `bin/yerdd` - so it can't just import that one).
const QUIET_DEPRECATIONS: [&str; 2] = ["-d", "error_reporting=E_ALL & ~E_DEPRECATED"];

/// [`QUIET_DEPRECATIONS`] only reaches the `wp` process we spawn directly - it
/// doesn't reach a PHP process WP-CLI spawns *internally* via
/// `WP_CLI::launch_self()` (used by several subcommands, `rewrite structure`
/// among them, to re-invoke themselves), since that re-invocation builds its
/// own bare `php <script>` command line with none of our flags. This writes a
/// tiny drop-in ini applying the same suppression, in a directory added to
/// `PHP_INI_SCAN_DIR` (see [`quiet_deprecations_scan_dir_env`]) - unlike a CLI
/// flag, an env var is inherited by any child process, so it still applies
/// after a `launch_self()` re-exec.
///
/// It also pins `display_errors = stderr`: this shim now sets `PHPRC` to the
/// generated CLI ini (so `memory_limit` etc. apply), and that ini carries the
/// user's `display_errors`, defaulting to `On` - which would route PHP warnings
/// to *stdout* and corrupt `wp ... --format=json` output. Scanned after the
/// main ini, this drop-in wins and forces errors back to stderr. Idempotent
/// (safe to call on every invocation). Kept in sync with the identical function
/// in `bin/yerdd/src/tools/wp_cli.rs` - see [`QUIET_DEPRECATIONS`]'s doc comment
/// for why this is duplicated rather than shared.
fn ensure_quiet_deprecations_scan_dir(dirs: &PlatformDirs) -> std::io::Result<PathBuf> {
    let dir = dirs.data.join("wp-cli-quiet.d");
    std::fs::create_dir_all(&dir)?;
    std::fs::write(
        dir.join("quiet-deprecations.ini"),
        "error_reporting = E_ALL & ~E_DEPRECATED\ndisplay_errors = stderr\n",
    )?;
    Ok(dir)
}

/// The `PHP_INI_SCAN_DIR` value for [`ensure_quiet_deprecations_scan_dir`]'s
/// directory - prefixed with the Unix path-list separator (`:`) so PHP scans
/// its compiled-in default ini directory first, then this one, rather than
/// replacing the default scan directory outright.
fn quiet_deprecations_scan_dir_env(dir: &Path) -> String {
    format!(":{}", dir.display())
}

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
        None => ScopeResolution::NoScope {
            daemon_unavailable: false,
        },
    };
    let scoped = match resolution {
        // A site whose served root is missing from disk can't be scoped: a
        // `--path=` aimed at the project root would point WP-CLI somewhere
        // WordPress isn't served from, so fall back exactly as an unmatched
        // cwd does. (Such a site is still *matched*, unlike before, so an
        // uninstalled pinned version now errors below rather than silently
        // falling back - the loud failure the pin is there to produce.)
        ScopeResolution::Scoped(s) => s.served_root.is_some().then_some(s),
        ScopeResolution::MatchedPhpMissing { php_version } => {
            return fail(format!(
                "this site is pinned to PHP {php_version}, which is not installed — run \
                 `yerd install php {php_version}`"
            ));
        }
        // Deliberately silent on an unanswered lookup, unlike `yerd exec`:
        // scoping is a convenience here (WP-CLI still runs, just unscoped),
        // `wp` is invoked far more often, and a stray line would corrupt
        // `wp ... --format=json` for anything parsing it.
        ScopeResolution::NoScope { .. } => None,
    };
    let (php_bin, minor, scope) = match scoped {
        Some(s) => (s.php_bin.clone(), s.php_minor.clone(), Some(s)),
        None => match resolve_default_php(&dirs) {
            Some((php, minor)) => (php, minor, None),
            None => return fail(crate::shim::no_default_php_message(&dirs)),
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
    if let Ok(dir) = ensure_quiet_deprecations_scan_dir(&dirs) {
        cmd.env("PHP_INI_SCAN_DIR", quiet_deprecations_scan_dir_env(&dir));
    }
    if let Some(phprc) = cli_phprc(&dirs, &minor) {
        cmd.env("PHPRC", phprc);
    }
    if let Some(served_root) = scope.as_ref().and_then(|s| s.served_root.as_ref()) {
        cmd.arg(format!("--path={}", served_root.display()));
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

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;
    use yerd_platform::PlatformDirs;

    #[test]
    fn quiet_deprecations_scan_dir_env_prefixes_the_default_scan_separator() {
        let dir = Path::new("/data/wp-cli-quiet.d");
        assert_eq!(
            quiet_deprecations_scan_dir_env(dir),
            ":/data/wp-cli-quiet.d"
        );
    }

    #[test]
    fn ensure_quiet_deprecations_scan_dir_writes_a_suppressing_ini() {
        let tmp = tempfile::tempdir().unwrap();
        let dirs = PlatformDirs {
            config: tmp.path().join("c"),
            data: tmp.path().join("d"),
            state: tmp.path().join("s"),
            cache: tmp.path().join("ca"),
            runtime: tmp.path().join("r"),
        };
        let dir = ensure_quiet_deprecations_scan_dir(&dirs).unwrap();
        let ini = std::fs::read_to_string(dir.join("quiet-deprecations.ini")).unwrap();
        assert!(ini.contains("error_reporting"));
        assert!(ini.contains("~E_DEPRECATED"));
        assert!(ini.contains("display_errors = stderr"));
        assert!(ensure_quiet_deprecations_scan_dir(&dirs).is_ok());
    }

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
}
