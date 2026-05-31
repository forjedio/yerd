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
//! of sites — not project size — and `vendor/`/`node_modules/` churn never
//! reaches a parent watch. The set is:
//! - every **parked root** (to notice child sites appearing/disappearing), plus
//! - the project root of every **unresolved** parked site (web root not detected
//!   yet) — so a project cloned in is picked up.
//!
//! A site is dropped from the watch set once it resolves (a framework/web root
//! was found) or is manually overridden — "don't watch what we already know".
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
//! it never races a concurrent mutation, and it **never writes config** — so it
//! cannot feed back into its own fs events.

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
    // notify runs the event handler on its own thread; bridge to async with an
    // unbounded channel. We only need a wake signal, not the event payload.
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<()>();
    let mut debouncer = match new_debouncer(DEBOUNCE, move |res: DebounceEventResult| {
        // Any successful batch is a wake; on error we still wake to re-sync.
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
    // Seed the watch set from the current routing (also fixes up the router if
    // the disk changed between startup's scan and now). `reconcile` is
    // synchronous and runs *after* the await, so the non-`Send` watcher handle
    // is never held across an `.await` (which would make this task un-spawnable).
    if let Some(desired) = recompute(&state).await {
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
        if let Some(desired) = recompute(&state).await {
            reconcile(debouncer.watcher(), &mut watched, &desired);
        }
    }
}

/// Rebuild the router from the current config and return the desired watch set
/// (every parked root + every unresolved project root). Returns `None` if the
/// rebuild failed (the previous router is left in place).
async fn recompute(state: &DaemonState) -> Option<HashSet<PathBuf>> {
    // Hold the config lock across the rebuild + router swap so we serialise with
    // the IPC mutation path (same config → router lock order). The detection
    // cache keeps the rescan cheap.
    let guard = state.config.lock().await;
    let (router, watch_roots) = match startup::build_routing(
        &guard,
        &state.dirs,
        &state.detect_cache,
    ) {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!(error = %e, "watcher router rebuild failed; keeping previous routing");
            return None;
        }
    };
    *state.router.write().await = router;

    let mut desired: HashSet<PathBuf> = watch_roots.into_iter().collect();
    for p in &guard.parked.paths {
        desired.insert(PathBuf::from(p));
    }
    Some(desired)
}

/// Add watches for newly-desired paths and drop watches for ones no longer
/// wanted. Watch/unwatch failures are logged at debug and otherwise ignored —
/// e.g. a parked root that doesn't exist on disk simply isn't watched (and is
/// retried on the next reconcile).
fn reconcile(
    watcher: &mut dyn Watcher,
    tracked: &mut HashSet<PathBuf>,
    desired: &HashSet<PathBuf>,
) {
    // Remove stale watches.
    let stale: Vec<PathBuf> = tracked.difference(desired).cloned().collect();
    for path in stale {
        if let Err(e) = watcher.unwatch(&path) {
            tracing::debug!(path = %path.display(), error = %e, "unwatch failed");
        }
        tracked.remove(&path);
    }
    // Add new watches (non-recursive).
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
