//! The service *types* Yerd can manage, expressed as classes behind the
//! [`ServiceDefinition`] trait, plus the [`ServiceRegistry`] that owns them.
//!
//! Pure: no I/O. A `ServiceDefinition` is the compile-time identity and
//! behaviour of one kind of service (its id, default port, server binary, how
//! its datadir is initialised, how its server command is built, how it is
//! stopped, how readiness is probed). The supervisor and daemon read these facts
//! and drive them; the manager keys running instances by their *wire id* string,
//! so a single-instance engine (`"redis"`) and a per-site instance
//! (`"reverb:blog"`) share one code path.
//!
//! The "Redis" slot is served by **Valkey** (the BSD-licensed fork) - Redis 7.4+
//! is SSPL/RSALv2 and not cleanly redistributable. It stays wire-compatible so
//! clients are unaffected.

use std::ffi::OsString;
use std::process::Command as StdCommand;
use std::sync::Arc;

use yerd_supervise::supervisor::{StopProtocol, SupervisorPolicy};

use crate::config_render;
use crate::error::ServiceError;

/// Whether a service is a cache, a SQL database, or a long-running app server -
/// gates the "Create Database" action and the version/site UI affordances.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServiceKind {
    /// In-memory cache / key-value store (no SQL databases).
    Cache,
    /// SQL database server (supports `CREATE DATABASE`).
    Database,
    /// A supervised application server (e.g. Laravel Reverb) - no databases, no
    /// downloadable version; runs against a linked site's PHP.
    AppServer,
}

/// How many instances of a service type may exist at once.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Multiplicity {
    /// At most one instance; its wire id equals the type id (`"redis"`).
    Single,
    /// One instance per linked site; wire id is `"{type}:{site}"`.
    PerSite,
}

/// The readiness-probe protocol the manager runs to end a service's `Starting`
/// window. Selected by [`ServiceDefinition::readiness`]; the concrete probes live
/// in [`crate::health`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReadinessKind {
    /// Redis/Valkey inline `PING` expecting `+PONG`.
    RedisPing,
    /// `MySQL`/`MariaDB` initial handshake packet.
    MySqlHandshake,
    /// `PostgreSQL` startup-message reply.
    PostgresStartup,
    /// A bare TCP connect succeeds (the listener is open). Used by app servers
    /// (Reverb) whose readiness is "the socket accepts connections".
    TcpConnect,
}

/// The SQL dialect of a database-capable service, returned by
/// [`ServiceDefinition::as_database`]. A small closed set: the create/drop/list
/// SQL, identifier quoting, and client/dump/restore argv differ only along these
/// three engines. Caches and app servers return `None`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SqlEngine {
    /// Oracle `MySQL`.
    MySql,
    /// `MariaDB` (shares `MySQL`'s supervision path; differs in client/dump argv).
    MariaDb,
    /// `PostgreSQL`.
    Postgres,
}

impl SqlEngine {
    /// The interactive client executable in the install's `bin/` dir used to run
    /// administrative SQL (and to replay a dump on restore).
    #[must_use]
    pub const fn client_binary(self) -> &'static str {
        match self {
            SqlEngine::MySql => "mysql",
            SqlEngine::MariaDb => "mariadb",
            SqlEngine::Postgres => "psql",
        }
    }

    /// The `bin/` tool that dumps a database to plain SQL on stdout.
    #[must_use]
    pub const fn dump_binary(self) -> &'static str {
        match self {
            SqlEngine::MySql => "mysqldump",
            SqlEngine::MariaDb => "mariadb-dump",
            SqlEngine::Postgres => "pg_dump",
        }
    }
}

/// Everything the manager needs to build a service's server command, resolved by
/// the daemon and passed to [`ServiceDefinition::plan_launch`]. Borrowed so the
/// plan stays cheap and I/O-free to construct.
pub struct LaunchContext<'a> {
    /// The chosen loopback port.
    pub port: u16,
    /// The program to exec: the engine's server binary, or (per-site) the linked
    /// site's PHP CLI binary.
    pub program: &'a std::path::Path,
    /// The rendered config-file path (database/cache engines).
    pub config_path: &'a std::path::Path,
    /// The engine datadir (database/cache engines).
    pub datadir: &'a std::path::Path,
    /// The per-instance log-file path.
    pub log_path: &'a std::path::Path,
    /// Extra environment to layer on (e.g. `PostGIS` `PROJ_DATA`/`GDAL_DATA`).
    pub geo_env: &'a [(OsString, OsString)],
    /// Working directory to launch in (per-site services: the site document root).
    pub cwd: Option<&'a std::path::Path>,
}

/// The concrete launch recipe a [`ServiceDefinition`] produces. The manager
/// applies process-group isolation and - when [`capture_output_to_log`] is set -
/// opens the log file and attaches the child's stdout/stderr, keeping the
/// fallible I/O out of the pure `plan_launch`.
///
/// [`capture_output_to_log`]: LaunchPlan::capture_output_to_log
pub struct LaunchPlan {
    /// The server command (program + args + env + cwd), without stdio or process
    /// group applied yet.
    pub command: StdCommand,
    /// Whether the manager should redirect the child's stdout/stderr into the
    /// log file. True for engines that log to their stdio (Postgres, Reverb);
    /// false for engines that write their own logfile via rendered config.
    pub capture_output_to_log: bool,
}

/// One kind of manageable service (a cache, SQL engine, or app server) as a class.
///
/// Implementations are zero-sized and registered in [`ServiceRegistry`]. All
/// methods are pure; side effects (spawning, datadir init, config writes) are
/// performed by the manager using the facts and the [`LaunchPlan`] returned here.
pub trait ServiceDefinition: Send + Sync + 'static {
    /// The stable, lowercase type id (config keys, IPC, on-disk dirs).
    fn id(&self) -> &'static str;

    /// Human-facing label for the GUI/CLI.
    fn display_name(&self) -> &'static str;

    /// Cache / database / app-server classification.
    fn kind(&self) -> ServiceKind;

    /// Single-instance vs one-per-site.
    fn multiplicity(&self) -> Multiplicity;

    /// Whether a fresh instance defaults to starting with Yerd. DB/cache engines
    /// default `true` (installing them is intent to run); app servers `false`.
    fn default_autostart(&self) -> bool;

    /// The default loopback port when the user does not choose one.
    fn default_port(&self) -> u16;

    /// Whether this type installs a downloadable version (DB/cache: yes;
    /// app servers run against a site's PHP: no).
    fn requires_version(&self) -> bool;

    /// Whether an instance must be linked to a site (per-site app servers).
    fn requires_site(&self) -> bool {
        matches!(self.multiplicity(), Multiplicity::PerSite)
    }

    /// The server executable's file name inside the install's `bin/` dir, or
    /// `None` for a type with no installed server binary (app servers).
    fn server_binary(&self) -> Option<&'static str>;

    /// Whether on-disk datadirs are incompatible across *major* versions (so the
    /// datadir path is pinned per major). True only for Postgres.
    fn datadir_pinned_to_major(&self) -> bool {
        false
    }

    /// The `bin/` tool performing one-time datadir init, or `None` for a type
    /// that needs none (Redis, app servers).
    fn init_binary(&self) -> Option<&'static str> {
        None
    }

    /// Whether this type requires one-time datadir initialisation before first
    /// start. The boolean view of [`init_binary`](Self::init_binary).
    fn needs_init(&self) -> bool {
        self.init_binary().is_some()
    }

    /// The supervisor policy (readiness window, backoff, stop grace) for this
    /// type.
    fn supervisor_policy(&self) -> SupervisorPolicy;

    /// The readiness protocol the manager probes to confirm the server is up.
    fn readiness(&self) -> ReadinessKind;

    /// How this service is gracefully stopped. Defaults to a group SIGTERM;
    /// Postgres overrides to `MasterInterrupt` (SIGINT "fast shutdown").
    fn stop_protocol(&self) -> StopProtocol {
        StopProtocol::GroupTerm
    }

    /// A reverse-proxy path prefix to auto-manage on the instance's linked site
    /// (e.g. Reverb's `/app` WebSocket endpoint), or `None` for a type that needs
    /// no proxy. When `Some`, the daemon adds/moves/removes this path rule in
    /// lockstep with the instance's add / re-link / removal, so browser traffic
    /// reaches the service over the site's domain (and TLS) instead of the raw
    /// loopback port.
    fn proxy_path(&self) -> Option<&'static str> {
        None
    }

    /// The SQL dialect if this type hosts databases, else `None`. Gates
    /// `supports_databases` and the "Manage databases" action.
    fn as_database(&self) -> Option<SqlEngine> {
        None
    }

    /// Whether the manager should probe the install tree for `PostGIS` geo-data
    /// env (`PROJ_DATA`/`GDAL_DATA`). True only for Postgres.
    fn injects_geo_data(&self) -> bool {
        false
    }

    /// One-time init-tool arguments populating the fresh `staging` datadir.
    /// Empty for a type with no init step.
    fn init_args(&self, staging: &std::path::Path) -> Vec<OsString> {
        let _ = staging;
        Vec::new()
    }

    /// Whether `datadir` already holds an initialised instance of this type.
    /// Types with no datadir report `true` (nothing to initialise).
    fn is_initialized(&self, datadir: &std::path::Path) -> bool {
        let _ = datadir;
        true
    }

    /// The bootstrap-SQL run on every start (MySQL/MariaDB passwordless-root
    /// setup), or `None` for a type that needs none.
    fn bootstrap_sql(&self) -> Option<&'static str> {
        None
    }

    /// Render this type's server config text, or `None` for a type with no config
    /// file (app servers).
    fn render_config(
        &self,
        port: u16,
        datadir: &std::path::Path,
        socket: &std::path::Path,
        log_path: &std::path::Path,
        init_file: &std::path::Path,
    ) -> Option<String> {
        let _ = (port, datadir, socket, log_path, init_file);
        None
    }

    /// Whether this type opens a Unix socket beside its TCP port (MySQL/MariaDB).
    fn uses_unix_socket(&self) -> bool {
        false
    }

    /// Build the server command for this type from the resolved context. Pure:
    /// no stdio or process group is applied here (the manager does that).
    fn plan_launch(&self, ctx: &LaunchContext<'_>) -> Result<LaunchPlan, ServiceError>;
}

/// The set of built-in service types, owned as trait objects.
#[derive(Clone)]
pub struct ServiceRegistry {
    types: Vec<Arc<dyn ServiceDefinition>>,
}

impl ServiceRegistry {
    /// The built-in registry of the four database/cache engines.
    #[must_use]
    pub fn builtin() -> Self {
        Self {
            types: vec![
                Arc::new(Redis),
                Arc::new(MySql),
                Arc::new(MariaDb),
                Arc::new(Postgres),
                Arc::new(Reverb),
            ],
        }
    }

    /// Look up a type by its id (`"redis"`, `"mysql"`, ...). `None` if unknown.
    #[must_use]
    pub fn get(&self, id: &str) -> Option<Arc<dyn ServiceDefinition>> {
        self.types.iter().find(|t| t.id() == id).map(Arc::clone)
    }

    /// Every registered type, in registration order.
    pub fn iter(&self) -> impl Iterator<Item = &Arc<dyn ServiceDefinition>> {
        self.types.iter()
    }

    /// The single-instance types, in registration order (the rows that always
    /// appear in `ListServices`).
    pub fn single_instance(&self) -> impl Iterator<Item = &Arc<dyn ServiceDefinition>> {
        self.types
            .iter()
            .filter(|t| matches!(t.multiplicity(), Multiplicity::Single))
    }
}

impl Default for ServiceRegistry {
    fn default() -> Self {
        Self::builtin()
    }
}

/// Redis (Valkey): in-memory cache, no init, no databases, no config-less stdio
/// (self-logs via `logfile` in its rendered config).
pub struct Redis;
/// Oracle `MySQL`.
pub struct MySql;
/// `MariaDB` - shares `MySQL`'s supervision path.
pub struct MariaDb;
/// `PostgreSQL`.
pub struct Postgres;

impl ServiceDefinition for Redis {
    fn id(&self) -> &'static str {
        "redis"
    }
    fn display_name(&self) -> &'static str {
        "Redis (Valkey)"
    }
    fn kind(&self) -> ServiceKind {
        ServiceKind::Cache
    }
    fn multiplicity(&self) -> Multiplicity {
        Multiplicity::Single
    }
    fn default_autostart(&self) -> bool {
        true
    }
    fn default_port(&self) -> u16 {
        6379
    }
    fn requires_version(&self) -> bool {
        true
    }
    fn server_binary(&self) -> Option<&'static str> {
        Some("valkey-server")
    }
    fn supervisor_policy(&self) -> SupervisorPolicy {
        SupervisorPolicy::database()
    }
    fn readiness(&self) -> ReadinessKind {
        ReadinessKind::RedisPing
    }
    fn render_config(
        &self,
        port: u16,
        datadir: &std::path::Path,
        _socket: &std::path::Path,
        log_path: &std::path::Path,
        _init_file: &std::path::Path,
    ) -> Option<String> {
        Some(config_render::render_redis_conf(port, datadir, log_path))
    }
    fn plan_launch(&self, ctx: &LaunchContext<'_>) -> Result<LaunchPlan, ServiceError> {
        let mut command = base_command(ctx);
        command.arg(ctx.config_path);
        Ok(LaunchPlan {
            command,
            capture_output_to_log: false,
        })
    }
}

impl ServiceDefinition for MySql {
    fn id(&self) -> &'static str {
        "mysql"
    }
    fn display_name(&self) -> &'static str {
        "MySQL"
    }
    fn kind(&self) -> ServiceKind {
        ServiceKind::Database
    }
    fn multiplicity(&self) -> Multiplicity {
        Multiplicity::Single
    }
    fn default_autostart(&self) -> bool {
        true
    }
    fn default_port(&self) -> u16 {
        3306
    }
    fn requires_version(&self) -> bool {
        true
    }
    fn server_binary(&self) -> Option<&'static str> {
        Some("mysqld")
    }
    fn init_binary(&self) -> Option<&'static str> {
        Some("mysqld")
    }
    fn supervisor_policy(&self) -> SupervisorPolicy {
        SupervisorPolicy::database()
    }
    fn readiness(&self) -> ReadinessKind {
        ReadinessKind::MySqlHandshake
    }
    fn as_database(&self) -> Option<SqlEngine> {
        Some(SqlEngine::MySql)
    }
    fn init_args(&self, staging: &std::path::Path) -> Vec<OsString> {
        vec![
            OsString::from("--initialize-insecure"),
            OsString::from(format!("--datadir={}", staging.display())),
        ]
    }
    fn is_initialized(&self, datadir: &std::path::Path) -> bool {
        datadir.join("mysql").is_dir()
    }
    fn bootstrap_sql(&self) -> Option<&'static str> {
        Some(config_render::render_my_bootstrap_sql())
    }
    fn uses_unix_socket(&self) -> bool {
        true
    }
    fn render_config(
        &self,
        port: u16,
        datadir: &std::path::Path,
        socket: &std::path::Path,
        log_path: &std::path::Path,
        init_file: &std::path::Path,
    ) -> Option<String> {
        Some(config_render::render_my_cnf(
            port, datadir, socket, log_path, init_file,
        ))
    }
    fn plan_launch(&self, ctx: &LaunchContext<'_>) -> Result<LaunchPlan, ServiceError> {
        Ok(my_family_plan(ctx))
    }
}

impl ServiceDefinition for MariaDb {
    fn id(&self) -> &'static str {
        "mariadb"
    }
    fn display_name(&self) -> &'static str {
        "MariaDB"
    }
    fn kind(&self) -> ServiceKind {
        ServiceKind::Database
    }
    fn multiplicity(&self) -> Multiplicity {
        Multiplicity::Single
    }
    fn default_autostart(&self) -> bool {
        true
    }
    fn default_port(&self) -> u16 {
        3306
    }
    fn requires_version(&self) -> bool {
        true
    }
    fn server_binary(&self) -> Option<&'static str> {
        Some("mariadbd")
    }
    fn init_binary(&self) -> Option<&'static str> {
        Some("mariadb-install-db")
    }
    fn supervisor_policy(&self) -> SupervisorPolicy {
        SupervisorPolicy::database()
    }
    fn readiness(&self) -> ReadinessKind {
        ReadinessKind::MySqlHandshake
    }
    fn as_database(&self) -> Option<SqlEngine> {
        Some(SqlEngine::MariaDb)
    }
    fn init_args(&self, staging: &std::path::Path) -> Vec<OsString> {
        vec![
            OsString::from("--basedir=."),
            OsString::from(format!("--datadir={}", staging.display())),
            OsString::from("--auth-root-authentication-method=normal"),
        ]
    }
    fn is_initialized(&self, datadir: &std::path::Path) -> bool {
        datadir.join("mysql").is_dir()
    }
    fn bootstrap_sql(&self) -> Option<&'static str> {
        Some(config_render::render_my_bootstrap_sql())
    }
    fn uses_unix_socket(&self) -> bool {
        true
    }
    fn render_config(
        &self,
        port: u16,
        datadir: &std::path::Path,
        socket: &std::path::Path,
        log_path: &std::path::Path,
        init_file: &std::path::Path,
    ) -> Option<String> {
        Some(config_render::render_my_cnf(
            port, datadir, socket, log_path, init_file,
        ))
    }
    fn plan_launch(&self, ctx: &LaunchContext<'_>) -> Result<LaunchPlan, ServiceError> {
        Ok(my_family_plan(ctx))
    }
}

impl ServiceDefinition for Postgres {
    fn id(&self) -> &'static str {
        "postgres"
    }
    fn display_name(&self) -> &'static str {
        "PostgreSQL"
    }
    fn kind(&self) -> ServiceKind {
        ServiceKind::Database
    }
    fn multiplicity(&self) -> Multiplicity {
        Multiplicity::Single
    }
    fn default_autostart(&self) -> bool {
        true
    }
    fn default_port(&self) -> u16 {
        5432
    }
    fn requires_version(&self) -> bool {
        true
    }
    fn server_binary(&self) -> Option<&'static str> {
        Some("postgres")
    }
    fn datadir_pinned_to_major(&self) -> bool {
        true
    }
    fn init_binary(&self) -> Option<&'static str> {
        Some("initdb")
    }
    fn supervisor_policy(&self) -> SupervisorPolicy {
        SupervisorPolicy::database()
    }
    fn readiness(&self) -> ReadinessKind {
        ReadinessKind::PostgresStartup
    }
    fn stop_protocol(&self) -> StopProtocol {
        StopProtocol::MasterInterrupt
    }
    fn as_database(&self) -> Option<SqlEngine> {
        Some(SqlEngine::Postgres)
    }
    fn injects_geo_data(&self) -> bool {
        true
    }
    fn init_args(&self, staging: &std::path::Path) -> Vec<OsString> {
        vec![
            OsString::from("-D"),
            staging.as_os_str().to_os_string(),
            OsString::from("--auth=trust"),
            OsString::from("-U"),
            OsString::from("postgres"),
            OsString::from("-E"),
            OsString::from("UTF8"),
        ]
    }
    fn is_initialized(&self, datadir: &std::path::Path) -> bool {
        datadir.join("PG_VERSION").is_file()
    }
    fn render_config(
        &self,
        port: u16,
        datadir: &std::path::Path,
        _socket: &std::path::Path,
        _log_path: &std::path::Path,
        _init_file: &std::path::Path,
    ) -> Option<String> {
        Some(config_render::render_postgresql_conf(port, datadir))
    }
    fn plan_launch(&self, ctx: &LaunchContext<'_>) -> Result<LaunchPlan, ServiceError> {
        let mut command = base_command(ctx);
        command
            .arg("-D")
            .arg(ctx.datadir)
            .arg("-c")
            .arg(format!("config_file={}", ctx.config_path.display()));
        Ok(LaunchPlan {
            command,
            capture_output_to_log: true,
        })
    }
}

/// Laravel Reverb: a per-site WebSocket app server, supervised as
/// `php{ver} artisan reverb:start` on a loopback port. No installed version, no
/// datadir, no config file - it runs against the linked site's PHP and code.
pub struct Reverb;

impl ServiceDefinition for Reverb {
    fn id(&self) -> &'static str {
        "reverb"
    }
    fn display_name(&self) -> &'static str {
        "Reverb"
    }
    fn kind(&self) -> ServiceKind {
        ServiceKind::AppServer
    }
    fn multiplicity(&self) -> Multiplicity {
        Multiplicity::PerSite
    }
    fn default_autostart(&self) -> bool {
        false
    }
    fn default_port(&self) -> u16 {
        8080
    }
    fn requires_version(&self) -> bool {
        false
    }
    fn server_binary(&self) -> Option<&'static str> {
        None
    }
    fn supervisor_policy(&self) -> SupervisorPolicy {
        SupervisorPolicy::reverb()
    }
    fn readiness(&self) -> ReadinessKind {
        ReadinessKind::TcpConnect
    }
    fn proxy_path(&self) -> Option<&'static str> {
        Some("/app")
    }
    fn plan_launch(&self, ctx: &LaunchContext<'_>) -> Result<LaunchPlan, ServiceError> {
        let mut command = StdCommand::new(ctx.program);
        command
            .arg("artisan")
            .arg("reverb:start")
            .arg("--host=127.0.0.1")
            .arg(format!("--port={}", ctx.port));
        if let Some(cwd) = ctx.cwd {
            command.current_dir(cwd);
        }
        Ok(LaunchPlan {
            command,
            capture_output_to_log: true,
        })
    }
}

/// Start a server command from the program + layered geo env. Shared by the
/// database/cache engines (app servers build their own from scratch).
fn base_command(ctx: &LaunchContext<'_>) -> StdCommand {
    let mut cmd = StdCommand::new(ctx.program);
    for (key, value) in ctx.geo_env {
        cmd.env(key, value);
    }
    cmd
}

/// The shared MySQL/MariaDB launch plan: `--defaults-file=<config>`, self-logging
/// via the rendered `my.cnf`, group SIGTERM to stop.
fn my_family_plan(ctx: &LaunchContext<'_>) -> LaunchPlan {
    let mut command = base_command(ctx);
    command.arg(format!("--defaults-file={}", ctx.config_path.display()));
    LaunchPlan {
        command,
        capture_output_to_log: false,
    }
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

    fn reg() -> ServiceRegistry {
        ServiceRegistry::builtin()
    }

    #[test]
    fn registry_lookup_round_trips_every_type() {
        let r = reg();
        for id in ["redis", "mysql", "mariadb", "postgres"] {
            assert_eq!(r.get(id).map(|d| d.id()), Some(id));
        }
        assert!(r.get("nope").is_none());
    }

    #[test]
    fn registry_has_four_single_and_one_per_site() {
        let r = reg();
        assert_eq!(r.iter().count(), 5);
        assert_eq!(r.single_instance().count(), 4);
        for id in ["redis", "mysql", "mariadb", "postgres"] {
            assert!(matches!(
                r.get(id).unwrap().multiplicity(),
                Multiplicity::Single
            ));
        }
    }

    #[test]
    fn reverb_is_a_per_site_versionless_app_server() {
        let d = reg().get("reverb").unwrap();
        assert!(matches!(d.multiplicity(), Multiplicity::PerSite));
        assert!(d.requires_site());
        assert!(!d.requires_version());
        assert!(!d.default_autostart());
        assert_eq!(d.kind(), ServiceKind::AppServer);
        assert_eq!(d.default_port(), 8080);
        assert!(d.server_binary().is_none());
        assert!(d.as_database().is_none());
        assert!(!d.needs_init());
        assert_eq!(d.readiness(), ReadinessKind::TcpConnect);
    }

    #[test]
    fn reverb_plan_launch_runs_artisan_reverb_start_in_cwd() {
        let php = std::path::Path::new("/php/bin/php");
        let docroot = std::path::Path::new("/sites/blog");
        let ctx = LaunchContext {
            port: 8081,
            program: php,
            config_path: std::path::Path::new(""),
            datadir: std::path::Path::new(""),
            log_path: std::path::Path::new("/l/reverb.log"),
            geo_env: &[],
            cwd: Some(docroot),
        };
        let plan = Reverb.plan_launch(&ctx).unwrap();
        assert_eq!(plan.command.get_program(), php.as_os_str());
        let args: Vec<String> = plan
            .command
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            args,
            vec!["artisan", "reverb:start", "--host=127.0.0.1", "--port=8081"]
        );
        assert_eq!(plan.command.get_current_dir(), Some(docroot));
        assert!(plan.capture_output_to_log);
    }

    #[test]
    fn default_ports_are_unprivileged() {
        for d in reg().iter() {
            assert!(d.default_port() > 1024, "{} port privileged", d.id());
        }
    }

    #[test]
    fn redis_is_cache_and_needs_no_init() {
        let d = reg().get("redis").unwrap();
        assert_eq!(d.kind(), ServiceKind::Cache);
        assert!(!d.needs_init());
        assert_eq!(d.server_binary(), Some("valkey-server"));
        assert!(d.as_database().is_none());
    }

    #[test]
    fn sql_engines_are_databases_and_need_init() {
        for id in ["mysql", "mariadb", "postgres"] {
            let d = reg().get(id).unwrap();
            assert_eq!(d.kind(), ServiceKind::Database);
            assert!(d.needs_init(), "{id} should need init");
            assert!(d.as_database().is_some());
        }
    }

    #[test]
    fn init_binary_matches_needs_init() {
        assert_eq!(reg().get("redis").unwrap().init_binary(), None);
        assert_eq!(reg().get("mysql").unwrap().init_binary(), Some("mysqld"));
        assert_eq!(
            reg().get("mariadb").unwrap().init_binary(),
            Some("mariadb-install-db")
        );
        assert_eq!(reg().get("postgres").unwrap().init_binary(), Some("initdb"));
        for d in reg().iter() {
            assert_eq!(d.needs_init(), d.init_binary().is_some(), "{}", d.id());
        }
    }

    #[test]
    fn only_postgres_pins_datadir_to_major() {
        assert!(reg().get("postgres").unwrap().datadir_pinned_to_major());
        for id in ["redis", "mysql", "mariadb"] {
            assert!(!reg().get(id).unwrap().datadir_pinned_to_major());
        }
    }

    #[test]
    fn only_postgres_injects_geo_data() {
        assert!(reg().get("postgres").unwrap().injects_geo_data());
        for id in ["redis", "mysql", "mariadb"] {
            assert!(!reg().get(id).unwrap().injects_geo_data());
        }
    }

    #[test]
    fn plan_launch_redis_passes_only_the_config_path() {
        let program = std::path::Path::new("/b/valkey-server");
        let config = std::path::Path::new("/c/redis.conf");
        let datadir = std::path::Path::new("/d");
        let log = std::path::Path::new("/l/redis.log");
        let ctx = LaunchContext {
            port: 6379,
            program,
            config_path: config,
            datadir,
            log_path: log,
            geo_env: &[],
            cwd: None,
        };
        let plan = Redis.plan_launch(&ctx).unwrap();
        assert_eq!(plan.command.get_program(), program.as_os_str());
        let args: Vec<_> = plan.command.get_args().collect();
        assert_eq!(args, vec![config.as_os_str()]);
        assert!(!plan.capture_output_to_log);
        assert_eq!(Redis.stop_protocol(), StopProtocol::GroupTerm);
    }

    #[test]
    fn plan_launch_mysql_passes_defaults_file_first() {
        let ctx = LaunchContext {
            port: 3306,
            program: std::path::Path::new("/b/mysqld"),
            config_path: std::path::Path::new("/c/my.cnf"),
            datadir: std::path::Path::new("/d"),
            log_path: std::path::Path::new("/l/mysql.log"),
            geo_env: &[],
            cwd: None,
        };
        let plan = MySql.plan_launch(&ctx).unwrap();
        let args: Vec<_> = plan
            .command
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert_eq!(args.len(), 1);
        assert!(args[0].starts_with("--defaults-file="), "got: {args:?}");
        assert!(args[0].contains("my.cnf"));
    }

    #[test]
    fn plan_launch_postgres_sets_datadir_and_captures_output() {
        let ctx = LaunchContext {
            port: 5432,
            program: std::path::Path::new("/b/postgres"),
            config_path: std::path::Path::new("/c/postgresql.conf"),
            datadir: std::path::Path::new("/d/data-16"),
            log_path: std::path::Path::new("/l/pg.log"),
            geo_env: &[],
            cwd: None,
        };
        let plan = Postgres.plan_launch(&ctx).unwrap();
        let args: Vec<_> = plan
            .command
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert_eq!(args[0], "-D");
        assert_eq!(args[1], "/d/data-16");
        assert_eq!(args[2], "-c");
        assert!(args[3].starts_with("config_file="));
        assert!(plan.capture_output_to_log);
        assert_eq!(Postgres.stop_protocol(), StopProtocol::MasterInterrupt);
    }

    #[test]
    fn plan_launch_layers_geo_env() {
        let env = vec![
            (OsString::from("PROJ_DATA"), OsString::from("/i/share/proj")),
            (OsString::from("GDAL_DATA"), OsString::from("/i/share/gdal")),
        ];
        let ctx = LaunchContext {
            port: 5432,
            program: std::path::Path::new("/b/postgres"),
            config_path: std::path::Path::new("/c/postgresql.conf"),
            datadir: std::path::Path::new("/d"),
            log_path: std::path::Path::new("/l/pg.log"),
            geo_env: &env,
            cwd: None,
        };
        let plan = Postgres.plan_launch(&ctx).unwrap();
        let got: std::collections::BTreeMap<_, _> = plan
            .command
            .get_envs()
            .filter_map(|(k, v)| v.map(|v| (k.to_owned(), v.to_owned())))
            .collect();
        assert_eq!(
            got.get(std::ffi::OsStr::new("PROJ_DATA"))
                .map(std::ffi::OsString::as_os_str),
            Some(std::ffi::OsStr::new("/i/share/proj"))
        );
    }

    #[test]
    fn init_args_and_is_initialized_match_engine_layout() {
        let staging = std::path::Path::new("/x/staging");
        let mysql = reg().get("mysql").unwrap();
        let m_args: Vec<String> = mysql
            .init_args(staging)
            .iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            m_args,
            vec!["--initialize-insecure", "--datadir=/x/staging"]
        );

        let maria = reg().get("mariadb").unwrap();
        let ma_args: Vec<String> = maria
            .init_args(staging)
            .iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert_eq!(ma_args[0], "--basedir=.");

        let pg = reg().get("postgres").unwrap();
        let pg_args: Vec<String> = pg
            .init_args(staging)
            .iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert_eq!(pg_args[0], "-D");
        assert_eq!(pg_args[1], "/x/staging");

        assert!(reg().get("redis").unwrap().init_args(staging).is_empty());
    }

    #[test]
    fn render_config_embeds_port_for_config_backed_engines() {
        let datadir = std::path::Path::new("/d");
        let socket = std::path::Path::new("/s/x.sock");
        let log = std::path::Path::new("/l/x.log");
        let init = std::path::Path::new("/i/x-init.sql");
        for id in ["redis", "mysql", "mariadb", "postgres"] {
            let d = reg().get(id).unwrap();
            let rendered = d.render_config(6543, datadir, socket, log, init).unwrap();
            assert!(rendered.contains("6543"), "{id} config missing port");
        }
    }
}
