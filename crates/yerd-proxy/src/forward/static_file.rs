//! Static-file serving - the `try_files`-style short-circuit in front of the
//! PHP front controller.
//!
//! A GET/HEAD whose URL resolves to a real, non-PHP file under the served root
//! is streamed from disk with a guessed `Content-Type`. Anything else (missing
//! file, directory, PHP source, non-idempotent method, traversal attempt)
//! returns `None`, and the caller forwards to FastCGI (`index.php`) exactly as
//! before. Without this, `/favicon.ico` and other static assets were handed to
//! the PHP framework, which has no route for them.

use std::path::Path;

use bytes::Bytes;
use http::{header, HeaderValue, Method, StatusCode};
use http_body_util::BodyExt;
use hyper::Response;

use crate::forward::{empty_body, BoxBody};
use crate::pure::try_files::{content_type_for, is_php_source, static_candidate};

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
    let candidate = served_root.join(&rel);

    let real_file = tokio::fs::canonicalize(&candidate).await.ok()?;
    let real_root = tokio::fs::canonicalize(served_root).await.ok()?;
    if !real_file.starts_with(&real_root) {
        return None;
    }

    if is_php_source(&real_file) {
        return None;
    }

    let meta = tokio::fs::metadata(&real_file).await.ok()?;
    if !meta.is_file() {
        return None;
    }

    let bytes = tokio::fs::read(&real_file).await.ok()?;
    let len = bytes.len();
    let content_type = content_type_for(&real_file);
    let head_only = *method == Method::HEAD;

    let body: BoxBody = if head_only || bytes.is_empty() {
        empty_body()
    } else {
        http_body_util::Full::new(Bytes::from(bytes))
            .map_err(|never| match never {})
            .boxed()
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
