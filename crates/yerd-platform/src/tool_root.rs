//! The single top-level directory of an unpacked tool archive.
//!
//! Node, pnpm, and Bun archives wrap their payload in one top-level directory
//! whose name encodes the version. The daemon (when linking shims) and the CLI
//! (when a shim resolves its target) must agree on that directory, so both use
//! [`single_top_level_dir`].

use std::path::{Path, PathBuf};

use thiserror::Error;

/// Why [`single_top_level_dir`] found no unique directory.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum RootDirError {
    /// `dir` could not be listed.
    #[error("{dir}: {source}")]
    Unreadable {
        /// The directory that was listed.
        dir: PathBuf,
        /// The underlying error.
        source: std::io::Error,
    },
    /// `dir` holds no child directory.
    #[error("no top-level dir in {}", .0.display())]
    Missing(PathBuf),
    /// `dir` holds more than one child directory.
    #[error("expected one top-level dir in {}, found multiple", .0.display())]
    Multiple(PathBuf),
}

/// The single child **directory** of `dir`. Files beside it (such as a
/// `.version` marker) are ignored.
///
/// # Errors
/// [`RootDirError`] unless exactly one child directory exists.
pub fn single_top_level_dir(dir: &Path) -> Result<PathBuf, RootDirError> {
    let entries = std::fs::read_dir(dir).map_err(|source| RootDirError::Unreadable {
        dir: dir.to_path_buf(),
        source,
    })?;
    let mut found: Option<PathBuf> = None;
    for entry in entries.flatten() {
        if entry.path().is_dir() {
            if found.is_some() {
                return Err(RootDirError::Multiple(dir.to_path_buf()));
            }
            found = Some(entry.path());
        }
    }
    found.ok_or_else(|| RootDirError::Missing(dir.to_path_buf()))
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod tests {
    use super::*;

    #[test]
    fn requires_exactly_one_directory() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        assert!(matches!(
            single_top_level_dir(root),
            Err(RootDirError::Missing(_))
        ));
        std::fs::create_dir(root.join("node-v24.17.0-darwin-arm64")).unwrap();
        std::fs::write(root.join(".version"), "x").unwrap();
        assert_eq!(
            single_top_level_dir(root).unwrap(),
            root.join("node-v24.17.0-darwin-arm64")
        );
        std::fs::create_dir(root.join("second")).unwrap();
        assert!(matches!(
            single_top_level_dir(root),
            Err(RootDirError::Multiple(_))
        ));
        assert!(matches!(
            single_top_level_dir(&root.join("absent")),
            Err(RootDirError::Unreadable { .. })
        ));
    }
}
