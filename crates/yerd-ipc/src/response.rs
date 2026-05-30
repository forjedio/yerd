//! Daemon → client response envelope and error-code enum.
//!
//! Internally tagged on `type`, `snake_case`. Wire-stability assertions
//! live in `tests/wire_stability.rs`.

use std::net::SocketAddr;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use yerd_core::{PhpVersion, Site};

use crate::status::{Diagnosis, FixReport, StatusReport};

// Same rule: no per-field serde renames.
/// A response sent from the daemon to a client.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[non_exhaustive]
pub enum Response {
    /// Reply to [`crate::Request::Ping`].
    Pong,
    /// Reply to [`crate::Request::ListSites`].
    Sites {
        /// The sites currently known to the daemon, in lexicographic
        /// name order.
        sites: Vec<Site>,
    },
    /// Generic success for mutating requests
    /// ([`crate::Request::Park`], [`crate::Request::Link`],
    /// [`crate::Request::Unlink`], [`crate::Request::SetPhp`],
    /// [`crate::Request::SetSecure`]).
    Ok,
    /// A request failed. `code` is machine-readable; `message` is for
    /// human display.
    Error {
        /// Machine-readable error category.
        code: ErrorCode,
        /// Human-readable error message.
        message: String,
    },
    /// Reply to [`crate::Request::DaemonInfo`] — read-only runtime facts.
    Info {
        /// Address the embedded DNS responder is bound on (`127.0.0.1:<port>`).
        dns_addr: SocketAddr,
        /// The TLD served (e.g. `"test"`).
        tld: String,
        /// Absolute path to the local CA certificate PEM.
        ca_path: PathBuf,
        /// SHA-256 fingerprint of the CA cert, 64 lowercase hex chars.
        ca_fingerprint: String,
    },
    /// Reply to [`crate::Request::ListPhp`] / `CheckPhpUpdates` / `UpdatePhp`.
    PhpVersions {
        /// Installed versions, ascending.
        installed: Vec<PhpVersion>,
        /// The current global default.
        default: PhpVersion,
        /// Installed minors with a newer patch available (from the daemon's
        /// update cache). Empty when none / cache cold.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        updates: Vec<PhpUpdate>,
    },
    /// Reply to [`crate::Request::AvailablePhp`].
    AvailablePhp {
        /// Installable major.minor versions from the distribution, ascending.
        available: Vec<PhpVersion>,
        /// Currently installed versions, ascending, so clients can hide (GUI
        /// dropdown) or tag (CLI) them.
        installed: Vec<PhpVersion>,
    },
    /// Reply to [`crate::Request::Status`] — a runtime health snapshot.
    ///
    /// Boxed so the (large) report does not bloat every `Response` value;
    /// `Box<T>` serializes transparently, so the wire bytes are unchanged.
    Status {
        /// The assembled health report.
        report: Box<StatusReport>,
    },
    /// Reply to [`crate::Request::Diagnose`] — the doctor findings.
    Diagnoses {
        /// One entry per check that produced a finding.
        items: Vec<Diagnosis>,
    },
    /// Reply to [`crate::Request::DoctorFix`] — what was fixed + what remains.
    DoctorFix {
        /// The fix outcome.
        report: FixReport,
    },
}

/// An available newer patch for an installed PHP minor.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PhpUpdate {
    /// The installed minor (e.g. `8.5`).
    pub version: PhpVersion,
    /// The installed patch (e.g. `"8.5.6"`).
    pub installed: String,
    /// The newest published patch (e.g. `"8.5.7"`).
    pub latest: String,
}

/// Machine-readable error category for [`Response::Error`].
///
/// Fail-closed on unknown variants from a newer daemon (no
/// `#[serde(other)]` catch-all) — an unknown code surfaces as
/// [`crate::IpcError::Decode`], which is the broader "version mismatch
/// signal" until a `Hello`/`Welcome` handshake lands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum ErrorCode {
    /// The requested site or resource does not exist.
    NotFound,
    /// A site with that name is already registered.
    AlreadyExists,
    /// The supplied path was rejected (does not exist, not a
    /// directory, outside an allowed root, etc.).
    InvalidPath,
    /// Catch-all for daemon-side failures that don't fit a typed code.
    /// Expand this enum rather than overloading `Internal`.
    Internal,
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

    #[allow(dead_code)]
    fn pin_response(r: Response) {
        match r {
            Response::Pong => {}
            Response::Sites { .. } => {}
            Response::Ok => {}
            Response::Error { .. } => {}
            Response::Info { .. } => {}
            Response::PhpVersions { .. } => {}
            Response::AvailablePhp { .. } => {}
            Response::Status { .. } => {}
            Response::Diagnoses { .. } => {}
            Response::DoctorFix { .. } => {}
        }
    }

    #[allow(dead_code)]
    fn pin_code(c: ErrorCode) {
        match c {
            ErrorCode::NotFound => {}
            ErrorCode::AlreadyExists => {}
            ErrorCode::InvalidPath => {}
            ErrorCode::Internal => {}
        }
    }

    #[test]
    fn touch_every_variant() {
        pin_response(Response::Pong);
        pin_response(Response::Sites { sites: vec![] });
        pin_response(Response::Ok);
        pin_response(Response::Error {
            code: ErrorCode::Internal,
            message: "x".into(),
        });
        pin_response(Response::Info {
            dns_addr: "127.0.0.1:1053".parse().unwrap(),
            tld: "test".into(),
            ca_path: PathBuf::from("/x/ca.cert.pem"),
            ca_fingerprint: "ab".repeat(32),
        });
        pin_response(Response::PhpVersions {
            installed: vec![PhpVersion::new(8, 5)],
            default: PhpVersion::new(8, 5),
            updates: vec![],
        });
        pin_response(Response::AvailablePhp {
            available: vec![PhpVersion::new(8, 4), PhpVersion::new(8, 5)],
            installed: vec![PhpVersion::new(8, 5)],
        });
        pin_response(Response::Status {
            report: Box::new(crate::status::StatusReport {
                daemon_pid: 1,
                uptime_secs: 0,
                tld: "test".into(),
                http: crate::status::PortStatus {
                    requested: 80,
                    bound: 8080,
                    fell_back: true,
                },
                https: crate::status::PortStatus {
                    requested: 443,
                    bound: 8443,
                    fell_back: true,
                },
                dns_addr: "127.0.0.1:1053".parse().unwrap(),
                ca: crate::status::CaStatus {
                    path: PathBuf::from("/x/ca.cert.pem"),
                    fingerprint: "ab".repeat(32),
                    trusted_system: Some(false),
                },
                resolver_installed: None,
                default_php: PhpVersion::new(8, 5),
                php: vec![],
                sites: crate::status::SiteCounts::default(),
                load_avg: Some([100, 50, 25]),
            }),
        });
        pin_response(Response::Diagnoses {
            items: vec![crate::status::Diagnosis {
                code: crate::status::DiagnosisCode::AllGood,
                severity: crate::status::Severity::Ok,
                title: "x".into(),
                detail: "x".into(),
                remedy: None,
            }],
        });
        pin_response(Response::DoctorFix {
            report: crate::status::FixReport {
                performed: vec![],
                manual: vec![],
            },
        });
        for c in [
            ErrorCode::NotFound,
            ErrorCode::AlreadyExists,
            ErrorCode::InvalidPath,
            ErrorCode::Internal,
        ] {
            pin_code(c);
        }
    }
}
