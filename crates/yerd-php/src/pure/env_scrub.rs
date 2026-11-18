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

/// Environment variables that override Yerd's managed Xdebug settings:
/// `XDEBUG_MODE` replaces `xdebug.mode`, and `XDEBUG_TRIGGER` /
/// `XDEBUG_SESSION` in the environment would start a session on every request.
pub const XDEBUG_OVERRIDES: [&str; 3] = ["XDEBUG_MODE", "XDEBUG_TRIGGER", "XDEBUG_SESSION"];

/// [`allowlist`] for one FPM pool. When the pool loads Yerd's managed Xdebug,
/// the [`XDEBUG_OVERRIDES`] are dropped too so the daemon's environment can't
/// override the version's Xdebug settings; other `XDEBUG_*` variables (such as
/// `XDEBUG_CONFIG`) are kept.
#[must_use]
pub fn pool_env(snapshot: &[(String, String)], managed_xdebug: bool) -> Vec<(String, String)> {
    snapshot
        .iter()
        .filter(|(k, _)| keep(k) && !(managed_xdebug && XDEBUG_OVERRIDES.contains(&k.as_str())))
        .cloned()
        .collect()
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
    fn pool_env_drops_xdebug_overrides_only_with_managed_xdebug() {
        let input = vec![
            s("PATH", "/bin"),
            s("XDEBUG_MODE", "coverage"),
            s("XDEBUG_TRIGGER", "1"),
            s("XDEBUG_SESSION", "PHPSTORM"),
            s("XDEBUG_CONFIG", "idekey=PHPSTORM"),
            s("SECRET", "x"),
        ];
        let cases: [(bool, &[&str]); 2] = [
            (
                false,
                &[
                    "PATH",
                    "XDEBUG_MODE",
                    "XDEBUG_TRIGGER",
                    "XDEBUG_SESSION",
                    "XDEBUG_CONFIG",
                ],
            ),
            (true, &["PATH", "XDEBUG_CONFIG"]),
        ];
        for (managed, want) in cases {
            let out = pool_env(&input, managed);
            let keys: Vec<&str> = out.iter().map(|(k, _)| k.as_str()).collect();
            assert_eq!(keys, want, "managed = {managed}");
        }
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
