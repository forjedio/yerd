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
///   - Windows only: the system variables `php-cgi.exe` needs to start at all
///     (`SystemRoot`, `WINDIR`, `TEMP`, `PATHEXT`, ...), which have no Unix
///     equivalent - without them the Windows loader can't resolve core DLLs.
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
        || keep_windows_system(key)
}

/// Windows system variables required for a native process to launch. Matched
/// case-insensitively (Windows env keys are case-insensitive). No-op on Unix.
#[cfg(windows)]
fn keep_windows_system(key: &str) -> bool {
    const WINDOWS_SYSTEM: &[&str] = &[
        "SYSTEMROOT",
        "SYSTEMDRIVE",
        "WINDIR",
        "TEMP",
        "TMP",
        "COMSPEC",
        "PATHEXT",
        "NUMBER_OF_PROCESSORS",
        "PROCESSOR_ARCHITECTURE",
        "USERPROFILE",
        "USERNAME",
        "COMPUTERNAME",
        "APPDATA",
        "LOCALAPPDATA",
        "PROGRAMDATA",
        "PROGRAMFILES",
        "PROGRAMFILES(X86)",
        "COMMONPROGRAMFILES",
    ];
    let upper = key.to_ascii_uppercase();
    WINDOWS_SYSTEM.contains(&upper.as_str())
}

#[cfg(not(windows))]
fn keep_windows_system(_key: &str) -> bool {
    false
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

    #[cfg(windows)]
    #[test]
    fn keeps_windows_system_vars_case_insensitively() {
        let input = vec![
            s("SystemRoot", r"C:\Windows"),
            s("windir", r"C:\Windows"),
            s("TEMP", r"C:\Temp"),
            s("PATHEXT", ".EXE;.BAT"),
            s("SECRET", "no"),
        ];
        let out = allowlist(&input);
        let keys: Vec<&str> = out.iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(keys, vec!["SystemRoot", "windir", "TEMP", "PATHEXT"]);
    }
}
