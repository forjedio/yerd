//! OS abstraction layer for Yerd.
//!
//! The core traits live here — [`Paths`], [`TrustStore`], [`ResolverInstaller`],
//! [`PortBinder`], and [`PortRedirector`] — each with a single thin
//! implementation per OS selected by `#[cfg(target_os = ...)]`. macOS and Linux
//! ship in Phase 1;
//! Windows compiles against the [`os::unsupported`] stub that returns
//! [`PlatformError::Unsupported`] for every method.
//!
//! ## Privilege boundary
//!
//! `yerd-platform` is unprivileged library code. Operations that need root
//! return [`PlatformError::NeedsHelper`]. The typed [`HelperInvocation`]
//! enum carries the request to the `yerd-helper` binary (a separate crate)
//! for execution. The OS impls never spawn the helper themselves — a
//! privileged caller owns the `Command::new(...)` call: the daemon for its
//! own setup, or the `yerd elevate` CLI when run under `sudo`.
//!
//! ## Purity
//!
//! Decision logic that does not need OS interaction lives in the
//! [`pure`] module and is fully unit-tested in-memory.

#![forbid(unsafe_code)]

pub mod detect;
pub mod error;
pub mod helper;
pub mod metrics;
pub mod paths;
pub mod port_binder;
pub mod port_redirect;
pub mod pure;
pub mod resolver;
pub mod trust_store;

mod os;

pub use detect::{gather_project_signals, FsSignalSource, ProjectSignalSource};
pub use error::{BindPairErrorReason, PlatformError, ResolverErrorReason, TrustStoreErrorReason};
pub use helper::{ArgvParseError, HelperInvocation};
pub use metrics::SystemMetrics;
pub use paths::{Paths, PlatformDirs};
pub use port_binder::{BoundPort, PortBinder, PortPair};
pub use port_redirect::PortRedirector;
pub use resolver::ResolverInstaller;
pub use trust_store::{CaFingerprint, FingerprintParseError, NssFailure, NssOutcome, TrustStore};

pub use os::active::{
    ActivePaths, ActivePortBinder, ActivePortRedirector, ActiveResolverInstaller,
    ActiveSystemMetrics, ActiveTrustStore,
};
