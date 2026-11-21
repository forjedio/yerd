//! Recover the launch path of a running executable that was renamed over.
//!
//! On Linux, `/proc/self/exe` (and so [`std::env::current_exe`]) reads
//! `"<path> (deleted)"` once the running binary's file has been replaced, as an
//! update or package upgrade does. The new binary sits at `<path>`, and that is
//! the file a restart should load.

use std::path::{Path, PathBuf};

/// Suffix Linux appends to `/proc/self/exe` once the file is unlinked.
const DELETED_SUFFIX: &str = " (deleted)";

/// `reported` minus the trailing `" (deleted)"`, or `None` when there is no
/// such suffix. Works on the raw bytes, so a non-UTF-8 directory survives. The
/// caller decides whether the result is the file to use, since a real path
/// can end in that text too.
#[cfg(unix)]
#[must_use]
pub fn strip_deleted_suffix(reported: &Path) -> Option<PathBuf> {
    use std::ffi::OsString;
    use std::os::unix::ffi::{OsStrExt as _, OsStringExt as _};

    let launch = reported
        .as_os_str()
        .as_bytes()
        .strip_suffix(DELETED_SUFFIX.as_bytes())
        .filter(|path| !path.is_empty())?;
    Some(PathBuf::from(OsString::from_vec(launch.to_vec())))
}

/// `reported` minus the trailing `" (deleted)"`, or `None` when there is no
/// such suffix (or the path is not Unicode). Only Linux reports the suffix.
#[cfg(not(unix))]
#[must_use]
pub fn strip_deleted_suffix(reported: &Path) -> Option<PathBuf> {
    reported
        .to_str()?
        .strip_suffix(DELETED_SUFFIX)
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_deleted_suffix_recovers_the_launch_path() {
        let cases: Vec<(&str, Option<PathBuf>)> = vec![
            (
                "/home/u/.yerd/bin/yerdd (deleted)",
                Some(PathBuf::from("/home/u/.yerd/bin/yerdd")),
            ),
            (
                "/usr/bin/yerdd (deleted)",
                Some(PathBuf::from("/usr/bin/yerdd")),
            ),
            (
                "/opt/a (deleted) dir/yerdd (deleted)",
                Some(PathBuf::from("/opt/a (deleted) dir/yerdd")),
            ),
            ("/home/u/.yerd/bin/yerdd", None),
            ("/home/u/.yerd/bin/yerdd (deleted) ", None),
            ("/home/u/.yerd/bin/yerdd(deleted)", None),
            (" (deleted)", None),
            ("", None),
        ];
        for (reported, expected) in cases {
            assert_eq!(
                strip_deleted_suffix(Path::new(reported)),
                expected,
                "{reported:?}"
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn strip_deleted_suffix_keeps_non_utf8_components() {
        use std::ffi::OsStr;
        use std::os::unix::ffi::OsStrExt as _;

        let reported = Path::new(OsStr::from_bytes(b"/home/u/\xff\xfe/yerdd (deleted)"));
        assert_eq!(
            strip_deleted_suffix(reported),
            Some(PathBuf::from(OsStr::from_bytes(b"/home/u/\xff\xfe/yerdd")))
        );
    }
}
