//! One-click, pre-authenticated `WordPress` admin login ("WP Admin" site
//! action). `LoginTokenRegistry` mints a short-TTL, single-use token per
//! `Request::MintWordpressLoginToken`; `yerd-proxy` (via the
//! [`yerd_proxy::LoginTokenConsumer`] trait, so the proxy crate never depends
//! on this concrete type) consumes it the moment it's presented on a
//! `/wp-admin` request for the same site, then adds a per-request
//! `auto_prepend_file` FastCGI param pointing at the `WordPress` bootstrap
//! script this module also writes out at daemon startup - see
//! [`write_prepend_script`].

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use rand::RngCore;
use yerd_ipc::{ErrorCode, Response};

use crate::state::DaemonState;

/// How long a minted token stays valid if never presented.
const TOKEN_TTL: Duration = Duration::from_secs(30);

/// Random token length in bytes (before hex-encoding, so the wire string is
/// twice this many hex characters) - enough entropy that guessing a live
/// token is infeasible within its 30s window.
const TOKEN_BYTES: usize = 32;

/// In-memory single-use token store. Keyed by the token itself (not the
/// site), so `consume` is a single locked lookup-and-remove.
pub struct LoginTokenRegistry {
    inner: Mutex<HashMap<String, (String, Instant)>>,
}

impl LoginTokenRegistry {
    /// An empty registry - no tokens minted yet.
    #[must_use]
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(HashMap::new()),
        }
    }

    /// Mint a new token valid for `site` until [`TOKEN_TTL`] elapses. Also
    /// sweeps out any already-expired entries, so a steady trickle of
    /// abandoned (never-presented) tokens doesn't grow the map unboundedly.
    #[allow(clippy::missing_panics_doc)]
    pub fn mint(&self, site: &str) -> String {
        let mut bytes = [0u8; TOKEN_BYTES];
        rand::thread_rng().fill_bytes(&mut bytes);
        let token = hex::encode(bytes);

        let mut guard = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let now = Instant::now();
        guard.retain(|_, (_, expires_at)| *expires_at > now);
        guard.insert(token.clone(), (site.to_owned(), now + TOKEN_TTL));
        token
    }
}

impl Default for LoginTokenRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl yerd_proxy::LoginTokenConsumer for LoginTokenRegistry {
    fn consume(&self, site: &str, token: &str) -> bool {
        let Ok(mut guard) = self.inner.lock() else {
            return false;
        };
        let Some((stored_site, expires_at)) = guard.remove(token) else {
            return false;
        };
        expires_at > Instant::now() && stored_site == site
    }
}

/// The `WordPress` auto-login bootstrap script, compiled into the binary.
/// Only ever loaded via a per-request `auto_prepend_file` FastCGI override
/// added by `yerd-proxy` for a single already-token-validated request - never
/// written into any site's own files, never reachable on an ordinary request.
const PREPEND_SCRIPT: &str = include_str!("../assets/wordpress_autologin_prepend.php");

/// Write the auto-login prepend script out to a stable path under `data_dir`,
/// overwriting any previous copy (so an updated yerd always refreshes it).
/// Returns the path on success; failures are logged and treated as "one-click
/// login is unavailable this boot" rather than fatal - the ordinary,
/// non-authenticated `/wp-admin/` link still works either way.
pub fn write_prepend_script(data_dir: &std::path::Path) -> Option<std::path::PathBuf> {
    let path = data_dir.join("wordpress-autologin-prepend.php");
    match std::fs::write(&path, PREPEND_SCRIPT) {
        Ok(()) => Some(path),
        Err(e) => {
            tracing::warn!(
                error = %e,
                path = %path.display(),
                "couldn't write the WordPress auto-login prepend script; one-click login is unavailable this boot"
            );
            None
        }
    }
}

/// `Request::MintWordpressLoginToken` handler. Confirms `site` exists and is
/// `WordPress` (the same narrow marker-file check `ListSites` uses for its
/// badge), then mints a token. Follows `ListSites`'s own lock discipline:
/// clone what's needed out from under the router's read guard, drop it,
/// *then* do the blocking filesystem check off the async executor -
/// `wordpress_detect::detect`'s doc comment requires this, and holding the
/// read guard across that I/O would block every writer for its duration.
pub async fn mint_wordpress_login_token(site: &str, state: &DaemonState) -> Response {
    let served_root = {
        let guard = state.router.read().await;
        match guard.get(site) {
            Some(s) => s.served_root(),
            None => {
                return Response::Error {
                    code: ErrorCode::NotFound,
                    message: format!("no site named \"{site}\""),
                }
            }
        }
    };
    let is_wordpress =
        tokio::task::spawn_blocking(move || crate::wordpress_detect::detect(&served_root).0)
            .await
            .unwrap_or(false);
    if !is_wordpress {
        return Response::Error {
            code: ErrorCode::NotFound,
            message: format!("\"{site}\" is not a WordPress site"),
        };
    }
    Response::WordpressLoginToken {
        token: state.wordpress_login_tokens.mint(site),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use yerd_proxy::LoginTokenConsumer;

    #[test]
    fn mint_then_consume_succeeds_once() {
        let reg = LoginTokenRegistry::new();
        let token = reg.mint("blog");
        assert!(reg.consume("blog", &token));
        assert!(
            !reg.consume("blog", &token),
            "a consumed token must not be usable again"
        );
    }

    #[test]
    fn consume_rejects_wrong_site() {
        let reg = LoginTokenRegistry::new();
        let token = reg.mint("blog");
        assert!(!reg.consume("other-site", &token));
        // Wrong-site presentation still consumes it - it must not remain
        // valid for a later, correct-site request either.
        assert!(!reg.consume("blog", &token));
    }

    #[test]
    fn consume_rejects_unknown_token() {
        let reg = LoginTokenRegistry::new();
        assert!(!reg.consume("blog", "never-minted"));
    }

    #[test]
    fn mint_produces_distinct_tokens() {
        let reg = LoginTokenRegistry::new();
        let a = reg.mint("blog");
        let b = reg.mint("blog");
        assert_ne!(a, b);
    }

    #[test]
    fn expired_token_is_rejected() {
        let reg = LoginTokenRegistry::new();
        let token = reg.mint("blog");
        {
            let mut guard = reg.inner.lock().unwrap();
            let (site, _) = guard.get(&token).unwrap().clone();
            guard.insert(
                token.clone(),
                (
                    site,
                    Instant::now().checked_sub(Duration::from_secs(1)).unwrap(),
                ),
            );
        }
        assert!(!reg.consume("blog", &token));
    }

    #[test]
    fn mint_sweeps_expired_entries() {
        let reg = LoginTokenRegistry::new();
        let stale = reg.mint("blog");
        {
            let mut guard = reg.inner.lock().unwrap();
            let (site, _) = guard.get(&stale).unwrap().clone();
            guard.insert(
                stale.clone(),
                (
                    site,
                    Instant::now().checked_sub(Duration::from_secs(1)).unwrap(),
                ),
            );
        }
        reg.mint("blog");
        let guard = reg.inner.lock().unwrap();
        assert!(
            !guard.contains_key(&stale),
            "an expired entry must be swept out by the next mint"
        );
    }
}
