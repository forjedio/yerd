//! Per-site leaf certificate store consumed by `yerd_proxy::ProxyServer`.
//!
//! On the first SNI miss for a given host the store issues a fresh leaf
//! via the in-memory CA, persists the PEM material under `leaves_dir`,
//! caches the parsed `CertifiedKey`, and returns it. Subsequent
//! handshakes hit the cache.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, RwLock};

/// In-memory cert store backed by a `yerd_tls::CertAuthority`.
pub struct DaemonCertStore {
    ca: yerd_tls::CertAuthority,
    leaves_dir: PathBuf,
    cache: RwLock<HashMap<String, Arc<rustls::sign::CertifiedKey>>>,
}

impl std::fmt::Debug for DaemonCertStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DaemonCertStore")
            .field("leaves_dir", &self.leaves_dir)
            .finish_non_exhaustive()
    }
}

impl DaemonCertStore {
    /// Construct.
    #[must_use]
    pub fn new(ca: yerd_tls::CertAuthority, leaves_dir: PathBuf) -> Self {
        Self {
            ca,
            leaves_dir,
            cache: RwLock::new(HashMap::new()),
        }
    }

    /// On cache miss: issue, persist, parse, cache.
    fn issue_and_cache(&self, host: &str) -> Option<Arc<rustls::sign::CertifiedKey>> {
        let validity = match leaf_validity() {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!(error = %e, host = %host, "leaf validity construction failed");
                return None;
            }
        };
        let leaf = match self.ca.issue_leaf(&[host.to_owned()], validity) {
            Ok(l) => l,
            Err(e) => {
                tracing::warn!(error = %e, host = %host, "CA refused to issue leaf");
                return None;
            }
        };
        let _ = std::fs::create_dir_all(&self.leaves_dir);
        let _ = std::fs::write(
            self.leaves_dir.join(format!("{host}.cert.pem")),
            leaf.cert_pem(),
        );
        let _ = std::fs::write(
            self.leaves_dir.join(format!("{host}.key.pem")),
            leaf.key_pem(),
        );
        let certified = Arc::new(parse_certified(leaf.cert_pem(), leaf.key_pem())?);
        if let Ok(mut guard) = self.cache.write() {
            guard.insert(host.to_owned(), Arc::clone(&certified));
        }
        Some(certified)
    }
}

impl yerd_proxy::CertStore for DaemonCertStore {
    fn certified_key(&self, sni_host: &str) -> Option<Arc<rustls::sign::CertifiedKey>> {
        let normalised = sni_host.trim_end_matches('.').to_ascii_lowercase();
        if let Ok(guard) = self.cache.read() {
            if let Some(key) = guard.get(&normalised).cloned() {
                return Some(key);
            }
        }
        self.issue_and_cache(&normalised)
    }
}

fn leaf_validity() -> Result<yerd_tls::Validity, yerd_tls::TlsError> {
    let now = time::OffsetDateTime::now_utc();
    yerd_tls::Validity::new(
        now - time::Duration::days(1),
        now + time::Duration::days(395),
    )
}

/// PEM → `CertifiedKey`. Mirrors the recipe in `yerd-proxy`'s
/// integration tests; failures are surfaced via `tracing::warn!` so an
/// operator hitting an opaque TLS handshake error gets a hint.
fn parse_chain_and_key(
    cert_pem: &str,
    key_pem: &str,
) -> Option<(
    Vec<rustls::pki_types::CertificateDer<'static>>,
    rustls::pki_types::PrivateKeyDer<'static>,
)> {
    use rustls::pki_types::pem::PemObject;
    use rustls::pki_types::{CertificateDer, PrivateKeyDer};

    let cert_chain: Vec<CertificateDer<'static>> =
        CertificateDer::pem_slice_iter(cert_pem.as_bytes())
            .filter_map(Result::ok)
            .map(CertificateDer::into_owned)
            .collect();
    if cert_chain.is_empty() {
        tracing::warn!("no certs parsed from PEM");
        return None;
    }
    let key_der: PrivateKeyDer<'static> = match PrivateKeyDer::from_pem_slice(key_pem.as_bytes()) {
        Ok(k) => k.clone_key(),
        Err(e) => {
            tracing::warn!(error = %e, "failed to parse private key PEM");
            return None;
        }
    };
    Some((cert_chain, key_der))
}

fn parse_certified(cert_pem: &str, key_pem: &str) -> Option<rustls::sign::CertifiedKey> {
    let (cert_chain, key_der) = parse_chain_and_key(cert_pem, key_pem)?;
    match rustls::crypto::ring::sign::any_supported_type(&key_der) {
        Ok(signing_key) => Some(rustls::sign::CertifiedKey::new(cert_chain, signing_key)),
        Err(e) => {
            tracing::warn!(error = %e, "certified_key: no rustls signing key type matches");
            None
        }
    }
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
    use yerd_proxy::CertStore;

    fn ca() -> yerd_tls::CertAuthority {
        let now = time::OffsetDateTime::now_utc();
        let validity = yerd_tls::Validity::new(
            now - time::Duration::days(1),
            now + time::Duration::days(365),
        )
        .unwrap();
        yerd_tls::CertAuthority::generate("Test CA", validity).unwrap()
    }

    #[test]
    fn issues_on_miss_then_caches() {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let tmp = tempfile::tempdir().unwrap();
        let store = DaemonCertStore::new(ca(), tmp.path().to_path_buf());
        let key1 = store.certified_key("app.test").expect("issued");
        let key2 = store.certified_key("app.test").expect("cached");
        assert!(Arc::ptr_eq(&key1, &key2));
        assert!(tmp.path().join("app.test.cert.pem").exists());
        assert!(tmp.path().join("app.test.key.pem").exists());
    }

    #[test]
    fn case_insensitive_sni_lookup() {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let tmp = tempfile::tempdir().unwrap();
        let store = DaemonCertStore::new(ca(), tmp.path().to_path_buf());
        let lower = store.certified_key("app.test").unwrap();
        let upper = store.certified_key("APP.TEST.").unwrap();
        assert!(Arc::ptr_eq(&lower, &upper));
    }
}
