//! Pure validation for user-registered custom PHP extensions.
//!
//! Yerd lets users register native extension files - `.so` on Unix, `.dll` on
//! Windows - that load into both a
//! PHP version's pool (`-d [zend_]extension=<path>`) and its CLI ini
//! (`[zend_]extension = "<path>"`). The path flows into an FPM command-line
//! argument and into a double-quoted ini value, so [`validate_ext_path`] is the
//! **injection boundary**: it runs when an extension is registered (CLI client +
//! daemon), when the config is loaded from disk (`yerd-config`), and defensively
//! before rendering (`yerd-php`, `bin/yerdd`).
//!
//! This module is pure: it does string validation only. It does **not** touch
//! the filesystem or run a load-probe. The daemon performs the strict, real
//! load-probe (spawning PHP) at the I/O edge; that lives in `yerd-php`.

use std::fmt;
use std::path::Path;

use thiserror::Error;

/// Longest accepted extension path, in bytes.
const MAX_PATH_LEN: usize = 4096;

/// Longest accepted extension name, in bytes.
const MAX_NAME_LEN: usize = 64;

/// Validate an extension name: the stable handle used to remove an entry and to
/// label it in the GUI. Non-empty, bounded, and restricted to
/// `[A-Za-z0-9_-]` so it is safe as a CLI argument, config value, and map-style
/// lookup key.
///
/// # Errors
/// [`ExtError::Name`] with the specific [`NameErrorReason`].
pub fn validate_ext_name(name: &str) -> Result<(), ExtError> {
    let err = |reason| Err(ExtError::Name { reason });
    if name.is_empty() {
        return err(NameErrorReason::Empty);
    }
    if name.len() > MAX_NAME_LEN {
        return err(NameErrorReason::TooLong);
    }
    if !name
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
    {
        return err(NameErrorReason::IllegalCharacter);
    }
    Ok(())
}

/// Validate an extension path. Must be an absolute path to a host-appropriate
/// dynamic library, free of the
/// characters that could break out of the double-quoted ini value or corrupt the
/// `-d` argument: control characters, NUL, newline, the double-quote, and `$`.
/// (`$` is rejected because PHP interpolates `${VAR}` inside a double-quoted ini
/// value - and the load-probe passes the raw path as a single `-d` argv, so it
/// would not catch a path the rendered ini later mangles.) Spaces are allowed
/// (the ini value is quoted and the `-d` value is a single argv element), so a
/// path under a spaced directory still validates.
///
/// Both the "absolute" and the suffix rule are **host-relative**, because an
/// extension path only ever names a file on the machine the daemon runs on.
/// Unix wants `/usr/lib/php/xdebug.so`; Windows wants `C:\php\ext\php_xdebug.dll`
/// (or a UNC path), which [`Path::is_absolute`] accepts and a leading-`/` test
/// would reject with the nonsensical "path must be absolute". The suffix comes
/// from [`crate::php_vocab::EXT_SUFFIX`], and is matched case-insensitively on
/// Windows only, where the filesystem itself is.
///
/// # Errors
/// [`ExtError::Path`] with the specific [`PathErrorReason`].
pub fn validate_ext_path(path: &str) -> Result<(), ExtError> {
    let err = |reason| Err(ExtError::Path { reason });
    if path.is_empty() {
        return err(PathErrorReason::Empty);
    }
    if path.len() > MAX_PATH_LEN {
        return err(PathErrorReason::TooLong);
    }
    if !Path::new(path).is_absolute() {
        return err(PathErrorReason::NotAbsolute);
    }
    if !has_host_ext_suffix(path) {
        return err(PathErrorReason::NotSharedObject);
    }
    if path
        .chars()
        .any(|c| c.is_control() || matches!(c, '"' | '\0' | '$'))
    {
        return err(PathErrorReason::IllegalCharacter);
    }
    Ok(())
}

/// Validate a whole entry (name + path). `zend` is accepted for a stable
/// signature; a boolean is always valid.
///
/// # Errors
/// The first failing of [`validate_ext_path`] / [`validate_ext_name`]. Path is
/// checked first: when a name is auto-derived from a path with the wrong suffix
/// it inherits the extension (e.g. `scrypt.dylib`) and would fail the name
/// charset, masking the clearer "must end in .so" / ".dll" reason.
pub fn validate_entry(name: &str, path: &str, zend: bool) -> Result<(), ExtError> {
    let _ = zend;
    validate_ext_path(path)?;
    validate_ext_name(name)?;
    Ok(())
}

/// The bare host extension suffix (no leading dot), as
/// [`crate::php_vocab::EXT_SUFFIX`] spells it for prose.
fn host_ext() -> &'static str {
    crate::php_vocab::EXT_SUFFIX.trim_start_matches('.')
}

/// Whether `path`'s final component carries the host's dynamic-library suffix.
/// Case-insensitive on Windows, where the filesystem is; exact elsewhere.
fn has_host_ext_suffix(path: &str) -> bool {
    let Some(ext) = Path::new(path).extension().and_then(|e| e.to_str()) else {
        return false;
    };
    if cfg!(windows) {
        ext.eq_ignore_ascii_case(host_ext())
    } else {
        ext == host_ext()
    }
}

/// Derive a default name from an extension path: the file stem (basename minus
/// the host suffix, e.g. `.so` or `.dll`). Returns `None` when the path has no
/// usable file name. The
/// result is not guaranteed to satisfy [`validate_ext_name`] (a stem may contain
/// dots or other characters), so callers still validate it.
#[must_use]
pub fn default_name_from_path(path: &str) -> Option<String> {
    let file = Path::new(path).file_name()?.to_str()?;
    let stem = strip_host_suffix(file);
    if stem.is_empty() {
        return None;
    }
    Some(stem.to_owned())
}

/// Strip the host's dynamic-library suffix from a file name, case-insensitively
/// on Windows. Returns `file` unchanged when the suffix is absent. Indexes
/// through `get` rather than `split_at` so a name ending in a multi-byte
/// character cannot panic on a non-char-boundary split.
fn strip_host_suffix(file: &str) -> &str {
    let suffix = crate::php_vocab::EXT_SUFFIX;
    if !cfg!(windows) {
        return file.strip_suffix(suffix).unwrap_or(file);
    }
    let Some(cut) = file.len().checked_sub(suffix.len()) else {
        return file;
    };
    match (file.get(..cut), file.get(cut..)) {
        (Some(head), Some(tail)) if tail.eq_ignore_ascii_case(suffix) => head,
        _ => file,
    }
}

/// Failure to validate a custom extension.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ExtError {
    /// The extension name was rejected.
    #[error("invalid extension name: {reason}")]
    Name {
        /// Why the name was rejected.
        reason: NameErrorReason,
    },
    /// The extension path was rejected.
    #[error("invalid extension path: {reason}")]
    Path {
        /// Why the path was rejected.
        reason: PathErrorReason,
    },
}

/// Specific failure modes for an extension name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum NameErrorReason {
    /// Empty string.
    Empty,
    /// Longer than the accepted maximum.
    TooLong,
    /// Contained a character outside `[A-Za-z0-9_-]`.
    IllegalCharacter,
}

impl fmt::Display for NameErrorReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let msg = match self {
            Self::Empty => "name must not be empty",
            Self::TooLong => "name is too long",
            Self::IllegalCharacter => "name may only contain letters, digits, '_' and '-'",
        };
        f.write_str(msg)
    }
}

/// Specific failure modes for an extension path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum PathErrorReason {
    /// Empty string.
    Empty,
    /// Longer than the accepted maximum.
    TooLong,
    /// Not an absolute path for this host (`/...` on Unix; a drive or UNC
    /// prefix on Windows).
    NotAbsolute,
    /// Does not carry the host's dynamic-library suffix (`.so`, or `.dll` on
    /// Windows).
    NotSharedObject,
    /// Contained a control character, NUL, newline, or a double-quote.
    IllegalCharacter,
}

impl fmt::Display for PathErrorReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let msg = match self {
            Self::Empty => "path must not be empty",
            Self::TooLong => "path is too long",
            Self::NotAbsolute => "path must be absolute",
            Self::NotSharedObject => {
                if cfg!(windows) {
                    "path must end in .dll"
                } else {
                    "path must end in .so"
                }
            }
            Self::IllegalCharacter => "path contains an illegal character",
        };
        f.write_str(msg)
    }
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

    /// An absolute path this host accepts, so the shared assertions below do
    /// not have to be written twice. Unix wants a leading slash and `.so`;
    /// Windows wants a drive prefix and `.dll`.
    fn host_path(stem: &str) -> String {
        host_path_with_suffix(stem, crate::php_vocab::EXT_SUFFIX)
    }

    /// [`host_path`] with an explicit suffix, for the wrong-suffix rejections:
    /// the path must still be *absolute* for this host, or the suffix check is
    /// never reached.
    fn host_path_with_suffix(stem: &str, suffix: &str) -> String {
        if cfg!(windows) {
            format!("C:\\php\\ext\\{stem}{suffix}")
        } else {
            format!("/a/{stem}{suffix}")
        }
    }

    #[test]
    fn valid_name_and_path_pass() {
        validate_ext_name("scrypt").unwrap();
        validate_ext_name("my_ext-2").unwrap();
        validate_ext_path(&host_path("scrypt")).unwrap();
        validate_entry("scrypt", &host_path("scrypt"), false).unwrap();
        validate_entry("xdebug", &host_path("xdebug"), true).unwrap();
    }

    /// A spaced directory is legal on both hosts (the ini value is quoted and
    /// the `-d` value is one argv element).
    #[test]
    fn spaced_directory_is_accepted() {
        let path = if cfg!(windows) {
            "C:\\space dir\\x.dll"
        } else {
            "/space dir/x.so"
        };
        validate_ext_path(path).unwrap();
    }

    /// Both rules follow the host, so a path valid on one is rejected on the
    /// other. Before this was host-aware, a Windows user typing a real
    /// `C:\php\ext\php_xdebug.dll` was told "path must be absolute".
    #[cfg(windows)]
    #[test]
    fn windows_accepts_drive_and_unc_dll_paths() {
        validate_ext_path("C:\\php\\ext\\php_xdebug.dll").unwrap();
        validate_ext_path("\\\\server\\share\\php_xdebug.dll").unwrap();
        validate_ext_path("C:\\php\\ext\\PHP_XDEBUG.DLL")
            .expect("NTFS is case-insensitive, so the suffix match must be too");
        assert!(matches!(
            validate_ext_path("/opt/homebrew/lib/scrypt.so"),
            Err(ExtError::Path {
                reason: PathErrorReason::NotAbsolute
            })
        ));
        assert!(matches!(
            validate_ext_path("C:\\php\\ext\\scrypt.so"),
            Err(ExtError::Path {
                reason: PathErrorReason::NotSharedObject
            })
        ));
    }

    /// See [`windows_accepts_drive_and_unc_dll_paths`]: the mirror case.
    #[cfg(not(windows))]
    #[test]
    fn unix_accepts_only_rooted_so_paths() {
        validate_ext_path("/opt/homebrew/lib/php/pecl/20250925/scrypt.so").unwrap();
        assert!(matches!(
            validate_ext_path("C:\\php\\ext\\php_xdebug.dll"),
            Err(ExtError::Path {
                reason: PathErrorReason::NotAbsolute
            })
        ));
        assert!(matches!(
            validate_ext_path("/a/x.dll"),
            Err(ExtError::Path {
                reason: PathErrorReason::NotSharedObject
            })
        ));
    }

    #[test]
    fn name_rejections() {
        assert!(matches!(
            validate_ext_name(""),
            Err(ExtError::Name {
                reason: NameErrorReason::Empty
            })
        ));
        assert!(matches!(
            validate_ext_name("bad name"),
            Err(ExtError::Name {
                reason: NameErrorReason::IllegalCharacter
            })
        ));
        assert!(matches!(
            validate_ext_name("dots.not.allowed"),
            Err(ExtError::Name {
                reason: NameErrorReason::IllegalCharacter
            })
        ));
        assert!(matches!(
            validate_ext_name(&"x".repeat(MAX_NAME_LEN + 1)),
            Err(ExtError::Name {
                reason: NameErrorReason::TooLong
            })
        ));
    }

    #[test]
    fn path_rejections() {
        assert!(matches!(
            validate_ext_path("relative/x.so"),
            Err(ExtError::Path {
                reason: PathErrorReason::NotAbsolute
            })
        ));
        assert!(matches!(
            validate_ext_path(&host_path_with_suffix("x", ".dylib")),
            Err(ExtError::Path {
                reason: PathErrorReason::NotSharedObject
            })
        ));
        for stem in ["\"evil\"", "new\nline", "${HOME}"] {
            assert!(
                matches!(
                    validate_ext_path(&host_path(stem)),
                    Err(ExtError::Path {
                        reason: PathErrorReason::IllegalCharacter
                    })
                ),
                "{stem}"
            );
        }
        assert!(matches!(
            validate_ext_path(""),
            Err(ExtError::Path {
                reason: PathErrorReason::Empty
            })
        ));
    }

    #[test]
    fn default_name_derivation() {
        assert_eq!(
            default_name_from_path(&host_path("scrypt")).as_deref(),
            Some("scrypt")
        );
        assert_eq!(
            default_name_from_path(&host_path("x")).as_deref(),
            Some("x")
        );
        assert_eq!(default_name_from_path("/").as_deref(), None);
        // A foreign suffix is kept, so the derived name fails the name charset
        // with the clearer "wrong suffix" reason rather than a silent truncation.
        assert_eq!(
            default_name_from_path(&host_path_with_suffix("scrypt", ".dylib")).as_deref(),
            Some("scrypt.dylib")
        );
    }

    /// The derived name must not keep a shouty suffix on a case-insensitive
    /// filesystem: `PHP_XDEBUG.DLL` registers as `PHP_XDEBUG`, not
    /// `PHP_XDEBUG.DLL` (which would fail the name charset).
    #[cfg(windows)]
    #[test]
    fn default_name_strips_a_windows_suffix_case_insensitively() {
        assert_eq!(
            default_name_from_path("C:\\php\\ext\\PHP_XDEBUG.DLL").as_deref(),
            Some("PHP_XDEBUG")
        );
    }

    #[test]
    fn error_display_non_empty() {
        for r in [
            NameErrorReason::Empty,
            NameErrorReason::TooLong,
            NameErrorReason::IllegalCharacter,
        ] {
            assert!(!r.to_string().is_empty());
        }
        for r in [
            PathErrorReason::Empty,
            PathErrorReason::TooLong,
            PathErrorReason::NotAbsolute,
            PathErrorReason::NotSharedObject,
            PathErrorReason::IllegalCharacter,
        ] {
            assert!(!r.to_string().is_empty());
        }
    }
}
