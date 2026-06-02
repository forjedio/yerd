//! Environment variable allowlist filter.
//!
//! Pure: takes the snapshot as a slice, never reads `std::env` itself.
//! Caller is responsible for snapshotting before invoking.

/// Filter a snapshot of environment variables down to an FPM-safe
/// allowlist.
///
/// Retained:
///   - Exact: `PATH`, `HOME`, `USER`, `LANG`
///   - Prefix: `LC_`, `XDEBUG_`, `PHP_`
///
/// Order of returned pairs matches the order of `snapshot`.
#[must_use]
pub fn allowlist(snapshot: &[(String, String)]) -> Vec<(String, String)> {
    snapshot.iter().filter(|(k, _)| keep(k)).cloned().collect()
}

fn keep(key: &str) -> bool {
    matches!(key, "PATH" | "HOME" | "USER" | "LANG")
        || key.starts_with("LC_")
        || key.starts_with("XDEBUG_")
        || key.starts_with("PHP_")
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

    fn s(k: &str, v: &str) -> (String, String) {
        (k.to_owned(), v.to_owned())
    }

    #[test]
    fn keeps_exact_matches() {
        let input = vec![
            s("PATH", "/usr/bin"),
            s("HOME", "/home/me"),
            s("USER", "me"),
            s("LANG", "en_US.UTF-8"),
            s("SECRET_KEY", "hunter2"),
        ];
        let out = allowlist(&input);
        let keys: Vec<&str> = out.iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(keys, vec!["PATH", "HOME", "USER", "LANG"]);
    }

    #[test]
    fn keeps_prefix_matches() {
        let input = vec![
            s("LC_ALL", "en_US.UTF-8"),
            s("LC_TIME", "C"),
            s("XDEBUG_CONFIG", "idekey=PHPSTORM"),
            s("PHP_INI_SCAN_DIR", "/etc"),
            s("MY_LANG", "fake"),
            s("ALC_FOO", "no"),
        ];
        let out = allowlist(&input);
        let keys: Vec<&str> = out.iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(
            keys,
            vec!["LC_ALL", "LC_TIME", "XDEBUG_CONFIG", "PHP_INI_SCAN_DIR"]
        );
    }

    #[test]
    fn preserves_input_order() {
        let input = vec![s("PHP_X", "1"), s("PATH", "/bin"), s("LC_TIME", "C")];
        let out = allowlist(&input);
        let keys: Vec<&str> = out.iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(keys, vec!["PHP_X", "PATH", "LC_TIME"]);
    }

    #[test]
    fn empty_input_yields_empty_output() {
        let out = allowlist(&[]);
        assert!(out.is_empty());
    }

    #[test]
    fn drops_unknown_keys() {
        let input = vec![
            s("AWS_SECRET_ACCESS_KEY", "x"),
            s("LANG_OVERRIDE", "no"),
            s("xdebug_lower", "no"),
        ];
        let out = allowlist(&input);
        assert!(out.is_empty());
    }
}
