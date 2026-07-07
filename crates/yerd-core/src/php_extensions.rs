//! Pure validation for user-registered custom PHP extensions.
//!
//! Yerd lets users register native extension `.so` files that load into both a
//! PHP version's FPM pool (`-d [zend_]extension=<path>`) and its CLI ini
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

/// Validate an extension path. Must be an absolute path to a `.so`, free of the
/// characters that could break out of the double-quoted ini value or corrupt the
/// `-d` argument: control characters, NUL, newline, and the double-quote itself.
/// Spaces are allowed (the ini value is quoted and the `-d` value is a single
/// argv element), so a path under a spaced directory still validates.
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
    if !path.starts_with('/') {
        return err(PathErrorReason::NotAbsolute);
    }
    if Path::new(path).extension().and_then(|e| e.to_str()) != Some("so") {
        return err(PathErrorReason::NotSharedObject);
    }
    if path
        .chars()
        .any(|c| c.is_control() || matches!(c, '"' | '\0'))
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
/// checked first: when a name is auto-derived from a non-`.so` path it inherits
/// the extension (e.g. `scrypt.dylib`) and would fail the name charset, masking
/// the clearer "must end in .so" reason.
pub fn validate_entry(name: &str, path: &str, zend: bool) -> Result<(), ExtError> {
    let _ = zend;
    validate_ext_path(path)?;
    validate_ext_name(name)?;
    Ok(())
}

/// Derive a default name from a `.so` path: the file stem (basename minus the
/// `.so` suffix). Returns `None` when the path has no usable file name. The
/// result is not guaranteed to satisfy [`validate_ext_name`] (a stem may contain
/// dots or other characters), so callers still validate it.
#[must_use]
pub fn default_name_from_path(path: &str) -> Option<String> {
    let stem = Path::new(path).file_name()?.to_str()?;
    let stem = stem.strip_suffix(".so").unwrap_or(stem);
    if stem.is_empty() {
        return None;
    }
    Some(stem.to_owned())
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
    /// Not an absolute path (no leading `/`).
    NotAbsolute,
    /// Does not end in `.so`.
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
            Self::NotSharedObject => "path must end in .so",
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

    #[test]
    fn valid_name_and_path_pass() {
        validate_ext_name("scrypt").unwrap();
        validate_ext_name("my_ext-2").unwrap();
        validate_ext_path("/opt/homebrew/lib/php/pecl/20250925/scrypt.so").unwrap();
        validate_ext_path("/space dir/x.so").unwrap();
        validate_entry("scrypt", "/a/scrypt.so", false).unwrap();
        validate_entry("xdebug", "/a/xdebug.so", true).unwrap();
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
            validate_ext_path("/a/x.dylib"),
            Err(ExtError::Path {
                reason: PathErrorReason::NotSharedObject
            })
        ));
        assert!(matches!(
            validate_ext_path("/a/\"evil\".so"),
            Err(ExtError::Path {
                reason: PathErrorReason::IllegalCharacter
            })
        ));
        assert!(matches!(
            validate_ext_path("/a/new\nline.so"),
            Err(ExtError::Path {
                reason: PathErrorReason::IllegalCharacter
            })
        ));
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
            default_name_from_path("/a/b/scrypt.so").as_deref(),
            Some("scrypt")
        );
        assert_eq!(default_name_from_path("/a/b/x.so").as_deref(), Some("x"));
        assert_eq!(default_name_from_path("/").as_deref(), None);
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
