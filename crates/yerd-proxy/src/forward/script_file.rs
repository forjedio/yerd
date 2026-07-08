//! Resolve which real, on-disk PHP script a request should execute - the
//! `try_files $uri $uri/index.php` half of the classic WordPress/nginx front-
//! controller policy, extending `pure::cgi_params`'s "everything to
//! `index.php`" fallback to first check for a real, more specific script
//! (`wp-admin/index.php`, `wp-login.php`, ...) before falling back to the
//! site root's `index.php`.
//!
//! Unlike [`crate::forward::static_file`], this applies to every HTTP method,
//! not just GET/HEAD - a real script like `wp-login.php` handles POST too.
//! It never reads or serves file *content*; it only decides which path FCGI
//! should be told to execute. Mirrors `static_file`'s canonicalise-and-check-
//! containment pattern: a symlinked script that resolves outside
//! `document_root` is treated as not found (falls back to the root
//! `index.php` policy) rather than handed to FastCGI, the same way a
//! symlinked static asset is refused - otherwise a symlink inside a site's
//! own tree could point FastCGI at an arbitrary `.php` file elsewhere on the
//! host's filesystem.

use std::path::{Path, PathBuf};

use crate::forward::static_file::{canonical_within, Containment};
use crate::pure::try_files::{directory_candidate, is_php_source, static_candidate};

/// The real, on-disk PHP script - relative to `served_root` - that `uri_path`
/// should execute, or `None` when there is no such real script and the
/// caller should fall back to the site's root `index.php` (today's
/// unconditional behavior, unchanged for every framework that has only one
/// front controller).
///
/// Checks, in order: an exact non-directory match (`/wp-login.php` ->
/// `wp-login.php`), then - for a directory-style request - that directory's
/// own index (`/wp-admin/` -> `wp-admin/index.php`).
pub async fn resolve_script(
    uri_path: &str,
    served_root: &Path,
    allowed_root: &Path,
    symlink_protection: bool,
) -> Option<PathBuf> {
    let real_root = tokio::fs::canonicalize(allowed_root).await.ok()?;

    if let Some(rel) = static_candidate(uri_path) {
        return existing_php_file(served_root, &real_root, &rel, symlink_protection).await;
    }

    let dir_rel = directory_candidate(uri_path)?;
    let script_rel = dir_rel.join("index.php");
    existing_php_file(served_root, &real_root, &script_rel, symlink_protection).await
}

/// `rel` (relative to `served_root`) if it's a real, on-disk `.php` file that
/// canonicalises within `real_root` - `None` otherwise (missing, a
/// directory, not `.php`, or a symlink escaping `real_root`).
///
/// When `symlink_protection` is `false`, a symlink escaping `real_root` is
/// accepted (its canonical path is used only for the `is_file` probe; the
/// returned value stays the `served_root`-relative `rel` so FastCGI's
/// `DOCUMENT_ROOT`/`SCRIPT_FILENAME` are unaffected and FPM follows the symlink
/// itself).
async fn existing_php_file(
    served_root: &Path,
    real_root: &Path,
    rel: &Path,
    symlink_protection: bool,
) -> Option<PathBuf> {
    if !is_php_source(rel) {
        return None;
    }
    let real_file = match canonical_within(&served_root.join(rel), real_root).await {
        Some(Containment::Ok(path)) => path,
        Some(Containment::Escaped(path)) if !symlink_protection => path,
        Some(Containment::Escaped(_)) | None => return None,
    };
    tokio::fs::metadata(&real_file)
        .await
        .ok()
        .filter(std::fs::Metadata::is_file)?;
    Some(rel.to_path_buf())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn resolves_exact_php_file_match() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("wp-login.php"), b"<?php").unwrap();

        let rel = resolve_script("/wp-login.php", root.path(), root.path(), true).await;
        assert_eq!(rel, Some(PathBuf::from("wp-login.php")));
    }

    #[tokio::test]
    async fn resolves_subdirectory_index_for_trailing_slash() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir(root.path().join("wp-admin")).unwrap();
        std::fs::write(root.path().join("wp-admin/index.php"), b"<?php").unwrap();

        let rel = resolve_script("/wp-admin/", root.path(), root.path(), true).await;
        assert_eq!(rel, Some(PathBuf::from("wp-admin/index.php")));
    }

    #[tokio::test]
    async fn subdirectory_with_no_index_php_falls_back() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir(root.path().join("empty")).unwrap();

        assert_eq!(
            resolve_script("/empty/", root.path(), root.path(), true).await,
            None
        );
    }

    #[tokio::test]
    async fn missing_exact_file_falls_back() {
        let root = tempfile::tempdir().unwrap();
        assert_eq!(
            resolve_script("/wp-login.php", root.path(), root.path(), true).await,
            None
        );
    }

    #[tokio::test]
    async fn non_php_exact_match_falls_back() {
        // A real, existing non-PHP file at this path is `static_file`'s job to
        // serve (it wins earlier in dispatch); resolve_script must never treat
        // it as a script candidate.
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("app.css"), b"body{}").unwrap();

        assert_eq!(
            resolve_script("/app.css", root.path(), root.path(), true).await,
            None
        );
    }

    #[tokio::test]
    async fn root_path_resolves_to_root_index_php() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("index.php"), b"<?php").unwrap();

        let rel = resolve_script("/", root.path(), root.path(), true).await;
        assert_eq!(rel, Some(PathBuf::from("index.php")));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn symlinked_script_escaping_document_root_falls_back() {
        let docroot = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("shell.php"), b"<?php").unwrap();
        std::os::unix::fs::symlink(
            outside.path().join("shell.php"),
            docroot.path().join("wp-login.php"),
        )
        .unwrap();

        assert_eq!(
            resolve_script("/wp-login.php", docroot.path(), docroot.path(), true).await,
            None
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn symlinked_script_escaping_document_root_resolves_when_protection_off() {
        let docroot = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("shared.php"), b"<?php").unwrap();
        std::os::unix::fs::symlink(
            outside.path().join("shared.php"),
            docroot.path().join("wp-login.php"),
        )
        .unwrap();

        assert_eq!(
            resolve_script("/wp-login.php", docroot.path(), docroot.path(), false).await,
            Some(PathBuf::from("wp-login.php")),
            "protection off resolves the escaping script by its served-root-relative path"
        );
    }

    #[tokio::test]
    async fn traversal_attempt_falls_back() {
        let root = tempfile::tempdir().unwrap();
        assert_eq!(
            resolve_script("/../../etc/passwd", root.path(), root.path(), true).await,
            None
        );
    }
}
