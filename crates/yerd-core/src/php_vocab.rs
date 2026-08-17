//! Host-appropriate nouns for the supervised PHP web runtime.
//!
//! Unix serves sites through PHP-FPM, which supervises a pool of workers per
//! version. Windows has no FPM SAPI: the daemon runs `php-cgi.exe` in `FastCGI`
//! mode, one process per version with no worker pool. User-facing text that
//! says "FPM pool" on Windows is therefore simply wrong, so every string the
//! daemon and `yerd-doctor` compose for a human reads these consts instead of
//! spelling the noun inline.
//!
//! Both arms compile on every OS (`if cfg!(windows)` in const position, not
//! `#[cfg]`), which keeps the table table-testable from Linux and macOS CI as
//! well. This mirrors the per-OS wording helpers in `yerd-doctor`
//! (`resolver_remedy`, `foreign_web_listener_remedy`) rather than inventing a
//! new pattern.
//!
//! Serialised identifiers are deliberately *not* covered here:
//! `DiagnosisCode::FpmPoolFailed`, the `pool` IPC field, the `max_children`
//! config key, and the `fpm-<version>-<id>.pid` / `.log` filenames are wire and
//! on-disk contracts that stay spelled "fpm" on every OS. Only human-readable
//! text changes.
//!
//! This module is the single definition of the vocabulary. The GUI does not
//! mirror it: the Tauri backend builds its `host_platform` response from these
//! items and the frontend renders whatever it is handed, so a change here
//! reaches every surface without a second table to keep in step.

/// The supervised web runtime's name, for prose about the daemon itself:
/// "yerdd supervises PHP-FPM".
pub const RUNTIME: &str = if cfg!(windows) { "php-cgi" } else { "PHP-FPM" };

/// The per-version serving unit, as used in "the PHP 8.3 {POOL} is not
/// running". Singular; see [`RUNTIME`] for the engine itself.
pub const POOL: &str = if cfg!(windows) {
    "FastCGI process"
} else {
    "FPM pool"
};

/// The plural of [`POOL`]. Deliberately not `POOL` plus an `s`: that would
/// render `FastCGI processs` on Windows. Both arms start capitalised, so a
/// caller can interpolate them sentence-initially without recasing.
pub const POOL_PLURAL: &str = if cfg!(windows) {
    "FastCGI processes"
} else {
    "FPM pools"
};

/// The short form for tight spaces such as a table column header or a download
/// progress label.
pub const POOL_SHORT: &str = if cfg!(windows) { "php-cgi" } else { "FPM" };

/// The host's dynamic-extension file suffix, including the leading dot.
pub const EXT_SUFFIX: &str = if cfg!(windows) { ".dll" } else { ".so" };

/// A realistic host-shaped extension path, for an input placeholder or a test
/// fixture.
///
/// Deliberately **prefix-free**: real Windows extension DLLs are usually named
/// `php_<stem>.dll`, but [`crate::php_extensions::default_name_from_path`]
/// derives an extension's name by stripping only the host *suffix*, never a
/// `php_` prefix. Baking the prefix in here would shift every derived-name
/// assertion on Windows. A caller that wants the prefix passes it as part of
/// `stem`.
#[must_use]
pub fn example_ext_path(stem: &str, suffix: &str) -> String {
    if cfg!(windows) {
        format!(r"C:\php\ext\{stem}{suffix}")
    } else {
        format!("/opt/homebrew/lib/php/pecl/20250925/{stem}{suffix}")
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

    /// The whole table, pinned per OS. Mirrors `yerd-doctor`'s
    /// `foreign_web_listener_remedy_is_platform_aware` style: assert the values
    /// this host must produce, and assert the other OS's noun is absent so a
    /// half-applied edit cannot pass.
    #[cfg(not(windows))]
    #[test]
    fn unix_table_is_fpm_flavoured() {
        assert_eq!(RUNTIME, "PHP-FPM");
        assert_eq!(POOL, "FPM pool");
        assert_eq!(POOL_PLURAL, "FPM pools");
        assert_eq!(POOL_SHORT, "FPM");
        assert_eq!(EXT_SUFFIX, ".so");
        assert_eq!(
            example_ext_path("scrypt", EXT_SUFFIX),
            "/opt/homebrew/lib/php/pecl/20250925/scrypt.so"
        );
    }

    #[cfg(windows)]
    #[test]
    fn windows_table_is_php_cgi_flavoured() {
        assert_eq!(RUNTIME, "php-cgi");
        assert_eq!(POOL, "FastCGI process");
        assert_eq!(POOL_PLURAL, "FastCGI processes");
        assert_eq!(POOL_SHORT, "php-cgi");
        assert_eq!(EXT_SUFFIX, ".dll");
        assert_eq!(
            example_ext_path("php_scrypt", EXT_SUFFIX),
            r"C:\php\ext\php_scrypt.dll"
        );
        for value in [RUNTIME, POOL, POOL_PLURAL, POOL_SHORT] {
            assert!(!value.contains("FPM"), "{value} still says FPM on Windows");
        }
    }

    /// On Windows the plural must not be the singular plus an `s`, which would
    /// render `FastCGI processs`. Unix's "FPM pool" pluralises regularly, so
    /// only the capitalisation invariant is shared. Previously owned by the
    /// GUI's vitest suite, which no longer holds a table of its own.
    #[test]
    fn pool_plural_is_well_formed() {
        assert!(POOL_PLURAL.starts_with(char::is_uppercase));
        assert!(!POOL_PLURAL.contains("sss"), "{POOL_PLURAL}");
        #[cfg(windows)]
        assert_ne!(POOL_PLURAL, format!("{POOL}s"));
    }

    /// The helper must not bake in a `php_` prefix: name derivation strips only
    /// the suffix, so a baked-in prefix would shift every derived-name
    /// assertion on Windows.
    #[test]
    fn example_ext_path_does_not_prefix_the_stem() {
        let path = example_ext_path("scrypt", EXT_SUFFIX);
        assert!(path.ends_with(&format!("scrypt{EXT_SUFFIX}")), "{path}");
        assert!(!path.contains("php_scrypt"), "{path}");
    }

    /// The suffix is interpolated straight into user-facing sentences, so it
    /// carries its own dot on both hosts.
    #[test]
    fn ext_suffix_carries_its_leading_dot() {
        assert!(EXT_SUFFIX.starts_with('.'), "{EXT_SUFFIX}");
    }
}
