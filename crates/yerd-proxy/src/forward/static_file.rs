//! Static-file serving - the `try_files`-style short-circuit in front of the
//! PHP front controller.
//!
//! A GET/HEAD whose URL resolves to a real, non-PHP file under the served root
//! is streamed from disk with a guessed `Content-Type`. Anything else (missing
//! file, directory, PHP source, non-idempotent method, traversal attempt)
//! reports [`StaticOutcome::NotFound`], and the caller forwards to FastCGI
//! (`index.php`) exactly as before. Without this, `/favicon.ico` and other
//! static assets were handed to the PHP framework, which has no route for
//! them.
//!
//! A candidate is allowed to resolve, via a symlink, anywhere within the
//! site's `document_root` - not just the served subdirectory - so a symlink
//! like Laravel's `public/storage -> ../storage/app/public` is served
//! normally. A candidate that resolves outside `document_root` entirely is
//! reported as [`StaticOutcome::SymlinkEscape`], which the caller turns into
//! an explicit `403` via [`symlink_escape_response`] instead of silently
//! falling through to PHP-FPM. The resolved path and the allowed root are
//! logged, not echoed in the response body, since a site can be exposed
//! beyond loopback via `yerd-tunnel`.
//!
//! A directory request (trailing `/`, including the site root) with no
//! `index.php` falls to [`try_serve_index`], which serves `index.html` or
//! `index.htm` from that directory - the same fallback Caddy/nginx apply, so
//! plain static sites work without a PHP front controller.

use std::path::{Path, PathBuf};

use bytes::Bytes;
use http::{header, HeaderValue, Method, StatusCode};
use http_body_util::BodyExt;
use hyper::Response;

use crate::forward::{empty_body, owned_bytes_body, BoxBody};
use crate::pure::try_files::{
    content_type_for, directory_candidate, is_php_source, static_candidate,
};

/// Outcome of a static-file or directory-index lookup.
pub enum StaticOutcome {
    /// A file was found and served - return this response as-is.
    Served(Response<BoxBody>),
    /// Not a static-file match (missing, directory, PHP source, wrong
    /// method, no index file present) - fall through to the front
    /// controller, exactly as a `None` result did before this type existed.
    NotFound,
    /// The candidate resolved, via a symlink, to a real path outside
    /// `allowed_root`. The caller must answer with a 403, not fall through -
    /// falling through here previously handed the request to PHP-FPM, which
    /// rejected it with its own unexplained, unlogged 403.
    SymlinkEscape {
        /// The original request-URI path, as received.
        requested_path: String,
        /// The canonicalised real path the candidate resolved to.
        resolved: PathBuf,
        /// The canonicalised root the resolved path must stay within.
        allowed_root: PathBuf,
    },
}

/// Try to serve `uri_path` as a static file under `served_root`, allowing
/// symlinks that resolve anywhere within `allowed_root` (the site's
/// `document_root` - a superset of `served_root` when the site is served
/// from a subdirectory, e.g. Laravel's `public/`).
pub async fn try_serve(
    method: &Method,
    uri_path: &str,
    served_root: &Path,
    allowed_root: &Path,
) -> StaticOutcome {
    if *method != Method::GET && *method != Method::HEAD {
        return StaticOutcome::NotFound;
    }

    let Some(rel) = static_candidate(uri_path) else {
        return StaticOutcome::NotFound;
    };
    let Ok(real_root) = tokio::fs::canonicalize(allowed_root).await else {
        return StaticOutcome::NotFound;
    };

    let real_file = match canonical_within(&served_root.join(&rel), &real_root).await {
        Some(Containment::Ok(path)) => path,
        Some(Containment::Escaped(resolved)) => {
            return StaticOutcome::SymlinkEscape {
                requested_path: uri_path.to_owned(),
                resolved,
                allowed_root: real_root,
            };
        }
        None => return StaticOutcome::NotFound,
    };

    if is_php_source(&real_file) {
        return StaticOutcome::NotFound;
    }

    let Ok(meta) = tokio::fs::metadata(&real_file).await else {
        return StaticOutcome::NotFound;
    };
    if !meta.is_file() {
        return StaticOutcome::NotFound;
    }

    match respond_with_file(method, &real_file).await {
        Some(resp) => StaticOutcome::Served(resp),
        None => StaticOutcome::NotFound,
    }
}

/// Try to serve a directory-index file (`index.html`, then `index.htm`) for a
/// directory-style request under `served_root`, when that directory has no
/// `index.php` - the front controller wins if one is present, matching the
/// FastCGI "everything to index.php" policy in `pure::cgi_params`.
///
/// Each candidate index file is canonicalised and re-checked against
/// `allowed_root` (not just the directory it lives in), so a symlinked
/// `index.html`/`index.htm` cannot serve a file - PHP source included -
/// from outside the site's document root. If one candidate escapes but
/// another is servable, the servable one still wins; an escape is only
/// reported when it's the reason nothing could be served.
///
/// `allowed_root` is the site's `document_root` - a superset of
/// `served_root` when the site is served from a subdirectory.
pub async fn try_serve_index(
    method: &Method,
    uri_path: &str,
    served_root: &Path,
    allowed_root: &Path,
) -> StaticOutcome {
    if *method != Method::GET && *method != Method::HEAD {
        return StaticOutcome::NotFound;
    }

    let Some(rel) = directory_candidate(uri_path) else {
        return StaticOutcome::NotFound;
    };
    let Ok(real_root) = tokio::fs::canonicalize(allowed_root).await else {
        return StaticOutcome::NotFound;
    };

    let real_dir = match canonical_within(&served_root.join(&rel), &real_root).await {
        Some(Containment::Ok(path)) => path,
        Some(Containment::Escaped(resolved)) => {
            return StaticOutcome::SymlinkEscape {
                requested_path: uri_path.to_owned(),
                resolved,
                allowed_root: real_root,
            };
        }
        None => return StaticOutcome::NotFound,
    };

    let Ok(dir_meta) = tokio::fs::metadata(&real_dir).await else {
        return StaticOutcome::NotFound;
    };
    if !dir_meta.is_dir() {
        return StaticOutcome::NotFound;
    }
    if tokio::fs::metadata(real_dir.join("index.php"))
        .await
        .is_ok()
    {
        return StaticOutcome::NotFound;
    }

    let mut first_escape: Option<PathBuf> = None;

    for name in ["index.html", "index.htm"] {
        match canonical_within(&real_dir.join(name), &real_root).await {
            Some(Containment::Ok(real_file)) => {
                if is_php_source(&real_file) {
                    continue;
                }
                let is_file = tokio::fs::metadata(&real_file)
                    .await
                    .is_ok_and(|meta| meta.is_file());
                if !is_file {
                    continue;
                }
                if let Some(resp) = respond_with_file(method, &real_file).await {
                    return StaticOutcome::Served(resp);
                }
            }
            Some(Containment::Escaped(resolved)) if first_escape.is_none() => {
                first_escape = Some(resolved);
            }
            Some(Containment::Escaped(_)) | None => {}
        }
    }

    match first_escape {
        Some(resolved) => StaticOutcome::SymlinkEscape {
            requested_path: uri_path.to_owned(),
            resolved,
            allowed_root: real_root,
        },
        None => StaticOutcome::NotFound,
    }
}

/// Whether canonicalising a path candidate stayed within `real_root` or
/// escaped it. Returned by [`canonical_within`] so callers can tell "escaped"
/// apart from "doesn't exist" (`None`), which they need to treat differently.
/// `pub(crate)` - also used by `forward::script_file` for the same
/// symlink-containment check on a resolved PHP script.
pub(crate) enum Containment {
    /// Canonicalised and still within `real_root`.
    Ok(PathBuf),
    /// Canonicalised fine, but resolved outside `real_root`.
    Escaped(PathBuf),
}

/// Canonicalise `candidate` and check it against `real_root`, defence-in-depth
/// against symlink traversal beyond the string-level guard in the `pure`
/// candidate functions. `real_root` is an already-canonicalised `allowed_root`,
/// passed in so a caller checking several candidates against the same root
/// (e.g. `try_serve_index`'s directory plus each index-file probe) only
/// resolves it once. `None` means `candidate` doesn't exist or otherwise
/// failed to canonicalise (missing file, broken symlink, permission error) -
/// distinct from [`Containment::Escaped`], which means it resolved to a real
/// path that just isn't under `real_root`. `pub(crate)` for the same reason
/// as [`Containment`].
pub(crate) async fn canonical_within(candidate: &Path, real_root: &Path) -> Option<Containment> {
    let real = tokio::fs::canonicalize(candidate).await.ok()?;
    if real.starts_with(real_root) {
        Some(Containment::Ok(real))
    } else {
        Some(Containment::Escaped(real))
    }
}

/// Build the `403 Forbidden` response for a [`StaticOutcome::SymlinkEscape`],
/// logging it first so the escape shows up in daemon logs even though the
/// client only sees the response body.
///
/// The resolved path and allowed root are only logged, not returned in the
/// body: sites can be exposed beyond loopback via `yerd-tunnel`, so absolute
/// local filesystem paths (which could reveal the OS username and directory
/// layout to a remote client) stay in the daemon log, which never leaves the
/// machine.
pub fn symlink_escape_response(
    requested_path: &str,
    resolved: &Path,
    allowed_root: &Path,
) -> Response<BoxBody> {
    tracing::warn!(
        target: "yerd_proxy::static_file",
        requested_path = %requested_path,
        resolved = %resolved.display(),
        allowed_root = %allowed_root.display(),
        "symlink escapes site's document root, refusing to serve",
    );

    let body = format!(
        "403 Forbidden\n\n\
The requested path \"{requested_path}\" resolves, via a symlink, to a location \
outside this site's root directory.\n\n\
See the yerdd daemon log for the resolved path and the site's document root.\n",
    );

    Response::builder()
        .status(StatusCode::FORBIDDEN)
        .header(
            header::CONTENT_TYPE,
            HeaderValue::from_static("text/plain; charset=utf-8"),
        )
        .header(
            header::SERVER,
            HeaderValue::from_static(yerd_core::PROXY_SERVER_ID),
        )
        .body(owned_bytes_body(body.into_bytes()))
        .unwrap_or_else(|_| Response::new(empty_body()))
}

/// Build a 200 response with a guessed `Content-Type`. `HEAD` stats `path`
/// for its length and returns an empty body without reading the file's
/// contents; any other method reads and returns the full body.
async fn respond_with_file(method: &Method, path: &Path) -> Option<Response<BoxBody>> {
    let content_type = content_type_for(path);

    let (body, len): (BoxBody, u64) = if *method == Method::HEAD {
        (empty_body(), tokio::fs::metadata(path).await.ok()?.len())
    } else {
        let bytes = tokio::fs::read(path).await.ok()?;
        let len = bytes.len() as u64;
        let body = if bytes.is_empty() {
            empty_body()
        } else {
            http_body_util::Full::new(Bytes::from(bytes))
                .map_err(|never| match never {})
                .boxed()
        };
        (body, len)
    };

    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, HeaderValue::from_static(content_type))
        .header(header::CONTENT_LENGTH, len.to_string())
        .header(
            header::SERVER,
            HeaderValue::from_static(yerd_core::PROXY_SERVER_ID),
        )
        .body(body)
        .ok()
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

    async fn body_bytes(resp: Response<BoxBody>) -> Vec<u8> {
        resp.into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes()
            .to_vec()
    }

    #[tokio::test]
    async fn try_serve_serves_plain_file_under_served_root() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("app.css"), b"body{}").unwrap();

        let outcome = try_serve(&Method::GET, "/app.css", root.path(), root.path()).await;
        match outcome {
            StaticOutcome::Served(resp) => {
                assert_eq!(resp.status(), StatusCode::OK);
                assert_eq!(body_bytes(resp).await, b"body{}");
            }
            _ => panic!("expected Served"),
        }
    }

    #[tokio::test]
    async fn try_serve_missing_file_is_not_found() {
        let root = tempfile::tempdir().unwrap();
        let outcome = try_serve(&Method::GET, "/nope.css", root.path(), root.path()).await;
        assert!(matches!(outcome, StaticOutcome::NotFound));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn try_serve_serves_symlink_inside_document_root_but_outside_served_root() {
        let docroot = tempfile::tempdir().unwrap();
        let storage_dir = docroot.path().join("storage/app/public");
        std::fs::create_dir_all(&storage_dir).unwrap();
        std::fs::write(storage_dir.join("logo.png"), b"logo-bytes").unwrap();

        let served_root = docroot.path().join("public");
        std::fs::create_dir_all(&served_root).unwrap();
        std::os::unix::fs::symlink(&storage_dir, served_root.join("storage")).unwrap();

        let outcome = try_serve(
            &Method::GET,
            "/storage/logo.png",
            &served_root,
            docroot.path(),
        )
        .await;
        match outcome {
            StaticOutcome::Served(resp) => {
                assert_eq!(body_bytes(resp).await, b"logo-bytes");
            }
            _ => panic!("expected Served"),
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn try_serve_symlink_escaping_document_root_is_reported() {
        let docroot = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("secret.txt"), b"leaked").unwrap();
        std::os::unix::fs::symlink(
            outside.path().join("secret.txt"),
            docroot.path().join("evil.txt"),
        )
        .unwrap();

        let outcome = try_serve(&Method::GET, "/evil.txt", docroot.path(), docroot.path()).await;
        match outcome {
            StaticOutcome::SymlinkEscape {
                requested_path,
                resolved,
                allowed_root,
            } => {
                assert_eq!(requested_path, "/evil.txt");
                assert_eq!(
                    resolved,
                    tokio::fs::canonicalize(outside.path().join("secret.txt"))
                        .await
                        .unwrap()
                );
                assert_eq!(
                    allowed_root,
                    tokio::fs::canonicalize(docroot.path()).await.unwrap()
                );
            }
            _ => panic!("expected SymlinkEscape"),
        }
    }

    #[tokio::test]
    async fn try_serve_index_serves_index_html() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("index.html"), b"<h1>hi</h1>").unwrap();

        let outcome = try_serve_index(&Method::GET, "/", root.path(), root.path()).await;
        match outcome {
            StaticOutcome::Served(resp) => {
                assert_eq!(body_bytes(resp).await, b"<h1>hi</h1>");
            }
            _ => panic!("expected Served"),
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn try_serve_index_falls_back_to_index_htm_when_index_html_escapes() {
        let docroot = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("secret.html"), b"leaked").unwrap();
        std::os::unix::fs::symlink(
            outside.path().join("secret.html"),
            docroot.path().join("index.html"),
        )
        .unwrap();
        std::fs::write(docroot.path().join("index.htm"), b"fallback").unwrap();

        let outcome = try_serve_index(&Method::GET, "/", docroot.path(), docroot.path()).await;
        match outcome {
            StaticOutcome::Served(resp) => {
                assert_eq!(body_bytes(resp).await, b"fallback");
            }
            _ => panic!("expected Served"),
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn try_serve_index_reports_escape_when_no_candidate_is_servable() {
        let docroot = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("secret.html"), b"leaked").unwrap();
        std::os::unix::fs::symlink(
            outside.path().join("secret.html"),
            docroot.path().join("index.html"),
        )
        .unwrap();

        let outcome = try_serve_index(&Method::GET, "/", docroot.path(), docroot.path()).await;
        assert!(matches!(outcome, StaticOutcome::SymlinkEscape { .. }));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn try_serve_index_directory_symlink_escaping_document_root_is_reported() {
        let docroot = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("index.html"), b"leaked").unwrap();
        std::os::unix::fs::symlink(outside.path(), docroot.path().join("photos")).unwrap();

        let outcome =
            try_serve_index(&Method::GET, "/photos/", docroot.path(), docroot.path()).await;
        assert!(matches!(outcome, StaticOutcome::SymlinkEscape { .. }));
    }

    #[tokio::test]
    async fn symlink_escape_response_shape() {
        let resp = symlink_escape_response(
            "/storage/x.png",
            Path::new("/outside/x.png"),
            Path::new("/project"),
        );
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
        assert_eq!(
            resp.headers()
                .get(header::CONTENT_TYPE)
                .and_then(|v| v.to_str().ok()),
            Some("text/plain; charset=utf-8")
        );
        assert_eq!(
            resp.headers()
                .get(header::SERVER)
                .and_then(|v| v.to_str().ok()),
            Some(yerd_core::PROXY_SERVER_ID)
        );
        let body = String::from_utf8(body_bytes(resp).await).unwrap();
        assert!(body.contains("/storage/x.png"));
        assert!(!body.contains("/outside/x.png"));
        assert!(!body.contains("/project"));
    }
}
