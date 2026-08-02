//! OS abstraction layer for Yerd.
//!
//! The core traits live here - [`Paths`], [`TrustStore`], [`ResolverInstaller`],
//! [`PortBinder`], [`PortRedirector`], and [`TerminalLauncher`] - each with a single thin
//! implementation per OS selected by `#[cfg(target_os = ...)]`. macOS and Linux
//! have full implementations. Windows has a real [`Paths`] impl (`os::windows`)
//! and aliases every other trait to the [`os::unsupported`] stub, which returns
//! [`PlatformError::Unsupported`] for every method until later phases replace
//! each alias with a real `Windows*` type.
//!
//! ## Privilege boundary
//!
//! `yerd-platform` is unprivileged library code. Operations that need root
//! return [`PlatformError::NeedsHelper`]. The typed [`HelperInvocation`]
//! enum carries the request to the `yerd-helper` binary (a separate crate)
//! for execution. The OS impls never spawn the helper themselves - a
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
pub mod lan_ip;
pub mod metrics;
pub mod nss_exec;
pub mod paths;
pub mod port_binder;
pub mod port_redirect;
pub mod pure;
pub mod resolver;
pub mod terminal;
pub mod trust_store;

mod os;

pub use detect::{gather_project_signals, FsSignalSource, ProjectSignalSource};
pub use error::{
    BindPairErrorReason, PlatformError, ResolverErrorReason, TerminalErrorReason,
    TrustStoreErrorReason,
};
pub use helper::{ArgvParseError, HelperInvocation};
pub use lan_ip::{ActiveLanIpProvider, FakeLanIpProvider, LanIpProvider};
pub use metrics::SystemMetrics;
pub use paths::{Paths, PlatformDirs};
pub use port_binder::{BoundPort, PortBinder, PortPair};
pub use port_redirect::PortRedirector;
pub use resolver::ResolverInstaller;
pub use terminal::TerminalLauncher;
pub use trust_store::{
    BrowserCaTrust, CaFingerprint, FingerprintParseError, NssFailure, NssOutcome, TrustStore,
};

pub use os::active::{
    ActivePaths, ActivePortBinder, ActivePortRedirector, ActiveResolverInstaller,
    ActiveSystemMetrics, ActiveTerminalLauncher, ActiveTrustStore,
};

/// Windows IPC identity helpers: the current user's SID and the derived daemon
/// pipe name, shared by the daemon listener and every client.
#[cfg(target_os = "windows")]
pub use os::active::{current_user_sid, daemon_pipe_name};
