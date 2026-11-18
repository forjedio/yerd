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
    /// PHP ran cleanly but did not list the extension among its loaded Zend
    /// modules. Returned by [`require_zend_module`].
    #[error("PHP started but did not list the extension as loaded")]
    NotListed,
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
    if s.contains("as it is a zend extension")
        || s.contains("is a zend extension and")
        || s.contains("must be loaded as a zend extension")
    {
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

/// Whether `php -m` output lists `module` under its `[Zend Modules]` heading:
/// some trimmed line after that heading (and before the next `[...]` heading)
/// equals `module` exactly.
#[must_use]
pub fn zend_module_listed(modules_output: &str, module: &str) -> bool {
    let mut in_zend = false;
    for line in modules_output.lines().map(str::trim) {
        if line.starts_with('[') && line.ends_with(']') {
            in_zend = line == "[Zend Modules]";
        } else if in_zend && line == module {
            return true;
        }
    }
    false
}

/// Classify a Zend-extension probe positively: [`interpret_probe`] must pass
/// and `module` must be listed under `[Zend Modules]` in `stdout`.
///
/// # Errors
/// The [`interpret_probe`] failure, else [`ExtLoadError::NotListed`].
pub fn require_zend_module(
    exit_ok: bool,
    stdout: &str,
    stderr: &str,
    module: &str,
) -> Result<(), ExtLoadError> {
    interpret_probe(exit_ok, &format!("{stdout}{stderr}"))?;
    if zend_module_listed(stdout, module) {
        Ok(())
    } else {
        Err(ExtLoadError::NotListed)
    }
}

/// Whether `php -m` output lists the FFI extension: true when some line,
/// trimmed (so `\r\n` endings work), is exactly `FFI`.
#[must_use]
pub fn ffi_listed(modules_output: &str) -> bool {
    modules_output.lines().any(|line| line.trim() == "FFI")
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
    fn zend_extension_custom_guard_is_iszend() {
        assert_eq!(
            interpret_probe(
                true,
                "PHP Warning: Xdebug MUST be loaded as a Zend extension in Unknown on line 0"
            ),
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

    #[test]
    fn zend_module_listed_cases() {
        let cases: &[(&str, bool)] = &[
            ("[PHP Modules]\nCore\n\n[Zend Modules]\nXdebug\n", true),
            (
                "[PHP Modules]\r\nCore\r\n\r\n[Zend Modules]\r\nXdebug\r\n",
                true,
            ),
            (
                "[PHP Modules]\nCore\n\n[Zend Modules]\nZend OPcache\nXdebug\n",
                true,
            ),
            ("[PHP Modules]\nXdebug\n\n[Zend Modules]\n", false),
            ("[PHP Modules]\nCore\n\n[Zend Modules]\nxdebug\n", false),
            ("[Zend Modules]\nXdebug3\n", false),
            ("Xdebug\n", false),
            ("", false),
        ];
        for (output, want) in cases {
            assert_eq!(zend_module_listed(output, "Xdebug"), *want, "{output:?}");
        }
    }

    #[test]
    fn require_zend_module_is_positive() {
        let listed = "[PHP Modules]\nCore\n\n[Zend Modules]\nXdebug\n";
        let unlisted = "[PHP Modules]\nCore\n\n[Zend Modules]\n";
        assert_eq!(require_zend_module(true, listed, "", "Xdebug"), Ok(()));
        assert_eq!(
            require_zend_module(true, unlisted, "", "Xdebug"),
            Err(ExtLoadError::NotListed)
        );
        assert_eq!(
            require_zend_module(true, listed, "Failed loading /x/xdebug.so", "Xdebug"),
            Err(ExtLoadError::NotLoadable)
        );
        assert_eq!(
            require_zend_module(false, listed, "", "Xdebug"),
            Err(ExtLoadError::Unknown)
        );
    }

    #[test]
    fn ffi_listed_matches_only_an_exact_ffi_line() {
        let cases: &[(&str, bool)] = &[
            (
                "[PHP Modules]\nCore\nctype\nFFI\nzlib\n\n[Zend Modules]\nZend OPcache\n",
                true,
            ),
            (
                "[PHP Modules]\nCore\nctype\nzlib\n\n[Zend Modules]\nZend OPcache\n",
                false,
            ),
            ("[PHP Modules]\r\nCore\r\nFFI\r\nzlib\r\n", true),
            ("[PHP Modules]\nFFI_extra\nffi\n", false),
            ("", false),
        ];
        for (output, want) in cases {
            assert_eq!(ffi_listed(output), *want, "{output:?}");
        }
    }
}
