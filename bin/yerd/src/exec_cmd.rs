//! `yerd exec` and `yerd which` - run CLI tools under the PHP version pinned
//! to a site, and report which binary that is.
//!
//! The bare `php` and `composer` shims always use the **global default**
//! version, so inside a site pinned to something else a CLI run (artisan,
//! Composer, a script) silently uses a different PHP than the site's own web
//! requests. These two commands close that gap without changing the shims:
//! `yerd exec php …` / `yerd exec composer …` run under the pinned version,
//! and `yerd which php` prints the binary they would use.
//!
//! Resolution is [`crate::site_scope`]'s: the current directory's site by
//! default, or `--site <name>` to name one explicitly. The two differ in how
//! they fail, deliberately - see [`select_php`]. Both commands are local: they
//! `exec` PHP directly rather than routing work through the daemon, exactly as
//! the shims and `yerd coverage` do. The one daemon round-trip is the
//! `ListSites` lookup behind the scoping.
//!
//! The resolution chain is cross-platform. Only the terminal step differs: on
//! Unix it replaces this process image, and on Windows it spawns the tool,
//! waits, and propagates the child's exit code.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

use yerd_platform::{ActivePaths, Paths, PlatformDirs};

use crate::cli::{ExecTool, WhichTool};
use crate::shim::{cli_phprc, resolve_default_php};
use crate::site_scope::{
    site_scope, site_scope_by_name, NamedScopeError, ScopeResolution, SiteScope,
};

/// Why `yerd exec` / `yerd which` couldn't resolve a PHP to run.
///
/// Carries the exit code alongside the message because these are real
/// subcommands, not shims: `2` means a client-side usage error and `69` an
/// unreachable daemon, and scripts branch on those. Collapsing everything to `1` (as the shims' `fail` does)
/// would make a typo'd `--site` indistinguishable from a daemon-side failure.
#[derive(Debug, PartialEq, Eq)]
pub struct SelectError {
    /// The ready-to-print message, without the `yerd: ` prefix.
    pub message: String,
    /// The documented process exit code for this class of failure.
    pub code: u8,
}

impl SelectError {
    /// A client-side usage error - a name that doesn't resolve, a version that
    /// isn't installed. Exit code `2`.
    fn usage(message: String) -> Self {
        Self { message, code: 2 }
    }

    /// The daemon was needed but unreachable. Exit code `69`.
    fn daemon(message: String) -> Self {
        Self { message, code: 69 }
    }

    /// Print to stderr and return the documented exit code.
    fn report(&self) -> ExitCode {
        eprintln!("yerd: {}", self.message);
        ExitCode::from(self.code)
    }
}

/// Which PHP a `yerd exec` / `yerd which` invocation resolved to.
#[derive(Debug)]
pub enum PhpSelection {
    /// A site's pinned version.
    Site(SiteScope),
    /// The global default, because the invocation isn't inside any site (or
    /// the daemon couldn't be reached to say otherwise).
    Default {
        /// The default PHP CLI binary.
        php_bin: PathBuf,
        /// Its `"major.minor"` version - the [`cli_phprc`] key.
        minor: String,
    },
}

impl PhpSelection {
    /// The PHP CLI binary to run.
    #[must_use]
    pub fn php_bin(&self) -> &Path {
        match self {
            PhpSelection::Site(scope) => &scope.php_bin,
            PhpSelection::Default { php_bin, .. } => php_bin,
        }
    }

    /// The `"major.minor"` version string.
    #[must_use]
    pub fn minor(&self) -> &str {
        match self {
            PhpSelection::Site(scope) => &scope.php_minor,
            PhpSelection::Default { minor, .. } => minor,
        }
    }
}

/// Resolve which PHP to use. `cwd` and `site` are explicit parameters (rather
/// than read from the process) so this is fully testable without mutating the
/// process-global current directory.
///
/// With `site = Some(name)` the named site must resolve: an unknown name, an
/// uninstalled pinned version, and an unreachable daemon are all errors, since
/// the user asked for that site specifically and running something else would
/// be wrong. Without it, resolution follows `cwd`: inside a site, its pinned
/// version (erroring if that version isn't installed, rather than silently
/// running an unrelated one); outside any site - or with no reachable daemon -
/// the global default.
///
/// A daemon that can't be reached for the cwd lookup is *not* an error - it
/// just means "not inside a site" - but it does warn on stderr, since silently
/// demoting a pinned site to the global default is the mismatch this command
/// exists to prevent.
///
/// # Errors
///
/// Returns a [`SelectError`] - message plus documented exit code - for every
/// unresolvable case.
pub fn select_php(
    dirs: &PlatformDirs,
    cwd: Option<&Path>,
    site: Option<&str>,
) -> Result<PhpSelection, SelectError> {
    if let Some(name) = site {
        return match site_scope_by_name(dirs, name) {
            Ok(scope) => Ok(PhpSelection::Site(scope)),
            Err(NamedScopeError::NotFound) => Err(SelectError::usage(format!(
                "no site named '{name}' — run `yerd sites` to see the registered sites"
            ))),
            Err(NamedScopeError::PhpMissing { php_version }) => Err(SelectError::usage(
                missing_php_message(&php_version.to_string()),
            )),
            Err(NamedScopeError::DaemonUnavailable) => Err(SelectError::daemon(
                "cannot reach the yerd daemon to look up that site — is it running?".to_owned(),
            )),
        };
    }

    let resolution = match cwd {
        Some(cwd) => site_scope(dirs, cwd),
        None => ScopeResolution::NoScope {
            daemon_unavailable: false,
        },
    };
    match resolution {
        ScopeResolution::Scoped(scope) => Ok(PhpSelection::Site(scope)),
        ScopeResolution::MatchedPhpMissing { php_version } => Err(SelectError::usage(
            missing_php_message(&php_version.to_string()),
        )),
        ScopeResolution::NoScope { daemon_unavailable } => {
            if daemon_unavailable {
                crate::site_scope::warn_daemon_unavailable();
            }
            match resolve_default_php(dirs) {
                Some((php_bin, minor)) => Ok(PhpSelection::Default { php_bin, minor }),
                None => Err(SelectError::usage(crate::shim::no_default_php_message(
                    dirs,
                ))),
            }
        }
    }
}

/// The "pinned version isn't installed" message, worded exactly as the `wp`
/// shim's so the two paths read identically.
fn missing_php_message(version: &str) -> String {
    format!(
        "this site is pinned to PHP {version}, which is not installed — run \
         `yerd install php {version}`"
    )
}

/// Resolve dirs and PHP the way both entry points need to.
///
/// [`select_php`]'s site lookup drives its own one-shot tokio runtime (the
/// shims that share it have none of their own), and `block_on` panics if it
/// runs on a thread already driving a runtime - which the CLI's `run()` is. So
/// the resolution is moved to a blocking-pool thread, exactly as the self-update
/// applier is.
async fn resolve(site: Option<&str>) -> Result<(PlatformDirs, PhpSelection), SelectError> {
    let dirs = ActivePaths::new().resolve().map_err(|e| SelectError {
        message: format!("cannot resolve yerd directories: {e}"),
        code: 74,
    })?;
    let owned_site = site.map(ToOwned::to_owned);
    let for_lookup = dirs.clone();
    let selection = tokio::task::spawn_blocking(move || {
        let cwd = current_dir();
        select_php(&for_lookup, cwd.as_deref(), owned_site.as_deref())
    })
    .await
    .map_err(|e| SelectError {
        message: format!("php resolution task failed: {e}"),
        code: 74,
    })??;
    Ok((dirs, selection))
}

/// `yerd exec <tool> [args…]`: run `tool` under the resolved PHP.
///
/// On Unix this replaces the process image and only returns on failure; on
/// Windows it spawns the tool, waits, and returns the child's exit code. The
/// console is inherited either way, so the tool keeps its stdio and a Ctrl+C
/// reaches it.
pub async fn run_exec(tool: ExecTool, site: Option<&str>, args: &[OsString]) -> ExitCode {
    let (dirs, selection) = match resolve(site).await {
        Ok(pair) => pair,
        Err(e) => return e.report(),
    };

    let php_bin = selection.php_bin().to_path_buf();
    let mut cmd = Command::new(&php_bin);
    if let ExecTool::Composer = tool {
        let phar = crate::shim::composer_phar(&dirs);
        if !phar.is_file() {
            return SelectError::usage(crate::shim::composer_missing_message()).report();
        }
        cmd.arg(&phar);
    }
    cmd.args(args);
    if let Some(phprc) = cli_phprc(&dirs, selection.minor()) {
        cmd.env("PHPRC", phprc);
    }

    match crate::shim::run_php(cmd) {
        Ok(code) => code,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => SelectError::usage(format!(
            "PHP binary not found at {} ({err}) — reinstall with `yerd install php`",
            php_bin.display()
        ))
        .report(),
        Err(err) => SelectError {
            message: format!("failed to exec {}: {err}", php_bin.display()),
            code: 74,
        }
        .report(),
    }
}

/// `yerd which <tool>`: print the binary `yerd exec` would use. Resolution and
/// every failure mode match [`run_exec`] exactly - this must never print a path
/// that `exec` wouldn't actually run.
pub async fn run_which(tool: WhichTool, site: Option<&str>, json: bool) -> ExitCode {
    let WhichTool::Php = tool;
    let selection = match resolve(site).await {
        Ok((_dirs, selection)) => selection,
        Err(e) => return e.report(),
    };
    println!("{}", which_output(&selection, json));
    ExitCode::SUCCESS
}

/// The canonicalized current directory, or `None` if it can't be read (a
/// deleted cwd, say) - treated as "not inside any site".
fn current_dir() -> Option<PathBuf> {
    std::env::current_dir()
        .ok()
        .and_then(|cwd| std::fs::canonicalize(cwd).ok())
}

/// Render `yerd which`'s output: the bare absolute path in human mode, or a
/// `{path, version, site, source}` object under `--json`. Built with
/// `serde_json` rather than `format!` because paths routinely contain
/// characters that need escaping (every macOS data path runs through
/// `Application Support`). Pure.
#[must_use]
pub fn which_output(selection: &PhpSelection, json: bool) -> String {
    if !json {
        return selection.php_bin().display().to_string();
    }
    let (site, source) = match selection {
        PhpSelection::Site(scope) => (serde_json::Value::String(scope.site_name.clone()), "site"),
        PhpSelection::Default { .. } => (serde_json::Value::Null, "default"),
    };
    let value = serde_json::json!({
        "path": selection.php_bin().to_string_lossy(),
        "version": selection.minor(),
        "site": site,
        "source": source,
    });
    value.to_string()
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::match_wildcard_for_single_variants
)]
mod tests {
    use super::*;

    fn dirs_at(tmp: &Path) -> PlatformDirs {
        PlatformDirs {
            config: tmp.join("c"),
            data: tmp.join("d"),
            state: tmp.join("s"),
            cache: tmp.join("ca"),
            runtime: tmp.join("r"),
        }
    }

    /// Lay down a managed CLI binary at the host's real layout (`bin/php` on
    /// Unix, a flat `php.exe` on Windows), so the resolver under test finds it
    /// on both.
    fn fake_cli(dirs: &PlatformDirs, minor: &str) -> PathBuf {
        let php = crate::shim::cli_binary(dirs, minor);
        std::fs::create_dir_all(php.parent().unwrap()).unwrap();
        std::fs::write(&php, b"#!/bin/sh\n").unwrap();
        php
    }

    fn site_selection(name: &str, minor: &str) -> PhpSelection {
        PhpSelection::Site(SiteScope {
            site_name: name.to_owned(),
            php_bin: PathBuf::from(format!("/d/php/php-{minor}/bin/php")),
            php_minor: minor.to_owned(),
            served_root: Some(PathBuf::from("/srv/blog")),
        })
    }

    #[test]
    fn which_output_human_is_the_bare_path() {
        let selection = site_selection("blog", "8.3");
        assert_eq!(which_output(&selection, false), "/d/php/php-8.3/bin/php");
    }

    #[test]
    fn which_output_json_reports_the_site_as_the_source() {
        let selection = site_selection("blog", "8.3");
        let value: serde_json::Value =
            serde_json::from_str(&which_output(&selection, true)).unwrap();
        assert_eq!(value["path"], "/d/php/php-8.3/bin/php");
        assert_eq!(value["version"], "8.3");
        assert_eq!(value["site"], "blog");
        assert_eq!(value["source"], "site");
    }

    /// On the default path there is no site, so `site` must be JSON `null`
    /// rather than an empty string.
    #[test]
    fn which_output_json_reports_a_null_site_for_the_default() {
        let selection = PhpSelection::Default {
            php_bin: PathBuf::from("/d/php/php-8.4/bin/php"),
            minor: "8.4".to_owned(),
        };
        let value: serde_json::Value =
            serde_json::from_str(&which_output(&selection, true)).unwrap();
        assert_eq!(value["site"], serde_json::Value::Null);
        assert_eq!(value["source"], "default");
        assert_eq!(value["version"], "8.4");
    }

    /// Every macOS data path contains a space (`Application Support`), so the
    /// JSON must be built by a real serializer, not `format!`.
    #[test]
    fn which_output_json_escapes_a_space_containing_path() {
        let selection = PhpSelection::Default {
            php_bin: PathBuf::from(
                "/Users/x/Library/Application Support/io.yerd.Yerd/php/php-8.4/bin/php",
            ),
            minor: "8.4".to_owned(),
        };
        let rendered = which_output(&selection, true);
        let value: serde_json::Value = serde_json::from_str(&rendered).unwrap();
        assert_eq!(
            value["path"],
            "/Users/x/Library/Application Support/io.yerd.Yerd/php/php-8.4/bin/php"
        );
    }

    /// With no daemon and a cwd outside any site, resolution falls back to the
    /// global default rather than erroring.
    #[test]
    fn select_php_falls_back_to_the_global_default() {
        let tmp = tempfile::tempdir().unwrap();
        let dirs = dirs_at(tmp.path());
        std::fs::create_dir_all(&dirs.runtime).unwrap();
        let php = fake_cli(&dirs, "8.3");
        let cwd = std::fs::canonicalize(tmp.path()).unwrap();

        match select_php(&dirs, Some(&cwd), None).unwrap() {
            PhpSelection::Default { php_bin, minor } => {
                assert_eq!(php_bin, php);
                assert_eq!(minor, "8.3");
            }
            other => panic!("expected Default, got {other:?}"),
        }
    }

    /// An explicit `--site` never falls back: with no daemon it must fail, not
    /// resolve to the default PHP - and with `69`, the documented "daemon
    /// unreachable" code, rather than a generic failure.
    #[test]
    fn select_php_named_site_errors_without_a_daemon() {
        let tmp = tempfile::tempdir().unwrap();
        let dirs = dirs_at(tmp.path());
        std::fs::create_dir_all(&dirs.runtime).unwrap();
        fake_cli(&dirs, "8.3");
        let err = select_php(&dirs, None, Some("blog")).unwrap_err();
        assert!(
            err.message.contains("cannot reach the yerd daemon"),
            "got: {}",
            err.message
        );
        assert_eq!(err.code, 69, "an unreachable daemon is exit 69");
    }

    #[test]
    fn select_php_reports_no_installed_php() {
        let tmp = tempfile::tempdir().unwrap();
        let dirs = dirs_at(tmp.path());
        std::fs::create_dir_all(&dirs.runtime).unwrap();
        let err = select_php(&dirs, None, None).unwrap_err();
        assert!(
            err.message.contains("no PHP installed"),
            "got: {}",
            err.message
        );
        assert_eq!(err.code, 2, "a client-side resolution failure is exit 2");
    }

    /// The two failure classes must not collapse onto one code: a bad `--site`
    /// is a usage error (`2`), an unreachable daemon is `69`. Scripts branch on
    /// the difference, so both codes are part of the CLI's public contract.
    #[test]
    fn select_error_codes_match_the_documented_table() {
        assert_eq!(SelectError::usage("x".to_owned()).code, 2);
        assert_eq!(SelectError::daemon("x".to_owned()).code, 69);
    }

    #[test]
    fn composer_phar_sits_under_the_tools_directory() {
        let tmp = tempfile::tempdir().unwrap();
        let dirs = dirs_at(tmp.path());
        assert_eq!(
            crate::shim::composer_phar(&dirs),
            dirs.data
                .join("tools")
                .join("composer")
                .join("composer.phar")
        );
    }

    #[test]
    fn missing_php_message_names_the_install_command() {
        let msg = missing_php_message("8.1");
        assert!(msg.contains("pinned to PHP 8.1"));
        assert!(msg.contains("yerd install php 8.1"));
    }
}
