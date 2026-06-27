//! rustls server-config construction and SNI cert resolution.

use std::sync::{Arc, OnceLock};

use rustls::server::{ClientHello, ResolvesServerCert};
use rustls::sign::CertifiedKey;
use rustls::ServerConfig;

use crate::traits::CertStore;

/// Install the ring `CryptoProvider` as the process-level default exactly once.
///
/// Required because the workspace pins rustls 0.23 with no global provider
/// preinstalled - the first call into `ServerConfig::builder()` would
/// otherwise panic. Idempotent via [`OnceLock`].
pub fn init_crypto_once() {
    static ONCE: OnceLock<()> = OnceLock::new();
    ONCE.get_or_init(|| {
        let _ = rustls::crypto::ring::default_provider().install_default();
    });
}

/// rustls SNI resolver that delegates to a [`CertStore`].
#[derive(Debug)]
pub struct SniResolver<C: CertStore> {
    store: Arc<C>,
}

impl<C: CertStore> SniResolver<C> {
    /// Wrap a [`CertStore`] for use as a rustls cert resolver.
    pub fn new(store: Arc<C>) -> Self {
        Self { store }
    }
}

impl<C: CertStore> ResolvesServerCert for SniResolver<C> {
    fn resolve(&self, client_hello: ClientHello<'_>) -> Option<Arc<CertifiedKey>> {
        let sni = client_hello.server_name()?;
        if let Some(key) = self.store.certified_key(sni) {
            Some(key)
        } else {
            tracing::debug!(
                target: "yerd_proxy::tls",
                sni = %sni,
                "SNI miss — dropping connection"
            );
            None
        }
    }
}

/// Build a `rustls::ServerConfig` keyed off a [`CertStore`].
pub fn build_server_config<C: CertStore>(store: Arc<C>) -> Arc<ServerConfig> {
    init_crypto_once();
    let resolver = Arc::new(SniResolver::new(store));
    let config = ServerConfig::builder()
        .with_no_client_auth()
        .with_cert_resolver(resolver);
    Arc::new(config)
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::Mutex;

    #[derive(Debug)]
    struct StubStore {
        misses: Mutex<HashMap<String, u32>>,
    }
    impl CertStore for StubStore {
        fn certified_key(&self, sni: &str) -> Option<Arc<CertifiedKey>> {
            *self
                .misses
                .lock()
                .unwrap()
                .entry(sni.to_owned())
                .or_insert(0) += 1;
            None
        }
    }

    #[test]
    fn init_crypto_is_idempotent() {
        init_crypto_once();
        init_crypto_once();
        init_crypto_once();
    }

    #[test]
    fn build_server_config_returns_arc() {
        let store = Arc::new(StubStore {
            misses: Mutex::new(HashMap::new()),
        });
        let cfg = build_server_config(store);
        assert!(Arc::strong_count(&cfg) >= 1);
    }
}
