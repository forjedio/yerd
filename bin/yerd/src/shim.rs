//! Shared helpers for the five multi-call shims: `php`/`php<ver>`,
//! `phpcover`/`php<ver>cover`, `composer`, `laravel` and `wp`.
//!
//! When the `yerd` binary is invoked under one of those names, it resolves the
//! right managed PHP and `exec`s it. Two invocation mechanisms reach here: an
//! `argv[0]` symlink basename on Unix, and the `__shim <tool>` sentinel that a
//! Windows `.cmd` wrapper passes because a batch file cannot set `argv[0]`.
//! [`dispatch_shim`] resolves the invocation once and offers it to [`SHIMS`] in
//! order; the rest of the module is the version resolution against `{data}/php`
//! and the `php` default shim, plus the pieces every shim shares.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

use yerd_core::PhpVersion;
use yerd_platform::{ActivePaths, Paths, PlatformDirs};

/// Whether a `"major.minor"` minor string names a legacy version (< 8.2). A
/// minor that doesn't parse is treated as non-legacy (it will fail elsewhere).
#[cfg(not(windows))]
fn minor_is_legacy(minor: &str) -> bool {
    minor.parse::<PhpVersion>().is_ok_and(PhpVersion::is_legacy)
}

/// `{data}/php/php-<minor>/bin/php` on Unix.
#[cfg(not(windows))]
#[must_use]
pub(crate) fn cli_binary(dirs: &PlatformDirs, minor: &str) -> PathBuf {
    dirs.data
        .join("php")
        .join(format!("php-{minor}"))
        .join("bin")
        .join("php")
}

/// `{data}/php/php-<minor>/php.exe` on Windows: the flat bundle layout the daemon
/// installs (`php_install::cli_binary_path`'s `#[cfg(windows)]` arm), which has no
/// `bin/` subdirectory. Kept byte-in-step with that daemon-side path.
#[cfg(windows)]
#[must_use]
pub(crate) fn cli_binary(dirs: &PlatformDirs, minor: &str) -> PathBuf {
    dirs.data
        .join("php")
        .join(format!("php-{minor}"))
        .join("php.exe")
}

/// The default PHP `(binary, "major.minor")` via the `php` shim's target, if the
/// shim is present and the derived binary exists.
///
/// Only meaningful when `php` is a *direct symlink* to a version's binary (the
/// pre-wrapper layout). Once `php` is a Yerd multi-call wrapper, the target has
/// no `php-<ver>` component and this returns `None`, so [`resolve_default_php`]
/// falls through to the config default. Unix-only: on Windows the shims are
/// `.cmd` wrappers, never symlinks, so `read_link` is meaningless.
#[cfg(not(windows))]
#[must_use]
pub(crate) fn default_from_shim(dirs: &PlatformDirs) -> Option<(PathBuf, String)> {
    let target = std::fs::read_link(dirs.data.join("bin").join("php")).ok()?;
    let minor = minor_from_php_path(&target)?;
    if minor_is_legacy(&minor) {
        return None;
    }
    let php = cli_binary(dirs, &minor);
    php.is_file().then_some((php, minor))
}

/// The default PHP `(binary, "major.minor")` from the persisted config
/// (`{config}/yerd.toml`), if it parses and that version's CLI binary exists.
///
/// Every error (missing/corrupt/locked config, uninstalled default) is swallowed
/// to `None` so a `php` launch never fails on config trouble - the caller falls
/// back to the highest installed version.
#[must_use]
pub(crate) fn default_from_config(dirs: &PlatformDirs) -> Option<(PathBuf, String)> {
    let cfg = yerd_config::Config::load(&dirs.config.join("yerd.toml")).ok()?;
    let v = cfg.php.default;
    if v.is_legacy() {
        return None;
    }
    let minor = format!("{}.{}", v.major, v.minor);
    let php = cli_binary(dirs, &minor);
    php.is_file().then_some((php, minor))
}

/// Path to the bundled Composer phar (`{data}/tools/composer/composer.phar`).
/// Shared by the `composer` shim and `yerd exec composer`, which run the same
/// phar under different PHP versions.
#[must_use]
pub(crate) fn composer_phar(dirs: &PlatformDirs) -> PathBuf {
    dirs.data
        .join("tools")
        .join("composer")
        .join("composer.phar")
}

/// Message for a bundled tool that isn't installed, where `display` names it as
/// prose and `slug` is its `yerd install tool` argument. One template so every
/// shim that runs a managed tool fails identically.
#[must_use]
fn tool_missing_message(display: &str, slug: &str) -> String {
    format!(
        "{display} is not installed — install it from the Tooling page \
         (or run `yerd install tool {slug}`)"
    )
}

/// Message for when [`composer_phar`] doesn't exist. Shared so the `composer`
/// shim and `yerd exec composer` fail identically.
#[must_use]
pub(crate) fn composer_missing_message() -> String {
    tool_missing_message("Composer", "composer")
}

/// Path to a managed tool's entry point, `segments` joined under `{data}/tools`.
///
/// `Err` carries [`tool_missing_message`] for the tool, so a caller only has to
/// pass it to [`fail`]. Used by the shims whose tool lives wholly under
/// `{data}/tools`; `composer` resolves its phar through [`composer_phar`]
/// instead, because `yerd exec` consumes that same path.
pub(crate) fn managed_tool(
    dirs: &PlatformDirs,
    display: &str,
    slug: &str,
    segments: &[&str],
) -> Result<PathBuf, String> {
    let mut path = dirs.data.join("tools");
    for segment in segments {
        path.push(segment);
    }
    if path.is_file() {
        Ok(path)
    } else {
        Err(tool_missing_message(display, slug))
    }
}

/// Path to a version's generated CLI ini (`{data}/php-cli-<minor>.ini`).
#[must_use]
pub(crate) fn cli_ini_path(dirs: &PlatformDirs, minor: &str) -> PathBuf {
    dirs.data.join(format!("php-cli-{minor}.ini"))
}

/// The `PHPRC` target for a version's CLI: its per-version ini if present, else
/// the base `php-cli.ini` if present, else `None` (leave `PHPRC` unset).
#[must_use]
pub(crate) fn cli_phprc(dirs: &PlatformDirs, minor: &str) -> Option<PathBuf> {
    let per_version = cli_ini_path(dirs, minor);
    if per_version.is_file() {
        return Some(per_version);
    }
    let base = dirs.data.join("php-cli.ini");
    base.is_file().then_some(base)
}

/// Extract `"major.minor"` from a `…/php/php-<major>.<minor>/…` path. Unix-only:
/// only [`default_from_shim`] (itself Unix-only) consumes it.
#[cfg(not(windows))]
#[must_use]
pub(crate) fn minor_from_php_path(p: &Path) -> Option<String> {
    p.components().find_map(|c| {
        let rest = c.as_os_str().to_str()?.strip_prefix("php-")?;
        let (maj, min) = rest.split_once('.')?;
        let ok = !maj.is_empty()
            && !min.is_empty()
            && maj.bytes().all(|b| b.is_ascii_digit())
            && min.bytes().all(|b| b.is_ascii_digit());
        ok.then(|| rest.to_owned())
    })
}

/// Whether any PHP version is installed under `{data}/php` (legacy included) -
/// used to distinguish "nothing installed" from "only legacy installed" in the
/// no-default message.
#[must_use]
pub(crate) fn any_installed(dirs: &PlatformDirs) -> bool {
    let root = dirs.data.join("php");
    let Ok(entries) = std::fs::read_dir(&root) else {
        return false;
    };
    for entry in entries.flatten() {
        let fname = entry.file_name();
        let Some(name) = fname.to_str() else { continue };
        let Some(rest) = name.strip_prefix("php-") else {
            continue;
        };
        if cli_binary(dirs, rest).is_file() {
            return true;
        }
    }
    false
}

/// Message for when [`resolve_default_php`] returns `None`: distinguishes
/// "nothing installed" from "only legacy installed" so the guidance is accurate.
/// Shared by every shim that resolves the default PHP (`php`, `phpcover`,
/// `composer`, `laravel`, `wp`).
#[must_use]
pub(crate) fn no_default_php_message(dirs: &PlatformDirs) -> String {
    if any_installed(dirs) {
        "No supported default PHP version. Legacy versions (7.4 / 8.0 / 8.1) can't be the \
         default - install a supported version (e.g. `yerd install php 8.4`), or invoke a \
         legacy version explicitly, e.g. `php7.4`."
            .to_owned()
    } else {
        "no PHP installed - run `yerd install php <version>`".to_owned()
    }
}

/// Highest installed **non-legacy** minor under `{data}/php` whose CLI binary
/// exists. Legacy minors (< 8.2) are skipped so bare `php` never resolves to a
/// legacy interpreter.
#[must_use]
pub(crate) fn highest_installed(dirs: &PlatformDirs) -> Option<(PathBuf, String)> {
    let root = dirs.data.join("php");
    let mut best: Option<(u8, u8)> = None;
    for entry in std::fs::read_dir(&root).ok()?.flatten() {
        let fname = entry.file_name();
        let Some(name) = fname.to_str() else { continue };
        let Some(rest) = name.strip_prefix("php-") else {
            continue;
        };
        let Some((maj, min)) = rest.split_once('.') else {
            continue;
        };
        let (Ok(maj), Ok(min)) = (maj.parse::<u8>(), min.parse::<u8>()) else {
            continue;
        };
        if PhpVersion::new(maj, min).is_legacy() {
            continue;
        }
        if !cli_binary(dirs, rest).is_file() {
            continue;
        }
        if best.map_or(true, |b| (maj, min) > b) {
            best = Some((maj, min));
        }
    }
    let (maj, min) = best?;
    let minor = format!("{maj}.{min}");
    Some((cli_binary(dirs, &minor), minor))
}

/// Resolve the default PHP `(binary, "major.minor")`: the config default first
/// (authoritative), then a legacy direct-symlink `php` shim target, then the
/// highest installed version. Every fallback excludes legacy (< 8.2) versions,
/// so bare `php` never runs a legacy interpreter. `None` when no *supported* PHP
/// is installed (even if a legacy version is) - callers render
/// [`no_default_php_message`] to explain.
#[must_use]
pub(crate) fn resolve_default_php(dirs: &PlatformDirs) -> Option<(PathBuf, String)> {
    if let Some(v) = default_from_config(dirs) {
        return Some(v);
    }
    #[cfg(not(windows))]
    if let Some(v) = default_from_shim(dirs) {
        return Some(v);
    }
    highest_installed(dirs)
}

/// The default PHP `(binary, "major.minor")`, or the message explaining why
/// there isn't one. The single pairing of [`resolve_default_php`] with
/// [`no_default_php_message`], which every shim needs together.
pub(crate) fn default_php_or_message(dirs: &PlatformDirs) -> Result<(PathBuf, String), String> {
    resolve_default_php(dirs).ok_or_else(|| no_default_php_message(dirs))
}

/// Print `yerd: <msg>` to stderr and return a failure exit code.
pub(crate) fn fail(msg: String) -> ExitCode {
    eprintln!("yerd: {msg}");
    ExitCode::FAILURE
}

/// Resolve Yerd's directories, or the exit code to return after reporting that
/// they could not be resolved. Every shim opens with this.
pub(crate) fn dirs_or_fail() -> Result<PlatformDirs, ExitCode> {
    ActivePaths::new()
        .resolve()
        .map_err(|e| fail(format!("cannot resolve yerd directories: {e}")))
}

/// Which PHP version a multi-call shim name targets.
pub(crate) enum ShimVersion {
    /// The bare affixes (`php`, `phpcover`): the default version, resolved at
    /// run time.
    Default,
    /// An explicit `<major>.<minor>` between the affixes.
    Version(u8, u8),
}

/// Parse a shim basename shaped `<prefix>[<major>.<minor>]<suffix>`.
///
/// [`ShimVersion::Default`] when nothing sits between the affixes, `None` when
/// the name doesn't match the shape at all. Shared by the `php` / `php<M>.<N>`
/// CLI shims and the `phpcover` / `php<M>.<N>cover` aliases, whose only
/// difference is the suffix.
pub(crate) fn parse_version_affix(name: &str, prefix: &str, suffix: &str) -> Option<ShimVersion> {
    let rest = name.strip_prefix(prefix)?;
    let rest = rest.strip_suffix(suffix)?;
    if rest.is_empty() {
        return Some(ShimVersion::Default);
    }
    let (maj, min) = rest.split_once('.')?;
    if maj.is_empty() || min.is_empty() {
        return None;
    }
    Some(ShimVersion::Version(maj.parse().ok()?, min.parse().ok()?))
}

/// Resolve the shim invocation for the current process: either the multi-call
/// basename (`argv[0]`, the Unix symlink mechanism) or the Windows `__shim
/// <tool>` sentinel that batch wrappers pass because a `.cmd` file cannot set
/// `argv[0]`. Returns `(tool, forwarded_args)` where `forwarded_args` are the
/// arguments to pass on to the resolved interpreter. `None` when neither applies
/// (a normal `yerd <subcommand>` invocation), so `main` falls through to clap.
#[must_use]
pub fn shim_invocation() -> Option<(String, Vec<OsString>)> {
    let args: Vec<OsString> = std::env::args_os().collect();
    shim_invocation_from(&args)
}

/// Dispatch helper for a shim matched by one literal name (`composer`,
/// `laravel`, `wp`). `None` when `name` isn't this shim's, so the caller falls
/// through to the next entry in [`SHIMS`].
#[must_use]
pub(crate) fn dispatch_named(
    tool: &str,
    name: &str,
    forward: &[OsString],
    run: fn(&[OsString]) -> ExitCode,
) -> Option<ExitCode> {
    (name == tool).then(|| run(forward))
}

/// One shim's dispatch entry: given the resolved invocation name and the
/// arguments to forward, either handle the call or decline it.
type ShimDispatch = fn(&str, &[OsString]) -> Option<ExitCode>;

/// The multi-call shims, in dispatch order.
///
/// The order is load-bearing: `php<major>.<minor>cover` matches the `php` prefix
/// that both [`crate::cover_shim`] and [`crate::cli_shim`] look for, so the cover
/// entry must come before the CLI one. [`crate::cli_shim`] independently rejects
/// a trailing `cover`, so the guard is doubled; keep both.
const SHIMS: &[ShimDispatch] = &[
    crate::composer_shim::dispatch,
    crate::cover_shim::dispatch,
    crate::laravel_shim::dispatch,
    crate::cli_shim::dispatch,
    crate::wp_shim::dispatch,
];

/// Run the multi-call shim for this process's invocation, if any.
///
/// Resolves the invocation once via [`shim_invocation`] and offers it to each
/// entry of [`SHIMS`] in order. `None` for a normal `yerd <subcommand>` call, so
/// `main` falls through to clap.
#[must_use]
pub fn dispatch_shim() -> Option<ExitCode> {
    let (name, forward) = shim_invocation()?;
    SHIMS.iter().find_map(|shim| shim(&name, &forward))
}

/// Pure core of [`shim_invocation`], taking the argv explicitly so it is
/// table-testable without touching the process globals.
#[must_use]
fn shim_invocation_from(args: &[OsString]) -> Option<(String, Vec<OsString>)> {
    let program = args.first()?;
    if args.get(1).and_then(|a| a.to_str()) == Some("__shim") {
        let tool = args.get(2)?.to_str()?.to_owned();
        let forward = args.get(3..).unwrap_or(&[]).to_vec();
        return Some((tool, forward));
    }
    let name = Path::new(program).file_name()?.to_str()?.to_owned();
    let forward = args.get(1..).unwrap_or(&[]).to_vec();
    Some((name, forward))
}

/// Run `cmd` as the terminal PHP process. On Unix, replaces this process image
/// via `exec` (never returns on success). On Windows, spawns, waits, and returns
/// the child's exit code (Windows exit codes are always numeric). `Err` means the
/// interpreter could not be started at all (Unix: `exec` failed; Windows: spawn
/// failed), so each shim can render its own "reinstall PHP" guidance.
///
/// Two callers: the shims through [`exec_php`], and `yerd exec`, which maps the
/// same outcomes onto its own exit-code table rather than the shims' flat
/// failure.
pub(crate) fn run_php(mut cmd: Command) -> Result<ExitCode, std::io::Error> {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt as _;
        Err(cmd.exec())
    }
    #[cfg(windows)]
    {
        let status = cmd.status()?;
        Ok(exit_code_from_status(status))
    }
}

/// Run `cmd` through [`run_php`] and turn a failure to start the interpreter
/// into the shared "reinstall PHP" guidance.
///
/// `minor` is the version to name in that guidance: `Some("8.4")` suggests
/// `yerd install php 8.4` for the shims that target one explicit version, and
/// `None` suggests a bare `yerd install php` for the tool shims, which run
/// whatever the default resolves to.
pub(crate) fn exec_php(cmd: Command, php_bin: &Path, minor: Option<&str>) -> ExitCode {
    let suffix = minor.map_or_else(String::new, |m| format!(" {m}"));
    match run_php(cmd) {
        Ok(code) => code,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => fail(format!(
            "PHP binary not found at {} ({err}) — reinstall with `yerd install php{suffix}`",
            php_bin.display()
        )),
        Err(err) => fail(format!("failed to exec {}: {err}", php_bin.display())),
    }
}

/// Map a finished child's Windows exit status to a process [`ExitCode`]. A code
/// in `0..=255` maps exactly; anything outside that (a large `0xC000_xxxx`-style
/// status, or a would-be-negative code) collapses to a generic failure `1` so a
/// non-zero status never truncates to a success `0`.
#[cfg(windows)]
fn exit_code_from_status(status: std::process::ExitStatus) -> ExitCode {
    match status.code() {
        Some(code) => ExitCode::from(u8::try_from(code).unwrap_or(1)),
        None => ExitCode::FAILURE,
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
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

    fn fake_cli(dirs: &PlatformDirs, minor: &str) {
        let bin = cli_binary(dirs, minor);
        std::fs::create_dir_all(bin.parent().unwrap()).unwrap();
        std::fs::write(&bin, b"x").unwrap();
    }

    #[test]
    fn highest_installed_skips_legacy_versions() {
        let tmp = tempfile::tempdir().unwrap();
        let dirs = dirs_at(tmp.path());
        fake_cli(&dirs, "8.0");
        fake_cli(&dirs, "8.2");
        assert_eq!(
            highest_installed(&dirs).map(|(_, m)| m),
            Some("8.2".to_owned()),
            "legacy 8.0 is skipped; 8.2 wins"
        );
    }

    #[test]
    fn highest_installed_none_when_only_legacy() {
        let tmp = tempfile::tempdir().unwrap();
        let dirs = dirs_at(tmp.path());
        fake_cli(&dirs, "7.4");
        assert_eq!(highest_installed(&dirs), None);
        assert!(any_installed(&dirs), "7.4 is still installed");
    }

    #[test]
    fn no_default_message_distinguishes_legacy_only_from_empty() {
        let tmp = tempfile::tempdir().unwrap();
        let dirs = dirs_at(tmp.path());
        std::fs::create_dir_all(dirs.data.join("php")).unwrap();
        assert!(no_default_php_message(&dirs).contains("no PHP installed"));
        fake_cli(&dirs, "7.4");
        let msg = no_default_php_message(&dirs);
        assert!(msg.contains("supported"));
        assert!(msg.contains("php7.4"));
    }

    #[cfg(not(windows))]
    #[test]
    fn minor_is_legacy_splits_at_floor() {
        assert!(minor_is_legacy("7.4"));
        assert!(minor_is_legacy("8.1"));
        assert!(!minor_is_legacy("8.2"));
        assert!(!minor_is_legacy("8.5"));
        assert!(!minor_is_legacy("garbage"));
    }

    #[cfg(not(windows))]
    #[test]
    fn minor_from_php_path_extracts_dotted_minor() {
        assert_eq!(
            minor_from_php_path(Path::new("/d/php/php-8.4/bin/php")).as_deref(),
            Some("8.4")
        );
        assert_eq!(minor_from_php_path(Path::new("/d/bin/php")), None);
    }

    #[test]
    fn shim_invocation_reads_argv0_basename() {
        let args = [OsString::from("/data/bin/php"), OsString::from("-v")];
        let (name, forward) = shim_invocation_from(&args).unwrap();
        assert_eq!(name, "php");
        assert_eq!(forward, vec![OsString::from("-v")]);
    }

    #[test]
    fn shim_invocation_reads_sentinel_tool_and_forwards_tail() {
        let args = [
            OsString::from(r"C:\bin\yerd.exe"),
            OsString::from("__shim"),
            OsString::from("php8.4cover"),
            OsString::from("artisan"),
            OsString::from("test"),
        ];
        let (name, forward) = shim_invocation_from(&args).unwrap();
        assert_eq!(name, "php8.4cover");
        assert_eq!(
            forward,
            vec![OsString::from("artisan"), OsString::from("test")]
        );
    }

    #[test]
    fn shim_invocation_sentinel_without_tool_is_none() {
        let args = [OsString::from("yerd"), OsString::from("__shim")];
        assert!(shim_invocation_from(&args).is_none());
    }

    #[test]
    fn shim_invocation_sentinel_with_empty_tail_forwards_nothing() {
        let args = [
            OsString::from("yerd"),
            OsString::from("__shim"),
            OsString::from("composer"),
        ];
        let (name, forward) = shim_invocation_from(&args).unwrap();
        assert_eq!(name, "composer");
        assert!(forward.is_empty());
    }

    #[test]
    fn shim_invocation_normal_invocation_keeps_full_tail() {
        let args = [
            OsString::from("yerd"),
            OsString::from("doctor"),
            OsString::from("--json"),
        ];
        let (name, forward) = shim_invocation_from(&args).unwrap();
        assert!(name == "yerd" || name == "yerd.exe");
        assert_eq!(
            forward,
            vec![OsString::from("doctor"), OsString::from("--json")]
        );
    }

    #[test]
    fn cli_phprc_prefers_per_version_then_base_then_none() {
        let tmp = tempfile::tempdir().unwrap();
        let dirs = PlatformDirs {
            config: tmp.path().join("c"),
            data: tmp.path().join("d"),
            state: tmp.path().join("s"),
            cache: tmp.path().join("ca"),
            runtime: tmp.path().join("r"),
        };
        std::fs::create_dir_all(&dirs.data).unwrap();

        assert_eq!(cli_phprc(&dirs, "8.4"), None);

        let base = dirs.data.join("php-cli.ini");
        std::fs::write(&base, "; base\n").unwrap();
        assert_eq!(cli_phprc(&dirs, "8.4"), Some(base.clone()));

        let per_version = dirs.data.join("php-cli-8.4.ini");
        std::fs::write(&per_version, "; per-version\n").unwrap();
        assert_eq!(cli_phprc(&dirs, "8.4"), Some(per_version));
    }
}
