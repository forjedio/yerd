//! PHP update polling and the daemon's update cache.
//!
//! The cache (`DaemonState::php_updates`) maps each installed minor → the newest
//! build `(patch, revision)` seen at the last manifest poll. [`poll_and_refresh`]
//! (network) repopulates it and logs available updates; [`cached_updates`] (no
//! network) serves the annotations shown on `list php`. The revision dimension
//! is what surfaces a c-ares-cutover rebuild (`8.5.7-1`) as an update to an
//! existing `8.5.7` install recorded as revision 0.

use std::collections::HashMap;

use yerd_core::PhpVersion;
use yerd_ipc::PhpUpdate;
use yerd_php::{
    current_os_arch, discover_bundled, display_build, is_newer_build, resolve_from_listing,
    Downloader,
};

use crate::php_install::{fetch_verified_listing, installed_patch, installed_revision};
use crate::state::DaemonState;

/// Installed minors, from on-disk bundled discovery (keyed on the FPM binary).
pub(crate) fn installed_minors(state: &DaemonState) -> Vec<PhpVersion> {
    discover_bundled(&state.dirs)
        .unwrap_or_default()
        .into_iter()
        .map(|(v, _)| v)
        .collect()
}

/// Poll the manifest once, refresh `state.php_updates`, and log (notify-only)
/// any installed minor with a newer build. **Failure-tolerant**: network,
/// signature, and platform errors are logged at `debug` and leave the cache
/// untouched. `public_key` is the minisign key the manifest is verified against
/// (prod passes [`yerd_update::PHP_LISTING_PUBLIC_KEY`]).
pub async fn poll_and_refresh(state: &DaemonState, dl: &dyn Downloader, public_key: &str) {
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
    let listing = match fetch_verified_listing(dl, public_key).await {
        Ok(body) => body,
        Err(e) => {
            tracing::debug!(error = %e, "php update poll skipped: listing fetch/verify failed");
            return;
        }
    };

    let mut latest: HashMap<PhpVersion, (String, u32)> = HashMap::new();
    for minor in minors {
        let Ok(artifact) = resolve_from_listing(&listing, minor, os, arch) else {
            continue;
        };
        if let Some(installed) = installed_patch(&state.dirs, minor) {
            let installed_rev = installed_revision(&state.dirs, minor);
            if is_newer_build(
                &installed,
                installed_rev,
                &artifact.full_version,
                artifact.revision,
            ) {
                tracing::info!(
                    version = %minor,
                    installed = %display_build(&installed, installed_rev),
                    latest = %display_build(&artifact.full_version, artifact.revision),
                    "a newer PHP build is available (run `yerd update php`)"
                );
            }
        }
        latest.insert(minor, (artifact.full_version, artifact.revision));
    }
    *state.php_updates.write().await = latest;
}

/// Available updates derived from the cache + installed markers (no network).
/// Only minors whose cached build is strictly newer than the installed one are
/// emitted; both sides are formatted as `<patch>-<revision>` for display.
pub async fn cached_updates(state: &DaemonState) -> Vec<PhpUpdate> {
    let cache = state.php_updates.read().await;
    let mut out = Vec::new();
    for minor in installed_minors(state) {
        let (Some(installed), Some((latest_patch, latest_rev))) =
            (installed_patch(&state.dirs, minor), cache.get(&minor))
        else {
            continue;
        };
        let installed_rev = installed_revision(&state.dirs, minor);
        if is_newer_build(&installed, installed_rev, latest_patch, *latest_rev) {
            out.push(PhpUpdate {
                version: minor,
                installed: display_build(&installed, installed_rev),
                latest: display_build(latest_patch, *latest_rev),
            });
        }
    }
    out
}
