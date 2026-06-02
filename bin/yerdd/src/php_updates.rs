//! PHP update polling and the daemon's update cache.
//!
//! The cache (`DaemonState::php_updates`) maps each installed minor → the newest
//! full patch seen at the last distribution poll. [`poll_and_refresh`] (network)
//! repopulates it and logs available updates; [`cached_updates`] (no network)
//! serves the annotations shown on `list php`.

use std::collections::HashMap;

use yerd_core::PhpVersion;
use yerd_ipc::PhpUpdate;
use yerd_php::{
    current_os_arch, discover_bundled, is_newer, listing_url, resolve_from_listing, Downloader,
};

use crate::php_install::installed_patch;
use crate::state::DaemonState;

/// Installed minors, from on-disk bundled discovery (keyed on the FPM binary).
pub(crate) fn installed_minors(state: &DaemonState) -> Vec<PhpVersion> {
    discover_bundled(&state.dirs)
        .unwrap_or_default()
        .into_iter()
        .map(|(v, _)| v)
        .collect()
}

/// Poll the distribution once, refresh `state.php_updates`, and log
/// (notify-only) any installed minor with a newer patch. **Failure-tolerant**:
/// network/platform errors are logged at `debug` and leave the cache untouched.
pub async fn poll_and_refresh(state: &DaemonState, dl: &dyn Downloader) {
    let minors = installed_minors(state);
    if minors.is_empty() {
        return;
    }
    let (os, arch) = match current_os_arch() {
        Ok(p) => p,
        Err(e) => {
            tracing::debug!(error = %e, "php update poll skipped: unsupported platform");
            return;
        }
    };
    let listing = match dl.download(&listing_url()).await {
        Ok(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
        Err(e) => {
            tracing::debug!(error = %e, "php update poll skipped: listing fetch failed");
            return;
        }
    };

    let mut latest: HashMap<PhpVersion, String> = HashMap::new();
    for minor in minors {
        let Ok(artifact) = resolve_from_listing(&listing, minor, os, arch) else {
            continue;
        };
        if let Some(installed) = installed_patch(&state.dirs, minor) {
            if is_newer(&installed, &artifact.full_version) {
                tracing::info!(
                    version = %minor,
                    installed = %installed,
                    latest = %artifact.full_version,
                    "a newer PHP patch is available (run `yerd update php`)"
                );
            }
        }
        latest.insert(minor, artifact.full_version);
    }
    *state.php_updates.write().await = latest;
}

/// Available updates derived from the cache + installed markers (no network).
pub async fn cached_updates(state: &DaemonState) -> Vec<PhpUpdate> {
    let cache = state.php_updates.read().await;
    let mut out = Vec::new();
    for minor in installed_minors(state) {
        let (Some(installed), Some(latest)) =
            (installed_patch(&state.dirs, minor), cache.get(&minor))
        else {
            continue;
        };
        if is_newer(&installed, latest) {
            out.push(PhpUpdate {
                version: minor,
                installed,
                latest: latest.clone(),
            });
        }
    }
    out
}
