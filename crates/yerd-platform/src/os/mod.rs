//! Per-OS implementations selected by `#[cfg(target_os = ...)]`.
//!
//! Exactly one of `linux`, `macos`, `windows`, or `unsupported` is active per build.
//! The `active` re-export below is the entry point used by `lib.rs`.

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
mod unsupported;
#[cfg(target_os = "windows")]
mod windows;

pub(crate) mod active {
    //! Type aliases for the currently-active OS implementation.

    #[cfg(target_os = "linux")]
    pub use super::linux::{
        LinuxPaths as ActivePaths, LinuxPortBinder as ActivePortBinder,
        LinuxPortRedirector as ActivePortRedirector,
        LinuxResolverInstaller as ActiveResolverInstaller,
        LinuxSystemMetrics as ActiveSystemMetrics, LinuxTerminalLauncher as ActiveTerminalLauncher,
        LinuxTrustStore as ActiveTrustStore,
    };

    #[cfg(target_os = "macos")]
    pub use super::macos::{
        MacosPaths as ActivePaths, MacosPortBinder as ActivePortBinder,
        MacosPortRedirector as ActivePortRedirector,
        MacosResolverInstaller as ActiveResolverInstaller,
        MacosSystemMetrics as ActiveSystemMetrics, MacosTerminalLauncher as ActiveTerminalLauncher,
        MacosTrustStore as ActiveTrustStore,
    };

    #[cfg(target_os = "windows")]
    pub use super::windows::{
        WindowsPaths as ActivePaths, WindowsPortBinder as ActivePortBinder,
        WindowsPortRedirector as ActivePortRedirector,
        WindowsResolverInstaller as ActiveResolverInstaller,
        WindowsSystemMetrics as ActiveSystemMetrics,
        WindowsTerminalLauncher as ActiveTerminalLauncher, WindowsTrustStore as ActiveTrustStore,
    };

    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    pub use super::unsupported::{
        UnsupportedPaths as ActivePaths, UnsupportedPortBinder as ActivePortBinder,
        UnsupportedPortRedirector as ActivePortRedirector,
        UnsupportedResolverInstaller as ActiveResolverInstaller,
        UnsupportedSystemMetrics as ActiveSystemMetrics,
        UnsupportedTerminalLauncher as ActiveTerminalLauncher,
        UnsupportedTrustStore as ActiveTrustStore,
    };
}
