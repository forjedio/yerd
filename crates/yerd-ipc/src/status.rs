//! Status (`yerd status`) and doctor (`yerd doctor`) payload types.
//!
//! These travel inside [`crate::Response::Status`],
//! [`crate::Response::Diagnoses`], and [`crate::Response::DoctorFix`]. As with
//! the rest of this crate they are a published contract: add fields/variants
//! additively, never rename, and let `rename_all` (never per-field renames)
//! handle casing. `tests/wire_stability.rs` pins the byte-exact shape.
//!
//! ## No `f64` on the wire
//!
//! [`crate::Response`] derives `Eq`, so nothing reachable from it may contain a
//! float. The system load average therefore crosses as integer hundredths
//! ([`StatusReport::load_avg`] = `load × 100`); the CLI renders it back to
//! `x.xx`. The daemon computes the conversion from the platform layer's `f64`
//! reading at assembly time.

use std::net::SocketAddr;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use yerd_core::PhpVersion;

/// A read-only snapshot of daemon runtime health, returned for
/// [`crate::Request::Status`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StatusReport {
    /// The daemon's process id.
    pub daemon_pid: u32,
    /// Seconds since the daemon finished starting up.
    pub uptime_secs: u64,
    /// The TLD served (e.g. `"test"`).
    pub tld: String,
    /// HTTP listener: requested vs bound port.
    pub http: PortStatus,
    /// HTTPS listener: requested vs bound port.
    pub https: PortStatus,
    /// Address the embedded DNS responder is bound on.
    pub dns_addr: SocketAddr,
    /// Local CA facts, including its system-store trust state.
    pub ca: CaStatus,
    /// Whether the OS resolver routes `*.<tld>` to Yerd. `None` = the probe
    /// could not determine it (treat as "unknown", **not** as `false`).
    pub resolver_installed: Option<bool>,
    /// The global default PHP version.
    pub default_php: PhpVersion,
    /// One entry per installed PHP version (bundled + mise), with live FPM state.
    pub php: Vec<PhpPoolStatus>,
    /// Site counts by kind.
    pub sites: SiteCounts,
    /// System load average for 1/5/15 minutes, each `× 100` (hundredths).
    /// `None` where unavailable (non-Linux, or a transient read failure).
    pub load_avg: Option<[u32; 3]>,
}

/// A listener's requested vs actually-bound port.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PortStatus {
    /// The port the config asked for.
    pub requested: u16,
    /// The port actually bound (differs from `requested` on rootless fallback).
    pub bound: u16,
    /// `true` when `bound != requested` (a rootless fallback fired).
    pub fell_back: bool,
}

/// Local CA facts surfaced in [`StatusReport`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CaStatus {
    /// Absolute path to the CA certificate PEM.
    pub path: PathBuf,
    /// SHA-256 fingerprint, 64 lowercase hex chars.
    pub fingerprint: String,
    /// Whether a CA matching `fingerprint` is present in the OS system store.
    /// `None` = the probe could not determine it (**not** `false`).
    pub trusted_system: Option<bool>,
}

/// Site counts by kind, for [`StatusReport`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SiteCounts {
    /// Number of parked sites.
    pub parked: usize,
    /// Number of linked sites.
    pub linked: usize,
    /// Number of sites served over HTTPS.
    pub secured: usize,
}

/// Per-version PHP-FPM status.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PhpPoolStatus {
    /// The installed minor version.
    pub version: PhpVersion,
    /// The installed full patch (e.g. `"8.5.6"`), if recorded.
    pub installed_patch: Option<String>,
    /// Live FPM run state for this version.
    pub state: PoolRunState,
    /// FPM master PID when running.
    pub pid: Option<u32>,
    /// FPM listen address (socket path, or `127.0.0.1:<port>`) when running.
    pub listen: Option<String>,
    /// Resident memory of the FPM master in bytes, when measurable.
    pub rss_bytes: Option<u64>,
    /// Newest published patch when newer than installed (from the update cache).
    pub update_available: Option<String>,
}

/// Live FPM run state for a single PHP version.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum PoolRunState {
    /// FPM is supervised and its master process is alive.
    Running,
    /// No FPM pool for this version (installed but never started, or stopped).
    Stopped,
    /// A supervised pool's master process has died.
    Failed,
}

/// A single doctor finding.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Diagnosis {
    /// Machine-readable check identifier.
    pub code: DiagnosisCode,
    /// How serious the finding is.
    pub severity: Severity,
    /// Short human-readable headline.
    pub title: String,
    /// Longer human-readable explanation.
    pub detail: String,
    /// An exact command (or guidance) to resolve it, when applicable.
    pub remedy: Option<String>,
}

/// Severity of a [`Diagnosis`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum Severity {
    /// Informational / healthy.
    Ok,
    /// A non-fatal problem the user should address.
    Warn,
    /// A problem that breaks expected behaviour.
    Fail,
}

/// Machine-readable identifier for a doctor check.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum DiagnosisCode {
    /// The daemon is not reachable.
    DaemonDown,
    /// A privileged port fell back to its rootless equivalent.
    PortFallback,
    /// The local CA is not trusted in the system store.
    CaNotTrusted,
    /// The OS resolver does not route `*.<tld>` to Yerd.
    ResolverNotInstalled,
    /// No PHP versions are installed.
    NoPhpInstalled,
    /// The configured default PHP version is not installed.
    DefaultPhpNotInstalled,
    /// A supervised FPM pool has failed.
    FpmPoolFailed,
    /// A newer PHP patch is available for an installed version.
    PhpUpdateAvailable,
    /// No sites are configured.
    NoSites,
    /// Everything checks out.
    AllGood,
}

/// Result of [`crate::Request::DoctorFix`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FixReport {
    /// Fixes the daemon attempted, in order.
    pub performed: Vec<FixResult>,
    /// Remaining findings the user must resolve manually (e.g. privileged ops).
    pub manual: Vec<Diagnosis>,
}

/// Outcome of one attempted auto-fix.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FixResult {
    /// Which check this fix addressed.
    pub code: DiagnosisCode,
    /// Whether the fix succeeded.
    pub ok: bool,
    /// Human-readable detail about what happened.
    pub message: String,
}
