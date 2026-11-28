//! Error type for the Insights crate.

use std::io;
use std::path::PathBuf;

/// Failures from decoding, encoding or storing Insights data.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum InsightsError {
    /// A filesystem operation on the store failed.
    #[error("insights store I/O at {path}: {source}")]
    Io {
        /// The path involved.
        path: PathBuf,
        /// The underlying OS error.
        source: io::Error,
    },
    /// A stored line or detail member could not be decoded (torn or garbage).
    #[error("insights decode: {0}")]
    Decode(String),
    /// A value could not be encoded for storage.
    #[error("insights encode: {0}")]
    Encode(String),
    /// [`crate::Aggregator::load_site`] was called for a site that already
    /// holds state.
    #[error("insights site `{site}` already has state")]
    SiteNotEmpty {
        /// The site name.
        site: String,
    },
}
