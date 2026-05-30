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
        }
    }

    #[test]
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
    }
}
