//! Stub implementations for unsupported OSes (Phase 1: Windows).
//!
//! Every trait method returns `Err(PlatformError::Unsupported { operation })`.
//! This lets `cargo check --workspace` stay green on every host while the
//! macOS + Linux impls are the only ones with behaviour.

use std::net::SocketAddr;

use crate::error::ops;
use crate::paths::{Paths, PlatformDirs};
use crate::port_binder::{BoundPort, PortBinder, PortPair};
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

    fn is_installed(&self, _: &str) -> Result<bool, PlatformError> {
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
