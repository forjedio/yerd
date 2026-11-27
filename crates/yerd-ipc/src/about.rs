//! Wire types for the About data (see [`crate::Response::ReleaseNotes`] and
//! [`crate::Response::Contributors`]). Field order is the contract.

use serde::{Deserialize, Serialize};

/// One change listed in a release's notes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReleaseNoteItem {
    /// The change title.
    pub title: String,
    /// The GitHub login of the author, without the `@`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
    /// The pull request URL.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
}

/// One release's categorised notes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReleaseNotesEntry {
    /// The bare semver version (e.g. `2.1.0`).
    pub version: String,
    /// The release tag (e.g. `v2.1.0`).
    pub tag: String,
    /// The RFC 3339 publish time.
    pub published_at: String,
    /// Whether this is a pre-release.
    pub prerelease: bool,
    /// A one-line summary: the first feature, else fix, else other title.
    pub summary: String,
    /// New features.
    pub features: Vec<ReleaseNoteItem>,
    /// Bug fixes.
    pub fixes: Vec<ReleaseNoteItem>,
    /// Everything else that is user-facing.
    pub others: Vec<ReleaseNoteItem>,
}

/// One human contributor to the Yerd repository.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Contributor {
    /// GitHub login.
    pub login: String,
    /// GitHub avatar image URL.
    pub avatar_url: String,
    /// Number of commits GitHub credits to this contributor.
    pub commits: u32,
}
