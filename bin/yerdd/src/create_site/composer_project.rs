//! `composer create-project` scaffolding for frameworks whose official starter
//! is a Composer project template (`CodeIgniter` 4, `CakePHP`, Slim).
//!
//! Preflight resolves PHP + Composer and builds the same per-job `PATH` the
//! Laravel job uses, so any `php`/`composer` the template's post-install
//! scripts shell out to runs on the requested version. Scaffolding runs
//! `composer create-project`; a per-framework post step then fixes up what the
//! template leaves unsuitable for local development; Registering reuses the
//! shared [`super::registration`].

use std::path::{Path, PathBuf};
use std::sync::Arc;

use tokio::sync::watch;

use yerd_ipc::{CreateSiteSpec, Framework};

use super::{Composer, Outcome, StreamedOutcome};
use crate::state::DaemonState;
use crate::tools;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Template {
    Codeigniter,
    Cakephp,
    Slim,
}

impl Template {
    pub(super) fn from_framework(framework: &Framework) -> Option<Self> {
        match framework {
            Framework::Codeigniter => Some(Self::Codeigniter),
            Framework::Cakephp => Some(Self::Cakephp),
            Framework::Slim => Some(Self::Slim),
            _ => None,
        }
    }

    fn package(self) -> &'static str {
        match self {
            Self::Codeigniter => "codeigniter4/appstarter",
            Self::Cakephp => "cakephp/app",
            Self::Slim => "slim/skeleton",
        }
    }
}

/// Preflight + Scaffolding + post step + Registering for a Composer template.
#[allow(clippy::too_many_lines)]
pub(super) async fn run(
    id: &str,
    name: &str,
    spec: &CreateSiteSpec,
    template: Template,
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

    let composer_phar = tools::composer::phar_path(dirs);
    let Some(composer) =
        super::resolve_composer(&composer_phar, &user_dirs, &data_bin, &dirs.data)
    else {
        return Outcome::Failed("Composer is not installed - install it first".to_owned());
    };

    if let Err(msg) = super::check_target_dir(&project_dir) {
        return Outcome::Failed(msg);
    }
    if let Err(msg) = super::probe_writable(&spec.parent_dir) {
        return Outcome::Failed(msg);
    }

    let job_bin = match super::build_job_bin(job_dir, &php_cli, composer.managed_phar()) {
        Ok(b) => b,
        Err(msg) => return Outcome::Failed(msg),
    };
    let path_env = super::composed_path(&job_bin, &data_bin, &user_dirs);
    let composer_home = tools::laravel::composer_home(dirs);

    state.jobs.set_phase(id, "Scaffolding").await;
    let args = create_project_args(template, name);
    state
        .jobs
        .push_log(id, format!("$ composer {CREATE_PROJECT} {}", args.join(" ")))
        .await;

    let command = composer_command(&composer, &php_cli, args);
    let scaffold = super::run_streamed(
        id,
        &command.program,
        &[],
        &command.entry_point,
        &command.args,
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
    if !project_dir.join("composer.json").is_file() {
        let _ = std::fs::remove_dir_all(&project_dir);
        return Outcome::Failed(format!(
            "composer {CREATE_PROJECT} finished without creating {}",
            project_dir.display()
        ));
    }

    let tld = state.config.lock().await.tld.as_str().to_owned();
    let url = crate::public_url::site_url(state, &format!("{name}.{tld}"), spec.secure).await;

    if template == Template::Codeigniter {
        if let Err(msg) = write_codeigniter_env(&project_dir, &format!("{url}/")) {
            state
                .jobs
                .push_log(id, format!("warning: could not write .env: {msg}"))
                .await;
        }
    }

    state.jobs.set_phase(id, "Registering").await;
    if let Err(msg) =
        super::registration::register(name, &spec.parent_dir, &project_dir, spec, state).await
    {
        return Outcome::Failed(format!("scaffolded, but registration failed: {msg}"));
    }
    state.jobs.push_log(id, format!("serving {url}")).await;
    Outcome::Succeeded
}

const CREATE_PROJECT: &str = "create-project";

#[derive(Debug, PartialEq, Eq)]
struct Command {
    program: PathBuf,
    entry_point: PathBuf,
    args: Vec<String>,
}

/// Map `composer create-project <args…>` onto [`super::run_streamed`]'s
/// `<program> <entry_point> <args…>` shape. Pure - unit-tested.
fn composer_command(composer: &Composer, php_cli: &Path, args: Vec<String>) -> Command {
    match composer {
        Composer::Managed(phar) => Command {
            program: php_cli.to_path_buf(),
            entry_point: phar.clone(),
            args: std::iter::once(CREATE_PROJECT.to_owned()).chain(args).collect(),
        },
        Composer::External(bin) => Command {
            program: bin.clone(),
            entry_point: PathBuf::from(CREATE_PROJECT),
            args,
        },
    }
}

/// The `composer create-project` arguments after the subcommand. Pure -
/// unit-tested.
///
/// `--no-interaction` accepts each template's post-install defaults (`CakePHP`
/// asks whether to set folder permissions); `--no-ansi` keeps the plain-text
/// job log readable, as for `laravel new`.
fn create_project_args(template: Template, name: &str) -> Vec<String> {
    vec![
        "--no-interaction".to_owned(),
        "--no-ansi".to_owned(),
        "--prefer-dist".to_owned(),
        template.package().to_owned(),
        name.to_owned(),
    ]
}

/// `base_url` must end in `/`, which `CodeIgniter`'s `app.baseURL` requires.
fn write_codeigniter_env(project_dir: &Path, base_url: &str) -> Result<(), String> {
    let dotenv = project_dir.join(".env");
    if dotenv.exists() {
        return Ok(());
    }
    let template = std::fs::read_to_string(project_dir.join("env")).unwrap_or_default();
    std::fs::write(&dotenv, codeigniter_env(&template, base_url))
        .map_err(|e| format!("{}: {e}", dotenv.display()))
}

/// Rewrite `CodeIgniter`'s `env` template for local development. Pure -
/// unit-tested.
///
/// The template ships every setting commented out, so the app would boot in
/// `production` mode (errors hidden) with an empty `app.baseURL`. The first
/// `CI_ENVIRONMENT` and `app.baseURL` lines, commented or not, are replaced;
/// either key missing from the template is appended instead.
fn codeigniter_env(template: &str, base_url: &str) -> String {
    let env_line = "CI_ENVIRONMENT = development".to_owned();
    let url_line = format!("app.baseURL = '{base_url}'");
    let mut env_set = false;
    let mut url_set = false;
    let mut lines: Vec<String> = Vec::new();
    for line in template.lines() {
        match env_key(line) {
            Some("CI_ENVIRONMENT") if !env_set => {
                lines.push(env_line.clone());
                env_set = true;
            }
            Some("app.baseURL") if !url_set => {
                lines.push(url_line.clone());
                url_set = true;
            }
            _ => lines.push(line.to_owned()),
        }
    }
    if !env_set {
        lines.push(env_line);
    }
    if !url_set {
        lines.push(url_line);
    }
    let mut out = lines.join("\n");
    out.push('\n');
    out
}

/// Key of a `key = value` line, active or commented out. `None` for prose.
fn env_key(line: &str) -> Option<&str> {
    let (key, _) = line.trim_start().trim_start_matches('#').split_once('=')?;
    let key = key.trim();
    (!key.is_empty() && !key.contains(char::is_whitespace)).then_some(key)
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

    #[test]
    fn from_framework_maps_each_composer_framework() {
        for (framework, template) in [
            (Framework::Codeigniter, Template::Codeigniter),
            (Framework::Cakephp, Template::Cakephp),
            (Framework::Slim, Template::Slim),
        ] {
            assert_eq!(Template::from_framework(&framework), Some(template));
        }
    }

    #[test]
    fn create_project_args_per_template() {
        for (template, package) in [
            (Template::Codeigniter, "codeigniter4/appstarter"),
            (Template::Cakephp, "cakephp/app"),
            (Template::Slim, "slim/skeleton"),
        ] {
            assert_eq!(
                create_project_args(template, "app"),
                vec!["--no-interaction", "--no-ansi", "--prefer-dist", package, "app"],
                "{template:?}"
            );
        }
    }

    #[test]
    fn composer_command_runs_managed_phar_under_job_php() {
        let cmd = composer_command(
            &Composer::Managed(PathBuf::from("/data/composer.phar")),
            Path::new("/data/php/8.3/bin/php"),
            vec!["--no-ansi".to_owned()],
        );
        assert_eq!(
            cmd,
            Command {
                program: PathBuf::from("/data/php/8.3/bin/php"),
                entry_point: PathBuf::from("/data/composer.phar"),
                args: vec!["create-project".to_owned(), "--no-ansi".to_owned()],
            }
        );
    }

    #[test]
    fn composer_command_executes_external_composer_directly() {
        let cmd = composer_command(
            &Composer::External(PathBuf::from("/home/me/.local/share/mise/shims/composer")),
            Path::new("/data/php/8.3/bin/php"),
            vec!["--no-ansi".to_owned()],
        );
        assert_eq!(
            cmd,
            Command {
                program: PathBuf::from("/home/me/.local/share/mise/shims/composer"),
                entry_point: PathBuf::from("create-project"),
                args: vec!["--no-ansi".to_owned()],
            }
        );
    }

    #[test]
    fn env_key_reads_active_and_commented_settings() {
        for (line, expected) in [
            ("CI_ENVIRONMENT = production", Some("CI_ENVIRONMENT")),
            ("# CI_ENVIRONMENT = production", Some("CI_ENVIRONMENT")),
            ("  #app.baseURL = ''", Some("app.baseURL")),
            (
                "# database.default.hostname = localhost",
                Some("database.default.hostname"),
            ),
            ("# If you use this file, rename it to .env", None),
            ("# Set it = to something", None),
            ("#--------------------------------", None),
            ("", None),
        ] {
            assert_eq!(env_key(line), expected, "{line:?}");
        }
    }

    #[test]
    fn codeigniter_env_replaces_commented_template_lines() {
        let template = "\
#--------------------------------------------------------------------
# ENVIRONMENT
#--------------------------------------------------------------------

# CI_ENVIRONMENT = production

#--------------------------------------------------------------------
# APP
#--------------------------------------------------------------------

# app.baseURL = ''
# app.forceGlobalSecureRequests = false
";
        let out = codeigniter_env(template, "https://blog.test/");
        assert!(out.contains("\nCI_ENVIRONMENT = development\n"), "{out}");
        assert!(out.contains("\napp.baseURL = 'https://blog.test/'\n"), "{out}");
        assert!(out.contains("# app.forceGlobalSecureRequests = false\n"));
        assert!(!out.contains("production"));
        assert_eq!(out.lines().count(), template.lines().count());
    }

    #[test]
    fn codeigniter_env_replaces_only_the_first_occurrence() {
        let out = codeigniter_env(
            "# CI_ENVIRONMENT = production\n# CI_ENVIRONMENT = testing\n",
            "http://a.test/",
        );
        assert_eq!(
            out,
            "CI_ENVIRONMENT = development\n# CI_ENVIRONMENT = testing\napp.baseURL = 'http://a.test/'\n"
        );
    }

    #[test]
    fn codeigniter_env_appends_missing_keys() {
        assert_eq!(
            codeigniter_env("", "http://a.test/"),
            "CI_ENVIRONMENT = development\napp.baseURL = 'http://a.test/'\n"
        );
    }

    #[test]
    fn write_codeigniter_env_creates_dotenv_from_template() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("env"), "# CI_ENVIRONMENT = production\n").unwrap();
        write_codeigniter_env(tmp.path(), "http://a.test/").unwrap();
        let dotenv = std::fs::read_to_string(tmp.path().join(".env")).unwrap();
        assert!(dotenv.starts_with("CI_ENVIRONMENT = development\n"));
    }

    #[test]
    fn write_codeigniter_env_keeps_existing_dotenv() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join(".env"), "KEEP=1\n").unwrap();
        write_codeigniter_env(tmp.path(), "http://a.test/").unwrap();
        let dotenv = std::fs::read_to_string(tmp.path().join(".env")).unwrap();
        assert_eq!(dotenv, "KEEP=1\n");
    }
}
