//! `BackendResolver` impl driving `yerd_php::PhpManager::ensure`.

use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::Mutex;

use yerd_php::{io::FastCgiProbe, PhpManager, SystemClock, TokioProcessSpawner};
use yerd_proxy::{Backend, BackendResolver, ProxyError};

/// Concrete `PhpManager` shape the daemon uses everywhere.
pub type DaemonPhpManager = PhpManager<TokioProcessSpawner, SystemClock, FastCgiProbe>;

/// Translates a routed `&Site` into a `Backend` by ensuring the matching
/// FPM pool is alive.
pub struct DaemonBackendResolver {
    /// Mutex-wrapped supervisor; the lock is held only for the duration
    /// of `ensure`, which has a fast-path for already-running pools.
    pub php_manager: Arc<Mutex<DaemonPhpManager>>,
}

#[async_trait]
impl BackendResolver for DaemonBackendResolver {
    async fn backend_for(&self, site: &yerd_core::Site) -> Result<Backend, ProxyError> {
        let listen = {
            let mut mgr = self.php_manager.lock().await;
            mgr.ensure(site.php())
                .await
                .map_err(|e| ProxyError::BackendResolver {
                    host: site.name().to_owned(),
                    source: Box::new(e),
                })?
        };
        match listen {
            yerd_php::Listen::UnixSocket(p) => Ok(Backend::PhpFpm { socket: p }),
            yerd_php::Listen::TcpLoopback(a) => Ok(Backend::PhpFpmTcp { addr: a }),
        }
    }
}
