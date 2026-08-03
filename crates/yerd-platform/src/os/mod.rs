//! Per-OS implementations selected by `#[cfg(target_os = ...)]`.
//!
//! Exactly one of `linux`, `macos`, `windows`, or `unsupported` is active per
//! build. Windows implements a growing subset with real `Windows*` types
//! (`Paths`, `PortBinder`, `PortRedirector`, `TrustStore`) and delegates the
//! remaining traits to the `unsupported` stub, so `unsupported` stays compiled
//! on Windows too. The `active` re-export below is the entry point used by
//! `lib.rs`.

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(not(any(target_os = "linux", target_os = "macos")))]
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
        current_user_sid, daemon_pipe_name, WindowsPaths as ActivePaths,
        WindowsPortBinder as ActivePortBinder, WindowsPortRedirector as ActivePortRedirector,
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
