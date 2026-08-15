//! Normalising a canonicalised path back to a form other programs accept.
//!
//! `std::fs::canonicalize` on Windows returns the **verbatim** (extended-length)
//! form: `\\?\C:\sites\shop`, or `\\?\UNC\server\share\x` for a network path.
//! That prefix tells the Win32 layer to skip path parsing, which is exactly why
//! Rust uses it - but most programs cannot open such a path. PHP is one of them:
//! given `SCRIPT_FILENAME=\\?\C:\sites\shop\index.php` it answers
//! `404 No input file specified.`, while the same path without the prefix
//! serves correctly (verified against a real `php-cgi.exe` 8.5).
//!
//! So every canonicalised path that is stored, compared against a stored path,
//! or handed to a child process goes through [`strip_verbatim`] first.
//!
//! Un-gated: compiled and table-tested on every OS, and a no-op off Windows,
//! where a backslash is an ordinary filename character and stripping one would
//! corrupt a legitimate path.

use std::path::{Path, PathBuf};

/// Drop the Windows verbatim prefix from `path` when it has a plain equivalent.
///
/// - `\\?\C:\x` becomes `C:\x`
/// - `\\?\UNC\server\share\x` becomes `\\server\share\x`
/// - anything else is returned unchanged, including `\\?\Volume{GUID}\...`,
///   which has no non-verbatim spelling, and every path on a non-Windows host.
#[must_use]
pub fn strip_verbatim(path: &Path) -> PathBuf {
    if !cfg!(windows) {
        return path.to_path_buf();
    }
    let Some(s) = path.to_str() else {
        return path.to_path_buf();
    };
    if let Some(rest) = s.strip_prefix(r"\\?\UNC\") {
        return PathBuf::from(format!(r"\\{rest}"));
    }
    match s.strip_prefix(r"\\?\") {
        // Only a drive-letter path has a short form; a volume-GUID path does
        // not, so leaving it verbatim is the correct answer rather than a
        // fallback.
        Some(rest) if starts_with_drive(rest) => PathBuf::from(rest),
        _ => path.to_path_buf(),
    }
}

/// Whether `s` opens with a `C:`-style drive designator.
fn starts_with_drive(s: &str) -> bool {
    let mut chars = s.chars();
    matches!(
        (chars.next(), chars.next()),
        (Some(letter), Some(':')) if letter.is_ascii_alphabetic()
    )
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

    #[cfg(windows)]
    #[test]
    fn strips_a_verbatim_drive_prefix() {
        assert_eq!(
            strip_verbatim(Path::new(r"\\?\C:\sites\shop")),
            PathBuf::from(r"C:\sites\shop")
        );
        assert_eq!(
            strip_verbatim(Path::new(r"\\?\c:\x")),
            PathBuf::from(r"c:\x")
        );
    }

    /// `\\?\UNC\server\share` is the verbatim spelling of `\\server\share`, so
    /// the prefix collapses to the two leading backslashes rather than vanishing.
    #[cfg(windows)]
    #[test]
    fn rewrites_a_verbatim_unc_prefix_to_the_plain_form() {
        assert_eq!(
            strip_verbatim(Path::new(r"\\?\UNC\server\share\x.php")),
            PathBuf::from(r"\\server\share\x.php")
        );
    }

    /// A volume-GUID path has no plain equivalent, so stripping the prefix
    /// would produce something that resolves nowhere.
    #[cfg(windows)]
    #[test]
    fn leaves_a_volume_guid_path_verbatim() {
        let p = Path::new(r"\\?\Volume{b75e2c83-0000-0000-0000-602f00000000}\x");
        assert_eq!(strip_verbatim(p), p.to_path_buf());
    }

    #[cfg(windows)]
    #[test]
    fn leaves_an_ordinary_windows_path_alone() {
        for p in [r"C:\sites\shop", r"\\server\share\x", r"relative\x"] {
            assert_eq!(strip_verbatim(Path::new(p)), PathBuf::from(p), "{p}");
        }
    }

    /// Off Windows a backslash is an ordinary filename character, so a path
    /// that merely looks verbatim must survive untouched.
    #[cfg(not(windows))]
    #[test]
    fn is_a_no_op_off_windows() {
        for p in [r"\\?\C:\x", "/srv/site", r"/srv/odd\\name"] {
            assert_eq!(strip_verbatim(Path::new(p)), PathBuf::from(p), "{p}");
        }
    }

    #[test]
    fn is_idempotent() {
        for p in [r"\\?\C:\x", r"C:\x", "/srv/site"] {
            let once = strip_verbatim(Path::new(p));
            assert_eq!(strip_verbatim(&once), once, "{p}");
        }
    }
}
