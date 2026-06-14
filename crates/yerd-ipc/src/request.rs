//! Client → daemon request envelope.
//!
//! Internally tagged on `type`, `snake_case`. Treat this enum as a
//! published contract — add variants and fields additively, never
//! rename, and let `tests/wire_stability.rs` pin the byte-exact wire
//! shape.

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use yerd_core::PhpVersion;

// IMPORTANT: per-field serde renames are forbidden in this crate. Add
// new variants/fields additively; let rename_all handle casing. See
// README and the verification script's grep gate.
/// A request sent from a client (CLI or GUI) to the daemon.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[non_exhaustive]
pub enum Request {
    /// Liveness check.
    Ping,
    /// Enumerate every parked or linked site.
    ListSites,
    /// Register a parked directory. `path` is opaque to `yerd-ipc`;
    /// the daemon canonicalises before storing. Windows paths arrive
    /// with backslashes — that is fine.
    Park {
        /// The directory to park.
        path: PathBuf,
    },
    /// Link a site by name to a directory.
    Link {
        /// The site name (a single DNS label).
        name: String,
        /// The directory to link.
        path: PathBuf,
    },
    /// Remove a linked or parked site by name.
    Unlink {
        /// The site name to remove.
        name: String,
    },
    /// Enumerate the registered parked directory roots (including empty ones,
    /// which produce no sites and so never appear in [`Self::ListSites`]).
    ListParked,
    /// Un-park a directory root: remove it from the parked set and re-scan.
    /// Its parked sites disappear; linked sites are untouched.
    Unpark {
        /// The parked root to remove. Deliberately a `String`, not a
        /// `PathBuf`: the daemon stores parked roots as canonical
        /// `String`s (`config.parked.paths` is a `BTreeSet<String>`), and
        /// clients echo a value straight from [`super::Response::Parked`].
        /// Keeping it a `String` makes the removal an exact identity match —
        /// a `PathBuf` round-trip risks lossy normalisation. The daemon does
        /// **not** canonicalise it (so a folder deleted from disk is still
        /// removable).
        path: String,
    },
    /// Change a site's PHP version.
    SetPhp {
        /// The site name.
        name: String,
        /// The new PHP version.
        version: PhpVersion,
    },
    /// Toggle whether a site is served over HTTPS.
    SetSecure {
        /// The site name.
        name: String,
        /// The desired HTTPS state.
        secure: bool,
    },
    /// Set or clear a site's served web root (the subdirectory served as the
    /// document root, e.g. `public` for Laravel).
    SetWebRoot {
        /// The site name.
        name: String,
        /// The served path. The daemon resolves it against the site's
        /// `document_root` (relative or absolute), validates containment, and
        /// stores the relative remainder. `None` resets the site to
        /// auto-detection.
        path: Option<String>,
    },
    /// Fetch read-only daemon runtime facts (DNS address, TLD, CA path +
    /// fingerprint). Used by `yerd elevate` to drive the privileged helper.
    DaemonInfo,
    /// Download + install a prebuilt PHP version into yerd's data dir.
    InstallPhp {
        /// The major.minor version to install (resolved to a pinned patch).
        version: PhpVersion,
    },
    /// Set the global default PHP version (terminal `php` shim + site fallback).
    SetDefaultPhp {
        /// The version to make the default; must already be installed.
        version: PhpVersion,
    },
    /// List installed PHP versions and the current default.
    ListPhp,
    /// Upgrade installed PHP to the latest published patch. `Some` = one minor,
    /// `None` = every installed version.
    UpdatePhp {
        /// The minor to update, or `None` for all installed.
        version: Option<PhpVersion>,
    },
    /// Force a poll of the distribution + refresh the update cache, then return
    /// the (enriched) version list.
    CheckPhpUpdates,
    /// List the major.minor versions installable from the distribution (the GUI
    /// install dropdown / `yerd list php --available`). Fetched on demand.
    AvailablePhp,
    /// Merge global PHP ini settings into the config and apply them to all
    /// installed versions' FPM pools. An empty-string value removes a key
    /// (resets it to PHP's built-in default).
    SetPhpSettings {
        /// Setting name → value (e.g. `"memory_limit" -> "512M"`); `""` removes.
        settings: BTreeMap<String, String>,
    },
    /// Restart one installed version's FPM pool (stop + ensure).
    RestartPhp {
        /// The version whose pool to restart.
        version: PhpVersion,
    },
    /// Restart every started FPM pool (running or failed).
    RestartAllPhp,
    /// Uninstall an installed PHP version. Blocked when the version is in use by
    /// a site, is the last version while sites remain, or is the current default
    /// while other versions are installed.
    UninstallPhp {
        /// The version to uninstall.
        version: PhpVersion,
    },
    /// Fetch a read-only [`crate::StatusReport`] of daemon/proxy/DNS/PHP health.
    Status,
    /// Run the doctor checks and return the resulting diagnoses.
    Diagnose,
    /// Run the doctor checks, attempt the safe auto-fixes, and report what
    /// happened plus what still needs manual action.
    DoctorFix,
    /// Restart the daemon's own process in place (re-exec). The daemon replies
    /// `Ok` *before* tearing down; the connection then closes as it restarts.
    /// Unix-only.
    RestartDaemon,
    /// List every manageable service with its live status (installed versions,
    /// run state, port, enabled flag).
    ListServices,
    /// List installable vs installed versions per service (the GUI install
    /// dropdown). Fetched on demand from yerd's services distribution.
    AvailableServices,
    /// Download + install a prebuilt service version into yerd's data dir.
    InstallService {
        /// Service id (`"redis"`, `"mysql"`, `"mariadb"`, `"postgres"`).
        service: String,
        /// The version to install.
        version: String,
    },
    /// Uninstall a service version. `purge` also deletes the datadir; without
    /// it the data is retained and its path reported.
    UninstallService {
        /// Service id.
        service: String,
        /// The version to remove.
        version: String,
        /// When true, also delete the engine's datadir (destructive).
        purge: bool,
    },
    /// Start (and enable) a service instance.
    StartService {
        /// Service id.
        service: String,
    },
    /// Stop (and disable auto-start for) a service instance.
    StopService {
        /// Service id.
        service: String,
    },
    /// Restart a service instance (stop + start).
    RestartService {
        /// Service id.
        service: String,
    },
    /// Change the port a service listens on. Takes effect on the next start /
    /// restart (no implicit hot restart of a live socket).
    SetServicePort {
        /// Service id.
        service: String,
        /// The new loopback port.
        port: u16,
    },
    /// Fetch the last `lines` lines of a service's log file.
    ServiceLogs {
        /// Service id.
        service: String,
        /// How many trailing lines to return.
        lines: u32,
    },
    /// Create a database in a running SQL service (no-op error for caches).
    CreateDatabase {
        /// Service id (must be a SQL engine).
        service: String,
        /// The database name to create (validated as a safe identifier).
        name: String,
    },
    /// List the user databases in a running SQL service (system schemas
    /// filtered out).
    ListDatabases {
        /// Service id (must be a SQL engine).
        service: String,
    },
    /// Drop a database from a running SQL service.
    DropDatabase {
        /// Service id (must be a SQL engine).
        service: String,
        /// The database name to drop (validated; system databases refused).
        name: String,
    },
    /// Back a database up to a plain-SQL file (streamed from the bundled dump tool).
    BackupDatabase {
        /// Service id (must be a SQL engine).
        service: String,
        /// The database name to dump (validated as a safe identifier).
        name: String,
        /// Absolute destination file the daemon writes the dump to. The client
        /// absolutises this against the user's cwd before sending (the daemon's cwd
        /// differs); the path never reaches the dump tool's argv.
        path: PathBuf,
    },
    /// Restore a database from a plain-SQL file (streamed into the bundled client's
    /// stdin). The target database must already exist.
    RestoreDatabase {
        /// Service id (must be a SQL engine).
        service: String,
        /// The database name to restore into (validated; system databases refused).
        name: String,
        /// Absolute source file the daemon reads the dump from. The client
        /// canonicalises this before sending; the path never reaches the client's argv.
        path: PathBuf,
    },
    /// Switch a service to a different version: install `version`, restart the
    /// running instance onto it, then remove the previously-installed version
    /// (the datadir is retained). A service holds one installed version at a
    /// time; this upgrades or downgrades it.
    ChangeServiceVersion {
        /// Service id.
        service: String,
        /// The version to switch to.
        version: String,
    },
    /// Page the buffered dump-telemetry events newer than `since_id` (0 = all),
    /// plus the ids removed since then and the current per-category counts.
    ListDumps {
        /// Return events with `id > since_id`. Clients track the latest id.
        since_id: u64,
    },
    /// Drop every buffered dump event (pinned ones included).
    ClearDumps,
    /// Delete one buffered dump event by id.
    DeleteDump {
        /// The event id to delete.
        id: u64,
    },
    /// Turn dump interception on or off (the "antenna"). Writes the runtime
    /// state file the extension reads; never restarts FPM.
    SetDumpsEnabled {
        /// Desired enabled state.
        enabled: bool,
    },
    /// Set the loopback port the dump server listens on and the extension
    /// connects to.
    SetDumpsPort {
        /// The new loopback port.
        port: u16,
    },
    /// Enable or disable capture of one telemetry feature (e.g. `"queries"`).
    SetDumpFeature {
        /// Feature key (`dumps`/`queries`/`jobs`/`views`/`requests`/`logs`/`cache`).
        feature: String,
        /// Desired enabled state.
        enabled: bool,
    },
    /// Toggle log persistence. `false` (default) clears the buffer on each new
    /// request (latest-request view); `true` accumulates across requests.
    SetDumpsPersist {
        /// Desired persist state.
        persist: bool,
    },
    /// Fetch dump-server status (enabled, port, running, per-version extension
    /// presence, current counts).
    DumpsStatus,
    /// List captured emails (metadata only), newest first.
    ListMails,
    /// Fetch one captured email's full decoded content by id.
    GetMail {
        /// The email id (from [`super::Response::Mails`]).
        id: String,
    },
    /// Delete every captured email.
    ClearMails,
    /// Delete a specific set of captured emails by id (e.g. all the mail shown
    /// for one application). Unknown ids are ignored.
    DeleteMails {
        /// The email ids to delete.
        ids: Vec<String>,
    },
    /// Set the mail-capture SMTP port. Takes effect on the next daemon
    /// start/restart (no implicit hot rebind), like [`Self::SetServicePort`].
    SetMailPort {
        /// The new loopback port (must be non-zero).
        port: u16,
    },
    /// Enable or disable the mail-capture server. Takes effect on the next
    /// daemon start/restart.
    SetMailEnabled {
        /// The desired enabled state.
        enabled: bool,
    },
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    // The rename-trap match arms are deliberately all `{}`; merging
    // them would collapse the per-variant check that catches Rust
    // variant renames.
    clippy::match_same_arms
)]
mod variant_name_pinning {
    use super::*;
    use std::path::PathBuf;

    // Inline (not in tests/) so the #[non_exhaustive] enum matches
    // exhaustively. A renamed Rust variant fails this match at compile
    // time.
    #[allow(dead_code)]
    fn pin(r: Request) {
        match r {
            Request::Ping => {}
            Request::ListSites => {}
            Request::Park { .. } => {}
            Request::Link { .. } => {}
            Request::Unlink { .. } => {}
            Request::ListParked => {}
            Request::Unpark { .. } => {}
            Request::SetPhp { .. } => {}
            Request::SetSecure { .. } => {}
            Request::SetWebRoot { .. } => {}
            Request::DaemonInfo => {}
            Request::InstallPhp { .. } => {}
            Request::SetDefaultPhp { .. } => {}
            Request::ListPhp => {}
            Request::UpdatePhp { .. } => {}
            Request::CheckPhpUpdates => {}
            Request::AvailablePhp => {}
            Request::SetPhpSettings { .. } => {}
            Request::RestartPhp { .. } => {}
            Request::RestartAllPhp => {}
            Request::UninstallPhp { .. } => {}
            Request::Status => {}
            Request::Diagnose => {}
            Request::DoctorFix => {}
            Request::RestartDaemon => {}
            Request::ListServices => {}
            Request::AvailableServices => {}
            Request::InstallService { .. } => {}
            Request::UninstallService { .. } => {}
            Request::StartService { .. } => {}
            Request::StopService { .. } => {}
            Request::RestartService { .. } => {}
            Request::SetServicePort { .. } => {}
            Request::ServiceLogs { .. } => {}
            Request::CreateDatabase { .. } => {}
            Request::ListDatabases { .. } => {}
            Request::DropDatabase { .. } => {}
            Request::BackupDatabase { .. } => {}
            Request::RestoreDatabase { .. } => {}
            Request::ChangeServiceVersion { .. } => {}
            Request::ListDumps { .. } => {}
            Request::ClearDumps => {}
            Request::DeleteDump { .. } => {}
            Request::SetDumpsEnabled { .. } => {}
            Request::SetDumpsPort { .. } => {}
            Request::SetDumpFeature { .. } => {}
            Request::SetDumpsPersist { .. } => {}
            Request::DumpsStatus => {}
            Request::ListMails => {}
            Request::GetMail { .. } => {}
            Request::ClearMails => {}
            Request::DeleteMails { .. } => {}
            Request::SetMailPort { .. } => {}
            Request::SetMailEnabled { .. } => {}
        }
    }

    #[test]
    #[allow(clippy::too_many_lines)] // one `pin(...)` per variant; grows with the enum
    fn touch_every_variant() {
        pin(Request::Ping);
        pin(Request::ListSites);
        pin(Request::Park {
            path: PathBuf::from("/x"),
        });
        pin(Request::Link {
            name: "x".into(),
            path: PathBuf::from("/x"),
        });
        pin(Request::Unlink { name: "x".into() });
        pin(Request::ListParked);
        pin(Request::Unpark { path: "/x".into() });
        pin(Request::SetPhp {
            name: "x".into(),
            version: PhpVersion::new(8, 3),
        });
        pin(Request::SetSecure {
            name: "x".into(),
            secure: true,
        });
        pin(Request::SetWebRoot {
            name: "x".into(),
            path: Some("public".into()),
        });
        pin(Request::DaemonInfo);
        pin(Request::InstallPhp {
            version: PhpVersion::new(8, 5),
        });
        pin(Request::SetDefaultPhp {
            version: PhpVersion::new(8, 5),
        });
        pin(Request::ListPhp);
        pin(Request::UpdatePhp {
            version: Some(PhpVersion::new(8, 5)),
        });
        pin(Request::CheckPhpUpdates);
        pin(Request::AvailablePhp);
        pin(Request::SetPhpSettings {
            settings: BTreeMap::new(),
        });
        pin(Request::RestartPhp {
            version: PhpVersion::new(8, 5),
        });
        pin(Request::RestartAllPhp);
        pin(Request::UninstallPhp {
            version: PhpVersion::new(8, 5),
        });
        pin(Request::Status);
        pin(Request::Diagnose);
        pin(Request::DoctorFix);
        pin(Request::RestartDaemon);
        pin(Request::ListServices);
        pin(Request::AvailableServices);
        pin(Request::InstallService {
            service: "redis".into(),
            version: "8".into(),
        });
        pin(Request::UninstallService {
            service: "redis".into(),
            version: "8".into(),
            purge: false,
        });
        pin(Request::StartService {
            service: "redis".into(),
        });
        pin(Request::StopService {
            service: "redis".into(),
        });
        pin(Request::RestartService {
            service: "redis".into(),
        });
        pin(Request::SetServicePort {
            service: "redis".into(),
            port: 6380,
        });
        pin(Request::ServiceLogs {
            service: "redis".into(),
            lines: 100,
        });
        pin(Request::CreateDatabase {
            service: "mysql".into(),
            name: "app".into(),
        });
        pin(Request::ListDatabases {
            service: "mysql".into(),
        });
        pin(Request::DropDatabase {
            service: "mysql".into(),
            name: "app".into(),
        });
        pin(Request::BackupDatabase {
            service: "mysql".into(),
            name: "app".into(),
            path: PathBuf::from("/x/app.sql"),
        });
        pin(Request::RestoreDatabase {
            service: "mysql".into(),
            name: "app".into(),
            path: PathBuf::from("/x/app.sql"),
        });
        pin(Request::ChangeServiceVersion {
            service: "redis".into(),
            version: "9.1.0".into(),
        });
        pin(Request::ListDumps { since_id: 0 });
        pin(Request::ClearDumps);
        pin(Request::DeleteDump { id: 1 });
        pin(Request::SetDumpsEnabled { enabled: true });
        pin(Request::SetDumpsPort { port: 2304 });
        pin(Request::SetDumpFeature {
            feature: "queries".into(),
            enabled: true,
        });
        pin(Request::SetDumpsPersist { persist: true });
        pin(Request::DumpsStatus);
        pin(Request::ListMails);
        pin(Request::GetMail {
            id: "000001".into(),
        });
        pin(Request::ClearMails);
        pin(Request::DeleteMails {
            ids: vec!["000001".into()],
        });
        pin(Request::SetMailPort { port: 2525 });
        pin(Request::SetMailEnabled { enabled: true });
    }
}
