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
    let Some(composer) = super::resolve_composer(&composer_phar, &user_dirs, &data_bin, data_root)
    else {
        return Outcome::Failed("Composer is not installed - install it first".to_owned());
    };

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

    let job_bin = match super::build_job_bin(job_dir, &php_cli, composer.managed_phar()) {
        Ok(b) => b,
        Err(msg) => return Outcome::Failed(msg),
    };
    let path_env = super::composed_path(&job_bin, &data_bin, &user_dirs);
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
        None,
        false,
        None,
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
    let tld = state.config.lock().await.tld.as_str().to_owned();
    let url = crate::public_url::browser_url(state, &format!("{name}.{tld}"), spec.secure).await;
    state.jobs.push_log(id, format!("serving {url}")).await;
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
}
