//! Per-OS implementations selected by `#[cfg(target_os = ...)]`.
//!
//! Exactly one of `linux`, `macos`, or `unsupported` is active per build.
//! The `active` re-export below is the entry point used by `lib.rs`.

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(not(any(target_os = "linux", target_os = "macos")))]
mod unsupported;

pub(crate) mod active {
    //! Type aliases for the currently-active OS implementation.

    #[cfg(target_os = "linux")]
    pub use super::linux::{
        LinuxPaths as ActivePaths, LinuxPortBinder as ActivePortBinder,
        LinuxResolverInstaller as ActiveResolverInstaller, LinuxTrustStore as ActiveTrustStore,
    };

    #[cfg(target_os = "macos")]
    pub use super::macos::{
        MacosPaths as ActivePaths, MacosPortBinder as ActivePortBinder,
        MacosResolverInstaller as ActiveResolverInstaller, MacosTrustStore as ActiveTrustStore,
    };

    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    pub use super::unsupported::{
        UnsupportedPaths as ActivePaths, UnsupportedPortBinder as ActivePortBinder,
        UnsupportedResolverInstaller as ActiveResolverInstaller,
        UnsupportedTrustStore as ActiveTrustStore,
    };
}
