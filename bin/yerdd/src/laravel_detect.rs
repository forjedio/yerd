//! Laravel marker detection, used to gate per-site services (Reverb) and to
//! populate the `is_laravel` flag on `SiteEntry`.
//!
//! Unlike [`crate::wordpress_detect`] (which checks the *served* root, e.g.
//! `public/`), Laravel's `artisan` entrypoint lives at the **project root** -
//! the site's `document_root`, one level above `public/`. This narrow check
//! (does `artisan` exist) is deliberately cheaper than
//! `yerd_platform::gather_project_signals`.

use std::path::Path;

/// Detect Laravel at `document_root`: is there an `artisan` file at the project
/// root. Callers pass `Site::document_root()`, not `served_root()`.
pub(crate) fn is_laravel(document_root: &Path) -> bool {
    document_root.join("artisan").is_file()
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn absent_when_no_artisan() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(!is_laravel(tmp.path()));
    }

    #[test]
    fn present_via_artisan_at_project_root() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("artisan"), b"#!/usr/bin/env php").unwrap();
        assert!(is_laravel(tmp.path()));
    }

    /// `artisan` under `public/` (the served root) must NOT count - detection is
    /// anchored at the project root.
    #[test]
    fn artisan_under_public_does_not_count_for_project_root() {
        let tmp = tempfile::tempdir().unwrap();
        let public = tmp.path().join("public");
        std::fs::create_dir_all(&public).unwrap();
        std::fs::write(public.join("artisan"), b"x").unwrap();
        assert!(!is_laravel(tmp.path()));
    }
}
