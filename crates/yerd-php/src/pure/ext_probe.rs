//! Pure interpretation of a PHP extension load-probe's output.
//!
//! The daemon load-probes a candidate extension by running
//! `php -n -d [zend_]extension=<path> -m` and capturing its output. **PHP prints
//! a load failure as a `PHP Warning: ... Unable to load dynamic library ...` and
//! still exits `0`**, so success cannot be judged from the exit code alone; and
//! PHP routes that warning to whichever stream `display_errors` selects (stdout
//! by default under `-n`), so the caller feeds *both* streams here.
//! [`interpret_probe`] keys on specific failure markers in that combined text,
//! and is a pure function so the classification is unit-tested without spawning
//! PHP.

use thiserror::Error;

/// Why a candidate extension failed its load-probe.
#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ExtLoadError {
    /// PHP could not load the shared object at all (bad path, wrong
    /// architecture, or an unresolved dependency).
    #[error("PHP could not load the extension (check the path, architecture, and dependencies)")]
    NotLoadable,
    /// The `.so` was built for a different PHP version (module API / build
    /// mismatch). Extensions are ABI-bound to a PHP minor.
    #[error("the extension was built for a different PHP version")]
    AbiMismatch,
    /// Loaded via `zend_extension=` but it is a regular extension. Retry without
    /// `--zend`.
    #[error("this is a regular extension, not a Zend extension - retry without --zend")]
    NotZend,
    /// Loaded via `extension=` but it is a Zend extension. Retry with `--zend`.
    #[error("this is a Zend extension - retry with --zend")]
    IsZend,
    /// The probe reported failure but no specific cause could be identified.
    #[error("the extension failed to load")]
    Unknown,
    /// The probe process could not be spawned or its output could not be read.
    /// Never returned by [`interpret_probe`] itself; produced at the I/O edge.
    #[error("could not run the extension load-probe")]
    SpawnFailed,
}

/// Classify a probe result from `(exit-was-success, combined stdout+stderr)`.
///
/// Returns `Ok(())` only when no load-failure marker is present in the
/// diagnostics; unrelated startup warnings (deprecations, "already loaded") are
/// not treated as failures. Marker checks are ordered most-specific first.
///
/// # Errors
/// An [`ExtLoadError`] classifying the failure.
pub fn interpret_probe(exit_ok: bool, diagnostics: &str) -> Result<(), ExtLoadError> {
    let s = diagnostics.to_ascii_lowercase();
    if s.contains("valid zend extension") {
        return Err(ExtLoadError::NotZend);
    }
    if s.contains("as it is a zend extension") || s.contains("is a zend extension and") {
        return Err(ExtLoadError::IsZend);
    }
    if s.contains("module api")
        || s.contains("which is not compatible")
        || s.contains("compiled with a different")
    {
        return Err(ExtLoadError::AbiMismatch);
    }
    if s.contains("unable to load dynamic library") || s.contains("failed loading") {
        return Err(ExtLoadError::NotLoadable);
    }
    if !exit_ok {
        return Err(ExtLoadError::Unknown);
    }
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

    #[test]
    fn clean_load_is_ok() {
        interpret_probe(true, "").unwrap();
        interpret_probe(true, "[PHP Modules]\nscrypt\nstandard\n").unwrap();
    }

    #[test]
    fn unrelated_warning_is_still_ok() {
        interpret_probe(
            true,
            "PHP Warning: Module 'scrypt' already loaded in Unknown",
        )
        .unwrap();
        interpret_probe(true, "PHP Deprecated: something in Unknown on line 0").unwrap();
    }

    #[test]
    fn unable_to_load_is_not_loadable_even_on_exit_zero() {
        let stderr = "PHP Warning:  PHP Startup: Unable to load dynamic library \
             'scrypt.so' (tried: /a/scrypt.so (dlopen failed)) in Unknown on line 0";
        assert_eq!(
            interpret_probe(true, stderr),
            Err(ExtLoadError::NotLoadable)
        );
    }

    #[test]
    fn zend_flag_mismatches() {
        assert_eq!(
            interpret_probe(
                true,
                "Failed loading /a/scrypt.so:  ... doesn't appear to be a valid Zend extension"
            ),
            Err(ExtLoadError::NotZend)
        );
        assert_eq!(
            interpret_probe(true, "Cannot load xdebug - it as it is a Zend extension"),
            Err(ExtLoadError::IsZend)
        );
    }

    #[test]
    fn abi_mismatch_detected() {
        let stderr = "PHP Warning:  PHP Startup: scrypt: Unable to initialize module\n\
             Module compiled with module API=20210902\nPHP    compiled with module API=20230831";
        assert_eq!(
            interpret_probe(true, stderr),
            Err(ExtLoadError::AbiMismatch)
        );
    }

    #[test]
    fn nonzero_exit_without_marker_is_unknown() {
        assert_eq!(
            interpret_probe(false, "segfault"),
            Err(ExtLoadError::Unknown)
        );
    }
}
