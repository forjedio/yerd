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
//! The GUI cannot read these consts (it learns the host OS at runtime over
//! IPC); `apps/yerd-gui/src/lib/phpVocab.ts` mirrors them, and its table test
//! pins the same values.
//!
//! Not every const has a Rust reader, and that is deliberate rather than dead
//! weight: [`POOL`] is the only one composed into a Rust string on both hosts,
//! [`POOL_SHORT`] is read by the Unix-only PHP download label, and [`RUNTIME`]
//! and [`EXT_SUFFIX`] exist so this module stays the single definition of the
//! vocabulary that the TypeScript mirror is checked against. Deleting the two
//! unread ones would move the definition of "what Yerd calls this on Windows"
//! into the GUI, which is the wrong place for it.

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

/// The short form for tight spaces such as a table column header or a download
/// progress label.
pub const POOL_SHORT: &str = if cfg!(windows) { "php-cgi" } else { "FPM" };

/// The host's dynamic-extension file suffix, including the leading dot.
pub const EXT_SUFFIX: &str = if cfg!(windows) { ".dll" } else { ".so" };

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
        assert_eq!(POOL_SHORT, "FPM");
        assert_eq!(EXT_SUFFIX, ".so");
    }

    #[cfg(windows)]
    #[test]
    fn windows_table_is_php_cgi_flavoured() {
        assert_eq!(RUNTIME, "php-cgi");
        assert_eq!(POOL, "FastCGI process");
        assert_eq!(POOL_SHORT, "php-cgi");
        assert_eq!(EXT_SUFFIX, ".dll");
        for value in [RUNTIME, POOL, POOL_SHORT] {
            assert!(!value.contains("FPM"), "{value} still says FPM on Windows");
        }
    }

    /// The suffix is interpolated straight into user-facing sentences, so it
    /// carries its own dot on both hosts.
    #[test]
    fn ext_suffix_carries_its_leading_dot() {
        assert!(EXT_SUFFIX.starts_with('.'), "{EXT_SUFFIX}");
    }
}
