//! Self-update wire types: the release channel and the freshness of a check.
//!
//! These mirror `yerd_update::Channel` and the persisted `update_channel`
//! config string; conversion happens in the daemon so neither the protocol
//! crate nor the config crate depends on the other.

use serde::{Deserialize, Serialize};

/// Self-update release channel.
///
/// Serialised lowercase (`"stable"` / `"edge"`). `#[non_exhaustive]` so a future
/// channel can be added without a hard wire break - an older peer that receives
/// an unknown channel fails closed as [`crate::IpcError::Decode`] (no
/// `#[serde(other)]` catch-all), consistent with the rest of this crate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum Channel {
    /// Track the latest non-pre-release.
    Stable,
    /// Track the latest version including pre-releases / release candidates.
    Edge,
}

/// Whether an [`crate::Response::UpdateStatus`] reflects a fresh network fetch
/// or a cached value served because the live fetch failed (offline / rate-limit).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum UpdateSource {
    /// The status came from a fresh fetch.
    Live,
    /// The fetch failed; this is the last cached result.
    Cached,
}

/// The kind of staged update artifact, which tells the applier how to install
/// it (mirrors `yerd_update::ArtifactKind`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum StagedArtifact {
    /// macOS `.app.tar.gz` - the applier extracts + swaps the bundle.
    AppTarGz,
    /// Linux `.deb` - the applier reinstalls via `dpkg -i`.
    Deb,
}
