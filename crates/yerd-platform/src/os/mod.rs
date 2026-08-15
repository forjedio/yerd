//! Per-OS implementations selected by `#[cfg(target_os = ...)]`.
//!
//! Exactly one of `linux`, `macos`, or `unsupported` is active per build.
//! The `active` re-export below is the entry point used by `lib.rs`.

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(any(target_os = "linux", target_os = "macos"))]
mod unix;
#[cfg(not(any(target_os = "linux", target_os = "macos")))]
mod unsupported;

pub(crate) mod active {
    //! Type aliases for the currently-active OS implementation.

    #[cfg(target_os = "linux")]
    pub use super::linux::{
        LinuxIdeLauncher as ActiveIdeLauncher, LinuxPaths as ActivePaths,
        LinuxPortBinder as ActivePortBinder, LinuxPortRedirector as ActivePortRedirector,
        LinuxResolverInstaller as ActiveResolverInstaller,
        LinuxSystemMetrics as ActiveSystemMetrics, LinuxSystemOpener as ActiveSystemOpener,
        LinuxTerminalLauncher as ActiveTerminalLauncher, LinuxTrustStore as ActiveTrustStore,
    };

    #[cfg(target_os = "macos")]
    pub use super::macos::{
        MacosIdeLauncher as ActiveIdeLauncher, MacosPaths as ActivePaths,
        MacosPortBinder as ActivePortBinder, MacosPortRedirector as ActivePortRedirector,
        MacosResolverInstaller as ActiveResolverInstaller,
        MacosSystemMetrics as ActiveSystemMetrics, MacosSystemOpener as ActiveSystemOpener,
        MacosTerminalLauncher as ActiveTerminalLauncher, MacosTrustStore as ActiveTrustStore,
    };

    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    pub use super::unsupported::{
        UnsupportedIdeLauncher as ActiveIdeLauncher, UnsupportedPaths as ActivePaths,
        UnsupportedPortBinder as ActivePortBinder,
        UnsupportedPortRedirector as ActivePortRedirector,
        UnsupportedResolverInstaller as ActiveResolverInstaller,
        UnsupportedSystemMetrics as ActiveSystemMetrics,
        UnsupportedSystemOpener as ActiveSystemOpener,
        UnsupportedTerminalLauncher as ActiveTerminalLauncher,
        UnsupportedTrustStore as ActiveTrustStore,
    };
}
