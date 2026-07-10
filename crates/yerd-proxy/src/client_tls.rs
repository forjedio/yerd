//! Client-side TLS configuration for forwarding to `https://` upstreams.
//!
//! [`ProxyClientTls`] bundles two prebuilt `rustls::ClientConfig`s: a **public**
//! verifier (trust anchors supplied by the daemon, which owns `webpki-roots` -
//! this crate must not) and a **local no-verify** verifier for
//! loopback/private/`.test` upstreams, where self-signed dev backends are the
//! norm. [`ProxyClientTls::config_for`] picks between them by
//! [`yerd_core::UpstreamTarget::is_local`].
//!
//! Both configs are built with an explicit `ring` provider
//! ([`rustls::ClientConfig::builder_with_provider`]) so they never depend on the
//! process-default `CryptoProvider` install order (which, in the daemon, only
//! happens later, inside the spawned proxy task).

use std::sync::Arc;

use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::ring::default_provider;
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{ClientConfig, DigitallySignedStruct, Error as TlsError, SignatureScheme};
use yerd_core::UpstreamTarget;

/// Client TLS policy for reverse-proxy upstreams.
#[derive(Debug, Clone)]
pub struct ProxyClientTls {
    local: Arc<ClientConfig>,
    public: Arc<ClientConfig>,
}

impl ProxyClientTls {
    /// Bundles a `local` no-verify config (see [`Self::no_verify_config`]) and a
    /// daemon-supplied `public` config whose trust anchors verify genuine public
    /// hosts.
    #[must_use]
    pub fn new(local: Arc<ClientConfig>, public: Arc<ClientConfig>) -> Self {
        Self { local, public }
    }

    /// The client config to use for `target` under `tld`: the no-verify config
    /// for a local host, the public verifier otherwise.
    #[must_use]
    pub fn config_for(&self, target: &UpstreamTarget, tld: &str) -> Arc<ClientConfig> {
        if target.is_local(tld) {
            self.local.clone()
        } else {
            self.public.clone()
        }
    }

    /// A rustls client config that accepts any server certificate. Only ever
    /// applied to local upstreams by [`Self::config_for`]. Errors only on an
    /// inconsistent crypto provider (never for `ring`).
    pub fn no_verify_config() -> Result<Arc<ClientConfig>, TlsError> {
        let cfg = ClientConfig::builder_with_provider(Arc::new(default_provider()))
            .with_safe_default_protocol_versions()?
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(NoVerify))
            .with_no_client_auth();
        Ok(Arc::new(cfg))
    }
}

/// A `ServerCertVerifier` that accepts any certificate. Scoped to local
/// upstreams only. Must implement the signature-verification methods (not just
/// `verify_server_cert`), or rustls 0.23 fails the handshake regardless.
#[derive(Debug)]
struct NoVerify;

impl ServerCertVerifier for NoVerify {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, TlsError> {
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, TlsError> {
        Ok(HandshakeSignatureValid::assertion())
    }

    fn verify_tls13_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, TlsError> {
        Ok(HandshakeSignatureValid::assertion())
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        default_provider()
            .signature_verification_algorithms
            .supported_schemes()
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

    #[test]
    fn no_verify_config_builds() {
        assert!(ProxyClientTls::no_verify_config().is_ok());
    }

    #[test]
    fn config_for_picks_local_vs_public() {
        let local = ProxyClientTls::no_verify_config().unwrap();
        let public = ProxyClientTls::no_verify_config().unwrap();
        let tls = ProxyClientTls::new(local.clone(), public.clone());
        let local_target = UpstreamTarget::from_url_str("https://127.0.0.1:8443").unwrap();
        let public_target = UpstreamTarget::from_url_str("https://api.example.com").unwrap();
        assert!(Arc::ptr_eq(&tls.config_for(&local_target, "test"), &local));
        assert!(Arc::ptr_eq(
            &tls.config_for(&public_target, "test"),
            &public
        ));
    }
}
