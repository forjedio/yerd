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
    /// The site name — a single DNS label. Becomes the directory name under
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
/// Only Laravel is supported today; the enum exists so other site types
/// (`CakePHP`, …) are additive. Internally tagged on `framework`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "framework", rename_all = "snake_case")]
#[non_exhaustive]
pub enum Framework {
    /// Scaffold via the official Laravel installer (`laravel new`).
    Laravel {
        /// Laravel-specific installer options.
        options: LaravelOptions,
    },
}

/// Options mapped onto `laravel new` flags.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[allow(clippy::struct_excessive_bools)] // independent installer flags, not a state machine
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
    /// Frontend dependency install/build (`--npm`/`--bun`/`--no-node`).
    pub js: JsRuntime,
    /// `--git` — initialise a git repository.
    pub git: bool,
    /// `--boost` — install Laravel Boost (AI assist).
    pub boost: bool,
}

/// The starter kit to install.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StarterKit {
    /// No starter kit — the plain skeleton.
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
    /// Skip frontend dependency install/build (`--no-node`).
    Skip,
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
