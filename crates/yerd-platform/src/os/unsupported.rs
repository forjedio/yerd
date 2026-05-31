//! Stub implementations for unsupported OSes (Phase 1: Windows).
//!
//! Every trait method returns `Err(PlatformError::Unsupported { operation })`.
//! This lets `cargo check --workspace` stay green on every host while the
//! macOS + Linux impls are the only ones with behaviour.

use std::net::SocketAddr;

use crate::error::ops;
use crate::metrics::SystemMetrics;
use crate::paths::{Paths, PlatformDirs};
use crate::port_binder::{BoundPort, PortBinder, PortPair};
use crate::port_redirect::PortRedirector;
use crate::resolver::ResolverInstaller;
use crate::trust_store::{CaFingerprint, NssOutcome, TrustStore};
use crate::PlatformError;

/// Stub `Paths` for unsupported OSes.
#[derive(Debug, Default, Clone, Copy)]
pub struct UnsupportedPaths;

impl Paths for UnsupportedPaths {
    fn resolve(&self) -> Result<PlatformDirs, PlatformError> {
        Err(PlatformError::Unsupported {
            operation: ops::PATHS_RESOLVE,
        })
    }
}

/// Stub `TrustStore` for unsupported OSes.
#[derive(Debug, Default, Clone, Copy)]
pub struct UnsupportedTrustStore;

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

    fn install_firefox_nss(&self, _: &str) -> Result<NssOutcome, PlatformError> {
        Err(PlatformError::Unsupported {
            operation: ops::INSTALL_FIREFOX_NSS,
        })
    }
}

/// Stub `ResolverInstaller` for unsupported OSes.
#[derive(Debug, Default, Clone, Copy)]
pub struct UnsupportedResolverInstaller;

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

/// Stub `PortBinder` for unsupported OSes.
#[derive(Debug, Default, Clone, Copy)]
pub struct UnsupportedPortBinder;

impl PortBinder for UnsupportedPortBinder {
    fn bind(&self, _: u16) -> Result<BoundPort, PlatformError> {
        Err(PlatformError::Unsupported {
            operation: ops::BIND,
        })
    }

    fn bind_pair(&self, _: (u16, u16), _: (u16, u16)) -> Result<PortPair, PlatformError> {
        Err(PlatformError::Unsupported {
            operation: ops::BIND_PAIR,
        })
    }
}

/// Stub `SystemMetrics` for unsupported OSes — metrics are best-effort, so this
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

/// Unsupported-OS `PortRedirector`: always `None` (not applicable).
#[derive(Debug, Default, Clone, Copy)]
pub struct UnsupportedPortRedirector;

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
    /// default would be meaningless — report "not probed".
    fn foreign_web_listener(&self) -> Option<bool> {
        None
    }
}
