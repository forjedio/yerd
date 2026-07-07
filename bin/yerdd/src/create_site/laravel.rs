//! Laravel scaffolding (`laravel new`) - the Laravel-specific body of the
//! create-site job. Preflight resolves PHP/Composer/the Laravel installer and
//! builds a per-job `PATH` that pins them for the installer's nested
//! `composer create-project`; Scaffolding runs `laravel new` with piped,
//! streamed stdio; Registering reuses the shared [`super::registration`].

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;

use tokio::sync::watch;

use yerd_ipc::{
    AuthProvider, CreateSiteSpec, Database, JsRuntime, LaravelOptions, StarterKit, Testing,
};

use super::{Outcome, StreamedOutcome};
use crate::state::DaemonState;
use crate::tools::{self, Tool};

/// Preflight + Scaffolding + Registering for a Laravel site.
#[allow(clippy::too_many_lines)]
pub(super) async fn run(
    id: &str,
    name: &str,
    spec: &CreateSiteSpec,
    options: &LaravelOptions,
    job_dir: &Path,
    state: &Arc<DaemonState>,
    mut cancel_rx: watch::Receiver<bool>,
) -> Outcome {
    let dirs = &state.dirs;
    let project_dir = spec.parent_dir.join(name);

    state.jobs.set_phase(id, "Preflight").await;

    let php_cli = crate::php_install::cli_binary_path(dirs, spec.php);
    if !php_cli.is_file() {
        return Outcome::Failed(format!(
            "PHP {}.{} is not installed",
            spec.php.major, spec.php.minor
        ));
    }

    let user_dirs = crate::tools::external::resolve_user_path()
        .await
        .unwrap_or_default();
    let data_bin = tools::bin_dir(dirs);
    let data_root = &dirs.data;

    let composer_phar = tools::composer::phar_path(dirs);
    let composer_managed = composer_phar.is_file();
    if !composer_managed
        && crate::tools::external::find_in_path(&user_dirs, "composer", &data_bin, data_root)
            .is_none()
    {
        return Outcome::Failed("Composer is not installed - install it first".to_owned());
    }

    let managed_installer = tools::laravel::installer_bin(dirs);
    let installer_bin = if managed_installer.is_file() {
        managed_installer
    } else if let Some(ext) =
        crate::tools::external::find_in_path(&user_dirs, "laravel", &data_bin, data_root)
    {
        ext
    } else {
        return Outcome::Failed(
            "the Laravel installer is not installed - install it first".to_owned(),
        );
    };

    if let Err(msg) = super::check_target_dir(&project_dir) {
        return Outcome::Failed(msg);
    }
    if let Err(msg) = super::probe_writable(&spec.parent_dir) {
        return Outcome::Failed(msg);
    }

    if let Err(msg) = ensure_js_runtime(id, options.js, &user_dirs, state).await {
        return Outcome::Failed(msg);
    }

    let job_bin = match build_job_bin(
        job_dir,
        &php_cli,
        composer_managed.then_some(composer_phar.as_path()),
    ) {
        Ok(b) => b,
        Err(msg) => return Outcome::Failed(msg),
    };
    let path_env = composed_path(&job_bin, &data_bin, &user_dirs);
    let composer_home = tools::laravel::composer_home(dirs);

    if needs_git(options) && !git_available(&path_env).await {
        return Outcome::Failed(
            "git was not found on PATH - install git to use a starter kit or git init".to_owned(),
        );
    }

    state.jobs.set_phase(id, "Scaffolding").await;
    let args = build_new_args(name, options);
    state
        .jobs
        .push_log(id, format!("$ laravel {}", args.join(" ")))
        .await;

    let scaffold = super::run_streamed(
        id,
        &php_cli,
        &[],
        &installer_bin,
        &args,
        &spec.parent_dir,
        Some(&path_env),
        Some(&composer_home),
        false,
        state,
        &mut cancel_rx,
    )
    .await;
    match scaffold {
        StreamedOutcome::Ok => {}
        StreamedOutcome::Failed(msg) => {
            let _ = std::fs::remove_dir_all(&project_dir);
            return Outcome::Failed(msg);
        }
        StreamedOutcome::Cancelled => {
            let _ = std::fs::remove_dir_all(&project_dir);
            return Outcome::Cancelled;
        }
    }

    state.jobs.set_phase(id, "Registering").await;
    if let Err(msg) =
        super::registration::register(name, &spec.parent_dir, &project_dir, spec, state).await
    {
        return Outcome::Failed(format!("scaffolded, but registration failed: {msg}"));
    }
    state
        .jobs
        .push_log(id, format!("serving https://{name}.test"))
        .await;
    Outcome::Succeeded
}

/// Build the `laravel new …` argument vector (after the installer binary).
/// Pure - unit-tested.
///
/// `--no-ansi` is included because the stream is rendered in a plain text panel
/// with no ANSI interpreter; it is forwarded to the composer/npm commands the
/// installer shells out to, cutting down on raw escape sequences (the daemon
/// also forces NO_COLOR/TERM=dumb on the child, see `super::run_streamed`).
/// Anything that still slips through is stripped defensively in
/// `crate::jobs::JobRegistry::push_log`.
fn build_new_args(name: &str, o: &LaravelOptions) -> Vec<String> {
    let mut a = vec![
        "new".to_owned(),
        name.to_owned(),
        "--no-interaction".to_owned(),
        "--no-ansi".to_owned(),
    ];
    match &o.starter_kit {
        StarterKit::None => {}
        StarterKit::React => a.push("--react".to_owned()),
        StarterKit::Vue => a.push("--vue".to_owned()),
        StarterKit::Livewire => a.push("--livewire".to_owned()),
        StarterKit::Svelte => a.push("--svelte".to_owned()),
        StarterKit::Community(pkg) => {
            a.push("--using".to_owned());
            a.push(pkg.clone());
        }
    }
    if matches!(o.auth, AuthProvider::WorkOs) {
        a.push("--workos".to_owned());
    }
    if o.livewire_class_components {
        a.push("--livewire-class-components".to_owned());
    }
    if o.teams {
        a.push("--teams".to_owned());
    }
    match o.testing {
        Testing::Pest => a.push("--pest".to_owned()),
        Testing::PhpUnit => a.push("--phpunit".to_owned()),
    }
    a.push("--database".to_owned());
    a.push(database_flag(o.database).to_owned());
    match o.js {
        JsRuntime::Npm => a.push("--npm".to_owned()),
        JsRuntime::Bun => a.push("--bun".to_owned()),
        JsRuntime::Skip => {}
    }
    if o.git {
        a.push("--git".to_owned());
    }
    a.push(if o.boost { "--boost" } else { "--no-boost" }.to_owned());
    a
}

fn database_flag(d: Database) -> &'static str {
    match d {
        Database::Sqlite => "sqlite",
        Database::Mysql => "mysql",
        Database::Mariadb => "mariadb",
        Database::Pgsql => "pgsql",
        Database::Sqlsrv => "sqlsrv",
    }
}

/// Whether the installer will run `git` (any starter kit, or an explicit
/// `--git`).
fn needs_git(o: &LaravelOptions) -> bool {
    o.git || !matches!(o.starter_kit, StarterKit::None)
}

/// Install Node/Bun if the chosen JS runtime needs it and it's neither managed
/// nor available externally on the user's PATH. A thin wrapper over
/// [`super::ensure_tool`] mapping [`JsRuntime`] to the [`Tool`] it needs.
async fn ensure_js_runtime(
    id: &str,
    js: JsRuntime,
    user_dirs: &[PathBuf],
    state: &Arc<DaemonState>,
) -> Result<(), String> {
    let tool = match js {
        JsRuntime::Npm => Tool::Node,
        JsRuntime::Bun => Tool::Bun,
        JsRuntime::Skip => return Ok(()),
    };
    super::ensure_tool(id, tool, user_dirs, state).await
}

/// Compose `PATH` = `<per-job bin> : <{data}/bin> : <user PATH> : <inherited>`.
/// The user's resolved PATH is appended so externally-installed
/// composer/node/bun/git/laravel are findable, while the per-job bin (managed
/// `php`) and Yerd shims keep precedence.
fn composed_path(job_bin: &Path, data_bin: &Path, user_dirs: &[PathBuf]) -> std::ffi::OsString {
    let mut entries = vec![job_bin.to_path_buf(), data_bin.to_path_buf()];
    entries.extend(user_dirs.iter().cloned());
    if let Some(existing) = std::env::var_os("PATH") {
        entries.extend(std::env::split_paths(&existing));
    }
    std::env::join_paths(entries).unwrap_or_else(|_| std::ffi::OsString::from(job_bin))
}

/// `git --version` resolves on the composed PATH.
async fn git_available(path_env: &std::ffi::OsString) -> bool {
    tokio::process::Command::new("git")
        .arg("--version")
        .env("PATH", path_env)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await
        .is_ok_and(|s| s.success())
}

/// Single-quote a path for safe inclusion in a `/bin/sh` script, escaping any
/// embedded single quotes (`'` → `'\''`). Without this a data dir containing a
/// `'` (e.g. `/Users/o'brien/…`) would produce a broken wrapper script.
#[cfg(unix)]
fn sh_quote(p: &Path) -> String {
    format!("'{}'", p.to_string_lossy().replace('\'', "'\\''"))
}

/// Build `{job_dir}/bin` containing a `php` symlink to the chosen version and,
/// when `composer_phar` is `Some` (Yerd-managed Composer), a `composer` wrapper
/// that runs that same PHP so the installer's nested `composer create-project`
/// uses the requested runtime (Composer derives its child PHP from `PHP_BINARY`).
/// When `None` (external Composer), no wrapper is written - Composer is found on
/// the composed PATH and runs under the managed `php` via its shebang. Unix-only.
#[cfg(unix)]
fn build_job_bin(
    job_dir: &Path,
    php_cli: &Path,
    composer_phar: Option<&Path>,
) -> Result<PathBuf, String> {
    use std::os::unix::fs::PermissionsExt;

    let bin = job_dir.join("bin");
    std::fs::create_dir_all(&bin).map_err(|e| format!("{}: {e}", bin.display()))?;

    let php_link = bin.join("php");
    let _ = std::fs::remove_file(&php_link);
    std::os::unix::fs::symlink(php_cli, &php_link).map_err(|e| format!("link php: {e}"))?;

    if let Some(phar) = composer_phar {
        let composer = bin.join("composer");
        let script = format!(
            "#!/bin/sh\nexec {} {} \"$@\"\n",
            sh_quote(php_cli),
            sh_quote(phar)
        );
        std::fs::write(&composer, script).map_err(|e| format!("write composer wrapper: {e}"))?;
        std::fs::set_permissions(&composer, std::fs::Permissions::from_mode(0o755))
            .map_err(|e| format!("chmod composer wrapper: {e}"))?;
    }
    Ok(bin)
}

#[cfg(not(unix))]
fn build_job_bin(
    _job_dir: &Path,
    _php_cli: &Path,
    _composer_phar: Option<&Path>,
) -> Result<PathBuf, String> {
    Err("site creation is not yet supported on this platform".to_owned())
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
    use yerd_ipc::{AuthProvider, Database, JsRuntime, LaravelOptions, StarterKit, Testing};

    fn opts() -> LaravelOptions {
        LaravelOptions {
            starter_kit: StarterKit::None,
            auth: AuthProvider::Laravel,
            livewire_class_components: false,
            teams: false,
            testing: Testing::Pest,
            database: Database::Sqlite,
            js: JsRuntime::Skip,
            git: false,
            boost: false,
        }
    }

    #[test]
    fn minimal_args() {
        let a = build_new_args("blog", &opts());
        assert_eq!(
            a,
            vec![
                "new",
                "blog",
                "--no-interaction",
                "--no-ansi",
                "--pest",
                "--database",
                "sqlite",
                "--no-boost",
            ]
        );
    }

    #[test]
    fn react_pest_sqlite_npm_git_args() {
        let mut o = opts();
        o.starter_kit = StarterKit::React;
        o.js = JsRuntime::Npm;
        o.git = true;
        let a = build_new_args("shop", &o);
        assert_eq!(
            a,
            vec![
                "new",
                "shop",
                "--no-interaction",
                "--no-ansi",
                "--react",
                "--pest",
                "--database",
                "sqlite",
                "--npm",
                "--git",
                "--no-boost",
            ]
        );
    }

    #[test]
    fn livewire_workos_teams_phpunit_pgsql_bun_boost_args() {
        let o = LaravelOptions {
            starter_kit: StarterKit::Livewire,
            auth: AuthProvider::WorkOs,
            livewire_class_components: true,
            teams: true,
            testing: Testing::PhpUnit,
            database: Database::Pgsql,
            js: JsRuntime::Bun,
            git: false,
            boost: true,
        };
        let a = build_new_args("crm", &o);
        assert_eq!(
            a,
            vec![
                "new",
                "crm",
                "--no-interaction",
                "--no-ansi",
                "--livewire",
                "--workos",
                "--livewire-class-components",
                "--teams",
                "--phpunit",
                "--database",
                "pgsql",
                "--bun",
                "--boost",
            ]
        );
    }

    #[test]
    fn community_kit_uses_using_with_package() {
        let mut o = opts();
        o.starter_kit = StarterKit::Community("acme/kit".to_owned());
        let a = build_new_args("x", &o);
        assert!(a.windows(2).any(|w| w == ["--using", "acme/kit"]));
    }

    #[test]
    fn needs_git_for_kits_and_explicit_flag() {
        let mut o = opts();
        assert!(!needs_git(&o));
        o.git = true;
        assert!(needs_git(&o));
        o.git = false;
        o.starter_kit = StarterKit::Vue;
        assert!(needs_git(&o));
    }

    #[test]
    fn database_flag_covers_every_engine() {
        for (db, flag) in [
            (Database::Sqlite, "sqlite"),
            (Database::Mysql, "mysql"),
            (Database::Mariadb, "mariadb"),
            (Database::Pgsql, "pgsql"),
            (Database::Sqlsrv, "sqlsrv"),
        ] {
            let mut o = opts();
            o.database = db;
            let a = build_new_args("app", &o);
            let idx = a.iter().position(|s| s == "--database").unwrap();
            assert_eq!(a[idx + 1], flag, "wrong flag for {db:?}");
        }
    }

    #[test]
    fn svelte_kit_and_skip_js_emit_expected_flags() {
        let mut o = opts();
        o.starter_kit = StarterKit::Svelte;
        let a = build_new_args("app", &o);
        assert!(a.iter().any(|s| s == "--svelte"));
        assert!(!a.iter().any(|s| s == "--npm" || s == "--bun"));
    }

    #[test]
    fn npm_runtime_emits_npm_flag() {
        let mut o = opts();
        o.js = JsRuntime::Npm;
        let a = build_new_args("app", &o);
        assert!(a.iter().any(|s| s == "--npm"));
    }

    #[test]
    fn needs_git_true_for_community_kit() {
        let mut o = opts();
        o.starter_kit = StarterKit::Community("acme/kit".to_owned());
        assert!(needs_git(&o));
    }

    #[cfg(unix)]
    #[test]
    fn composed_path_puts_job_bin_first() {
        let job_bin = Path::new("/jobs/abc/bin");
        let data_bin = Path::new("/data/bin");
        let user = vec![PathBuf::from("/opt/homebrew/bin")];
        let composed = composed_path(job_bin, data_bin, &user);
        let entries: Vec<PathBuf> = std::env::split_paths(&composed).collect();
        assert_eq!(entries.first().unwrap(), job_bin);
        assert_eq!(entries.get(1).unwrap(), data_bin);
        assert!(entries.iter().any(|p| p == Path::new("/opt/homebrew/bin")));
    }

    #[cfg(unix)]
    #[test]
    fn sh_quote_escapes_embedded_single_quotes() {
        assert_eq!(sh_quote(Path::new("/Users/obrien")), "'/Users/obrien'");
        assert_eq!(
            sh_quote(Path::new("/Users/o'brien/data")),
            "'/Users/o'\\''brien/data'"
        );
    }

    #[cfg(unix)]
    #[test]
    fn build_job_bin_links_php_and_writes_composer_wrapper() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().unwrap();
        let job_dir = tmp.path().join("job");
        let php = tmp.path().join("php-bin");
        std::fs::write(&php, b"#!fake-php").unwrap();
        let phar = tmp.path().join("composer.phar");
        std::fs::write(&phar, b"phar").unwrap();

        let bin = build_job_bin(&job_dir, &php, Some(phar.as_path())).unwrap();
        assert_eq!(std::fs::read_link(bin.join("php")).unwrap(), php);
        let wrapper = std::fs::read_to_string(bin.join("composer")).unwrap();
        assert!(wrapper.starts_with("#!/bin/sh\n"));
        assert!(wrapper.contains(&php.to_string_lossy().into_owned()));
        assert!(wrapper.contains(&phar.to_string_lossy().into_owned()));
        let mode = std::fs::metadata(bin.join("composer"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o111, 0o111, "wrapper should be executable");
    }

    #[cfg(unix)]
    #[test]
    fn build_job_bin_without_phar_writes_no_composer_wrapper() {
        let tmp = tempfile::tempdir().unwrap();
        let job_dir = tmp.path().join("job");
        let php = tmp.path().join("php-bin");
        std::fs::write(&php, b"#!fake-php").unwrap();

        let bin = build_job_bin(&job_dir, &php, None).unwrap();
        assert!(bin.join("php").exists());
        assert!(!bin.join("composer").exists());
    }
}
