//! OS abstraction layer for Yerd.
//!
//! The core traits live here - [`Paths`], [`TrustStore`], [`ResolverInstaller`],
//! [`PortBinder`], [`PortRedirector`], [`TerminalLauncher`], [`IdeLauncher`], and
//! [`SystemOpener`] - each with a single thin
//! implementation per OS selected by `#[cfg(target_os = ...)]`. macOS and Linux
//! have full implementations. Windows (`os::windows`) has real `Windows*` impls
//! for [`Paths`], [`TrustStore`], [`ResolverInstaller`], [`PortBinder`],
//! [`PortRedirector`] and [`TerminalLauncher`], and still aliases
//! [`IdeLauncher`], [`SystemOpener`] and [`SystemMetrics`] to the
//! [`os::unsupported`] stub, which returns [`PlatformError::Unsupported`] for
//! every method until a later phase replaces each remaining alias with a real
//! `Windows*` type.
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
pub mod ide;
pub mod lan_ip;
pub mod metrics;
pub mod nss_exec;
pub mod opener;
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
    BindPairErrorReason, IdeErrorReason, OpenErrorReason, PlatformError, ResolverErrorReason,
    TerminalErrorReason, TrustStoreErrorReason,
};
pub use helper::{ArgvParseError, HelperInvocation};
pub use ide::{DetectedIde, FakeIdeLauncher, IdeLauncher, LaunchTarget};
pub use lan_ip::{ActiveLanIpProvider, FakeLanIpProvider, LanIpProvider};
pub use metrics::SystemMetrics;
pub use opener::{FakeSystemOpener, SystemOpener};
pub use paths::{Paths, PlatformDirs};
pub use port_binder::{BoundPort, PortBinder, PortPair};
pub use port_redirect::PortRedirector;
pub use resolver::ResolverInstaller;
pub use terminal::TerminalLauncher;
pub use trust_store::{
    BrowserCaTrust, CaFingerprint, FingerprintParseError, NssFailure, NssOutcome, TrustStore,
};

pub use os::active::{
    ActiveIdeLauncher, ActivePaths, ActivePortBinder, ActivePortRedirector,
    ActiveResolverInstaller, ActiveSystemMetrics, ActiveSystemOpener, ActiveTerminalLauncher,
    ActiveTrustStore,
};

/// Windows IPC identity helpers: the current user's SID and the derived daemon
/// pipe name, shared by the daemon listener and every client.
#[cfg(target_os = "windows")]
pub use os::active::{current_user_sid, daemon_pipe_name};

/// Windows privilege + NRPT helpers: the elevated-token probe (shared by the CLI
/// and the helper) and the `.test` NRPT rule-GUID discovery (used by the helper
/// through this crate so `winreg` stays out of its own dependency graph).
#[cfg(target_os = "windows")]
pub use os::active::{is_token_elevated, nrpt_guids_for_tld};

/// Windows doctor-depth probes: which servers the `.tld` NRPT rule forwards to,
/// and the image name squatting a UDP port. Both are read-only, unprivileged,
/// and exist so the daemon can put a name in a diagnosis the bare
/// `is_installed`/bind-failure booleans cannot supply.
#[cfg(target_os = "windows")]
pub use os::active::{nrpt_servers_for_tld, udp_port_owner};

/// Windows user-`PATH` (`HKCU\Environment`) helpers: read the current value,
/// write a new one (preserving the `REG_EXPAND_SZ` type), and broadcast the
/// change so fresh shells see it. Used by the CLI's `yerd path`/tool-install PATH
/// wiring and the daemon's shim-dir-on-PATH doctor probe. Keeps `winreg` a
/// single-crate dependency (this crate), off the binaries' own graphs.
#[cfg(target_os = "windows")]
pub use os::active::{broadcast_user_env_marker, set_user_path, user_path};
