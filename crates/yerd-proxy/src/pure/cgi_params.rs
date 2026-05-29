//! Build the CGI parameter list for one FastCGI request.
//!
//! Policy (MVP): Caddy-style "everything to index.php".
//!
//! - `SCRIPT_FILENAME = document_root / "index.php"`
//! - `SCRIPT_NAME     = "/index.php"`
//! - `PATH_INFO       = <original path>`
//! - `REQUEST_URI     = <original path_and_query>`
//!
//! Plus the standard CGI/1.1 vars and `HTTP_*`-translated headers.

use std::net::SocketAddr;
use std::path::Path;

/// Build the CGI parameter pairs.
#[must_use]
pub fn build_params(
    method: &str,
    path_and_query: &str,
    headers: &http::HeaderMap,
    document_root: &Path,
    https: bool,
    remote_addr: SocketAddr,
    server_addr: SocketAddr,
) -> Vec<(Vec<u8>, Vec<u8>)> {
    let mut out: Vec<(Vec<u8>, Vec<u8>)> = Vec::with_capacity(16 + headers.len());

    let (path, query) = split_path_query(path_and_query);
    let script_filename = document_root.join("index.php");

    push(&mut out, b"GATEWAY_INTERFACE", b"CGI/1.1");
    push(&mut out, b"SERVER_PROTOCOL", b"HTTP/1.1");
    push(&mut out, b"REQUEST_METHOD", method.as_bytes());
    push(&mut out, b"REQUEST_URI", path_and_query.as_bytes());
    push(&mut out, b"QUERY_STRING", query.as_bytes());
    push(&mut out, b"SCRIPT_NAME", b"/index.php");
    push(
        &mut out,
        b"SCRIPT_FILENAME",
        script_filename.to_string_lossy().as_bytes(),
    );
    push(
        &mut out,
        b"DOCUMENT_ROOT",
        document_root.to_string_lossy().as_bytes(),
    );
    push(&mut out, b"PATH_INFO", path.as_bytes());
    push(
        &mut out,
        b"REMOTE_ADDR",
        remote_addr.ip().to_string().as_bytes(),
    );
    push(
        &mut out,
        b"REMOTE_PORT",
        remote_addr.port().to_string().as_bytes(),
    );
    push(
        &mut out,
        b"SERVER_ADDR",
        server_addr.ip().to_string().as_bytes(),
    );
    push(
        &mut out,
        b"SERVER_PORT",
        server_addr.port().to_string().as_bytes(),
    );
    push(&mut out, b"SERVER_SOFTWARE", b"yerd");
    if https {
        push(&mut out, b"HTTPS", b"on");
    }

    // Standard headers FPM expects un-prefixed.
    if let Some(host) = headers
        .get(http::header::HOST)
        .and_then(|v| v.to_str().ok())
    {
        push(&mut out, b"SERVER_NAME", host.as_bytes());
        push(&mut out, b"HTTP_HOST", host.as_bytes());
    }
    if let Some(ct) = headers
        .get(http::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
    {
        push(&mut out, b"CONTENT_TYPE", ct.as_bytes());
    }
    if let Some(cl) = headers
        .get(http::header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
    {
        push(&mut out, b"CONTENT_LENGTH", cl.as_bytes());
    }

    // Generic HTTP_* translation for everything else.
    for (name, value) in headers {
        if matches!(
            name,
            &http::header::HOST | &http::header::CONTENT_TYPE | &http::header::CONTENT_LENGTH
        ) {
            continue;
        }
        let mut key = b"HTTP_".to_vec();
        for byte in name.as_str().as_bytes() {
            key.push(if *byte == b'-' {
                b'_'
            } else {
                byte.to_ascii_uppercase()
            });
        }
        push(&mut out, &key, value.as_bytes());
    }

    out
}

fn push(out: &mut Vec<(Vec<u8>, Vec<u8>)>, name: &[u8], value: &[u8]) {
    out.push((name.to_vec(), value.to_vec()));
}

fn split_path_query(path_and_query: &str) -> (&str, &str) {
    match path_and_query.split_once('?') {
        Some((p, q)) => (p, q),
        None => (path_and_query, ""),
    }
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
    use http::HeaderMap;
    use std::path::PathBuf;

    fn lookup<'a>(pairs: &'a [(Vec<u8>, Vec<u8>)], key: &[u8]) -> Option<&'a [u8]> {
        pairs
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_slice())
    }

    fn make_headers(host: &str) -> HeaderMap {
        let mut h = HeaderMap::new();
        h.insert(http::header::HOST, host.parse().unwrap());
        h
    }

    #[test]
    fn caddy_style_everything_to_index_php() {
        let root = PathBuf::from("/srv/www/app");
        let pairs = build_params(
            "GET",
            "/foo/bar?a=1&b=2",
            &make_headers("app.test"),
            &root,
            false,
            "127.0.0.1:54321".parse().unwrap(),
            "127.0.0.1:80".parse().unwrap(),
        );
        assert_eq!(
            lookup(&pairs, b"SCRIPT_NAME"),
            Some(b"/index.php".as_slice())
        );
        assert_eq!(
            lookup(&pairs, b"SCRIPT_FILENAME"),
            Some("/srv/www/app/index.php".as_bytes())
        );
        assert_eq!(lookup(&pairs, b"PATH_INFO"), Some(b"/foo/bar".as_slice()));
        assert_eq!(
            lookup(&pairs, b"REQUEST_URI"),
            Some(b"/foo/bar?a=1&b=2".as_slice())
        );
        assert_eq!(lookup(&pairs, b"QUERY_STRING"), Some(b"a=1&b=2".as_slice()));
        assert_eq!(lookup(&pairs, b"REQUEST_METHOD"), Some(b"GET".as_slice()));
        assert_eq!(lookup(&pairs, b"SERVER_NAME"), Some(b"app.test".as_slice()));
        assert_eq!(lookup(&pairs, b"HTTP_HOST"), Some(b"app.test".as_slice()));
        assert_eq!(
            lookup(&pairs, b"DOCUMENT_ROOT"),
            Some(b"/srv/www/app".as_slice())
        );
        assert!(lookup(&pairs, b"HTTPS").is_none());
    }

    #[test]
    fn https_param_is_on_when_secure() {
        let pairs = build_params(
            "POST",
            "/",
            &make_headers("app.test"),
            Path::new("/srv/www/app"),
            true,
            "1.2.3.4:1000".parse().unwrap(),
            "127.0.0.1:443".parse().unwrap(),
        );
        assert_eq!(lookup(&pairs, b"HTTPS"), Some(b"on".as_slice()));
    }

    #[test]
    fn http_headers_translated_to_http_underscore() {
        let mut headers = make_headers("app.test");
        headers.insert("X-Custom", "yes".parse().unwrap());
        headers.insert(http::header::ACCEPT, "text/html".parse().unwrap());
        let pairs = build_params(
            "GET",
            "/",
            &headers,
            Path::new("/srv"),
            false,
            "127.0.0.1:1".parse().unwrap(),
            "127.0.0.1:80".parse().unwrap(),
        );
        assert_eq!(lookup(&pairs, b"HTTP_X_CUSTOM"), Some(b"yes".as_slice()));
        assert_eq!(
            lookup(&pairs, b"HTTP_ACCEPT"),
            Some(b"text/html".as_slice())
        );
    }

    #[test]
    fn content_type_and_length_pulled_out_of_http_prefix() {
        let mut headers = make_headers("app.test");
        headers.insert(
            http::header::CONTENT_TYPE,
            "application/json".parse().unwrap(),
        );
        headers.insert(http::header::CONTENT_LENGTH, "42".parse().unwrap());
        let pairs = build_params(
            "POST",
            "/",
            &headers,
            Path::new("/srv"),
            false,
            "127.0.0.1:1".parse().unwrap(),
            "127.0.0.1:80".parse().unwrap(),
        );
        assert_eq!(
            lookup(&pairs, b"CONTENT_TYPE"),
            Some(b"application/json".as_slice())
        );
        assert_eq!(lookup(&pairs, b"CONTENT_LENGTH"), Some(b"42".as_slice()));
        assert!(lookup(&pairs, b"HTTP_CONTENT_TYPE").is_none());
        assert!(lookup(&pairs, b"HTTP_CONTENT_LENGTH").is_none());
    }

    #[test]
    fn no_query_string_yields_empty_query() {
        let pairs = build_params(
            "GET",
            "/just/path",
            &make_headers("a.test"),
            Path::new("/srv"),
            false,
            "127.0.0.1:1".parse().unwrap(),
            "127.0.0.1:80".parse().unwrap(),
        );
        assert_eq!(lookup(&pairs, b"PATH_INFO"), Some(b"/just/path".as_slice()));
        assert_eq!(lookup(&pairs, b"QUERY_STRING"), Some(b"".as_slice()));
    }
}
