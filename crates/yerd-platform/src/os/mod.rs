//! Per-OS implementations selected by `#[cfg(target_os = ...)]`.
//!
//! Exactly one of `linux`, `macos`, `windows`, or `unsupported` is active per
//! build. Windows has a real `Windows*` type for every trait, so nothing there
//! delegates to the `unsupported` stub, though that module stays compiled on
//! Windows. The `active` re-export below is the entry point used by `lib.rs`.

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
mod port_bind;
#[cfg(any(target_os = "linux", target_os = "macos"))]
mod unix;
#[cfg(not(any(target_os = "linux", target_os = "macos")))]
mod unsupported;
#[cfg(target_os = "windows")]
mod windows;

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

    #[cfg(target_os = "windows")]
    pub use super::windows::{
        broadcast_user_env_marker, current_user_sid, daemon_pipe_name, hidden_command,
        is_token_elevated, nrpt_guids_for_tld, nrpt_servers_for_tld, set_user_path, system32_exe,
        system_root, udp_port_owner, user_path, WindowsIdeLauncher as ActiveIdeLauncher,
        WindowsPaths as ActivePaths, WindowsPortBinder as ActivePortBinder,
        WindowsPortRedirector as ActivePortRedirector,
        WindowsResolverInstaller as ActiveResolverInstaller,
        WindowsSystemMetrics as ActiveSystemMetrics, WindowsSystemOpener as ActiveSystemOpener,
        WindowsTerminalLauncher as ActiveTerminalLauncher, WindowsTrustStore as ActiveTrustStore,
        CREATE_NEW_PROCESS_GROUP, CREATE_NO_WINDOW,
    };

    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
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
