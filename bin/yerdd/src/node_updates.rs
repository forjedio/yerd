//! Node update polling and the daemon's update cache.
//!
//! The cache (`DaemonState::node_updates`) maps each published major to its
//! newest installable patch at the last release-index fetch. It is refreshed by
//! [`poll_and_refresh`] (network, on the PHP update timer), by `AvailableNode`,
//! and by every Node install (from the index the installer already fetched).
//! [`cached_updates`] (no network) serves the per-release `update` on
//! `ListNode`, and [`recount`] keeps `DaemonState::node_update_count` (served on
//! `Status`) in step after any cache or install change.

use std::collections::BTreeMap;
use std::sync::atomic::Ordering;

use yerd_php::Downloader;

use crate::state::DaemonState;

/// Fetch the release index once and refresh the cache, logging (notify-only)
/// any installed release with a newer patch. Skipped when no Node release is
/// installed; a fetch failure keeps the previous cache.
pub async fn poll_and_refresh(state: &DaemonState, dl: &dyn Downloader) {
    if yerd_platform::node::installed(&state.dirs).is_empty() {
        return;
    }
    match crate::tools::node::latest_patches(dl).await {
        Ok(latest) => store(state, latest).await,
        Err(e) => tracing::debug!(error = %e, "node update poll failed, keeping cached updates"),
    }
}

/// Replace the cache with `latest` and recount pending updates.
pub async fn store(state: &DaemonState, latest: BTreeMap<u32, String>) {
    let mut cache = state.node_updates.write().await;
    *cache = latest;
    let pending = publish(state, &cache);
    drop(cache);
    for (from, to) in pending {
        tracing::info!(from = %from, to = %to, "Node update available");
    }
}

/// Recompute the pending-update count from disk and the cache (no network),
/// returning the pending updates.
pub async fn recount(state: &DaemonState) -> BTreeMap<String, String> {
    let cache = state.node_updates.write().await;
    publish(state, &cache)
}

/// Scan the installed releases and publish their count while the caller holds
/// the cache's write lock, so concurrent recounts publish in lock order and a
/// stale scan can never overwrite a newer one.
fn publish(state: &DaemonState, cache: &BTreeMap<u32, String>) -> BTreeMap<String, String> {
    let installed = yerd_platform::node::installed(&state.dirs);
    let versions: Vec<&str> = installed.iter().map(|n| n.version.as_str()).collect();
    let pending = yerd_core::node::release_updates(&versions, cache);
    state.node_update_count.store(
        u32::try_from(pending.len()).unwrap_or(u32::MAX),
        Ordering::Relaxed,
    );
    pending
}

/// Installed version → newer patch, from the cache (no network).
pub async fn cached_updates(state: &DaemonState, installed: &[&str]) -> BTreeMap<String, String> {
    yerd_core::node::release_updates(installed, &*state.node_updates.read().await)
}
