//! Stub implementations for OSes without a real impl.
//!
//! Every trait method returns `Err(PlatformError::Unsupported { operation })`.
//! This lets `cargo check --workspace` stay green on every host while the
//! macOS + Linux impls are the only ones with full behaviour. Windows reuses
//! these stubs for every trait except `Paths` (`os::windows` aliases them),
//! replacing one at a time as later phases implement real `Windows*` types.

use std::net::SocketAddr;
use std::path::Path;

use crate::error::ops;
use crate::ide::{DetectedIde, IdeLauncher};
use crate::metrics::SystemMetrics;
use crate::opener::SystemOpener;
#[cfg(not(target_os = "windows"))]
use crate::paths::{Paths, PlatformDirs};
use crate::port_binder::{BoundPort, PortBinder, PortPair};
use crate::port_redirect::PortRedirector;
use crate::resolver::ResolverInstaller;
use crate::terminal::TerminalLauncher;
use crate::trust_store::{CaFingerprint, NssOutcome, TrustStore};
use crate::PlatformError;

/// Stub terminal launcher for unsupported OSes. Windows now has a real
/// [`super::windows::WindowsTerminalLauncher`], so this stub is unused there (but
/// the module stays compiled on Windows for the other still-stubbed traits).
#[cfg_attr(windows, allow(dead_code))]
#[derive(Debug, Default, Clone, Copy)]
pub struct UnsupportedTerminalLauncher;

#[cfg_attr(windows, allow(dead_code))]
impl UnsupportedTerminalLauncher {
    /// Construct.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

impl TerminalLauncher for UnsupportedTerminalLauncher {
    fn open_terminal(&self, _: &Path) -> Result<(), PlatformError> {
        Err(PlatformError::Unsupported {
            operation: ops::OPEN_TERMINAL,
        })
    }
}

/// Stub IDE launcher for unsupported OSes.
#[derive(Debug, Default, Clone, Copy)]
pub struct UnsupportedIdeLauncher;

impl UnsupportedIdeLauncher {
    /// Construct.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

impl IdeLauncher for UnsupportedIdeLauncher {
    fn detect(&self) -> Vec<DetectedIde> {
        Vec::new()
    }

    fn launch(&self, _: &DetectedIde, _: &Path) -> Result<(), PlatformError> {
        Err(PlatformError::Unsupported {
            operation: ops::OPEN_IDE,
        })
    }
}

/// Stub system opener for unsupported OSes.
#[derive(Debug, Default, Clone, Copy)]
pub struct UnsupportedSystemOpener;

impl UnsupportedSystemOpener {
    /// Construct.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

impl SystemOpener for UnsupportedSystemOpener {
    fn open_path(&self, _: &Path) -> Result<(), PlatformError> {
        Err(PlatformError::Unsupported {
            operation: ops::OPEN_DEFAULT,
        })
    }
}

/// Stub `Paths` for OSes with no real path resolution. Windows has a real
/// `Paths` impl (`os::windows::WindowsPaths`), so this stub is not compiled
/// there.
#[cfg(not(target_os = "windows"))]
#[derive(Debug, Default, Clone, Copy)]
pub struct UnsupportedPaths;

#[cfg(not(target_os = "windows"))]
impl UnsupportedPaths {
    /// Construct.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

#[cfg(not(target_os = "windows"))]
impl Paths for UnsupportedPaths {
    fn resolve(&self) -> Result<PlatformDirs, PlatformError> {
        Err(PlatformError::Unsupported {
            operation: ops::PATHS_RESOLVE,
        })
    }
}

/// Stub `TrustStore` for unsupported OSes. Windows now has a real
/// [`super::windows::WindowsTrustStore`], so this stub is unused there (but the
/// module stays compiled on Windows for the other still-stubbed traits).
#[cfg_attr(windows, allow(dead_code))]
#[derive(Debug, Default, Clone, Copy)]
pub struct UnsupportedTrustStore;

#[cfg_attr(windows, allow(dead_code))]
impl UnsupportedTrustStore {
    /// Construct.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

impl TrustStore for UnsupportedTrustStore {
    fn install_system(&self, _: &str, _: &CaFingerprint) -> Result<(), PlatformError> {
        Err(PlatformError::Unsupported {
            operation: ops::INSTALL_CA,
        })
    }

    fn uninstall_system(&self, _: &CaFingerprint) -> Result<(), PlatformError> {
        Err(PlatformError::Unsupported {
            operation: ops::UNINSTALL_CA,
        })
    }

    fn is_present_system(&self, _: &CaFingerprint) -> Result<bool, PlatformError> {
        Err(PlatformError::Unsupported {
            operation: ops::IS_PRESENT_SYSTEM,
        })
    }

    fn install_firefox_nss(&self, _: &Path) -> Result<NssOutcome, PlatformError> {
        Err(PlatformError::Unsupported {
            operation: ops::INSTALL_FIREFOX_NSS,
        })
    }

    fn uninstall_firefox_nss(&self) -> Result<NssOutcome, PlatformError> {
        Err(PlatformError::Unsupported {
            operation: ops::UNINSTALL_FIREFOX_NSS,
        })
    }

    /// No host root source on unsupported OSes: reports "no roots" (`Ok(None)`)
    /// rather than erroring, so the daemon simply leaves PHP's default trust
    /// store untouched.
    fn system_root_bundle(&self) -> Result<Option<String>, PlatformError> {
        Ok(None)
    }
}

/// Stub `ResolverInstaller` for unsupported OSes. Windows now has a real
/// [`super::windows::WindowsResolverInstaller`], so this stub is unused there
/// (but the module stays compiled on Windows for the other still-stubbed traits).
#[cfg_attr(windows, allow(dead_code))]
#[derive(Debug, Default, Clone, Copy)]
pub struct UnsupportedResolverInstaller;

#[cfg_attr(windows, allow(dead_code))]
impl UnsupportedResolverInstaller {
    /// Construct.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

impl ResolverInstaller for UnsupportedResolverInstaller {
    fn install(&self, _: &str, _: SocketAddr) -> Result<(), PlatformError> {
        Err(PlatformError::Unsupported {
            operation: ops::INSTALL_RESOLVER,
        })
    }

    fn uninstall(&self, _: &str) -> Result<(), PlatformError> {
        Err(PlatformError::Unsupported {
            operation: ops::UNINSTALL_RESOLVER,
        })
    }

    fn is_installed(&self, _: &str, _: SocketAddr) -> Result<bool, PlatformError> {
        Err(PlatformError::Unsupported {
            operation: ops::IS_INSTALLED_RESOLVER,
        })
    }
}

/// Stub `PortBinder` for unsupported OSes. Windows now has a real
/// [`super::windows::WindowsPortBinder`], so this stub is unused there (but the
/// module stays compiled on Windows for the other still-stubbed traits).
#[cfg_attr(windows, allow(dead_code))]
#[derive(Debug, Default, Clone, Copy)]
pub struct UnsupportedPortBinder;

#[cfg_attr(windows, allow(dead_code))]
impl UnsupportedPortBinder {
    /// Construct.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

impl PortBinder for UnsupportedPortBinder {
    fn bind(&self, _: u16) -> Result<BoundPort, PlatformError> {
        Err(PlatformError::Unsupported {
            operation: ops::BIND,
        })
    }

    fn bind_pair(&self, _: bool, _: (u16, u16), _: (u16, u16)) -> Result<PortPair, PlatformError> {
        Err(PlatformError::Unsupported {
            operation: ops::BIND_PAIR,
        })
    }
}

/// Stub `SystemMetrics` for unsupported OSes - metrics are best-effort, so this
/// returns `None` (no metrics) rather than an error.
#[derive(Debug, Default, Clone, Copy)]
pub struct UnsupportedSystemMetrics;

impl UnsupportedSystemMetrics {
    /// Construct.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

impl SystemMetrics for UnsupportedSystemMetrics {
    fn rss_bytes(&self, _: u32) -> Option<u64> {
        None
    }

    fn load_average(&self) -> Option<[f64; 3]> {
        None
    }
}

/// Unsupported-OS `PortRedirector`: always `None` (not applicable). Windows now
/// has a real [`super::windows::WindowsPortRedirector`], so this stub is unused
/// there (but the module stays compiled on Windows for the other still-stubbed
/// traits).
#[cfg_attr(windows, allow(dead_code))]
#[derive(Debug, Default, Clone, Copy)]
pub struct UnsupportedPortRedirector;

#[cfg_attr(windows, allow(dead_code))]
impl UnsupportedPortRedirector {
    /// Construct.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

impl PortRedirector for UnsupportedPortRedirector {
    fn is_active(&self) -> Option<bool> {
        None
    }

    /// The proxy doesn't run on unsupported platforms, so the loopback-probe
    /// default would be meaningless - report "not probed".
    fn foreign_web_listener(&self) -> Option<bool> {
        None
    }
}
