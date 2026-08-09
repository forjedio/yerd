//! Resolve which real, on-disk PHP script a request should execute - the
//! `try_files $uri $uri/index.php` half of the classic WordPress/nginx front-
//! controller policy, extending `pure::cgi_params`'s "everything to
//! `index.php`" fallback to first check for a real, more specific script
//! (`wp-admin/index.php`, `wp-login.php`, ...) before falling back to the
//! site root's `index.php`. It also decides the third outcome web servers
//! have always had here: a path that names a real directory but arrived
//! without its trailing slash earns a `301` to the slashed form rather than
//! silently running the root front controller.
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
//! host's filesystem. (For GET/HEAD, `static_file::try_serve` will already
//! have answered `403` for the escaping path before resolution runs; the
//! fallback here is what the other methods observe.)

use std::path::{Path, PathBuf};

use crate::forward::static_file::{canonical_within, Containment};
use crate::pure::try_files::{directory_candidate, is_php_source, static_candidate};

/// Outcome of resolving a direct-mode request against the on-disk tree.
#[derive(Debug, PartialEq, Eq)]
pub enum ScriptResolution {
    /// A real, on-disk PHP script to execute, relative to `served_root`.
    Script(PathBuf),
    /// The path names a real directory without a trailing slash; answer
    /// `301` to the trailing-slash form.
    DirectoryRedirect,
    /// Nothing matched - fall back to the site root's `index.php`.
    Fallback,
}

/// How `uri_path` resolves against the site's real, on-disk tree: a specific
/// script to execute, a trailing-slash redirect, or a fallback to the site
/// root's `index.php` (today's unconditional behavior, unchanged for every
/// framework that has only one front controller).
///
/// Checks, in order: an exact non-directory match (`/wp-login.php` ->
/// `wp-login.php`), then that same path as a directory missing its trailing
/// slash (`/sub` -> redirect to `/sub/`), then - for a directory-style request
/// - that directory's own index (`/wp-admin/` -> `wp-admin/index.php`).
///
/// The redirect fires whether or not the directory holds an `index.php`, which
/// is what Apache's `DirectorySlash` and nginx's `try_files $uri $uri/` do: a
/// static-only `/assets` redirects to `/assets/`, where
/// [`crate::forward::static_file::try_serve_index`] can serve its
/// `index.html`.
pub async fn resolve_script(
    uri_path: &str,
    served_root: &Path,
    allowed_root: &Path,
    symlink_protection: bool,
) -> ScriptResolution {
    let Ok(real_root) = tokio::fs::canonicalize(allowed_root).await else {
        return ScriptResolution::Fallback;
    };

    if let Some(rel) = static_candidate(uri_path) {
        if let Some(script) =
            existing_php_file(served_root, &real_root, &rel, symlink_protection).await
        {
            return ScriptResolution::Script(script);
        }
        if is_existing_directory(served_root, &real_root, &rel, symlink_protection).await {
            return ScriptResolution::DirectoryRedirect;
        }
        return ScriptResolution::Fallback;
    }

    let Some(dir_rel) = directory_candidate(uri_path) else {
        return ScriptResolution::Fallback;
    };
    let script_rel = dir_rel.join("index.php");
    match existing_php_file(served_root, &real_root, &script_rel, symlink_protection).await {
        Some(script) => ScriptResolution::Script(script),
        None => ScriptResolution::Fallback,
    }
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
pub(crate) async fn existing_php_file(
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

/// Resolve a routing rule's PHP target (relative to `served_root`) to the
/// `script_rel` FastCGI should execute, or `None` when it is missing, is not
/// PHP source, or escapes `allowed_root`.
///
/// The caller has already established that the target *looks* like PHP source;
/// this re-checks that against the resolved path and applies the same
/// containment discipline as [`resolve_script`], so an operator-configured rule
/// can never point FastCGI outside the site's tree. Canonicalises
/// `allowed_root` itself, so callers pass the raw site paths.
pub(crate) async fn resolve_rule_target(
    served_root: &Path,
    allowed_root: &Path,
    target_rel: &Path,
    symlink_protection: bool,
) -> Option<PathBuf> {
    let real_root = tokio::fs::canonicalize(allowed_root).await.ok()?;
    existing_php_file(served_root, &real_root, target_rel, symlink_protection).await
}

/// Whether `rel` (relative to `served_root`) is a real, on-disk directory that
/// canonicalises within `real_root`. Deliberately mirrors
/// [`existing_php_file`]'s containment `match` so the two probes can't drift
/// apart on symlink semantics: with protection on, a directory symlink
/// escaping `real_root` is never a redirect candidate; with it off, the
/// symlink target is accepted. Note that a GET/HEAD request for such an
/// escaping path never actually reaches this refusal:
/// [`crate::forward::static_file::try_serve`] has already answered `403` for
/// it before script resolution runs. Only non-GET/HEAD methods, which the
/// caller never redirects anyway, get here and fall back to the root
/// `index.php`.
async fn is_existing_directory(
    served_root: &Path,
    real_root: &Path,
    rel: &Path,
    symlink_protection: bool,
) -> bool {
    let real_dir = match canonical_within(&served_root.join(rel), real_root).await {
        Some(Containment::Ok(path)) => path,
        Some(Containment::Escaped(path)) if !symlink_protection => path,
        Some(Containment::Escaped(_)) | None => return false,
    };
    tokio::fs::metadata(&real_dir)
        .await
        .is_ok_and(|meta| meta.is_dir())
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
        assert_eq!(rel, ScriptResolution::Script(PathBuf::from("wp-login.php")));
    }

    #[tokio::test]
    async fn resolves_subdirectory_index_for_trailing_slash() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir(root.path().join("wp-admin")).unwrap();
        std::fs::write(root.path().join("wp-admin/index.php"), b"<?php").unwrap();

        let rel = resolve_script("/wp-admin/", root.path(), root.path(), true).await;
        assert_eq!(
            rel,
            ScriptResolution::Script(PathBuf::from("wp-admin/index.php"))
        );
    }

    #[tokio::test]
    async fn subdirectory_with_no_index_php_falls_back() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir(root.path().join("empty")).unwrap();

        assert_eq!(
            resolve_script("/empty/", root.path(), root.path(), true).await,
            ScriptResolution::Fallback
        );
    }

    #[tokio::test]
    async fn directory_without_trailing_slash_redirects() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir(root.path().join("sub")).unwrap();
        std::fs::write(root.path().join("sub/index.php"), b"<?php").unwrap();

        assert_eq!(
            resolve_script("/sub", root.path(), root.path(), true).await,
            ScriptResolution::DirectoryRedirect
        );
    }

    #[tokio::test]
    async fn directory_without_index_php_still_redirects() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir(root.path().join("assets")).unwrap();
        std::fs::write(root.path().join("assets/index.html"), b"<h1>").unwrap();

        assert_eq!(
            resolve_script("/assets", root.path(), root.path(), true).await,
            ScriptResolution::DirectoryRedirect
        );
    }

    /// Unit-level contract only: `resolve_script` refuses to treat the
    /// escaping symlink as a directory, so it is not a redirect candidate.
    /// In the server, a GET/HEAD for this path never reaches
    /// `resolve_script` at all - `static_file::try_serve` answers `403`
    /// first (see `is_existing_directory`'s doc).
    #[cfg(unix)]
    #[tokio::test]
    async fn symlinked_directory_escaping_document_root_is_not_a_redirect_candidate() {
        let docroot = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::fs::create_dir(outside.path().join("secrets")).unwrap();
        std::os::unix::fs::symlink(outside.path().join("secrets"), docroot.path().join("sub"))
            .unwrap();

        assert_eq!(
            resolve_script("/sub", docroot.path(), docroot.path(), true).await,
            ScriptResolution::Fallback
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn symlinked_directory_escaping_document_root_redirects_when_protection_off() {
        let docroot = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::fs::create_dir(outside.path().join("shared")).unwrap();
        std::os::unix::fs::symlink(outside.path().join("shared"), docroot.path().join("sub"))
            .unwrap();

        assert_eq!(
            resolve_script("/sub", docroot.path(), docroot.path(), false).await,
            ScriptResolution::DirectoryRedirect,
            "protection off treats the escaping symlink as the real directory it points at"
        );
    }

    #[tokio::test]
    async fn missing_exact_file_falls_back() {
        let root = tempfile::tempdir().unwrap();
        assert_eq!(
            resolve_script("/wp-login.php", root.path(), root.path(), true).await,
            ScriptResolution::Fallback
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
            ScriptResolution::Fallback
        );
    }

    #[tokio::test]
    async fn root_path_resolves_to_root_index_php() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("index.php"), b"<?php").unwrap();

        let rel = resolve_script("/", root.path(), root.path(), true).await;
        assert_eq!(rel, ScriptResolution::Script(PathBuf::from("index.php")));
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
            ScriptResolution::Fallback
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
            ScriptResolution::Script(PathBuf::from("wp-login.php")),
            "protection off resolves the escaping script by its served-root-relative path"
        );
    }

    #[tokio::test]
    async fn traversal_attempt_falls_back() {
        let root = tempfile::tempdir().unwrap();
        assert_eq!(
            resolve_script("/../../etc/passwd", root.path(), root.path(), true).await,
            ScriptResolution::Fallback
        );
    }

    #[tokio::test]
    async fn rule_target_resolves_to_a_nested_front_controller() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir(root.path().join("api")).unwrap();
        std::fs::write(root.path().join("api/index.php"), b"<?php").unwrap();

        assert_eq!(
            resolve_rule_target(root.path(), root.path(), Path::new("api/index.php"), true).await,
            Some(PathBuf::from("api/index.php"))
        );
    }

    #[tokio::test]
    async fn rule_target_missing_or_not_php_is_none() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("index.html"), b"spa").unwrap();

        assert_eq!(
            resolve_rule_target(root.path(), root.path(), Path::new("api/index.php"), true).await,
            None,
            "missing target"
        );
        assert_eq!(
            resolve_rule_target(root.path(), root.path(), Path::new("index.html"), true).await,
            None,
            "target is not PHP source"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn rule_target_escaping_document_root_is_refused_unless_protection_is_off() {
        let docroot = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("shared.php"), b"<?php").unwrap();
        std::os::unix::fs::symlink(
            outside.path().join("shared.php"),
            docroot.path().join("api.php"),
        )
        .unwrap();

        assert_eq!(
            resolve_rule_target(docroot.path(), docroot.path(), Path::new("api.php"), true).await,
            None,
            "protection on refuses the escaping target"
        );
        assert_eq!(
            resolve_rule_target(docroot.path(), docroot.path(), Path::new("api.php"), false).await,
            Some(PathBuf::from("api.php")),
            "protection off resolves it by its served-root-relative path"
        );
    }
}
