//! Build the CGI parameter list for one FastCGI request.
//!
//! Policy: a `try_files`-style front controller. The caller (`forward::
//! script_file::resolve_script`) resolves the request path against the real
//! filesystem first - an exact `.php` match (`/wp-login.php`), or a
//! directory's own `index.php` (`/wp-admin/` -> `wp-admin/index.php`) - and
//! passes the result in as `script_rel`. When it finds a real script:
//!
//! - `SCRIPT_FILENAME = document_root / <script_rel>`
//! - `SCRIPT_NAME     = "/" + <script_rel>`
//!
//! Otherwise (Caddy-style "everything to index.php", the original MVP
//! policy and still correct for single-front-controller frameworks like
//! Laravel):
//!
//! - `SCRIPT_FILENAME = document_root / "index.php"`
//! - `SCRIPT_NAME     = "/index.php"`
//!
//! `PATH_INFO` is always `<original path>` either way - WordPress and
//! Laravel both route on `REQUEST_URI`, not `PATH_INFO`, so leaving it as the
//! full original path (rather than splitting "extra path after the script",
//! full CGI/1.1 `PATH_INFO` semantics) keeps this a minimal, low-risk change
//! on top of already-pinned behavior.
//!
//! Plus the standard CGI/1.1 vars and `HTTP_*`-translated headers.

use std::net::SocketAddr;
use std::path::Path;

/// Build the CGI parameter pairs. `script_rel`, if given, is a real,
/// on-disk `.php` file's path relative to `document_root` (see the module
/// doc) - `None` falls back to the root `index.php` policy.
#[must_use]
#[allow(clippy::too_many_arguments)]
pub fn build_params(
    method: &str,
    path_and_query: &str,
    headers: &http::HeaderMap,
    document_root: &Path,
    script_rel: Option<&Path>,
    https: bool,
    remote_addr: SocketAddr,
    server_addr: SocketAddr,
) -> Vec<(Vec<u8>, Vec<u8>)> {
    let mut out: Vec<(Vec<u8>, Vec<u8>)> = Vec::with_capacity(16 + headers.len());

    let (path, query) = split_path_query(path_and_query);
    let (script_filename, script_name) = match script_rel {
        Some(rel) => (
            document_root.join(rel),
            format!("/{}", rel.to_string_lossy().replace('\\', "/")),
        ),
        None => (document_root.join("index.php"), "/index.php".to_owned()),
    };

    push(&mut out, b"GATEWAY_INTERFACE", b"CGI/1.1");
    push(&mut out, b"SERVER_PROTOCOL", b"HTTP/1.1");
    push(&mut out, b"REQUEST_METHOD", method.as_bytes());
    push(&mut out, b"REQUEST_URI", path_and_query.as_bytes());
    push(&mut out, b"QUERY_STRING", query.as_bytes());
    push(&mut out, b"SCRIPT_NAME", script_name.as_bytes());
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
    // Deliberately contains "nginx", not just "yerd": frameworks (WordPress
    // in particular - see `got_url_rewrite()`/`$is_nginx` in
    // wp-admin/includes/misc.php and wp-includes/vars.php) parse this CGI
    // var for known-good server names to decide whether extension-less
    // "pretty" URLs are safe to offer, since a plain front-controller
    // fallback isn't universal. yerd's front controller resolution (see
    // `script_file::resolve_script`) is exactly nginx's classic
    // `try_files $uri $uri/ /index.php` policy, so this is an accurate
    // capability signal, not a spoof - and it's this CGI var PHP sees, not
    // the client-facing `Server:` HTTP header (see `yerd_core::PROXY_SERVER_ID`),
    // which still identifies yerd honestly to browsers/tools.
    push(&mut out, b"SERVER_SOFTWARE", b"yerd (nginx-compatible)");
    if https {
        push(&mut out, b"HTTPS", b"on");
    }

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
            None,
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
    fn server_software_contains_nginx_for_framework_rewrite_detection() {
        // WordPress (and other frameworks) gate "pretty"/extension-less
        // permalink options on this CGI var containing a known-good server
        // name - see the module doc for the full explanation.
        let pairs = build_params(
            "GET",
            "/",
            &make_headers("app.test"),
            Path::new("/srv"),
            None,
            false,
            "127.0.0.1:1".parse().unwrap(),
            "127.0.0.1:80".parse().unwrap(),
        );
        let software = String::from_utf8_lossy(lookup(&pairs, b"SERVER_SOFTWARE").unwrap());
        assert!(software.contains("nginx"), "got {software:?}");
    }

    #[test]
    fn web_root_subdir_drives_script_filename_and_document_root() {
        let mut site =
            yerd_core::Site::linked("app", "/srv/www/app", yerd_core::PhpVersion::new(8, 3))
                .unwrap();
        site.set_web_subpath("public");
        let served = site.served_root();
        let pairs = build_params(
            "GET",
            "/login",
            &make_headers("app.test"),
            &served,
            None,
            false,
            "127.0.0.1:1".parse().unwrap(),
            "127.0.0.1:80".parse().unwrap(),
        );
        assert_eq!(
            lookup(&pairs, b"DOCUMENT_ROOT"),
            Some("/srv/www/app/public".as_bytes())
        );
        assert_eq!(
            lookup(&pairs, b"SCRIPT_FILENAME"),
            Some("/srv/www/app/public/index.php".as_bytes())
        );
    }

    #[test]
    fn https_param_is_on_when_secure() {
        let pairs = build_params(
            "POST",
            "/",
            &make_headers("app.test"),
            Path::new("/srv/www/app"),
            None,
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
            None,
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
            None,
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
            None,
            false,
            "127.0.0.1:1".parse().unwrap(),
            "127.0.0.1:80".parse().unwrap(),
        );
        assert_eq!(lookup(&pairs, b"PATH_INFO"), Some(b"/just/path".as_slice()));
        assert_eq!(lookup(&pairs, b"QUERY_STRING"), Some(b"".as_slice()));
    }

    #[test]
    fn resolved_script_drives_script_name_and_filename() {
        let pairs = build_params(
            "GET",
            "/wp-admin/?page=1",
            &make_headers("blog.test"),
            Path::new("/srv/www/blog"),
            Some(Path::new("wp-admin/index.php")),
            false,
            "127.0.0.1:1".parse().unwrap(),
            "127.0.0.1:80".parse().unwrap(),
        );
        assert_eq!(
            lookup(&pairs, b"SCRIPT_NAME"),
            Some(b"/wp-admin/index.php".as_slice())
        );
        assert_eq!(
            lookup(&pairs, b"SCRIPT_FILENAME"),
            Some("/srv/www/blog/wp-admin/index.php".as_bytes())
        );
        // PATH_INFO stays the full original path either way - WordPress and
        // Laravel both route on REQUEST_URI, not PATH_INFO (see module doc).
        assert_eq!(lookup(&pairs, b"PATH_INFO"), Some(b"/wp-admin/".as_slice()));
    }

    #[test]
    fn resolved_exact_script_match_drives_script_name_and_filename() {
        let pairs = build_params(
            "POST",
            "/wp-login.php",
            &make_headers("blog.test"),
            Path::new("/srv/www/blog"),
            Some(Path::new("wp-login.php")),
            false,
            "127.0.0.1:1".parse().unwrap(),
            "127.0.0.1:80".parse().unwrap(),
        );
        assert_eq!(
            lookup(&pairs, b"SCRIPT_NAME"),
            Some(b"/wp-login.php".as_slice())
        );
        assert_eq!(
            lookup(&pairs, b"SCRIPT_FILENAME"),
            Some("/srv/www/blog/wp-login.php".as_bytes())
        );
    }
}
