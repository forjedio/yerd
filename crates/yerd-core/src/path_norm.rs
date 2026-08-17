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
//!
//! Windows programs also expect the native separator. `Path::join` inserts one
//! between components but never rewrites separators *inside* a component, so a
//! configured value like `api/index.php` joined onto `C:\sites\shop` yields the
//! mixed `C:\sites\shop\api/index.php`. PHP itself reports the all-backslash
//! form through `__FILE__` and `realpath()`, and `WordPress`'s `get_home_path()`
//! rewrites backslashes precisely because it expects them, so [`php_path`]
//! settles every path handed to PHP on the native form.
//!
//! The module also owns [`is_safe_member`], the zip-slip guard applied to every
//! archive member name before it is joined onto an install directory. It sits
//! here for the same reason: it judges whether a path *form* is safe to trust,
//! and every managed download (PHP, service engines, Node, Bun, cloudflared)
//! needs it.

use std::path::{Path, PathBuf};

/// Zip-slip guard: an archive member name is safe to trust only if it is
/// relative and contains no `..`, root, or prefix components.
///
/// On Windows a `:` is additionally refused anywhere in the name. `std::path`
/// reads `bin\php.exe:evil` as one ordinary component, but NTFS reads it as an
/// alternate data stream, so extracting it would write a stream onto the real
/// `bin\php.exe` rather than create a new file. A drive prefix is already
/// refused as a `Prefix` component; this closes the stream spelling too.
#[must_use]
pub fn is_safe_member(name: &str) -> bool {
    use std::path::Component;
    if name.is_empty() || (cfg!(windows) && name.contains(':')) {
        return false;
    }
    Path::new(name)
        .components()
        .all(|c| matches!(c, Component::Normal(_) | Component::CurDir))
}

/// Drop the Windows verbatim prefix from `path` when it has a plain equivalent.
///
/// Only a drive-letter path has a short form; a volume-GUID path does not, so
/// leaving one verbatim is the correct answer rather than a fallback.
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
        Some(rest) if starts_with_drive(rest) => PathBuf::from(rest),
        _ => path.to_path_buf(),
    }
}

/// Rewrite `/` to the native separator, on Windows only.
///
/// A no-op off Windows, where `/` is the separator, and a no-op for a path that
/// is not valid UTF-8, matching [`strip_verbatim`]'s handling of the same case.
fn native_separators(path: &Path) -> PathBuf {
    if !cfg!(windows) {
        return path.to_path_buf();
    }
    match path.to_str() {
        Some(s) if s.contains('/') => PathBuf::from(s.replace('/', "\\")),
        _ => path.to_path_buf(),
    }
}

/// The single form in which a filesystem path is handed to PHP.
///
/// Strips a verbatim prefix, which PHP cannot open at all, and settles the
/// separator on the native form, which is what PHP reports for the same file
/// through `__FILE__` and `realpath()`. Applying only half the rule leaves the
/// mixed `C:\sites\shop\api/index.php` shape that a `join` onto a configured
/// relative target produces, so this is the only public entry point.
///
/// A no-op off Windows.
#[must_use]
pub fn php_path(path: &Path) -> PathBuf {
    native_separators(&strip_verbatim(path))
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

    #[test]
    fn is_safe_member_rejects_traversal_and_absolute() {
        assert!(is_safe_member("php"));
        assert!(is_safe_member("./php"));
        assert!(!is_safe_member("../php"));
        assert!(!is_safe_member("/etc/php"));
        assert!(!is_safe_member("a/../../b"));
        assert!(!is_safe_member(""));
    }

    /// An NTFS alternate-data-stream name is one ordinary `std::path` component,
    /// so only the explicit `:` check refuses it. Windows-only: `:` is a legal
    /// filename byte on Unix and rejecting it there would be a regression.
    #[test]
    fn is_safe_member_rejects_ntfs_streams_on_windows() {
        assert_eq!(is_safe_member("bin/php.exe:evil"), !cfg!(windows));
        assert_eq!(is_safe_member("php.exe:$DATA"), !cfg!(windows));
        assert!(is_safe_member("bin/php.exe"));
    }

    /// One table, run on every OS: the Windows column is the expectation there,
    /// and every input is unchanged elsewhere. The Windows assertions execute
    /// only on a Windows host.
    #[test]
    fn php_path_settles_prefix_and_separator() {
        let rows = [
            (r"C:\sites\shop", r"C:\sites\shop"),
            (r"C:/sites/shop", r"C:\sites\shop"),
            (
                r"C:\sites\shop\api/index.php",
                r"C:\sites\shop\api\index.php",
            ),
            (r"\\?\C:\sites\shop\index.php", r"C:\sites\shop\index.php"),
            (r"\\?\UNC\server\share\x.php", r"\\server\share\x.php"),
            ("/srv/www/app/index.php", r"\srv\www\app\index.php"),
        ];
        for (input, on_windows) in rows {
            let got = php_path(Path::new(input));
            let want = if cfg!(windows) { on_windows } else { input };
            assert_eq!(got, PathBuf::from(want), "{input}");
        }
    }

    /// A volume-GUID path has no plain equivalent and contains no forward
    /// slashes, so neither half of the rule may touch it.
    #[test]
    fn php_path_leaves_a_volume_guid_path_alone() {
        let p = Path::new(r"\\?\Volume{b75e2c83-0000-0000-0000-602f00000000}\x");
        assert_eq!(php_path(p), p.to_path_buf());
    }

    #[test]
    fn php_path_is_idempotent() {
        for p in [
            r"\\?\C:\x",
            r"C:\x",
            r"C:/x/y",
            "/srv/site",
            r"C:\sites\shop\api/index.php",
        ] {
            let once = php_path(Path::new(p));
            assert_eq!(php_path(&once), once, "{p}");
        }
    }
}
