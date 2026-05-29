//! Atomic file write via tempfile + rename.
//!
//! Inlined here rather than reused from `yerd-config` because that crate's
//! equivalent (`io::save`) is `pub(crate)`.

use std::io::{self, Write};
use std::path::Path;

/// Write `bytes` to `path` atomically (tempfile in the same directory +
/// `rename`).
///
/// If `path`'s parent doesn't exist, returns an `io::Error` of kind
/// `NotFound` rather than attempting `create_dir_all` — directory creation
/// is the caller's contract (`PlatformDirs` documents the same convention).
pub fn write(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let parent = path.parent().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "atomic_write: path has no parent",
        )
    })?;

    let mut tmp = tempfile::NamedTempFile::new_in(parent)?;
    tmp.write_all(bytes)?;
    tmp.flush()?;
    tmp.persist(path).map_err(|e| e.error)?;
    Ok(())
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
    use std::fs;

    #[test]
    fn writes_then_reads() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("out.txt");
        write(&path, b"hello world").unwrap();
        let got = fs::read(&path).unwrap();
        assert_eq!(got, b"hello world");
    }

    #[test]
    fn overwrites_existing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("out.txt");
        write(&path, b"first").unwrap();
        write(&path, b"second").unwrap();
        let got = fs::read(&path).unwrap();
        assert_eq!(got, b"second");
    }

    #[test]
    fn errors_when_parent_missing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nonexistent").join("out.txt");
        let err = write(&path, b"x").unwrap_err();
        // `tempfile::NamedTempFile::new_in` returns NotFound here.
        assert_eq!(err.kind(), io::ErrorKind::NotFound);
    }
}
