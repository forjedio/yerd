//! Site-creation request payloads and long-running-job progress types.
//!
//! Scaffolding a new project (e.g. `laravel new`) takes far longer than a
//! single request/response round-trip, so [`crate::Request::CreateSite`] starts
//! a background **job** on the daemon and returns immediately with a
//! [`crate::Response::JobStarted`]. The client then polls
//! [`crate::Request::JobStatus`] for the streamed log + phase until the job
//! reaches a terminal [`JobState`].
//!
//! Same rule as the rest of this crate: no per-field serde renames; add
//! variants/fields additively and let `rename_all` handle casing.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use yerd_core::PhpVersion;

/// Opaque identifier for a long-running daemon job. Allocated by the daemon and
/// echoed back by the client on every [`crate::Request::JobStatus`] poll.
pub type JobId = String;

/// Everything needed to scaffold and register one new site.
///
/// Framework-agnostic fields live here; per-framework knobs live in
/// [`Framework`], so a new site type is an additive enum variant rather than a
/// reshuffle of this struct.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateSiteSpec {
    /// The site name - a single DNS label. Becomes the directory name under
    /// `parent_dir` and the `<name>.test` domain.
    pub name: String,
    /// The directory the new project directory is created *inside*. May be an
    /// existing parked root (the site then auto-serves) or any other folder
    /// (the site is then linked).
    pub parent_dir: PathBuf,
    /// The PHP version to serve the new site with.
    pub php: PhpVersion,
    /// Whether to serve the new site over HTTPS.
    pub secure: bool,
    /// Which framework to scaffold, plus its options.
    pub framework: Framework,
}

/// The framework to scaffold and its per-framework options.
///
/// The enum exists so site types are additive (`CakePHP`, …). Internally
/// tagged on `framework`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "framework", rename_all = "snake_case")]
#[non_exhaustive]
pub enum Framework {
    /// Scaffold via the official Laravel installer (`laravel new`).
    Laravel {
        /// Laravel-specific installer options.
        options: LaravelOptions,
    },
    /// Scaffold via WP-CLI (`wp core download` / `wp config create` /
    /// `wp core install` or `wp core multisite-install`), provisioning a
    /// MySQL/MariaDB database along the way. Spelled `Wordpress` (one
    /// capital) rather than `WordPress` so `rename_all = "snake_case"`
    /// produces the wire tag `"wordpress"` rather than `"word_press"` -
    /// same reason [`Database::Mariadb`] avoids `MariaDb`.
    Wordpress {
        /// WordPress-specific installer options.
        options: WordPressOptions,
    },
}

/// Options mapped onto `laravel new` flags.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[allow(clippy::struct_excessive_bools)]
pub struct LaravelOptions {
    /// The starter kit (`--react`/`--vue`/`--livewire`/`--svelte`/`--using`, or none).
    pub starter_kit: StarterKit,
    /// Authentication provider for kits that scaffold auth (`--workos` vs built-in).
    pub auth: AuthProvider,
    /// `--livewire-class-components` (Livewire kit only).
    pub livewire_class_components: bool,
    /// `--teams` (team support, where the kit supports it).
    pub teams: bool,
    /// Testing framework (`--pest`/`--phpunit`).
    pub testing: Testing,
    /// Database driver written into `.env` (`--database`).
    pub database: Database,
    /// Frontend dependency install/build (`--npm`/`--bun`/none-to-skip).
    pub js: JsRuntime,
    /// `--git` - initialise a git repository.
    pub git: bool,
    /// `--boost` - install Laravel Boost (AI assist).
    pub boost: bool,
}

/// The starter kit to install.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StarterKit {
    /// No starter kit - the plain skeleton.
    None,
    /// React + Inertia + TypeScript (`--react`).
    React,
    /// Vue + Inertia + TypeScript (`--vue`).
    Vue,
    /// Livewire (`--livewire`).
    Livewire,
    /// Svelte + Inertia + TypeScript (`--svelte`).
    Svelte,
    /// A community kit installed via `--using <package>`.
    Community(String),
}

/// Authentication provider for starter kits that scaffold auth.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthProvider {
    /// Laravel's built-in authentication (the default).
    Laravel,
    /// `WorkOS` `AuthKit` (`--workos`).
    WorkOs,
}

/// Testing framework selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Testing {
    /// Pest (`--pest`).
    Pest,
    /// `PHPUnit` (`--phpunit`).
    PhpUnit,
}

/// Database driver written into the new app's `.env`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Database {
    /// `SQLite` (zero-config; the installer creates the file and migrates).
    Sqlite,
    /// `MySQL`.
    Mysql,
    /// `MariaDB`.
    Mariadb,
    /// `PostgreSQL`.
    Pgsql,
    /// SQL Server.
    Sqlsrv,
}

/// How (or whether) to install + build frontend dependencies.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JsRuntime {
    /// Install + build via npm (`--npm`).
    Npm,
    /// Install + build via Bun (`--bun`).
    Bun,
    /// Skip frontend dependency install/build (no package-manager flag passed,
    /// so the installer does not run install/build).
    Skip,
}

/// Options mapped onto the WP-CLI scaffolding sequence.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WordPressOptions {
    /// `WordPress` core version to install (`wp core download --version`).
    /// `None` installs the latest stable release.
    pub core_version: Option<String>,
    /// Locale to install (`wp core download --locale`), e.g. `"en_GB"`.
    pub locale: String,
    /// Admin account username (`wp core install --admin_user`).
    pub admin_user: String,
    /// Admin account email (`wp core install --admin_email`).
    pub admin_email: String,
    /// Admin account password (`wp core install --admin_password`).
    pub admin_password: String,
    /// Site title (`wp core install --title`).
    pub site_title: String,
    /// Database table prefix (`wp config create --dbprefix`).
    pub table_prefix: String,
    /// Multisite network mode, if any.
    pub multisite: Multisite,
    /// The database engine + name to provision for this site.
    pub database: WordPressDatabase,
}

/// `WordPress` Multisite network mode (`wp core install` vs.
/// `wp core multisite-install [--subdomains]`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Multisite {
    /// A single-site install - no network.
    Off,
    /// A multisite network addressed by subdirectory (`/site2/`).
    Subdirectory,
    /// A multisite network addressed by subdomain (`site2.<name>.test`).
    Subdomain,
}

/// The database engine and name to provision for a new `WordPress` site.
///
/// Only the two engines `WordPress` core itself supports without a plugin are
/// offered - no `Postgres`, no `Sqlite`, unlike [`Database`] (Laravel's much
/// broader driver list, which only ever writes a `.env` value and never
/// provisions anything).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WordPressDatabase {
    /// Which engine to provision.
    pub engine: WordPressDbEngine,
    /// The database name to create. Validated by the daemon against the
    /// same allowlist [`crate::Request::CreateDatabase`] uses.
    pub name: String,
}

/// SQL engine offered for a `WordPress` site's database. Variant names are
/// deliberately spelled so `rename_all = "snake_case"` reproduces the same
/// wire ids the Services subsystem's `Service::id()` already uses
/// (`"mysql"`/`"mariadb"`), matching [`Database::Mariadb`]'s naming for the
/// same reason.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WordPressDbEngine {
    /// `MySQL`.
    Mysql,
    /// `MariaDB`.
    Mariadb,
}

/// Lifecycle state of a long-running job.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobState {
    /// Still working.
    Running,
    /// Finished successfully (the site is created + registered).
    Succeeded,
    /// Finished with an error (see [`crate::Response::JobProgress::error`]).
    Failed,
    /// Cancelled by the client.
    Cancelled,
}
