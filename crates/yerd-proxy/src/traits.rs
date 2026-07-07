//! Trait seams injected by the daemon.
//!
//! `CertStore` is consulted per TLS handshake (synchronously, as rustls
//! requires). `BackendResolver` is consulted per request to map a routed
//! `&Site` to a concrete `Backend`. `LoginTokenConsumer` is consulted per
//! request to check a one-click `WordPress` login token. All three keep
//! `yerd-proxy` free of direct dependencies on `yerd-tls`, `yerd-php`, and
//! `yerdd`'s concrete daemon state.

use std::sync::Arc;

use async_trait::async_trait;

use crate::backend::Backend;
use crate::error::ProxyError;

/// SNI-keyed lookup of a rustls keypair.
///
/// Synchronous because rustls's [`rustls::server::ResolvesServerCert::resolve`]
/// is synchronous. Daemon impls are expected to hold the active cert
/// material in an in-memory map and refresh it out-of-band.
pub trait CertStore: std::fmt::Debug + Send + Sync + 'static {
    /// Return a usable [`rustls::sign::CertifiedKey`] for the given SNI
    /// host, or `None` to refuse the handshake.
    fn certified_key(&self, sni_host: &str) -> Option<Arc<rustls::sign::CertifiedKey>>;
}

/// Map a routed `&Site` to a concrete [`Backend`].
///
/// The daemon's impl typically calls
/// `yerd_php::PhpManager::ensure(site.php())` and translates the
/// returned `Listen` into a [`Backend`].
///
/// Implementer note: copy out the `Site` fields you need before any
/// `.await`, so the per-request closure doesn't have to hold the
/// `Arc<SiteRouter>` across an `.await` point.
#[async_trait]
pub trait BackendResolver: Send + Sync + 'static {
    /// Resolve. May return any [`ProxyError`] variant; the
    /// recommended one for foreign errors is
    /// [`ProxyError::BackendResolver`].
    async fn backend_for(&self, site: &yerd_core::Site) -> Result<Backend, ProxyError>;
}

/// Check and invalidate a one-click `WordPress` login token (the "WP Admin"
/// site action).
///
/// Implementer note: `consume` must check and invalidate atomically (e.g. a
/// single locked remove-and-compare), so a token can never be consumed twice
/// even under concurrent requests for the same token.
pub trait LoginTokenConsumer: Send + Sync + 'static {
    /// `true` if `token` is currently valid for `site` - unexpired, matching,
    /// and not already consumed. Always consumes the token (removes it from
    /// the pending set) regardless of whether it matched, so a token is never
    /// checked more than once.
    fn consume(&self, site: &str, token: &str) -> bool;
}
