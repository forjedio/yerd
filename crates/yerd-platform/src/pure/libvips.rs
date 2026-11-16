//! Where the libvips shared library php-vips loads usually lives.
//!
//! php-vips drives libvips through FFI and opens it by soname
//! (`libvips.42.dylib` on macOS, `libvips.so.42` on Linux). Yerd does not
//! bundle libvips, so the GUI shows an install hint when none of these
//! well-known locations holds it. The lists cover Homebrew (Apple Silicon and
//! Intel), `MacPorts`, the Debian/Ubuntu multiarch dirs, Fedora's `lib64`,
//! Arch's `lib`, `/usr/local` and Linuxbrew. A library under any other prefix
//! reads as "not found", which only shows the advisory hint.

use std::path::{Path, PathBuf};

/// Candidate libvips paths on macOS.
#[must_use]
pub fn candidates_macos() -> Vec<PathBuf> {
    [
        "/opt/homebrew/lib/libvips.42.dylib",
        "/usr/local/lib/libvips.42.dylib",
        "/opt/local/lib/libvips.42.dylib",
    ]
    .into_iter()
    .map(PathBuf::from)
    .collect()
}

/// Candidate libvips paths on Linux.
#[must_use]
pub fn candidates_linux() -> Vec<PathBuf> {
    [
        "/usr/lib/x86_64-linux-gnu/libvips.so.42",
        "/usr/lib/aarch64-linux-gnu/libvips.so.42",
        "/usr/lib64/libvips.so.42",
        "/usr/lib/libvips.so.42",
        "/usr/local/lib/libvips.so.42",
        "/home/linuxbrew/.linuxbrew/lib/libvips.so.42",
    ]
    .into_iter()
    .map(PathBuf::from)
    .collect()
}

/// Whether any of `candidates` exists according to `exists` (the real impls
/// pass [`Path::is_file`]; tests pass an in-memory set).
#[must_use]
pub fn any_present(candidates: &[PathBuf], exists: impl Fn(&Path) -> bool) -> bool {
    candidates.iter().any(|p| exists(p))
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod tests {
    use std::collections::HashSet;

    use super::*;

    #[test]
    fn any_present_matches_against_the_candidate_lists() {
        let cases: &[(Vec<PathBuf>, &[&str], bool)] = &[
            (
                candidates_macos(),
                &["/opt/homebrew/lib/libvips.42.dylib"],
                true,
            ),
            (
                candidates_macos(),
                &["/opt/local/lib/libvips.42.dylib"],
                true,
            ),
            (candidates_macos(), &["/usr/lib/libvips.so.42"], false),
            (candidates_linux(), &["/usr/lib64/libvips.so.42"], true),
            (
                candidates_linux(),
                &["/usr/lib/x86_64-linux-gnu/libvips.so.42"],
                true,
            ),
            (
                candidates_linux(),
                &["/opt/homebrew/lib/libvips.42.dylib"],
                false,
            ),
            (candidates_linux(), &[], false),
        ];
        for (candidates, present, want) in cases {
            let set: HashSet<PathBuf> = present.iter().map(PathBuf::from).collect();
            assert_eq!(
                any_present(candidates, |p| set.contains(p)),
                *want,
                "{present:?}"
            );
        }
    }
}
