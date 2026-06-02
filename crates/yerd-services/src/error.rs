//! Error types for `yerd-services`.
//!
//! [`ServiceError`] is **not** `Clone + Eq` (it wraps `std::io::Error` and
//! `yerd_platform::PlatformError`), mirroring `yerd_php::PhpError`. The daemon is
//! the only consumer.

use std::io;
use std::path::PathBuf;

use thiserror::Error;
use yerd_supervise::{DownloadError, ExitReason, SpawnFailureReason};

use crate::service::Service;
use crate::version::ServiceVersion;

/// Errors produced by `yerd-services`.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum ServiceError {
    /// The operation is not supported for this service on this platform yet
    /// (e.g. a non-Redis engine in the Phase 1 build, or Windows).
    #[error("{service} is not supported yet: {detail}")]
    Unsupported {
        /// The service in question.
        service: Service,
        /// What specifically is unsupported.
        detail: String,
    },

    /// The requested version of `service` is not installed on disk.
    #[error("{service} {version} is not installed")]
    VersionNotInstalled {
        /// The service.
        service: Service,
        /// The version that was requested.
        version: ServiceVersion,
    },

    /// Scanning the services data root failed for a reason other than
    /// `NotFound`.
    #[error("scan {} for installed services: {source}", dir.display())]
    DiscoveryIo {
        /// The directory being scanned.
        dir: PathBuf,
        /// Underlying OS error.
        #[source]
        source: io::Error,
    },

    /// One-time datadir initialisation failed (initdb / `--initialize` / etc.).
    #[error("initialise {service} datadir at {}: {detail}", datadir.display())]
    Init {
        /// The service being initialised.
        service: Service,
        /// The datadir we were initialising.
        datadir: PathBuf,
        /// Human-readable failure detail.
        detail: String,
    },

    /// Spawning the server process failed (or the wait failed mid-supervision).
    #[error("spawn {service} ({reason:?}): {source}")]
    Spawn {
        /// Which service's server we tried to spawn.
        service: Service,
        /// Classification of the underlying `io::Error`.
        reason: SpawnFailureReason,
        /// Underlying OS error.
        #[source]
        source: io::Error,
    },

    /// Writing the rendered service config file failed.
    #[error("write {service} config to {}: {source}", path.display())]
    ConfigWrite {
        /// The config path we were trying to write.
        path: PathBuf,
        /// The service.
        service: Service,
        /// Underlying OS error.
        #[source]
        source: io::Error,
    },

    /// The readiness window elapsed without the server accepting connections.
    /// The child has been killed before this error surfaces.
    #[error("{service} health check timed out after {attempts} attempts")]
    HealthCheckTimedOut {
        /// Which service was being health-checked.
        service: Service,
        /// How many `Starting` attempts had accumulated.
        attempts: u32,
    },

    /// The server crashed repeatedly past the restart budget.
    #[error("{service} crashed repeatedly (last exit: {reason})")]
    PermanentFailure {
        /// Which service exhausted its restart budget.
        service: Service,
        /// The most recent exit reason recorded by the supervisor.
        reason: ExitReason,
    },

    /// The configured port is already in use by another listener.
    #[error("{service} port {port} is already in use")]
    PortInUse {
        /// The service.
        service: Service,
        /// The port that could not be bound.
        port: u16,
    },

    /// Binding / pre-flighting the listen port failed for a non-conflict reason.
    #[error("bind {service} port {port}: {source}")]
    Bind {
        /// The service.
        service: Service,
        /// The port we tried to bind.
        port: u16,
        /// Underlying platform error.
        #[source]
        source: yerd_platform::PlatformError,
    },

    /// Sending a signal to the server child failed.
    #[error("kill {service}: {source}")]
    Kill {
        /// Which service's server we tried to signal.
        service: Service,
        /// Underlying OS error.
        #[source]
        source: io::Error,
    },

    /// No prebuilt build of the requested version is published for this platform
    /// (discovered from yerd's services listing).
    #[error("no prebuilt {service} {version} found for this platform")]
    VersionUnavailable {
        /// The service.
        service: Service,
        /// The version that was requested.
        version: ServiceVersion,
    },

    /// The running OS/architecture has no prebuilt service builds.
    #[error("unsupported platform: {detail}")]
    UnsupportedPlatform {
        /// Which dimension is unsupported.
        detail: String,
    },

    /// The fetched services listing was not valid JSON in the expected shape.
    #[error("parse services listing: {detail}")]
    ListingParse {
        /// Human-readable parse failure detail.
        detail: String,
    },

    /// The listing declared a `schema` version this build does not understand.
    /// A schema bump signals an incompatible format change — the user should
    /// update yerd rather than have us misread it.
    #[error("services listing schema {found} is unsupported (this build understands {supported})")]
    UnsupportedListingSchema {
        /// The schema version the listing declared.
        found: u32,
        /// The schema version this build supports.
        supported: u32,
    },

    /// Downloading an artifact failed.
    #[error(transparent)]
    Download(#[from] DownloadError),

    /// Unpacking a downloaded archive failed (bad/empty/unsafe tar, or write).
    #[error("extract {what}: {detail}")]
    Extract {
        /// What we were extracting (e.g. the artifact URL).
        what: String,
        /// Human-readable failure detail.
        detail: String,
    },
}
