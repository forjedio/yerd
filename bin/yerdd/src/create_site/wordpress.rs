//! WordPress scaffolding via WP-CLI - the WordPress-specific body of the
//! create-site job. Preflight ensures the *managed* WP-CLI is installed (an
//! external `wp` on PATH doesn't count, since every step runs the managed
//! `boot-fs.php` entry point directly); Provisioning
//! database ensures a MySQL/MariaDB engine is installed+running and creates
//! the site's database; Downloading/Configuring/Installing run `wp core
//! download` / `wp config create` / `wp core install` with piped, streamed
//! stdio; Registering reuses the shared [`super::registration`].

use std::path::Path;
use std::sync::Arc;

use tokio::sync::watch;
use yerd_services::{database, ServiceDefinition, ServiceRegistry, ServiceVersion};

use yerd_ipc::{CreateSiteSpec, WordPressDbEngine, WordPressOptions};

use super::{Outcome, StreamedOutcome};
use crate::state::DaemonState;
use crate::tools::{self, Tool};

/// Preflight + Provisioning database + Downloading/Configuring/Installing +
/// Registering for a WordPress site.
#[allow(clippy::too_many_lines)]
pub(super) async fn run(
    id: &str,
    name: &str,
    spec: &CreateSiteSpec,
    options: &WordPressOptions,
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
    let phprc = crate::php_install::cli_phprc(dirs, spec.php);
    if let Err(e) = validate_admin_credentials(options) {
        return Outcome::Failed(format!("invalid WordPress admin account: {e}"));
    }

    if let Err(msg) = ensure_wp_cli(id, state).await {
        return Outcome::Failed(msg);
    }
    let boot_fs = tools::wp_cli::boot_path(dirs);
    if !boot_fs.is_file() {
        return Outcome::Failed(format!(
            "WP-CLI is installed but {} is missing - reinstall WP-CLI from the Tooling page \
             (or run `yerd install tool wp-cli`)",
            boot_fs.display()
        ));
    }
    if let Err(msg) = super::check_target_dir(&project_dir) {
        return Outcome::Failed(msg);
    }
    if let Err(msg) = super::probe_writable(&spec.parent_dir) {
        return Outcome::Failed(msg);
    }
    let db_name = resolve_db_name(name, options);
    if let Err(e) = database::validate_db_name(&db_name) {
        return Outcome::Failed(format!("invalid database name: {e}"));
    }
    if super::is_cancelled(&cancel_rx) {
        return Outcome::Cancelled;
    }

    state.jobs.set_phase(id, "Provisioning database").await;
    let Some(def) = ServiceRegistry::builtin().get(engine_type_id(options.database.engine)) else {
        return Outcome::Failed("unknown database engine".to_owned());
    };
    if let Err(msg) = ensure_database_engine(id, &def, state).await {
        return Outcome::Failed(msg);
    }
    if super::is_cancelled(&cancel_rx) {
        return Outcome::Cancelled;
    }
    let db_created = match crate::db_admin::create(def.id(), &db_name, state).await {
        yerd_ipc::Response::Ok => true,
        yerd_ipc::Response::Error { message, .. } => return Outcome::Failed(message),
        other => return Outcome::Failed(format!("unexpected response: {other:?}")),
    };
    if super::is_cancelled(&cancel_rx) {
        rollback(&project_dir, db_created, &def, &db_name, state).await;
        return Outcome::Cancelled;
    }

    let db_port = service_port(&def, state).await;

    state.jobs.set_phase(id, "Downloading WordPress").await;
    if let Err(e) = std::fs::create_dir_all(&project_dir) {
        rollback(&project_dir, db_created, &def, &db_name, state).await;
        return Outcome::Failed(format!("{}: {e}", project_dir.display()));
    }
    let download_args = download_args(options);
    state
        .jobs
        .push_log(id, format!("$ wp {}", download_args.join(" ")))
        .await;
    match run_wp_step(
        id,
        &php_cli,
        &boot_fs,
        &download_args,
        &project_dir,
        phprc.as_deref(),
        None,
        state,
        &mut cancel_rx,
    )
    .await
    {
        StreamedOutcome::Ok => {}
        StreamedOutcome::Failed(msg) => {
            rollback(&project_dir, db_created, &def, &db_name, state).await;
            return Outcome::Failed(msg);
        }
        StreamedOutcome::Cancelled => {
            rollback(&project_dir, db_created, &def, &db_name, state).await;
            return Outcome::Cancelled;
        }
    }

    state.jobs.set_phase(id, "Configuring").await;
    let config_args = config_create_args(options, &db_name, db_port);
    state
        .jobs
        .push_log(id, "$ wp config create ...".to_owned())
        .await;
    match run_wp_step(
        id,
        &php_cli,
        &boot_fs,
        &config_args,
        &project_dir,
        phprc.as_deref(),
        None,
        state,
        &mut cancel_rx,
    )
    .await
    {
        StreamedOutcome::Ok => {}
        StreamedOutcome::Failed(msg) => {
            rollback(&project_dir, db_created, &def, &db_name, state).await;
            return Outcome::Failed(msg);
        }
        StreamedOutcome::Cancelled => {
            rollback(&project_dir, db_created, &def, &db_name, state).await;
            return Outcome::Cancelled;
        }
    }

    state.jobs.set_phase(id, "Installing").await;
    let tld = state.config.lock().await.tld.as_str().to_owned();
    let install_args = install_args(name, &tld, spec.secure, options);
    state
        .jobs
        .push_log(id, "$ wp core install ...".to_owned())
        .await;
    match run_wp_step(
        id,
        &php_cli,
        &boot_fs,
        &install_args,
        &project_dir,
        phprc.as_deref(),
        Some(options.admin_password.as_str()),
        state,
        &mut cancel_rx,
    )
    .await
    {
        StreamedOutcome::Ok => {}
        StreamedOutcome::Failed(msg) => {
            rollback(&project_dir, db_created, &def, &db_name, state).await;
            return Outcome::Failed(msg);
        }
        StreamedOutcome::Cancelled => {
            rollback(&project_dir, db_created, &def, &db_name, state).await;
            return Outcome::Cancelled;
        }
    }

    if apply_permalink_structure(
        id,
        &php_cli,
        &boot_fs,
        &project_dir,
        phprc.as_deref(),
        state,
        &mut cancel_rx,
    )
    .await
        == PermalinkOutcome::Cancelled
    {
        rollback(&project_dir, db_created, &def, &db_name, state).await;
        return Outcome::Cancelled;
    }

    state.jobs.set_phase(id, "Registering").await;
    if let Err(msg) =
        super::registration::register(name, &spec.parent_dir, &project_dir, spec, state).await
    {
        return Outcome::Failed(format!("scaffolded, but registration failed: {msg}"));
    }
    enable_default_admin_login(name, state).await;
    let scheme = if spec.secure { "https" } else { "http" };
    state
        .jobs
        .push_log(id, format!("serving {scheme}://{name}.{tld}"))
        .await;
    Outcome::Succeeded
}

/// Ensure the managed WP-CLI, which [`tools::wp_cli::install`] builds by
/// running yerd's own Composer phar - it refuses outright without one, and an
/// external Composer can't stand in.
///
/// WP-CLI is installed inline because it *is* what was asked for: no managed
/// build, no WordPress site. Composer is only its builder, so a missing one is
/// reported rather than installed: yerd's tools are symlinked into `{data}/bin`,
/// which the shell profile *prepends* to `PATH`, so quietly installing it would
/// take over the `composer` command in every one of the user's unrelated
/// projects and run it under yerd's PHP. That's the user's call to make, not a
/// side effect of creating a site - the wizard offers it as an explicit prereq,
/// and this mirrors the Laravel flow, which likewise refuses rather than
/// installing a Composer nobody asked for. Only reached when WP-CLI has to be
/// built; once installed, scaffolding never touches Composer at all.
async fn ensure_wp_cli(id: &str, state: &Arc<DaemonState>) -> Result<(), String> {
    if tools::installed_version(&state.dirs, Tool::WpCli).is_some() {
        return Ok(());
    }
    if tools::installed_version(&state.dirs, Tool::Composer).is_none() {
        return Err(
            "Yerd's own Composer is required to build WP-CLI (an external one can't) - \
                    install it from the Tooling page, or run `yerd install tool composer`"
                .to_owned(),
        );
    }
    super::ensure_managed_tool(id, Tool::WpCli, state).await
}

/// The database name to provision: `options.database.name` if given,
/// otherwise derived from `name`. The wizard pre-fills and validates this
/// client-side, but an empty name (a lazy/scripted `Request::CreateSite`
/// caller, not just the GUI) falls back to the same derivation rather than
/// failing the whole job on `DbNameError::Empty` when a sensible default is
/// derivable.
fn resolve_db_name(name: &str, options: &WordPressOptions) -> String {
    if options.database.name.is_empty() {
        derive_db_name(name)
    } else {
        options.database.name.clone()
    }
}

/// Outcome of [`apply_permalink_structure`]: whether the caller must roll
/// back and stop (`Cancelled`), or carry on regardless (`Applied` - a
/// `wp rewrite structure` failure only logs a warning, it never fails the
/// job; see the function's own doc comment for why).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PermalinkOutcome {
    Applied,
    Cancelled,
}

/// Best-effort: enable pretty (postname-based) permalinks so pages/posts
/// (and `yerd-proxy`'s try_files-style routing, see
/// `script_file::resolve_script`) resolve human-readable URLs by default. A
/// fresh `wp core install` otherwise defaults to "Plain" permalinks
/// (`?p=123`), under which `WordPress` doesn't parse pretty paths as routes
/// at all and silently falls back to the front page. Not fatal, a working
/// `WordPress` install with the default permalink structure is still a
/// successful create, so a `wp rewrite structure` failure is only logged,
/// never rolled back (unlike a genuine scaffolding failure).
#[allow(clippy::too_many_arguments)]
async fn apply_permalink_structure(
    id: &str,
    php_cli: &Path,
    boot_fs: &Path,
    project_dir: &Path,
    phprc: Option<&Path>,
    state: &Arc<DaemonState>,
    cancel_rx: &mut watch::Receiver<bool>,
) -> PermalinkOutcome {
    let permalink_args = permalink_structure_args();
    state
        .jobs
        .push_log(id, "$ wp rewrite structure '/%postname%/'".to_owned())
        .await;
    match run_wp_step(
        id,
        php_cli,
        boot_fs,
        &permalink_args,
        project_dir,
        phprc,
        None,
        state,
        cancel_rx,
    )
    .await
    {
        StreamedOutcome::Ok => PermalinkOutcome::Applied,
        StreamedOutcome::Failed(msg) => {
            state
                .jobs
                .push_log(
                    id,
                    format!("warning: couldn't set pretty permalinks: {msg}"),
                )
                .await;
            PermalinkOutcome::Applied
        }
        StreamedOutcome::Cancelled => PermalinkOutcome::Cancelled,
    }
}

/// Best-effort: enable one-click admin login by default for a site created
/// through this wizard - the whole point of the post-creation "WP Admin"
/// button. Never fails the job. A parked/pre-existing `WordPress` site (not
/// created through this wizard) keeps this off by default; yerd has no basis
/// for assuming that's wanted for an arbitrary directory.
async fn enable_default_admin_login(name: &str, state: &Arc<DaemonState>) {
    let _ = crate::ipc_server::handle_mutation(
        yerd_ipc::Request::SetWordpressAutoLogin {
            name: name.to_owned(),
            enabled: true,
            user: None,
        },
        state,
    )
    .await;
}

/// Run one `wp <subcommand>` invocation, streaming its output into the job
/// log. No custom `PATH`/`COMPOSER_HOME` needed - WP-CLI's own subcommands
/// don't shell out to Composer or rely on PATH-resolved tools, unlike the
/// Laravel installer's nested `composer create-project`.
///
/// The child's cwd is `boot_fs`'s own directory and `boot_fs` is invoked by
/// its bare file name (not `project_dir`, and not `boot_fs`'s full path) -
/// `--path={project_dir}` points WP-CLI at the site instead. This works
/// around a real WP-CLI bug: some subcommands (`rewrite structure` among
/// them) re-invoke themselves via `WP_CLI::launch_self()`, which builds a raw
/// shell string that escapes the PHP binary and arguments but not
/// `$GLOBALS['argv'][0]` (the script path WP-CLI itself was launched with).
/// On macOS that path always runs through `~/Library/Application
/// Support/...`, which always contains a space, so the re-invocation's shell
/// command silently splits mid-path and fails with "Could not open input
/// file". Invoking `boot-fs.php` as a bare relative name keeps that captured
/// argv[0] space-free; `--path=` decouples "which `WordPress` install" from
/// "process cwd" so this has no effect on where WP-CLI actually operates.
#[allow(clippy::too_many_arguments)]
async fn run_wp_step(
    id: &str,
    php_cli: &Path,
    boot_fs: &Path,
    args: &[String],
    project_dir: &Path,
    phprc: Option<&Path>,
    stdin_data: Option<&str>,
    state: &Arc<DaemonState>,
    cancel_rx: &mut watch::Receiver<bool>,
) -> StreamedOutcome {
    let Some((boot_dir, boot_name, full_args)) = wp_step_invocation(boot_fs, project_dir, args)
    else {
        return StreamedOutcome::Failed(format!("{}: not a valid file path", boot_fs.display()));
    };

    let php_flags: Vec<String> = crate::tools::wp_cli::QUIET_DEPRECATIONS
        .iter()
        .map(|s| (*s).to_owned())
        .collect();
    super::run_streamed(
        id, php_cli, &php_flags, &boot_name, &full_args, &boot_dir, None, None, phprc, true,
        stdin_data, state, cancel_rx,
    )
    .await
}

/// Pure - splits `boot_fs` into its own directory and bare file name, and
/// appends `--path={project_dir}` to `args`. `None` if `boot_fs` has no
/// parent/file name (never true for a real path, but `Path` doesn't rule it
/// out statically). See [`run_wp_step`]'s doc comment for why the invocation
/// is split this way instead of just running `boot_fs` from `project_dir`.
fn wp_step_invocation(
    boot_fs: &Path,
    project_dir: &Path,
    args: &[String],
) -> Option<(std::path::PathBuf, std::path::PathBuf, Vec<String>)> {
    let boot_dir = boot_fs.parent()?.to_path_buf();
    let boot_name = std::path::PathBuf::from(boot_fs.file_name()?);
    let mut full_args: Vec<String> = args.to_vec();
    full_args.push(format!("--path={}", project_dir.display()));
    Some((boot_dir, boot_name, full_args))
}

/// Best-effort cleanup on any pre-Registering failure or cancellation: remove
/// the project directory, and drop the database **only if this job itself
/// created it** - a name collision with a pre-existing database never sets
/// `db_created`, so an unrelated database is never touched.
async fn rollback(
    project_dir: &Path,
    db_created: bool,
    def: &Arc<dyn ServiceDefinition>,
    db_name: &str,
    state: &Arc<DaemonState>,
) {
    let _ = std::fs::remove_dir_all(project_dir);
    if db_created {
        let _ = crate::db_admin::drop(def.id(), db_name, state).await;
    }
}

/// Ensure the chosen SQL engine is installed and running, persisting it as
/// the selected instance. If nothing is installed yet, resolves + installs
/// the newest available build for this platform (the daemon-side equivalent
/// of `Request::InstallService`, run inline so its progress streams into this
/// job's own log - see [`super::ensure_tool`] for the same pattern applied to
/// dev tools). Installing/starting the engine is never rolled back on
/// failure - a database engine is shared, persistent infrastructure, not a
/// per-site artifact.
async fn ensure_database_engine(
    id: &str,
    def: &Arc<dyn ServiceDefinition>,
    state: &Arc<DaemonState>,
) -> Result<(), String> {
    let (configured_version, port) = {
        let cfg = state.config.lock().await;
        let inst = cfg.services.instances.get(def.id());
        (
            inst.and_then(|i| i.version.clone()),
            inst.and_then(|i| i.port).unwrap_or(def.default_port()),
        )
    };

    let version =
        match crate::services::resolve_version(def, configured_version.as_deref(), &state.dirs) {
            Ok(v) => v,
            Err(_not_found) => {
                state
                    .jobs
                    .set_phase(id, format!("Installing {}", def.display_name()))
                    .await;
                resolve_and_install_latest(def, state).await?
            }
        };

    crate::services::ensure_and_persist(state, def, def.id(), Some(version), port, None, None)
        .await
        .map_err(response_message)
}

/// Fetch the services listing, pick the newest build available for this
/// platform, and install it.
async fn resolve_and_install_latest(
    def: &Arc<dyn ServiceDefinition>,
    state: &Arc<DaemonState>,
) -> Result<ServiceVersion, String> {
    use yerd_supervise::Downloader;

    let dl = crate::php_install::ReqwestDownloader::new();
    let (os, arch) = yerd_services::current_os_arch().map_err(|e| e.to_string())?;
    let listing_bytes = dl
        .download(&yerd_services::listing_url())
        .await
        .map_err(|e| format!("couldn't reach the services distribution: {e}"))?;
    let listing = String::from_utf8_lossy(&listing_bytes).into_owned();
    let version = yerd_services::available_versions(&listing, def.id(), os, arch)
        .into_iter()
        .last()
        .ok_or_else(|| {
            format!(
                "no {} build is available for this platform",
                def.display_name()
            )
        })?;
    crate::service_install::install(def.id(), def.server_binary(), &version, &state.dirs, &dl)
        .await
        .map_err(|e| e.to_string())?;
    Ok(version)
}

/// The configured (or default) port for `def` - used to build
/// `wp config create --dbhost`.
async fn service_port(def: &Arc<dyn ServiceDefinition>, state: &Arc<DaemonState>) -> u16 {
    let cfg = state.config.lock().await;
    cfg.services
        .instances
        .get(def.id())
        .and_then(|i| i.port)
        .unwrap_or(def.default_port())
}

fn response_message(resp: yerd_ipc::Response) -> String {
    match resp {
        yerd_ipc::Response::Error { message, .. } => message,
        other => format!("unexpected response: {other:?}"),
    }
}

fn engine_type_id(engine: WordPressDbEngine) -> &'static str {
    match engine {
        WordPressDbEngine::Mysql => "mysql",
        WordPressDbEngine::Mariadb => "mariadb",
    }
}

/// Derive a valid database name from a site name: site names may contain
/// hyphens and start with a digit, database names may do neither. Maps
/// hyphens to underscores, prefixes with a letter if the result still
/// doesn't start with one, then truncates to the database-name length cap so
/// the prefix can never push a name over it. Pure - unit-tested. The wizard
/// mirrors this client-side to pre-fill the Database step; the daemon is the
/// authority (`database::validate_db_name` re-checks whatever it's given).
#[must_use]
pub fn derive_db_name(site_name: &str) -> String {
    let mut name: String = site_name
        .chars()
        .map(|c| if c == '-' { '_' } else { c })
        .collect();
    let starts_ok = name
        .chars()
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_');
    if !starts_ok {
        name = format!("wp_{name}");
    }
    if name.len() > MAX_DB_NAME_LEN {
        name.truncate(MAX_DB_NAME_LEN);
    }
    name
}

/// Kept in sync with `yerd_services::database`'s private constant of the same
/// value (the lowest common engine cap - Postgres truncates at 63).
const MAX_DB_NAME_LEN: usize = 63;

/// `wp core download` argument vector.
fn download_args(o: &WordPressOptions) -> Vec<String> {
    let mut a = vec!["core".to_owned(), "download".to_owned()];
    if let Some(v) = &o.core_version {
        a.push(format!("--version={v}"));
    }
    a.push(format!("--locale={}", o.locale));
    a
}

/// `wp config create` argument vector - `--dbpass=` is deliberately empty (the
/// passwordless local-dev `root@127.0.0.1` account the services subsystem
/// grants; see `render_my_bootstrap_sql`).
fn config_create_args(o: &WordPressOptions, db_name: &str, db_port: u16) -> Vec<String> {
    vec![
        "config".to_owned(),
        "create".to_owned(),
        format!("--dbname={db_name}"),
        "--dbuser=root".to_owned(),
        "--dbpass=".to_owned(),
        format!("--dbhost=127.0.0.1:{db_port}"),
        format!("--dbprefix={}", o.table_prefix),
        "--skip-check".to_owned(),
    ]
}

/// The shortest admin password [`validate_admin_credentials`] accepts,
/// mirroring the GUI wizard's own client-side minimum
/// (`CreateWordPressWizard.vue`). The daemon is the authority here, the same
/// way [`database::validate_db_name`] is for database names, so a scripted
/// `Request::CreateSite` caller can't bypass the wizard's own floor.
const MIN_ADMIN_PASSWORD_LEN: usize = 8;

/// Why proposed `WordPress` admin credentials were rejected.
#[derive(Debug, Clone, PartialEq, Eq)]
enum AdminCredentialsError {
    /// `admin_user` was empty or whitespace-only.
    EmptyUser,
    /// `admin_password` was shorter than [`MIN_ADMIN_PASSWORD_LEN`].
    PasswordTooShort,
    /// `admin_email` didn't look like `local@domain.tld`.
    InvalidEmail,
}

impl std::fmt::Display for AdminCredentialsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AdminCredentialsError::EmptyUser => f.write_str("admin username must not be empty"),
            AdminCredentialsError::PasswordTooShort => write!(
                f,
                "admin password must be at least {MIN_ADMIN_PASSWORD_LEN} characters"
            ),
            AdminCredentialsError::InvalidEmail => {
                f.write_str("admin email is not a valid address")
            }
        }
    }
}

/// Validate the admin account WP-CLI will create, server-side - a scripted
/// or malformed `Request::CreateSite` shouldn't be able to hand `wp core
/// install` an empty username, a trivially short password, or a malformed
/// email and have it silently create a barely-secured site.
fn validate_admin_credentials(o: &WordPressOptions) -> Result<(), AdminCredentialsError> {
    if o.admin_user.trim().is_empty() {
        return Err(AdminCredentialsError::EmptyUser);
    }
    if o.admin_password.chars().count() < MIN_ADMIN_PASSWORD_LEN {
        return Err(AdminCredentialsError::PasswordTooShort);
    }
    if !looks_like_email(&o.admin_email) {
        return Err(AdminCredentialsError::InvalidEmail);
    }
    Ok(())
}

/// A deliberately loose `local@domain.tld` shape check, not full RFC 5322 -
/// good enough to catch empty/malformed input from a scripted caller without
/// rejecting real addresses WP-CLI itself would accept.
fn looks_like_email(s: &str) -> bool {
    let Some((local, domain)) = s.split_once('@') else {
        return false;
    };
    !local.is_empty() && domain.contains('.') && !domain.starts_with('.') && !domain.ends_with('.')
}

/// `wp core install` argument vector. `--url` sets `siteurl`/`home` directly
/// during install, so no follow-up `wp option update` is needed.
///
/// The admin password is deliberately **not** an argument: `--prompt=admin_password`
/// makes WP-CLI read it from stdin instead, so it never lands in the process's
/// argv (world-readable via `ps` / `/proc/<pid>/cmdline`). The caller pipes the
/// password in through [`run_wp_step`]'s `stdin_data`.
fn install_args(name: &str, tld: &str, secure: bool, o: &WordPressOptions) -> Vec<String> {
    let scheme = if secure { "https" } else { "http" };
    let mut a = vec!["core".to_owned(), "install".to_owned()];
    a.push(format!("--url={scheme}://{name}.{tld}"));
    a.push(format!("--title={}", o.site_title));
    a.push(format!("--admin_user={}", o.admin_user));
    a.push(format!("--admin_email={}", o.admin_email));
    a.push("--prompt=admin_password".to_owned());
    a.push("--skip-email".to_owned());
    a
}

/// `wp rewrite structure` argument vector - enables pretty (postname-based)
/// permalinks, since `wp core install` otherwise leaves a fresh site on
/// WordPress's "Plain" default (`?p=123`), under which pretty URLs like
/// `/wp-admin/` still work (they're real files) but anything else silently
/// falls back to the front page instead of routing or 404ing.
fn permalink_structure_args() -> Vec<String> {
    vec![
        "rewrite".to_owned(),
        "structure".to_owned(),
        "/%postname%/".to_owned(),
    ]
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::panic, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use yerd_ipc::WordPressDatabase;

    /// Writes `tool`'s `.version` marker, the sole signal
    /// [`tools::installed_version`] reads to call a tool managed-installed.
    fn mark_installed(state: &DaemonState, tool: Tool) {
        let dir = state.dirs.data.join("tools").join(tool.id());
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(".version"), "v1.2.3\n").unwrap();
    }

    #[tokio::test]
    async fn ensure_wp_cli_is_satisfied_by_a_managed_wp_cli_alone() {
        let tmp = tempfile::tempdir().unwrap();
        let state = Arc::new(crate::test_support::state_in(tmp.path()));
        mark_installed(&state, Tool::WpCli);
        assert!(ensure_wp_cli("job", &state).await.is_ok());
    }

    /// Yerd's tools are symlinked into `{data}/bin`, which the shell profile
    /// prepends to `PATH` - so installing Composer here would silently take
    /// over the user's `composer` in unrelated projects. Creating a site must
    /// report it instead, never install it.
    #[tokio::test]
    async fn ensure_wp_cli_reports_a_missing_composer_rather_than_installing_it() {
        let tmp = tempfile::tempdir().unwrap();
        let state = Arc::new(crate::test_support::state_in(tmp.path()));
        let err = ensure_wp_cli("job", &state).await.unwrap_err();
        assert!(err.contains("Composer"), "{err}");
        assert!(err.contains("yerd install tool composer"), "{err}");
        assert!(
            tools::installed_version(&state.dirs, Tool::Composer).is_none(),
            "a create-site job must never install Composer behind the user's back"
        );
    }

    fn opts() -> WordPressOptions {
        WordPressOptions {
            core_version: None,
            locale: "en_GB".to_owned(),
            admin_user: "admin".to_owned(),
            admin_email: "admin@blog.test".to_owned(),
            admin_password: "hunter2hunter2".to_owned(),
            site_title: "My Blog".to_owned(),
            table_prefix: "wp_".to_owned(),
            database: WordPressDatabase {
                engine: WordPressDbEngine::Mysql,
                name: "blog".to_owned(),
            },
        }
    }

    #[test]
    fn engine_type_id_maps_both_variants() {
        assert_eq!(engine_type_id(WordPressDbEngine::Mysql), "mysql");
        assert_eq!(engine_type_id(WordPressDbEngine::Mariadb), "mariadb");
    }

    #[test]
    fn resolve_db_name_uses_explicit_name_when_given() {
        let mut o = opts();
        o.database.name = "custom_db".to_owned();
        assert_eq!(resolve_db_name("blog", &o), "custom_db");
    }

    #[test]
    fn resolve_db_name_derives_from_site_name_when_empty() {
        let mut o = opts();
        o.database.name = String::new();
        assert_eq!(resolve_db_name("my-blog", &o), derive_db_name("my-blog"));
    }

    #[test]
    fn validate_admin_credentials_accepts_reasonable_input() {
        assert!(validate_admin_credentials(&opts()).is_ok());
    }

    #[test]
    fn validate_admin_credentials_rejects_empty_or_whitespace_user() {
        let mut o = opts();
        o.admin_user = "   ".to_owned();
        assert_eq!(
            validate_admin_credentials(&o),
            Err(AdminCredentialsError::EmptyUser)
        );
    }

    #[test]
    fn validate_admin_credentials_rejects_short_password() {
        let mut o = opts();
        o.admin_password = "short1".to_owned();
        assert_eq!(
            validate_admin_credentials(&o),
            Err(AdminCredentialsError::PasswordTooShort)
        );
    }

    #[test]
    fn validate_admin_credentials_accepts_password_at_the_minimum_length() {
        let mut o = opts();
        o.admin_password = "a".repeat(MIN_ADMIN_PASSWORD_LEN);
        assert!(validate_admin_credentials(&o).is_ok());
    }

    #[test]
    fn validate_admin_credentials_rejects_malformed_email() {
        for bad in [
            "not-an-email",
            "admin@",
            "@blog.test",
            "admin@.test",
            "admin@blog.",
        ] {
            let mut o = opts();
            o.admin_email = bad.to_owned();
            assert_eq!(
                validate_admin_credentials(&o),
                Err(AdminCredentialsError::InvalidEmail),
                "{bad:?} should be rejected"
            );
        }
    }

    #[test]
    fn derive_db_name_maps_hyphens_to_underscores() {
        assert_eq!(derive_db_name("my-blog"), "my_blog");
    }

    #[test]
    fn derive_db_name_prefixes_digit_leading_names() {
        assert_eq!(derive_db_name("3d-shop"), "wp_3d_shop");
        assert!(database::validate_db_name(&derive_db_name("3d-shop")).is_ok());
    }

    #[test]
    fn derive_db_name_leaves_letter_leading_names_unprefixed() {
        assert_eq!(derive_db_name("blog"), "blog");
    }

    #[test]
    fn derive_db_name_truncates_after_prefix_overflow() {
        let site_name = "9".repeat(63);
        let derived = derive_db_name(&site_name);
        assert_eq!(derived.len(), 63);
        assert!(database::validate_db_name(&derived).is_ok());
    }

    #[test]
    fn derive_db_name_all_hyphen_and_digit_name_is_still_valid() {
        let derived = derive_db_name("42");
        assert!(database::validate_db_name(&derived).is_ok());
    }

    #[test]
    fn download_args_omits_version_when_latest() {
        let a = download_args(&opts());
        assert_eq!(a, vec!["core", "download", "--locale=en_GB"]);
    }

    #[test]
    fn download_args_includes_version_when_pinned() {
        let mut o = opts();
        o.core_version = Some("6.4.2".to_owned());
        let a = download_args(&o);
        assert_eq!(
            a,
            vec!["core", "download", "--version=6.4.2", "--locale=en_GB"]
        );
    }

    #[test]
    fn config_create_args_has_empty_dbpass_for_passwordless_root() {
        let a = config_create_args(&opts(), "blog", 3306);
        assert!(a.contains(&"--dbpass=".to_owned()));
        assert!(a.contains(&"--dbhost=127.0.0.1:3306".to_owned()));
        assert!(a.contains(&"--dbname=blog".to_owned()));
        assert!(a.contains(&"--dbprefix=wp_".to_owned()));
    }

    #[test]
    fn install_args_single_site_uses_core_install() {
        let a = install_args("blog", "test", true, &opts());
        assert_eq!(a[0], "core");
        assert_eq!(a[1], "install");
        assert!(a.contains(&"--url=https://blog.test".to_owned()));
        assert!(!a.iter().any(|s| s == "--subdomains"));
    }

    #[test]
    fn install_args_uses_configured_tld() {
        let a = install_args("blog", "dev.local", true, &opts());
        assert!(a.contains(&"--url=https://blog.dev.local".to_owned()));
    }

    #[test]
    fn install_args_never_puts_the_password_in_argv() {
        let a = install_args("blog", "test", true, &opts());
        assert!(a.contains(&"--prompt=admin_password".to_owned()));
        assert!(
            !a.iter().any(|s| s.contains("hunter2hunter2")),
            "the admin password must never appear in argv: {a:?}"
        );
        assert!(!a.iter().any(|s| s.starts_with("--admin_password")));
    }

    #[test]
    fn install_args_insecure_uses_http_scheme() {
        let a = install_args("blog", "test", false, &opts());
        assert!(a.contains(&"--url=http://blog.test".to_owned()));
    }

    #[test]
    fn permalink_structure_args_sets_postname_structure() {
        assert_eq!(
            permalink_structure_args(),
            vec!["rewrite", "structure", "/%postname%/"]
        );
    }

    #[test]
    fn wp_step_invocation_splits_boot_fs_and_appends_path() {
        let boot_fs = Path::new("/Users/x/Library/Application Support/io.yerd.Yerd/boot-fs.php");
        let project_dir = Path::new("/Users/x/Yerd/blog");
        let (boot_dir, boot_name, args) =
            wp_step_invocation(boot_fs, project_dir, &["core".to_owned()]).unwrap();
        assert_eq!(
            boot_dir,
            Path::new("/Users/x/Library/Application Support/io.yerd.Yerd")
        );
        assert_eq!(boot_name, Path::new("boot-fs.php"));
        assert_eq!(args, vec!["core", "--path=/Users/x/Yerd/blog"]);
    }

    #[test]
    fn wp_step_invocation_none_for_rootless_boot_fs() {
        assert!(wp_step_invocation(Path::new("/"), Path::new("/tmp"), &[]).is_none());
    }
}
