//! One-click, pre-authenticated `WordPress` admin login ("WP Admin" site
//! action, opt-in per site via `Site::wp_auto_login`).
//! `LoginTokenRegistry` mints a short-TTL, single-use token per
//! `Request::MintWordpressLoginToken`; `yerd-proxy` (via the
//! [`yerd_proxy::LoginTokenConsumer`] trait, so the proxy crate never depends
//! on this concrete type) consumes it the moment it's presented on a
//! `/wp-admin` request for the same site, then adds a per-request
//! `auto_prepend_file` FastCGI param pointing at the `WordPress` bootstrap
//! script this module also writes out at daemon startup - see
//! [`write_prepend_script`] - plus a `YERD_LOGIN_USER` param carrying the
//! target admin's username, resolved once at mint time (see
//! [`mint_wordpress_login_token`]) since `yerd-proxy` has no daemon-config
//! access.

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
/// site), so `consume` is a single locked lookup-and-remove. The stored
/// target-user string is `""` for "no preference" (fall back to the
/// earliest-created administrator).
pub struct LoginTokenRegistry {
    inner: Mutex<HashMap<String, (String, String, Instant)>>,
}

impl LoginTokenRegistry {
    /// An empty registry - no tokens minted yet.
    #[must_use]
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(HashMap::new()),
        }
    }

    /// Mint a new token valid for `site` until [`TOKEN_TTL`] elapses, resolved
    /// to sign in as `target_user` (`None`/`""` = no preference). Also sweeps
    /// out any already-expired entries, so a steady trickle of abandoned
    /// (never-presented) tokens doesn't grow the map unboundedly.
    #[allow(clippy::missing_panics_doc)]
    pub fn mint(&self, site: &str, target_user: Option<&str>) -> String {
        let mut bytes = [0u8; TOKEN_BYTES];
        rand::thread_rng().fill_bytes(&mut bytes);
        let token = hex::encode(bytes);

        let mut guard = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let now = Instant::now();
        guard.retain(|_, (_, _, expires_at)| *expires_at > now);
        guard.insert(
            token.clone(),
            (
                site.to_owned(),
                target_user.unwrap_or("").to_owned(),
                now + TOKEN_TTL,
            ),
        );
        token
    }
}

impl Default for LoginTokenRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl yerd_proxy::LoginTokenConsumer for LoginTokenRegistry {
    fn consume(&self, site: &str, token: &str) -> Option<String> {
        let mut guard = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let (stored_site, target_user, expires_at) = guard.remove(token)?;
        (expires_at > Instant::now() && stored_site == site).then_some(target_user)
    }
}

/// The `WordPress` auto-login bootstrap script, compiled into the binary.
/// Only ever loaded via a per-request `auto_prepend_file` FastCGI override
/// added by `yerd-proxy` for a single already-token-validated request - never
/// written into any site's own files, never reachable on an ordinary request.
pub const PREPEND_SCRIPT: &str = include_str!("../assets/wordpress_autologin_prepend.php");

/// File name the auto-login prepend script is written to under `{data}`.
pub const PREPEND_SCRIPT_FILE: &str = "wordpress-autologin-prepend.php";

/// The `WordPress` public-URL override for tunnelled requests, compiled into the
/// binary. Only ever loaded via the tunnel listener's per-request
/// `auto_prepend_file` override, and gated on the `YERD_PUBLIC_URL` param that
/// only tunnelled requests carry.
pub const PUBLIC_URL_PREPEND_SCRIPT: &str =
    include_str!("../assets/wordpress_public_url_prepend.php");

/// File name the public-URL prepend script is written to under `{data}`.
pub const PUBLIC_URL_PREPEND_SCRIPT_FILE: &str = "wordpress-public-url-prepend.php";

/// Write a prepend script out to `data_dir/file_name`, overwriting any previous
/// copy (so an updated yerd always refreshes it). Returns the path on success;
/// failures are logged and treated as "this prepend is unavailable this boot"
/// rather than fatal - for the auto-login script the ordinary,
/// non-authenticated `/wp-admin/` link still works, and for the public-URL
/// script a tunnelled `WordPress` site simply keeps its stored URLs.
pub fn write_prepend_script(
    data_dir: &std::path::Path,
    file_name: &str,
    contents: &str,
) -> Option<std::path::PathBuf> {
    let path = data_dir.join(file_name);
    match std::fs::write(&path, contents) {
        Ok(()) => Some(path),
        Err(e) => {
            tracing::warn!(
                error = %e,
                path = %path.display(),
                "couldn't write a WordPress prepend script; the feature it backs is unavailable this boot"
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
///
/// Refuses to mint when [`DaemonState::wordpress_login_prepend_script`] is
/// `None` (the prepend script failed to write at startup - see
/// [`write_prepend_script`]): `yerd-proxy` only adds `auto_prepend_file` when
/// both a consumed token *and* that path are present, so minting anyway
/// would burn a token on presentation without ever logging the user in.
pub async fn mint_wordpress_login_token(site: &str, state: &DaemonState) -> Response {
    let (auto_login, target_user) = {
        let guard = state.router.read().await;
        match guard.get(site) {
            Some(s) => (s.wp_auto_login(), s.wp_auto_login_user().map(str::to_owned)),
            None => {
                return Response::Error {
                    code: ErrorCode::NotFound,
                    message: format!("no site named \"{site}\""),
                }
            }
        }
    };
    let is_wordpress = state
        .wordpress_sites
        .read()
        .await
        .get(site)
        .copied()
        .unwrap_or(false);
    if !is_wordpress {
        return Response::Error {
            code: ErrorCode::NotFound,
            message: format!("\"{site}\" is not a WordPress site"),
        };
    }
    if !auto_login {
        return Response::Error {
            code: ErrorCode::NotFound,
            message: format!("WordPress auto-login is not enabled for \"{site}\""),
        };
    }
    if state.wordpress_login_prepend_script.is_none() {
        return Response::Error {
            code: ErrorCode::NotFound,
            message: "WordPress one-click login is unavailable this boot".to_owned(),
        };
    }
    Response::WordpressLoginToken {
        token: state
            .wordpress_login_tokens
            .mint(site, target_user.as_deref()),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};

    use yerd_core::{PhpVersion, Site};
    use yerd_proxy::LoginTokenConsumer;

    /// [`crate::test_support::state_in`] with a prepend script configured,
    /// so [`mint_wordpress_login_token`]'s prepend-script-availability gate
    /// doesn't reject every test by default - tests exercising that specific
    /// gate override the field back to `None` themselves.
    fn state_in(tmp: &Path) -> DaemonState {
        let mut state = crate::test_support::state_in(tmp);
        state.wordpress_login_prepend_script = Some(PathBuf::from("/opt/yerd/prepend.php"));
        state
    }

    /// Inserts `name` into `state`'s router (auto-login set per
    /// `auto_login`/`auto_login_user`) and marks it `WordPress` in
    /// `state.wordpress_sites`, so [`mint_wordpress_login_token`] can resolve
    /// it past both its site-lookup and `is_wordpress` gates.
    async fn insert_wordpress_site(
        state: &DaemonState,
        name: &str,
        auto_login: bool,
        auto_login_user: Option<&str>,
    ) {
        let mut site = Site::linked(name, "/srv/www/blog", PhpVersion::new(8, 3)).unwrap();
        site.set_wp_auto_login(auto_login);
        site.set_wp_auto_login_user(auto_login_user.map(str::to_owned));
        state.router.write().await.insert(site).unwrap();
        state
            .wordpress_sites
            .write()
            .await
            .insert(name.to_owned(), true);
    }

    #[tokio::test]
    async fn mint_login_token_not_found_for_unknown_site() {
        let tmp = tempfile::tempdir().unwrap();
        let state = state_in(tmp.path());
        let resp = mint_wordpress_login_token("blog", &state).await;
        assert!(matches!(
            resp,
            Response::Error {
                code: ErrorCode::NotFound,
                ..
            }
        ));
    }

    #[tokio::test]
    async fn mint_login_token_not_found_when_not_wordpress() {
        let tmp = tempfile::tempdir().unwrap();
        let state = state_in(tmp.path());
        let mut site = Site::linked("blog", "/srv/www/blog", PhpVersion::new(8, 3)).unwrap();
        site.set_wp_auto_login(true);
        state.router.write().await.insert(site).unwrap();

        let resp = mint_wordpress_login_token("blog", &state).await;
        assert!(matches!(
            resp,
            Response::Error {
                code: ErrorCode::NotFound,
                ..
            }
        ));
    }

    #[tokio::test]
    async fn mint_login_token_not_found_when_auto_login_disabled() {
        let tmp = tempfile::tempdir().unwrap();
        let state = state_in(tmp.path());
        insert_wordpress_site(&state, "blog", false, None).await;

        let resp = mint_wordpress_login_token("blog", &state).await;
        assert!(matches!(
            resp,
            Response::Error {
                code: ErrorCode::NotFound,
                ..
            }
        ));
    }

    #[tokio::test]
    async fn mint_login_token_not_found_when_prepend_script_unavailable() {
        let tmp = tempfile::tempdir().unwrap();
        let mut state = state_in(tmp.path());
        state.wordpress_login_prepend_script = None;
        insert_wordpress_site(&state, "blog", true, None).await;

        let resp = mint_wordpress_login_token("blog", &state).await;
        assert!(matches!(
            resp,
            Response::Error {
                code: ErrorCode::NotFound,
                ..
            }
        ));
    }

    #[tokio::test]
    async fn mint_login_token_succeeds_and_token_resolves_target_user() {
        let tmp = tempfile::tempdir().unwrap();
        let state = state_in(tmp.path());
        insert_wordpress_site(&state, "blog", true, Some("editor")).await;

        let resp = mint_wordpress_login_token("blog", &state).await;
        let Response::WordpressLoginToken { token } = resp else {
            panic!("expected WordpressLoginToken, got {resp:?}");
        };
        assert_eq!(
            state
                .wordpress_login_tokens
                .consume("blog", &token)
                .as_deref(),
            Some("editor")
        );
    }

    #[test]
    fn mint_then_consume_succeeds_once() {
        let reg = LoginTokenRegistry::new();
        let token = reg.mint("blog", None);
        assert_eq!(reg.consume("blog", &token), Some(String::new()));
        assert_eq!(
            reg.consume("blog", &token),
            None,
            "a consumed token must not be usable again"
        );
    }

    #[test]
    fn mint_with_target_user_carries_it_through_consume() {
        let reg = LoginTokenRegistry::new();
        let token = reg.mint("blog", Some("editor"));
        assert_eq!(reg.consume("blog", &token).as_deref(), Some("editor"));
    }

    #[test]
    fn consume_rejects_wrong_site() {
        let reg = LoginTokenRegistry::new();
        let token = reg.mint("blog", None);
        assert_eq!(reg.consume("other-site", &token), None);
        // Wrong-site presentation still consumes it - it must not remain
        // valid for a later, correct-site request either.
        assert_eq!(reg.consume("blog", &token), None);
    }

    #[test]
    fn consume_rejects_unknown_token() {
        let reg = LoginTokenRegistry::new();
        assert_eq!(reg.consume("blog", "never-minted"), None);
    }

    #[test]
    fn mint_produces_distinct_tokens() {
        let reg = LoginTokenRegistry::new();
        let a = reg.mint("blog", None);
        let b = reg.mint("blog", None);
        assert_ne!(a, b);
    }

    #[test]
    fn expired_token_is_rejected() {
        let reg = LoginTokenRegistry::new();
        let token = reg.mint("blog", None);
        {
            let mut guard = reg.inner.lock().unwrap();
            let (site, target_user, _) = guard.get(&token).unwrap().clone();
            guard.insert(
                token.clone(),
                (
                    site,
                    target_user,
                    Instant::now().checked_sub(Duration::from_secs(1)).unwrap(),
                ),
            );
        }
        assert_eq!(reg.consume("blog", &token), None);
    }

    #[test]
    fn write_prepend_script_writes_and_overwrites_both_scripts() {
        let tmp = tempfile::tempdir().unwrap();
        for (file_name, contents) in [
            (PREPEND_SCRIPT_FILE, PREPEND_SCRIPT),
            (PUBLIC_URL_PREPEND_SCRIPT_FILE, PUBLIC_URL_PREPEND_SCRIPT),
        ] {
            let path = tmp.path().join(file_name);
            std::fs::write(&path, "stale").unwrap();

            let written = write_prepend_script(tmp.path(), file_name, contents).unwrap();

            assert_eq!(written, path);
            assert_eq!(std::fs::read_to_string(&path).unwrap(), contents);
        }
    }

    #[test]
    fn write_prepend_script_failure_returns_none() {
        let tmp = tempfile::tempdir().unwrap();
        let missing_dir = tmp.path().join("missing");
        assert!(write_prepend_script(&missing_dir, PREPEND_SCRIPT_FILE, PREPEND_SCRIPT).is_none());
    }

    #[test]
    fn public_url_script_gates_on_yerd_public_url_first() {
        let first_statement = PUBLIC_URL_PREPEND_SCRIPT
            .lines()
            .map(str::trim)
            .find(|l| !l.is_empty() && *l != "<?php")
            .unwrap();
        assert_eq!(
            first_statement,
            "if (!isset($_SERVER['YERD_PUBLIC_URL'])) {"
        );
    }

    #[test]
    fn public_url_script_overrides_and_protects_home_and_siteurl() {
        for hook in [
            "'option_home'",
            "'option_siteurl'",
            "'pre_update_option_home'",
            "'pre_update_option_siteurl'",
            "PHP_INT_MAX",
            "'accepted_args' => 2",
            "PHP_URL_PATH",
        ] {
            assert!(PUBLIC_URL_PREPEND_SCRIPT.contains(hook), "missing {hook}");
        }
    }

    #[test]
    fn mint_sweeps_expired_entries() {
        let reg = LoginTokenRegistry::new();
        let stale = reg.mint("blog", None);
        {
            let mut guard = reg.inner.lock().unwrap();
            let (site, target_user, _) = guard.get(&stale).unwrap().clone();
            guard.insert(
                stale.clone(),
                (
                    site,
                    target_user,
                    Instant::now().checked_sub(Duration::from_secs(1)).unwrap(),
                ),
            );
        }
        reg.mint("blog", None);
        let guard = reg.inner.lock().unwrap();
        assert!(
            !guard.contains_key(&stale),
            "an expired entry must be swept out by the next mint"
        );
    }
}
