//! `WordPress` marker detection, used to populate `DaemonState.wordpress_sites`
//! (see `startup::build_routing`) - not called on the `ListSites` poll path
//! itself; see that module's doc comment for why.
//!
//! Deliberately **not** `yerd_platform::gather_project_signals` (the
//! heavier, general-purpose detector `Link`-time detection uses) - that
//! probes roughly ten root markers, reads `composer.json`, and stats several
//! web-root candidates per call. This only needs a narrow check: does either
//! `WordPress` marker file exist.

use std::path::Path;

/// Detect `WordPress` at `served_root`: does either marker file exist.
pub(crate) fn is_wordpress(served_root: &Path) -> bool {
    served_root.join("wp-config.php").is_file() || served_root.join("wp-load.php").is_file()
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn absent_when_no_marker_files() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(!is_wordpress(tmp.path()));
    }

    #[test]
    fn true_via_wp_config() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("wp-config.php"), b"<?php").unwrap();
        assert!(is_wordpress(tmp.path()));
    }

    #[test]
    fn true_via_wp_load() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("wp-load.php"), b"<?php").unwrap();
        assert!(is_wordpress(tmp.path()));
    }
}
