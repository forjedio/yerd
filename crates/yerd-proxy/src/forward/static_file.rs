//! Static-file serving - the `try_files`-style short-circuit in front of the
//! PHP front controller.
//!
//! A GET/HEAD whose URL resolves to a real, non-PHP file under the served root
//! is streamed from disk with a guessed `Content-Type`. Anything else (missing
//! file, directory, PHP source, non-idempotent method, traversal attempt)
//! returns `None`, and the caller forwards to FastCGI (`index.php`) exactly as
//! before. Without this, `/favicon.ico` and other static assets were handed to
//! the PHP framework, which has no route for them.
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

use crate::forward::{empty_body, BoxBody};
use crate::pure::try_files::{
    content_type_for, directory_candidate, is_php_source, static_candidate,
};

/// Try to serve `uri_path` as a static file under `served_root`.
///
/// `Some(response)` - a file was found and served (200). `None` - the request
/// should fall through to the PHP front controller.
pub async fn try_serve(
    method: &Method,
    uri_path: &str,
    served_root: &Path,
) -> Option<Response<BoxBody>> {
    if *method != Method::GET && *method != Method::HEAD {
        return None;
    }

    let rel = static_candidate(uri_path)?;
    let real_root = tokio::fs::canonicalize(served_root).await.ok()?;
    let real_file = canonical_within(&served_root.join(&rel), &real_root).await?;

    if is_php_source(&real_file) {
        return None;
    }

    let meta = tokio::fs::metadata(&real_file).await.ok()?;
    if !meta.is_file() {
        return None;
    }

    respond_with_file(method, &real_file).await
}

/// Try to serve a directory-index file (`index.html`, then `index.htm`) for a
/// directory-style request under `served_root`, when that directory has no
/// `index.php` - the front controller wins if one is present, matching the
/// FastCGI "everything to index.php" policy in `pure::cgi_params`.
///
/// Each candidate index file is canonicalised and re-checked against
/// `served_root` (not just the directory it lives in), so a symlinked
/// `index.html`/`index.htm` cannot serve a file - PHP source included -
/// from outside the site root.
///
/// `Some(response)` - an index file was found and served (200). `None` - the
/// request isn't a directory request, the directory has an `index.php`, or no
/// index file exists there; the caller should fall through to the PHP front
/// controller.
pub async fn try_serve_index(
    method: &Method,
    uri_path: &str,
    served_root: &Path,
) -> Option<Response<BoxBody>> {
    if *method != Method::GET && *method != Method::HEAD {
        return None;
    }

    let rel = directory_candidate(uri_path)?;
    let real_root = tokio::fs::canonicalize(served_root).await.ok()?;
    let real_dir = canonical_within(&served_root.join(&rel), &real_root).await?;

    if !tokio::fs::metadata(&real_dir).await.ok()?.is_dir() {
        return None;
    }
    if tokio::fs::metadata(real_dir.join("index.php"))
        .await
        .is_ok()
    {
        return None;
    }

    for name in ["index.html", "index.htm"] {
        let Some(real_file) = canonical_within(&real_dir.join(name), &real_root).await else {
            continue;
        };
        if is_php_source(&real_file) {
            continue;
        }
        if tokio::fs::metadata(&real_file)
            .await
            .is_ok_and(|meta| meta.is_file())
        {
            return respond_with_file(method, &real_file).await;
        }
    }

    None
}

/// Canonicalise `candidate` and verify it's still within `real_root`,
/// defence-in-depth against symlink traversal beyond the string-level guard
/// in the `pure` candidate functions. `real_root` is an already-canonicalised
/// `served_root`, passed in so a caller checking several candidates against
/// the same root (e.g. `try_serve_index`'s directory plus each index-file
/// probe) only resolves `served_root` once.
async fn canonical_within(candidate: &Path, real_root: &Path) -> Option<PathBuf> {
    let real = tokio::fs::canonicalize(candidate).await.ok()?;
    real.starts_with(real_root).then_some(real)
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
