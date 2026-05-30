//! CLI surface (clap-derived).

use std::path::PathBuf;

/// Top-level parser. `yerd` is a thin `yerd-ipc` client of the `yerdd` daemon.
#[derive(clap::Parser, Debug)]
#[command(name = "yerd", version, about = "Yerd CLI — talks to the yerdd daemon")]
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
    /// Link a single directory as a named site.
    Link {
        /// Site name (a single DNS label).
        name: String,
        /// Directory to serve.
        path: PathBuf,
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
    /// default — the terminal `php` shim and the site fallback. Two arguments
    /// (`yerd use <site> 8.5`) set a single site's version.
    Use {
        /// A PHP version (global) or a site name (when `version` is given).
        first: String,
        /// PHP version for the named site; omit to set the global default.
        version: Option<String>,
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
    /// Uninstall a component (currently: a PHP version).
    Uninstall {
        /// What to uninstall.
        #[command(subcommand)]
        target: UninstallTarget,
    },
    /// List installed components (currently: PHP versions).
    List {
        /// What to list.
        #[command(subcommand)]
        target: ListTarget,
    },
    /// Upgrade an installed component to the latest release.
    Update {
        /// What to update.
        #[command(subcommand)]
        target: UpdateTarget,
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
}

/// Action of `yerd doctor`.
#[derive(clap::Subcommand, Debug, Clone, Copy, PartialEq, Eq)]
pub enum DoctorAction {
    /// Attempt safe, unprivileged repairs (e.g. restart a crashed FPM pool).
    Fix,
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
}

/// Target of `yerd install`.
#[derive(clap::Subcommand, Debug, Clone)]
pub enum InstallTarget {
    /// Install a PHP version (downloads a prebuilt static build).
    Php {
        /// PHP version, e.g. `8.5`.
        version: String,
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
