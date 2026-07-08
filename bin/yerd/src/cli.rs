//! CLI surface (clap-derived).

use std::path::PathBuf;

/// Top-level parser. `yerd` is a thin `yerd-ipc` client of the `yerdd` daemon.
#[derive(clap::Parser, Debug)]
#[command(name = "yerd", version, about = "Yerd CLI - talks to the yerdd daemon")]
pub struct Cli {
    /// Emit machine-readable JSON instead of human-readable text.
    #[arg(long, global = true)]
    pub json: bool,
    /// Subcommand to run.
    #[command(subcommand)]
    pub command: Command,
}

/// CLI subcommands. Each maps to exactly one [`yerd_ipc::Request`].
#[derive(clap::Subcommand, Debug, Clone)]
pub enum Command {
    /// Check that the daemon is alive.
    Ping,
    /// List every parked or linked site.
    Sites,
    /// Park a directory: each of its child directories becomes a `.test` site.
    Park {
        /// Directory to park.
        path: PathBuf,
    },
    /// Link a single directory as a named site. With one argument, infers
    /// whichever of name/path is missing; with none, links the current
    /// directory under its folder name.
    Link {
        /// A site name (bare word), or a directory to link (its folder name
        /// becomes the site name). Omit entirely to link the current
        /// directory.
        name_or_path: Option<String>,
        /// Directory to serve, when the first argument is a name. Omit to
        /// use the current directory.
        path: Option<PathBuf>,
    },
    /// Remove a linked site by name.
    Unlink {
        /// Site name to remove.
        name: String,
    },
    /// Un-park a directory: removes it from the parked set so its child
    /// directories stop being served. Linked sites are untouched.
    Unpark {
        /// Directory to un-park (run `yerd list parked` to see the exact paths).
        path: PathBuf,
    },
    /// Set the PHP version. One argument (`yerd use 8.5`) sets the **global**
    /// default - the terminal `php` shim and the site fallback. Two arguments
    /// (`yerd use <site> 8.5`) set a single site's version.
    Use {
        /// A PHP version (global) or a site name (when `version` is given).
        first: String,
        /// PHP version for the named site; omit to set the global default.
        version: Option<String>,
    },
    /// Manage per-version PHP settings (currently: custom extensions).
    Php {
        /// What to manage.
        #[command(subcommand)]
        action: PhpAction,
    },
    /// Set a global PHP ini default (e.g. `yerd set php memory_limit 512M`).
    Set {
        /// What to set.
        #[command(subcommand)]
        target: SetTarget,
    },
    /// Reset a global PHP ini default to PHP's built-in value.
    Unset {
        /// What to reset.
        #[command(subcommand)]
        target: UnsetTarget,
    },
    /// Install a component (currently: a PHP version).
    Install {
        /// What to install.
        #[command(subcommand)]
        target: InstallTarget,
    },
    /// Restart a component's process (currently: a PHP FPM pool).
    Restart {
        /// What to restart.
        #[command(subcommand)]
        target: RestartTarget,
    },
    /// Uninstall a component, or yerd itself.
    ///
    /// With a subcommand (`php`/`tool`) removes that component via the daemon.
    /// With no subcommand, fully uninstalls yerd from this machine: config,
    /// data, downloads, the PATH entry, the daemon service, and - when run with
    /// `sudo` - the system-level trust/resolver/port changes from `elevate`.
    /// Prompts for confirmation first unless `--yes` is given.
    Uninstall {
        /// What to uninstall; omit to uninstall yerd entirely.
        #[command(subcommand)]
        target: Option<UninstallTarget>,
        /// Skip the confirmation prompt (only affects the full uninstall).
        #[arg(short = 'y', long)]
        yes: bool,
    },
    /// List installed components (currently: PHP versions).
    List {
        /// What to list.
        #[command(subcommand)]
        target: ListTarget,
    },
    /// Check for a Yerd self-update, or update an installed component.
    ///
    /// `yerd update` (no subcommand) reports whether a newer Yerd is available
    /// on your channel; `yerd update php [version]` upgrades PHP patches.
    Update {
        /// What to update. Omit to check for a Yerd self-update.
        #[command(subcommand)]
        target: Option<UpdateTarget>,
        /// Apply the self-update: download, verify, and install the new version,
        /// then restart. Without it, only check and report.
        #[arg(long)]
        yes: bool,
        /// Use the edge (pre-release / RC) channel for this run. With `--yes`,
        /// also makes edge the saved default.
        #[arg(long, conflicts_with = "stable")]
        edge: bool,
        /// Use the stable channel for this run. With `--yes`, also resets the
        /// saved default to stable.
        #[arg(long)]
        stable: bool,
        /// Allow a downgrade (e.g. moving from a newer pre-release to stable).
        #[arg(long, requires = "yes")]
        force: bool,
    },
    /// List local database / cache services and their status.
    Services,
    /// List installable dev tools (Composer, Node, Bun) and their install status.
    Tools,
    /// Manage a local database or cache service (redis, mysql, mariadb, postgres).
    Service {
        /// What to do.
        #[command(subcommand)]
        action: ServiceAction,
    },
    /// Manage databases inside a running SQL service (mysql, mariadb, postgres).
    Db {
        /// What to do.
        #[command(subcommand)]
        action: DbAction,
    },
    /// Publish a local site to the internet via a Cloudflare Tunnel.
    Tunnel {
        /// What to do.
        #[command(subcommand)]
        action: TunnelAction,
    },
    /// Inspect emails captured by the built-in mail server.
    Mail {
        /// What to do.
        #[command(subcommand)]
        action: MailAction,
    },
    /// Show a snapshot of daemon, proxy, DNS, ports, CA, and PHP health.
    Status,
    /// Diagnose common problems; `yerd doctor fix` attempts safe repairs.
    Doctor {
        /// Optional action; omit to only report, `fix` to attempt repairs.
        #[command(subcommand)]
        action: Option<DoctorAction>,
    },
    /// Serve a site over HTTPS (promotes a parked site to a linked entry).
    Secure {
        /// Site name.
        name: String,
    },
    /// Stop serving a site over HTTPS.
    Unsecure {
        /// Site name.
        name: String,
    },
    /// Set the directory a site is served from (its web root), e.g.
    /// `yerd root myapp public` for a Laravel app. With `--auto` (or no path),
    /// reset the site to automatic framework detection.
    Root {
        /// Site name.
        name: String,
        /// Served directory, relative to the site's folder (or an absolute path
        /// inside it). Omit with `--auto` to reset to auto-detection.
        path: Option<String>,
        /// Reset the site to automatic web-root detection.
        #[arg(long)]
        auto: bool,
    },
    /// Route all requests through a site's front controller (`index.php`), or
    /// execute named `.php` files directly. Frameworks served from a subdir
    /// (Laravel, ...) default `on`; plain and `WordPress` sites default `off`.
    FrontController {
        /// Site name.
        name: String,
        /// `on` funnels every request through `index.php`; `off` executes the
        /// named `.php` directly.
        state: OnOff,
    },
    /// Manage a site's routable domains (add/remove domains, subdomains and
    /// wildcards, and change the primary domain).
    Domain {
        /// What to do.
        #[command(subcommand)]
        action: DomainAction,
    },
    /// Grant yerd OS-level privileges (run via `sudo`). No subcommand = all.
    Elevate {
        /// Which privilege to grant; omit to grant all.
        #[command(subcommand)]
        target: Option<ElevateTarget>,
    },
    /// Revert what `elevate` configured (run via `sudo`). No subcommand = all.
    Unelevate {
        /// Which privilege to revert; omit to revert all.
        #[command(subcommand)]
        target: Option<ElevateTarget>,
    },
    /// Add or remove yerd's shim directory (`php`, `composer`, …) from your
    /// shell's PATH. Local - does not talk to the daemon.
    Path {
        /// What to do.
        #[command(subcommand)]
        action: PathAction,
    },
}

/// A binary on/off toggle argument (e.g. `yerd front-controller <name> on`).
#[derive(clap::ValueEnum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum OnOff {
    /// Turn the setting on.
    On,
    /// Turn the setting off.
    Off,
}

impl OnOff {
    /// `true` for [`OnOff::On`].
    #[must_use]
    pub fn is_on(self) -> bool {
        matches!(self, OnOff::On)
    }
}

/// Action of `yerd path`.
#[derive(clap::Subcommand, Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathAction {
    /// Add yerd's bin dir to your shell startup file (idempotent).
    Install,
    /// Remove the yerd PATH block from your shell startup file.
    Uninstall,
    /// Print the shell snippet without modifying any file (for manual `eval`).
    Print,
}

/// Action of `yerd domain`.
#[derive(clap::Subcommand, Debug, Clone)]
pub enum DomainAction {
    /// List a site's domains (primary marked), or all sites' domains with no
    /// argument.
    List {
        /// Site name; omit to list every site's domains.
        site: Option<String>,
    },
    /// Add a domain to a site: an exact host (`api.myapp.test`) or a single-label
    /// wildcard (`*.myapp.test`).
    Add {
        /// Site name.
        site: String,
        /// Full domain FQDN under the configured TLD.
        domain: String,
    },
    /// Remove a domain from a site. A site must keep at least one exact domain.
    Remove {
        /// Site name.
        site: String,
        /// Full domain FQDN to remove.
        domain: String,
    },
    /// Set a site's primary (canonical) domain. Must be an exact domain; it is
    /// added to the site if not already present.
    Primary {
        /// Site name.
        site: String,
        /// Full domain FQDN to make primary.
        domain: String,
    },
    /// Reset a site's domains to the default (its `{name}.{tld}` apex only).
    Reset {
        /// Site name.
        site: String,
    },
}

/// Action of `yerd service`.
#[derive(clap::Subcommand, Debug, Clone)]
pub enum ServiceAction {
    /// List installable versions per service (queries the distribution).
    Available,
    /// Install a service version (downloads a prebuilt build).
    Install {
        /// Service id: `redis`, `mysql`, `mariadb`, or `postgres`.
        service: String,
        /// Version to install, e.g. `8` (see `yerd service available`).
        version: String,
    },
    /// Switch a service to a different version (upgrade or downgrade). Installs
    /// the new version, restarts onto it, and removes the old one.
    ChangeVersion {
        /// Service id.
        service: String,
        /// Version to switch to, e.g. `9.1.0` (see `yerd service available`).
        version: String,
    },
    /// Uninstall a service version. Keeps the datadir unless `--purge`.
    Uninstall {
        /// Service id.
        service: String,
        /// Version to remove.
        version: String,
        /// Also delete the engine's stored data (destructive).
        #[arg(long)]
        purge: bool,
    },
    /// Start (and enable auto-start for) a service.
    Start {
        /// Service id.
        service: String,
    },
    /// Stop (and disable auto-start for) a service.
    Stop {
        /// Service id.
        service: String,
    },
    /// Restart a service.
    Restart {
        /// Service id.
        service: String,
    },
    /// Set the port a service listens on (applies on next start/restart).
    SetPort {
        /// Service id.
        service: String,
        /// Loopback port.
        port: u16,
    },
    /// Show the last lines of a service's log.
    Logs {
        /// Service id.
        service: String,
        /// Number of trailing lines to show.
        #[arg(long, default_value_t = 100)]
        lines: u32,
    },
}

/// Action of `yerd tunnel`.
#[derive(clap::Subcommand, Debug, Clone)]
pub enum TunnelAction {
    /// Download the `cloudflared` binary (required before sharing a site).
    Install,
    /// Share a site publicly via a Quick Tunnel (a random `*.trycloudflare.com`
    /// URL). Requires `cloudflared` to be installed.
    Share {
        /// Site name (e.g. `app` or `app.test`).
        site: String,
    },
    /// Stop sharing a site.
    Stop {
        /// Site name whose tunnel to stop.
        site: String,
    },
    /// Show live tunnels and `cloudflared` install status.
    Status,
    /// Log in to a Cloudflare account (opens a browser) for Named Tunnels.
    Login,
    /// Create a named tunnel on the logged-in account.
    Create {
        /// The tunnel name to create.
        name: String,
    },
    /// Delete a named tunnel from the account and forget it locally.
    Delete {
        /// The tunnel name to delete.
        name: String,
    },
    /// List the named tunnels recorded locally.
    List,
    /// Route a public hostname to a named tunnel (creates the DNS record).
    Route {
        /// Tunnel name (or UUID) to route to.
        tunnel: String,
        /// Public hostname to create (on your Cloudflare domain).
        hostname: String,
    },
    /// Set (or clear, with `--clear`) a site's persisted public hostname.
    SetHost {
        /// Site name.
        site: String,
        /// The public hostname to assign. Required unless `--clear` is given, so
        /// forgetting it can't silently clear the mapping.
        #[arg(required_unless_present = "clear")]
        hostname: Option<String>,
        /// Clear the site's hostname instead of setting one.
        #[arg(long, conflicts_with = "hostname")]
        clear: bool,
    },
    /// (Re)start the named tunnel, exposing every site that has a hostname set.
    /// Requires login and a created tunnel.
    Publish,
    /// Stop the named tunnel (takes every named site offline).
    Unpublish,
}

/// Action of `yerd db`.
#[derive(clap::Subcommand, Debug, Clone)]
pub enum DbAction {
    /// List the databases in a running SQL service.
    List {
        /// Service id: `mysql`, `mariadb`, or `postgres`.
        service: String,
    },
    /// Create a database.
    Create {
        /// Service id.
        service: String,
        /// Database name (letters, digits, underscores; must start with a
        /// letter or underscore).
        name: String,
    },
    /// Drop a database (irreversible).
    Drop {
        /// Service id.
        service: String,
        /// Database name to drop.
        name: String,
    },
    /// Back a database up to a plain-SQL file.
    Backup {
        /// Service id.
        service: String,
        /// Database name to dump.
        name: String,
        /// Destination file (relative paths resolve against your current directory).
        path: PathBuf,
    },
    /// Restore a database from a plain-SQL file (the database must already exist).
    Restore {
        /// Service id.
        service: String,
        /// Database name to restore into.
        name: String,
        /// Source file to replay (relative paths resolve against your current directory).
        path: PathBuf,
    },
}

/// Action of `yerd mail`.
#[derive(clap::Subcommand, Debug, Clone)]
pub enum MailAction {
    /// List captured emails (newest first).
    List,
    /// Show one captured email's headers and body by id.
    Show {
        /// The email id (from `yerd mail list`).
        id: String,
    },
    /// Delete every captured email.
    Clear,
}

/// Action of `yerd doctor`.
#[derive(clap::Subcommand, Debug, Clone, Copy, PartialEq, Eq)]
pub enum DoctorAction {
    /// Attempt safe, unprivileged repairs (e.g. restart a crashed FPM pool).
    Fix,
}

/// Action of `yerd php`.
#[derive(clap::Subcommand, Debug, Clone)]
pub enum PhpAction {
    /// Manage custom PHP extensions (`.so`) loaded in both web (FPM) and CLI.
    Ext {
        /// The extension action.
        #[command(subcommand)]
        action: PhpExtAction,
    },
}

/// Action of `yerd php ext`.
#[derive(clap::Subcommand, Debug, Clone)]
pub enum PhpExtAction {
    /// Register a custom extension for a PHP version. The `.so` is load-probed
    /// against that version before it is saved.
    Add {
        /// PHP version, e.g. `8.5`.
        version: String,
        /// Absolute path to the `.so`.
        path: PathBuf,
        /// Load as a Zend extension (e.g. xdebug / opcache style) rather than a
        /// plain extension.
        #[arg(long)]
        zend: bool,
        /// Name used to display and remove the extension; defaults to the `.so`
        /// basename.
        #[arg(long)]
        name: Option<String>,
    },
    /// Remove a registered extension by name for a version.
    Remove {
        /// PHP version, e.g. `8.5`.
        version: String,
        /// The extension's registered name.
        name: String,
    },
    /// List registered custom extensions, grouped by version.
    List,
}

/// Target of `yerd set`.
#[derive(clap::Subcommand, Debug, Clone)]
pub enum SetTarget {
    /// Set a global PHP ini default applied to every installed version.
    Php {
        /// Setting name, e.g. `memory_limit`.
        setting: String,
        /// Setting value, e.g. `512M`.
        value: String,
    },
}

/// Target of `yerd unset`.
#[derive(clap::Subcommand, Debug, Clone)]
pub enum UnsetTarget {
    /// Reset a global PHP ini default to PHP's built-in value.
    Php {
        /// Setting name, e.g. `memory_limit`.
        setting: String,
    },
}

/// Target of `yerd restart`.
#[derive(clap::Subcommand, Debug, Clone)]
pub enum RestartTarget {
    /// Restart a PHP FPM pool. Omit the version to restart every running pool.
    Php {
        /// PHP version, e.g. `8.5`; omit to restart all running pools.
        version: Option<String>,
    },
    /// Restart the daemon itself (briefly interrupts all sites + this command).
    Daemon,
}

/// Target of `yerd uninstall`.
#[derive(clap::Subcommand, Debug, Clone)]
pub enum UninstallTarget {
    /// Uninstall a PHP version (removes its files; blocked if in use).
    Php {
        /// PHP version, e.g. `8.5`.
        version: String,
    },
    /// Uninstall a dev tool (`composer`, `node`, `bun`, `laravel`, `wp-cli`).
    Tool {
        /// Tool id: `composer`, `node`, `bun`, `laravel`, or `wp-cli`.
        id: String,
    },
}

/// Target of `yerd install`.
#[derive(clap::Subcommand, Debug, Clone)]
pub enum InstallTarget {
    /// Install a PHP version (downloads a prebuilt static build).
    Php {
        /// PHP version, e.g. `8.5`.
        version: String,
    },
    /// Install a dev tool (`composer`, `node`, `bun`, `laravel`, `wp-cli`) at
    /// its latest release.
    Tool {
        /// Tool id: `composer`, `node`, `bun`, `laravel`, or `wp-cli`.
        id: String,
    },
}

/// Target of `yerd list`.
#[derive(clap::Subcommand, Debug, Clone, Copy, PartialEq, Eq)]
pub enum ListTarget {
    /// List installed PHP versions and the global default.
    Php {
        /// Poll the distribution now to refresh "update available" status
        /// (otherwise served from the daemon's cache, no network).
        #[arg(long)]
        check: bool,
        /// List the versions installable from the distribution instead, tagging
        /// ones already installed. Takes precedence over `--check`.
        #[arg(long)]
        available: bool,
    },
    /// List the registered parked directory roots (including empty ones, which
    /// produce no sites and so don't appear in `yerd sites`).
    Parked,
}

/// Target of `yerd update`.
#[derive(clap::Subcommand, Debug, Clone)]
pub enum UpdateTarget {
    /// Update a PHP version (omit the version to update all installed).
    Php {
        /// PHP version, e.g. `8.5`; omit to update every installed version.
        version: Option<String>,
    },
}

/// A single privilege managed by `yerd elevate` / `yerd unelevate`.
#[derive(clap::Subcommand, Debug, Clone, Copy, PartialEq, Eq)]
pub enum ElevateTarget {
    /// Trust the local CA in the OS system store.
    Trust,
    /// Route `*.<tld>` queries to yerd's DNS responder.
    Resolver,
    /// Allow the daemon to bind privileged ports 80/443 (setcap).
    Ports,
}
