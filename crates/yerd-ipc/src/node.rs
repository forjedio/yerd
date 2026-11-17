//! Wire types for managed Node releases (see [`crate::Response::NodeVersions`]
//! and [`crate::Response::NodeSite`]). Field order is the contract.

use serde::{Deserialize, Serialize};
use yerd_core::node::NodeSource;

/// One installed Node release (reply element of [`crate::Response::NodeVersions`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NodeRelease {
    /// Exact version, without a leading `v`.
    pub version: String,
    /// Whether the release was an LTS release when downloaded.
    pub lts: bool,
    /// LTS codename, when recorded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub codename: Option<String>,
    /// Whether the effective global default resolves to this release.
    pub default: bool,
    /// Whether uninstalling it is refused because the global default would
    /// dangle.
    pub uninstall_blocked: bool,
    /// Saved site preferences that uninstalling this release would clear.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub clears_preferences: Vec<NodePreferenceRef>,
    /// The newer published patch of this release's major, from the daemon's
    /// last release-index check. Only set on the highest installed patch of a
    /// major.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub update: Option<String>,
}

/// A saved per-project Node preference, keyed by project root.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NodePreferenceRef {
    /// The project root the preference is keyed by.
    pub root: String,
    /// The registered site with that root, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub site: Option<String>,
}

/// A project's Node version file (see [`crate::Response::NodeSite`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NodeProjectFile {
    /// Only ever [`NodeSource::Nvmrc`] or [`NodeSource::NodeVersion`].
    pub kind: NodeSource,
    /// The selector in the file.
    pub selector: String,
}
