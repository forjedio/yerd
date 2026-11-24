//! Filesystem watcher: keeps the live router fresh as projects appear or change.
//!
//! Detection runs at scan time (startup + every mutation), but a project cloned
//! into a parked folder *after* the last scan would otherwise serve from the
//! wrong directory until the next mutation or restart. This task watches the
//! relevant directories and rebuilds the router on change.
//!
//! ## What we watch (and what we don't)
//!
//! Watches are **non-recursive**, so the descriptor count scales with the number
//! of sites - not project size - and `vendor/`/`node_modules/` churn never
//! reaches a parent watch. The set is:
//! - every **parked root** (to notice child sites appearing/disappearing), plus
//! - the project root of every **unresolved** parked site (web root not detected
//!   yet) - so a project cloned in is picked up.
//!
//! A site is dropped from the watch set once it resolves (a framework/web root
//! was found) or is manually overridden - "don't watch what we already know".
//! The trade-off: deleting a resolved site's web root in place is not noticed
//! until the next scan from another trigger (a mutation, a sibling change, or a
//! restart).
//!
//! ## Wake sources
//!
//! - A debounced batch of filesystem events (project changed on disk).
//! - [`DaemonState::watch_dirty`], pinged after a mutation commits, so a freshly
//!   parked root is watched without waiting for an unrelated fs event.
//!
//! On either wake the task rebuilds the router from the *current* config under
//! the config lock (same lock order as the mutation path: config → router), so
//! it never races a concurrent mutation. It saves config only when the parked
//! name claims or a renamed site's settings changed (or an earlier save failed);
//! the config file lives outside the watched directories, so a save never
//! feeds back into its own fs events.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use notify_debouncer_mini::notify::{RecursiveMode, Watcher};
use notify_debouncer_mini::{new_debouncer, DebounceEventResult};
use tokio::sync::watch;

use crate::startup;
use crate::state::DaemonState;

/// Debounce window for coalescing a burst of filesystem events (e.g. the many
/// writes of a `git clone`) into a single rebuild.
const DEBOUNCE: Duration = Duration::from_millis(500);

/// Run the watcher until shutdown. Best-effort: if the OS watcher can't be
/// created the task logs and exits, leaving on-demand rescans (mutations,
/// restart) as the freshness path.
pub async fn run(state: Arc<DaemonState>, mut shutdown_rx: watch::Receiver<bool>) {
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<()>();
    let mut debouncer = match new_debouncer(DEBOUNCE, move |res: DebounceEventResult| {
        let _ = res;
        let _ = tx.send(());
    }) {
        Ok(d) => d,
        Err(e) => {
            tracing::warn!(error = %e, "filesystem watcher unavailable; sites refresh on mutation/restart only");
            return;
        }
    };

    let mut watched: HashSet<PathBuf> = HashSet::new();
    let mut unsaved = false;
    if let Some(desired) = recompute(&state, &mut unsaved).await {
        reconcile(debouncer.watcher(), &mut watched, &desired);
    }

    loop {
        let wake = tokio::select! {
            _ = rx.recv() => true,
            () = state.watch_dirty.notified() => true,
            _ = shutdown_rx.changed() => false,
        };
        if !wake {
            break;
        }
        if let Some(desired) = recompute(&state, &mut unsaved).await {
            reconcile(debouncer.watcher(), &mut watched, &desired);
        }
    }
}

/// Rebuild the router from the current config and return the desired watch set
/// (every parked root + every unresolved project root). A parked site renamed
/// by the rebuild (see [`startup::live_parked_renames`]) has its name-keyed
/// settings moved to the new name and its app-side URL synced, and the parked
/// name claims are brought in line with the new router. Config is saved when
/// either changed, or when an earlier save failed (`unsaved`, cleared only by a
/// successful save). The tunnel route map is then rebuilt (after the config
/// lock is released), so a shared site that just appeared becomes routable.
/// Returns `None` if the rebuild failed (the previous router is left in place).
async fn recompute(state: &DaemonState, unsaved: &mut bool) -> Option<HashSet<PathBuf>> {
    let mut guard = state.config.lock().await;
    let rebuild = |cfg: &yerd_config::Config| {
        startup::build_routing(cfg, &state.dirs, &state.detect_cache).map_err(|e| {
            tracing::warn!(error = %e, "watcher router rebuild failed; keeping previous routing");
        })
    };
    let mut routing = rebuild(&guard).ok()?;
    let renames = startup::live_parked_renames(
        &*state.router.read().await,
        &routing.0,
        &guard.parked.claims,
    );
    let mut next = guard.clone();
    let moved = startup::rekey_parked_names(&mut next, &renames);
    if moved {
        routing = rebuild(&next).ok()?;
    }
    let pending: HashSet<&str> = renames.iter().map(|r| r.doc_root.as_str()).collect();
    let claimed = startup::reconcile_parked_claims(&mut next, &routing.0, &pending);
    if moved || claimed {
        *guard = next;
        *unsaved = true;
    }
    if *unsaved {
        match guard.save(&state.config_path) {
            Ok(()) => *unsaved = false,
            Err(e) => {
                tracing::warn!(error = %e, "could not persist parked site names; retrying on the next rescan");
            }
        }
    }
    let (router, wordpress_sites, laravel_sites, framework_sites, watch_roots) = routing;
    *state.router.write().await = router;
    *state.wordpress_sites.write().await = wordpress_sites;
    *state.laravel_sites.write().await = laravel_sites;
    *state.framework_sites.write().await = framework_sites;

    let mut desired: HashSet<PathBuf> = watch_roots.into_iter().collect();
    for p in &guard.parked.paths {
        desired.insert(PathBuf::from(p));
    }
    drop(guard);
    crate::tunnel::rebuild_routes(state).await;
    startup::sync_renamed_site_urls(state, &startup::renamed_sites(&renames)).await;
    Some(desired)
}

/// Add watches for newly-desired paths and drop watches for ones no longer
/// wanted. Watch/unwatch failures are logged at debug and otherwise ignored -
/// e.g. a parked root that doesn't exist on disk simply isn't watched (and is
/// retried on the next reconcile).
fn reconcile(
    watcher: &mut dyn Watcher,
    tracked: &mut HashSet<PathBuf>,
    desired: &HashSet<PathBuf>,
) {
    let stale: Vec<PathBuf> = tracked.difference(desired).cloned().collect();
    for path in stale {
        if let Err(e) = watcher.unwatch(&path) {
            tracing::debug!(path = %path.display(), error = %e, "unwatch failed");
        }
        tracked.remove(&path);
    }
    let additions: Vec<PathBuf> = desired.difference(tracked).cloned().collect();
    for path in additions {
        match watcher.watch(&path, RecursiveMode::NonRecursive) {
            Ok(()) => {
                tracked.insert(path);
            }
            Err(e) => {
                tracing::debug!(path = %path.display(), error = %e, "watch failed (will retry)");
            }
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::test_support::state_in;

    #[tokio::test]
    async fn recompute_routes_a_shared_site_that_appears_in_a_parked_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let parked = tmp.path().join("Sites");
        std::fs::create_dir_all(&parked).unwrap();
        let state = state_in(tmp.path());
        {
            let mut cfg = state.config.lock().await;
            cfg.parked
                .paths
                .insert(parked.to_string_lossy().into_owned());
            cfg.tunnel
                .sites
                .insert("blog".into(), "blog.example.com".into());
        }
        recompute(&state, &mut false).await.unwrap();
        assert!(state.tunnel_routes.read().await.is_empty());

        std::fs::create_dir_all(parked.join("blog")).unwrap();
        recompute(&state, &mut false).await.unwrap();

        assert_eq!(
            state
                .tunnel_routes
                .read()
                .await
                .get("blog.example.com")
                .map(|r| r.site.as_str()),
            Some("blog")
        );
    }

    #[tokio::test]
    async fn recompute_moves_settings_when_a_contested_name_frees_up() {
        let tmp = tempfile::tempdir().unwrap();
        let parked = tmp.path().join("Sites");
        std::fs::create_dir_all(parked.join("laravel.com")).unwrap();
        std::fs::create_dir_all(parked.join("laravel.app")).unwrap();
        let state = state_in(tmp.path());
        {
            let mut cfg = state.config.lock().await;
            cfg.parked
                .paths
                .insert(parked.to_string_lossy().into_owned());
            cfg.favourites.sites.insert("laravel-com".to_owned());
        }
        recompute(&state, &mut false).await.unwrap();
        assert!(state.router.read().await.get("laravel-com").is_some());

        std::fs::remove_dir(parked.join("laravel.app")).unwrap();
        recompute(&state, &mut false).await.unwrap();

        assert!(state.router.read().await.get("laravel").is_some());
        let cfg = state.config.lock().await;
        assert!(cfg.favourites.sites.contains("laravel"));
        assert!(!cfg.favourites.sites.contains("laravel-com"));
        let root = parked.join("laravel.com").to_string_lossy().into_owned();
        let claim = cfg.parked.claims.get(&root).unwrap();
        assert_eq!(claim.name, "laravel");
        assert!(claim.url_synced);
    }

    #[tokio::test]
    async fn recompute_gives_a_new_folder_no_settings() {
        let tmp = tempfile::tempdir().unwrap();
        let parked = tmp.path().join("Sites");
        std::fs::create_dir_all(&parked).unwrap();
        let state = state_in(tmp.path());
        {
            let mut cfg = state.config.lock().await;
            cfg.parked
                .paths
                .insert(parked.to_string_lossy().into_owned());
            cfg.favourites.sites.insert("laravel-com".to_owned());
        }
        recompute(&state, &mut false).await.unwrap();

        std::fs::create_dir_all(parked.join("laravel.com")).unwrap();
        recompute(&state, &mut false).await.unwrap();

        let cfg = state.config.lock().await;
        assert!(cfg.favourites.sites.contains("laravel-com"));
        assert!(!cfg.favourites.sites.contains("laravel"));
    }
}
