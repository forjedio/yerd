//! In-memory mocks for downstream crates and per-OS smoke tests.

#![allow(
    dead_code,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::cell::RefCell;
use std::net::SocketAddr;
use std::path::PathBuf;

use yerd_platform::{
    BoundPort, CaFingerprint, NssOutcome, Paths, PlatformDirs, PlatformError, PortBinder, PortPair,
    ResolverInstaller, TrustStore,
};

/// In-memory `Paths` fixture. Returns the directories it was constructed
/// with; useful for daemon tests that need to run without touching the
/// host filesystem.
#[derive(Debug, Clone)]
pub struct MockPaths(pub PlatformDirs);

impl MockPaths {
    pub fn under_tempdir(base: &std::path::Path) -> Self {
        Self(PlatformDirs {
            config: base.join("config"),
            data: base.join("data"),
            state: base.join("state"),
            cache: base.join("cache"),
            runtime: base.join("runtime"),
        })
    }
}

impl Paths for MockPaths {
    fn resolve(&self) -> Result<PlatformDirs, PlatformError> {
        Ok(self.0.clone())
    }
}

/// `TrustStore` fake. `is_present_system` returns whatever fingerprints
/// were `installed`; `install_system` records the call rather than
/// returning `NeedsHelper`.
#[derive(Debug, Default)]
pub struct MockTrustStore {
    pub installed: RefCell<Vec<CaFingerprint>>,
    pub nss_calls: RefCell<usize>,
}

impl TrustStore for MockTrustStore {
    fn install_system(&self, _: &str, fp: &CaFingerprint) -> Result<(), PlatformError> {
        self.installed.borrow_mut().push(*fp);
        Ok(())
    }

    fn uninstall_system(&self, fp: &CaFingerprint) -> Result<(), PlatformError> {
        self.installed.borrow_mut().retain(|x| x != fp);
        Ok(())
    }

    fn is_present_system(&self, fp: &CaFingerprint) -> Result<bool, PlatformError> {
        Ok(self.installed.borrow().contains(fp))
    }

    fn install_firefox_nss(&self, _: &str) -> Result<NssOutcome, PlatformError> {
        *self.nss_calls.borrow_mut() += 1;
        Ok(NssOutcome {
            profiles_attempted: 0,
            profiles_succeeded: 0,
            failures: vec![],
            certutil_missing: true,
        })
    }

    fn system_root_bundle(&self) -> Result<Option<String>, PlatformError> {
        Ok(None)
    }
}

/// `ResolverInstaller` fake.
#[derive(Debug, Default)]
pub struct MockResolverInstaller {
    pub installed: RefCell<Vec<(String, SocketAddr)>>,
}

impl ResolverInstaller for MockResolverInstaller {
    fn install(&self, tld: &str, addr: SocketAddr) -> Result<(), PlatformError> {
        self.installed.borrow_mut().push((tld.to_owned(), addr));
        Ok(())
    }

    fn uninstall(&self, tld: &str) -> Result<(), PlatformError> {
        self.installed.borrow_mut().retain(|(t, _)| t != tld);
        Ok(())
    }

    fn is_installed(&self, tld: &str, addr: SocketAddr) -> Result<bool, PlatformError> {
        Ok(self
            .installed
            .borrow()
            .iter()
            .any(|(t, a)| t == tld && *a == addr))
    }
}

/// `PortBinder` fake - binds on `127.0.0.1` using `std::net` so the
/// returned `BoundPort` is a real listener that downstream tests can
/// connect to.
#[derive(Debug, Default)]
pub struct MockPortBinder;

impl PortBinder for MockPortBinder {
    fn bind(&self, port: u16) -> Result<BoundPort, PlatformError> {
        std::net::TcpListener::bind(("127.0.0.1", port))
            .map(|listener| BoundPort { listener })
            .map_err(|source| PlatformError::Bind { port, source })
    }

    fn bind_pair(
        &self,
        desired: (u16, u16),
        _fallback: (u16, u16),
    ) -> Result<PortPair, PlatformError> {
        Ok(PortPair {
            http: self.bind(desired.0)?,
            https: self.bind(desired.1)?,
        })
    }
}

/// Helper to silence "unused" warnings when only a subset of the
/// mocks is consumed by a given integration test file.
pub fn _all_mocks_compile() {
    fn assert_paths<T: Paths>() {}
    fn assert_trust<T: TrustStore>() {}
    fn assert_resolver<T: ResolverInstaller>() {}
    fn assert_binder<T: PortBinder>() {}
    assert_paths::<MockPaths>();
    assert_trust::<MockTrustStore>();
    assert_resolver::<MockResolverInstaller>();
    assert_binder::<MockPortBinder>();
}

/// Random fingerprint helper for tests.
#[must_use]
pub fn random_fingerprint(seed: u8) -> CaFingerprint {
    CaFingerprint::new([seed; 32])
}

/// Re-export a `PathBuf` alias to silence rust-analyzer noise.
pub type Path = PathBuf;
