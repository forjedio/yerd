//! `BackendResolver` impl driving `yerd_php::PhpManager::ensure`.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::{Mutex, RwLock};

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
    /// Mirrors [`crate::state::DaemonState::wordpress_sites`] - supplies the
    /// runtime `is_wordpress` fact to [`yerd_core::Site::uses_front_controller`]
    /// so a site's front-controller default is resolved correctly (WordPress,
    /// any layout, defaults to direct script execution).
    pub wordpress_sites: Arc<RwLock<HashMap<String, bool>>>,
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

    async fn allows_direct_script_execution(&self, site: &yerd_core::Site) -> bool {
        let is_wordpress = self
            .wordpress_sites
            .read()
            .await
            .get(site.name())
            .copied()
            .unwrap_or(false);
        !site.uses_front_controller(is_wordpress)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use yerd_core::{PhpVersion, Site};

    fn resolver(wordpress_sites: HashMap<String, bool>) -> DaemonBackendResolver {
        let tmp = tempfile::tempdir().unwrap();
        let state = crate::test_support::state_in(tmp.path());
        DaemonBackendResolver {
            php_manager: state.php_manager.clone(),
            wordpress_sites: Arc::new(RwLock::new(wordpress_sites)),
        }
    }

    fn site(name: &str, subpath: &str, front_controller: Option<bool>) -> Site {
        let mut s = Site::parked(name, "/srv/site", PhpVersion::new(8, 3)).unwrap();
        s.set_web_subpath(subpath);
        s.set_front_controller(front_controller);
        s
    }

    #[tokio::test]
    async fn direct_execution_follows_detection_and_wordpress_fact() {
        // Framework in a subdir: front controller -> no direct execution.
        let r = resolver(HashMap::new());
        assert!(
            !r.allows_direct_script_execution(&site("app", "public", None))
                .await
        );
        // Plain root-served site: direct execution.
        assert!(
            r.allows_direct_script_execution(&site("plain", "", None))
                .await
        );

        // WordPress served from a subdir stays direct (is_wordpress wins).
        let wp = resolver(HashMap::from([("blog".to_owned(), true)]));
        assert!(
            wp.allows_direct_script_execution(&site("blog", "web", None))
                .await
        );
    }

    #[tokio::test]
    async fn explicit_override_wins() {
        let r = resolver(HashMap::new());
        // Force front-controller on an otherwise-direct plain site.
        assert!(
            !r.allows_direct_script_execution(&site("a", "", Some(true)))
                .await
        );
        // Force direct execution on an otherwise-funnelled framework site.
        assert!(
            r.allows_direct_script_execution(&site("b", "public", Some(false)))
                .await
        );
    }
}
