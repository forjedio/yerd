//! Live WordPress detection for [`Response::Sites`](yerd_ipc::Response::Sites).
//!
//! Deliberately **not** `yerd_platform::gather_project_signals` (the
//! heavier, general-purpose detector `Link`-time detection uses) - that
//! probes roughly ten root markers, reads `composer.json`, and stats several
//! web-root candidates per call. This only needs a narrow check: does either
//! WordPress marker file exist, and if so, what does its version file say.
//! Blocking filesystem I/O - callers on a hot, frequently-polled path (the
//! `ListSites` handler) must run this off the async executor, e.g. inside
//! `tokio::task::spawn_blocking`.

use std::path::Path;

/// Detect WordPress at `served_root`: `(is_wordpress, version)`. `version` is
/// `None` whenever `is_wordpress` is `false`, and may also be `None` even
/// when `is_wordpress` is `true` if `wp-includes/version.php` is missing or
/// doesn't parse - a real WordPress site with an unreadable version file
/// still counts as WordPress.
pub(crate) fn detect(served_root: &Path) -> (bool, Option<String>) {
    let is_wordpress =
        served_root.join("wp-config.php").is_file() || served_root.join("wp-load.php").is_file();
    if !is_wordpress {
        return (false, None);
    }
    (true, read_version(served_root))
}

fn read_version(served_root: &Path) -> Option<String> {
    let text = std::fs::read_to_string(served_root.join("wp-includes").join("version.php")).ok()?;
    parse_version(&text)
}

/// Pure - parses a `$wp_version = '6.4.2';`-shaped line out of
/// `wp-includes/version.php`. Tolerates a missing/reformatted line by
/// returning `None` rather than erroring: the core version-file format is
/// not a stability contract WordPress makes to consumers.
fn parse_version(text: &str) -> Option<String> {
    for line in text.lines() {
        let Some(rest) = line.trim().strip_prefix("$wp_version") else {
            continue;
        };
        let rest = rest.trim_start().strip_prefix('=')?.trim_start();
        let rest = rest.strip_prefix('\'').or_else(|| rest.strip_prefix('"'))?;
        let end = rest.find(['\'', '"'])?;
        return rest.get(..end).map(str::to_owned);
    }
    None
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn detect_absent_when_no_marker_files() {
        let tmp = tempfile::tempdir().unwrap();
        assert_eq!(detect(tmp.path()), (false, None));
    }

    #[test]
    fn detect_true_via_wp_config() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("wp-config.php"), b"<?php").unwrap();
        assert_eq!(detect(tmp.path()), (true, None));
    }

    #[test]
    fn detect_true_via_wp_load_with_no_version_file() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("wp-load.php"), b"<?php").unwrap();
        assert_eq!(detect(tmp.path()), (true, None));
    }

    #[test]
    fn detect_reads_version_when_present() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("wp-config.php"), b"<?php").unwrap();
        std::fs::create_dir_all(tmp.path().join("wp-includes")).unwrap();
        std::fs::write(
            tmp.path().join("wp-includes").join("version.php"),
            "<?php\n$wp_version = '6.4.2';\n$wp_db_version = 53496;\n",
        )
        .unwrap();
        assert_eq!(detect(tmp.path()), (true, Some("6.4.2".to_owned())));
    }

    #[test]
    fn parse_version_handles_double_quotes() {
        assert_eq!(
            parse_version("$wp_version = \"6.5\";"),
            Some("6.5".to_owned())
        );
    }

    #[test]
    fn parse_version_none_when_line_absent() {
        assert_eq!(parse_version("<?php\n// nothing here\n"), None);
    }

    #[test]
    fn parse_version_none_when_malformed() {
        assert_eq!(parse_version("$wp_version;"), None);
    }
}
