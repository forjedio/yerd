//! WordPress scaffolding via WP-CLI - the WordPress-specific body of the
//! create-site job. Preflight ensures WP-CLI is installed; Provisioning
//! database ensures a MySQL/MariaDB engine is installed+running and creates
//! the site's database; Downloading/Configuring/Installing run `wp core
//! download` / `wp config create` / `wp core install` (or
//! `wp core multisite-install`) with piped, streamed stdio; Registering
//! reuses the shared [`super::registration`].

use std::path::Path;
use std::sync::Arc;

use tokio::sync::watch;
use yerd_services::{database, Service, ServiceVersion};

use yerd_ipc::{CreateSiteSpec, Multisite, WordPressDbEngine, WordPressOptions};

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

    let user_dirs = crate::tools::external::resolve_user_path()
        .await
        .unwrap_or_default();
    if let Err(msg) = super::ensure_tool(id, Tool::WpCli, &user_dirs, state).await {
        return Outcome::Failed(msg);
    }
    if let Err(msg) = super::check_target_dir(&project_dir) {
        return Outcome::Failed(msg);
    }
    if let Err(msg) = super::probe_writable(&spec.parent_dir) {
        return Outcome::Failed(msg);
    }
    // The wizard pre-fills and validates this client-side, but an empty name
    // (a lazy/scripted `Request::CreateSite` caller, not just the GUI) falls
    // back to the same derivation rather than failing the whole job on
    // `DbNameError::Empty` when a sensible default is derivable.
    let db_name = if options.database.name.is_empty() {
        derive_db_name(name)
    } else {
        options.database.name.clone()
    };
    if let Err(e) = database::validate_db_name(&db_name) {
        return Outcome::Failed(format!("invalid database name: {e}"));
    }
    if super::is_cancelled(&cancel_rx) {
        return Outcome::Cancelled;
    }

    // ---- Provisioning database ----
    state.jobs.set_phase(id, "Provisioning database").await;
    let service = engine_service(options.database.engine);
    if let Err(msg) = ensure_database_engine(id, service, state).await {
        return Outcome::Failed(msg);
    }
    if super::is_cancelled(&cancel_rx) {
        return Outcome::Cancelled;
    }
    let db_created = match crate::db_admin::create(service.id(), &db_name, state).await {
        yerd_ipc::Response::Ok => true,
        yerd_ipc::Response::Error { message, .. } => return Outcome::Failed(message),
        other => return Outcome::Failed(format!("unexpected response: {other:?}")),
    };
    if super::is_cancelled(&cancel_rx) {
        rollback(&project_dir, db_created, service, &db_name, state).await;
        return Outcome::Cancelled;
    }

    let db_port = service_port(service, state).await;

    // ---- Downloading WordPress ----
    state.jobs.set_phase(id, "Downloading WordPress").await;
    if let Err(e) = std::fs::create_dir_all(&project_dir) {
        rollback(&project_dir, db_created, service, &db_name, state).await;
        return Outcome::Failed(format!("{}: {e}", project_dir.display()));
    }
    let boot_fs = tools::wp_cli::boot_path(dirs);
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
        state,
        &mut cancel_rx,
    )
    .await
    {
        StreamedOutcome::Ok => {}
        StreamedOutcome::Failed(msg) => {
            rollback(&project_dir, db_created, service, &db_name, state).await;
            return Outcome::Failed(msg);
        }
        StreamedOutcome::Cancelled => {
            rollback(&project_dir, db_created, service, &db_name, state).await;
            return Outcome::Cancelled;
        }
    }

    // ---- Configuring ----
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
        state,
        &mut cancel_rx,
    )
    .await
    {
        StreamedOutcome::Ok => {}
        StreamedOutcome::Failed(msg) => {
            rollback(&project_dir, db_created, service, &db_name, state).await;
            return Outcome::Failed(msg);
        }
        StreamedOutcome::Cancelled => {
            rollback(&project_dir, db_created, service, &db_name, state).await;
            return Outcome::Cancelled;
        }
    }

    // ---- Installing ----
    state.jobs.set_phase(id, "Installing").await;
    let install_args = install_args(name, spec.secure, options);
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
        state,
        &mut cancel_rx,
    )
    .await
    {
        StreamedOutcome::Ok => {}
        StreamedOutcome::Failed(msg) => {
            rollback(&project_dir, db_created, service, &db_name, state).await;
            return Outcome::Failed(msg);
        }
        StreamedOutcome::Cancelled => {
            rollback(&project_dir, db_created, service, &db_name, state).await;
            return Outcome::Cancelled;
        }
    }

    // Best-effort: enable pretty permalinks so pages/posts (and yerd-proxy's
    // try_files-style routing, see `script_file::resolve_script`) resolve
    // human-readable URLs by default. A fresh `wp core install` otherwise
    // defaults to "Plain" permalinks (`?p=123`), under which WordPress
    // doesn't parse pretty paths as routes at all and silently falls back to
    // the front page. Not fatal - a working WordPress install with the
    // default permalink structure is still a successful create, so a
    // failure here is only logged, never rolled back (unlike a genuine
    // scaffolding failure above).
    let permalink_args = permalink_structure_args();
    state
        .jobs
        .push_log(id, "$ wp rewrite structure '/%postname%/'".to_owned())
        .await;
    match run_wp_step(
        id,
        &php_cli,
        &boot_fs,
        &permalink_args,
        &project_dir,
        state,
        &mut cancel_rx,
    )
    .await
    {
        StreamedOutcome::Ok => {}
        StreamedOutcome::Failed(msg) => {
            state
                .jobs
                .push_log(
                    id,
                    format!("warning: couldn't set pretty permalinks: {msg}"),
                )
                .await;
        }
        StreamedOutcome::Cancelled => {
            rollback(&project_dir, db_created, service, &db_name, state).await;
            return Outcome::Cancelled;
        }
    }

    // ---- Registering ----
    state.jobs.set_phase(id, "Registering").await;
    if let Err(msg) =
        super::registration::register(name, &spec.parent_dir, &project_dir, spec, state).await
    {
        return Outcome::Failed(format!("scaffolded, but registration failed: {msg}"));
    }
    let scheme = if spec.secure { "https" } else { "http" };
    state
        .jobs
        .push_log(id, format!("serving {scheme}://{name}.test"))
        .await;
    Outcome::Succeeded
}

/// Silences PHP-engine `E_DEPRECATED` notices from WP-CLI's own bundled
/// Composer dependencies (`react/promise`, `wp-cli/php-cli-tools`), which are
/// not kept current with newer PHP releases and otherwise flood the job log
/// with dozens of near-duplicate "Deprecated: ..." lines on every step - pure
/// noise from yerd's PHP, not a failure signal, so it's suppressed at the
/// engine level rather than filtered after the fact. Real errors/warnings
/// still surface normally; only this one severity class is dropped.
const QUIET_DEPRECATIONS: [&str; 2] = ["-d", "error_reporting=E_ALL & ~E_DEPRECATED"];

/// Run one `wp <subcommand>` invocation, streaming its output into the job
/// log. No custom `PATH`/`COMPOSER_HOME` needed - WP-CLI's own subcommands
/// don't shell out to Composer or rely on PATH-resolved tools, unlike the
/// Laravel installer's nested `composer create-project`.
async fn run_wp_step(
    id: &str,
    php_cli: &Path,
    boot_fs: &Path,
    args: &[String],
    project_dir: &Path,
    state: &Arc<DaemonState>,
    cancel_rx: &mut watch::Receiver<bool>,
) -> StreamedOutcome {
    let php_flags: Vec<String> = QUIET_DEPRECATIONS.iter().map(|s| (*s).to_owned()).collect();
    super::run_streamed(
        id,
        php_cli,
        &php_flags,
        boot_fs,
        args,
        project_dir,
        None,
        None,
        state,
        cancel_rx,
    )
    .await
}

/// Best-effort cleanup on any pre-Registering failure or cancellation: remove
/// the project directory, and drop the database **only if this job itself
/// created it** - a name collision with a pre-existing database never sets
/// `db_created`, so an unrelated database is never touched.
async fn rollback(
    project_dir: &Path,
    db_created: bool,
    service: Service,
    db_name: &str,
    state: &Arc<DaemonState>,
) {
    let _ = std::fs::remove_dir_all(project_dir);
    if db_created {
        let _ = crate::db_admin::drop(service.id(), db_name, state).await;
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
    service: Service,
    state: &Arc<DaemonState>,
) -> Result<(), String> {
    let (configured_version, port) = {
        let cfg = state.config.lock().await;
        let inst = cfg.services.instances.get(service.id());
        (
            inst.and_then(|i| i.version.clone()),
            inst.and_then(|i| i.port).unwrap_or(service.default_port()),
        )
    };

    let version =
        match crate::services::resolve_version(service, configured_version.as_deref(), &state.dirs)
        {
            Ok(v) => v,
            Err(_not_found) => {
                state
                    .jobs
                    .set_phase(id, format!("Installing {}", service.display_name()))
                    .await;
                resolve_and_install_latest(service, state).await?
            }
        };

    crate::services::ensure_and_persist(state, service, version, port)
        .await
        .map_err(response_message)
}

/// Fetch the services listing, pick the newest build available for this
/// platform, and install it.
async fn resolve_and_install_latest(
    service: Service,
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
    let version = yerd_services::available_versions(&listing, service, os, arch)
        .into_iter()
        .last()
        .ok_or_else(|| {
            format!(
                "no {} build is available for this platform",
                service.display_name()
            )
        })?;
    crate::service_install::install(service, &version, &state.dirs, &dl)
        .await
        .map_err(|e| e.to_string())?;
    Ok(version)
}

/// The configured (or default) port for `service` - used to build
/// `wp config create --dbhost`.
async fn service_port(service: Service, state: &Arc<DaemonState>) -> u16 {
    let cfg = state.config.lock().await;
    cfg.services
        .instances
        .get(service.id())
        .and_then(|i| i.port)
        .unwrap_or(service.default_port())
}

fn response_message(resp: yerd_ipc::Response) -> String {
    match resp {
        yerd_ipc::Response::Error { message, .. } => message,
        other => format!("unexpected response: {other:?}"),
    }
}

fn engine_service(engine: WordPressDbEngine) -> Service {
    match engine {
        WordPressDbEngine::Mysql => Service::MySql,
        WordPressDbEngine::Mariadb => Service::MariaDb,
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

/// `wp core install` or `wp core multisite-install` argument vector. `--url`
/// sets `siteurl`/`home` directly during install, so no follow-up
/// `wp option update` is needed. Multisite uses its own subcommand (not an
/// add-on flag to `core install`) and writes the network's domain-mapping
/// constants into `wp-config.php` itself; per-subsite URLs for subdomain-mode
/// networks are WordPress's own concern, not something set here.
fn install_args(name: &str, secure: bool, o: &WordPressOptions) -> Vec<String> {
    let scheme = if secure { "https" } else { "http" };
    let mut a = match o.multisite {
        Multisite::Off => vec!["core".to_owned(), "install".to_owned()],
        Multisite::Subdirectory => vec!["core".to_owned(), "multisite-install".to_owned()],
        Multisite::Subdomain => vec![
            "core".to_owned(),
            "multisite-install".to_owned(),
            "--subdomains".to_owned(),
        ],
    };
    a.push(format!("--url={scheme}://{name}.test"));
    a.push(format!("--title={}", o.site_title));
    a.push(format!("--admin_user={}", o.admin_user));
    a.push(format!("--admin_password={}", o.admin_password));
    a.push(format!("--admin_email={}", o.admin_email));
    a.push("--skip-email".to_owned());
    a
}

/// `wp rewrite structure` argument vector - enables pretty (postname-based)
/// permalinks, since `wp core install`/`multisite-install` otherwise leave a
/// fresh site on WordPress's "Plain" default (`?p=123`), under which pretty
/// URLs like `/wp-admin/` still work (they're real files) but anything else
/// silently falls back to the front page instead of routing or 404ing.
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

    fn opts() -> WordPressOptions {
        WordPressOptions {
            core_version: None,
            locale: "en_GB".to_owned(),
            admin_user: "admin".to_owned(),
            admin_email: "admin@blog.test".to_owned(),
            admin_password: "hunter2hunter2".to_owned(),
            site_title: "My Blog".to_owned(),
            table_prefix: "wp_".to_owned(),
            multisite: Multisite::Off,
            database: WordPressDatabase {
                engine: WordPressDbEngine::Mysql,
                name: "blog".to_owned(),
            },
        }
    }

    #[test]
    fn engine_service_maps_both_variants() {
        assert_eq!(engine_service(WordPressDbEngine::Mysql), Service::MySql);
        assert_eq!(engine_service(WordPressDbEngine::Mariadb), Service::MariaDb);
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
        // Site names may be up to 63 chars of [a-z0-9-] with no leading/trailing
        // hyphen; an all-digit name is the sparsest case that still needs the
        // "wp_" prefix.
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
        let a = install_args("blog", true, &opts());
        assert_eq!(a[0], "core");
        assert_eq!(a[1], "install");
        assert!(a.contains(&"--url=https://blog.test".to_owned()));
        assert!(!a.iter().any(|s| s == "--subdomains"));
    }

    #[test]
    fn install_args_insecure_uses_http_scheme() {
        let a = install_args("blog", false, &opts());
        assert!(a.contains(&"--url=http://blog.test".to_owned()));
    }

    #[test]
    fn install_args_subdirectory_multisite_uses_multisite_install_without_subdomains_flag() {
        let mut o = opts();
        o.multisite = Multisite::Subdirectory;
        let a = install_args("blog", true, &o);
        assert_eq!(a[1], "multisite-install");
        assert!(!a.iter().any(|s| s == "--subdomains"));
    }

    #[test]
    fn install_args_subdomain_multisite_adds_subdomains_flag() {
        let mut o = opts();
        o.multisite = Multisite::Subdomain;
        let a = install_args("blog", true, &o);
        assert_eq!(a[1], "multisite-install");
        assert!(a.iter().any(|s| s == "--subdomains"));
    }

    #[test]
    fn permalink_structure_args_sets_postname_structure() {
        assert_eq!(
            permalink_structure_args(),
            vec!["rewrite", "structure", "/%postname%/"]
        );
    }
}
